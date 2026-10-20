import { createHmac, generateKeyPairSync } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { afterEach, beforeEach, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { AppStore } from "../src/persistence/store.ts";
import { openApp } from "../src/runtime/app.ts";
import { REVIEW_TASK_NAME } from "../src/runtime/task.ts";
import { type Service, startService } from "../src/server/service.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, type Fixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

const SECRET = "s3cret";
const ADMIN = "admin";
const KEY = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey.export({ type: "pkcs8", format: "pem" }).toString();
const config = {
  ...DEFAULT_CONFIG,
  review: { ...DEFAULT_CONFIG.review, debounce_seconds: 0, publish_mode: "comment" as const },
  github: { ...DEFAULT_CONFIG.github, timeout_ms: 300 },
};

let fake: FakeGitHub;
let fixture: Fixture;
const services: { service: Service; close(): Promise<void> }[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fixture.pull = { ...fixture.pull, installationId: 42 };
  fake.setPull(fixture.pull);
});
afterEach(async () => {
  for (const s of services.splice(0)) await s.close();
  await fake.close();
});

async function serve(dataDir: string) {
  const clients = new Map<number, GitHubClient>();
  const github = (installationId: number) => {
    if (!clients.has(installationId)) {
      clients.set(
        installationId,
        new GitHubClient({ auth: { kind: "app", appId: "1", privateKeyPem: KEY, installationId }, baseUrl: fake.url, maxRetries: 1, backoffMs: 1, concurrency: 2, timeoutMs: 5000 }),
      );
    }
    return clients.get(installationId)!;
  };
  const pulls = new GitHubPullSource(github);
  const app = await openApp({
    dataDir,
    config,
    pulls,
    github,
    gitToken: async () => undefined,
    engine: { skipped: "Jev disabled by --no-jev" },
    models: fauxModels([readDiff(), submitDefect(fixture)]),
    model: { provider: "faux", modelId: "reviewer" },
    reviewerVersion: "m2+faux/reviewer",
  });
  const service = await startService({ app, config, dataDir, pulls, github, webhookSecret: SECRET, adminToken: ADMIN, port: 0, publish: true, tickMs: 20, report: () => {} });
  const handle = {
    service,
    app,
    async close() {
      await service.close();
      await service.idle();
      await app.close();
    },
  };
  services.push(handle);
  return handle;
}

function deliver(url: string, deliveryId: string, event: string, payload: object) {
  const body = JSON.stringify({ repository: { full_name: "acme/shop" }, installation: { id: 42 }, sender: { type: "User" }, ...payload });
  return fetch(`${url}/webhooks/github`, {
    method: "POST",
    body,
    headers: { "x-github-delivery": deliveryId, "x-github-event": event, "x-hub-signature-256": `sha256=${createHmac("sha256", SECRET).update(body).digest("hex")}` },
  });
}

async function until(condition: () => boolean, ms = 20_000) {
  for (const start = Date.now(); !condition(); ) {
    if (Date.now() - start > ms) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

it("turns duplicate webhooks into exactly one review task and one published review", async () => {
  const { service, app } = await serve(mkdtempSync(join(tmpdir(), "service-")));
  const opened = { action: "opened", pull_request: { number: 1 } };
  const statuses = [];
  for (const id of ["d-1", "d-1", "d-1"]) statuses.push((await deliver(service.url, id, "pull_request", opened)).status);
  statuses.push((await deliver(service.url, "d-2", "pull_request", { action: "review_requested", pull_request: { number: 1 } })).status);
  expect(statuses).toEqual([202, 200, 200, 202]);
  await until(() => app.store.db.prepare("SELECT count(*) AS n FROM review_revisions WHERE state = 'COMPLETED'").get()?.n === 1);
  await service.idle();

  // Bot loops: a bot sender, the app's own comment, and a comment without the mention start nothing.
  const comment = (login: string, body: string, type = "User") => ({ action: "created", sender: { type }, issue: { number: 1, pull_request: {} }, comment: { body, user: { login } } });
  await deliver(service.url, "d-3", "issue_comment", comment("ci[bot]", "@durable-review review", "Bot"));
  await deliver(service.url, "d-4", "issue_comment", comment("durable-review[bot]", "@durable-review review"));
  await deliver(service.url, "d-5", "issue_comment", comment("alice", "thanks"));
  await until(() => app.store.countUnprocessedWebhooks() === 0);
  await service.idle();

  const revisions = app.store.db.prepare("SELECT id, state, policy_version FROM review_revisions").all() as { id: string }[];
  const tasks = await app.harness.commit((tx) => tx.scanTasks({ kind: REVIEW_TASK_NAME }, 10), BACKGROUND_CONTEXT);
  expect([revisions.length, tasks.items.length, fake.postAttempts, fake.reviews.map((r) => r.user.login)]).toEqual([1, 1, 1, ["durable-review[bot]"]]);
  expect(app.store.db.prepare("SELECT delivery_id, outcome FROM webhook_events ORDER BY delivery_id").all()).toEqual([
    { delivery_id: "d-1", outcome: "PR_OPENED -> ACTIVE" },
    { delivery_id: "d-2", outcome: "REVIEW_REQUESTED -> ACTIVE" },
    { delivery_id: "d-3", outcome: "ignored: bot sender" },
    { delivery_id: "d-4", outcome: "ignored: own comment" },
    { delivery_id: "d-5", outcome: "ignored: no review mention" },
  ]);

  const id = revisions[0]!.id;
  expect((await fetch(`${service.url}/reviews/${id}`)).status).toBe(401);
  const inspected = await (await fetch(`${service.url}/reviews/${id}`, { headers: { authorization: `Bearer ${ADMIN}` } })).json();
  expect([inspected.state, inspected.published, inspected.githubReviewId, inspected.outboxEvents.length]).toEqual(["COMPLETED", true, fake.reviews[0]!.id, 2]);
  const ready = await (await fetch(`${service.url}/readyz`, { headers: { authorization: `Bearer ${ADMIN}` } })).json();
  expect(ready).toEqual({ ok: true, checks: { db: true, worker: true, storage: true } });
  const retry = await fetch(`${service.url}/reviews/${id}/retry`, { method: "POST", headers: { authorization: `Bearer ${ADMIN}` } });
  expect([retry.status, await retry.json()]).toEqual([409, { error: "MANUAL_RETRY is not legal in COMPLETED" }]);
}, 60_000);

it("reconciles a failed revision's uncertain outbox on restart before posting anything", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "service-restart-"));
  fake.postBehaviors = ["store-then-drop"];
  fake.onPost = () => void (fake.listStatus = 500);
  const first = await serve(dataDir);
  await deliver(first.service.url, "d-1", "pull_request", { action: "opened", pull_request: { number: 1 } });
  await until(() => first.app.store.db.prepare("SELECT count(*) AS n FROM review_revisions WHERE state = 'FAILED'").get()?.n === 1);
  await first.service.idle();
  const revisionId = (first.app.store.db.prepare("SELECT id FROM review_revisions").get() as { id: string }).id;
  expect(first.app.store.outboxForRevision(revisionId)!.status).toBe("needs_reconciliation");
  await first.close();
  services.splice(0);

  fake.listStatus = 200;
  const second = await serve(dataDir);
  await until(() => second.app.store.outboxForRevision(revisionId)!.status === "published");
  await second.service.idle();
  expect([second.app.store.revision(revisionId)!.state, fake.postAttempts, fake.reviews.length]).toEqual(["COMPLETED", 1, 1]);
  expect(second.app.store.auditEvents(revisionId).slice(-2).map((e) => `${e.from_state} ${e.event} ${e.to_state}`)).toEqual([
    "RECONCILE TERMINAL_ERROR FAILED",
    "FAILED REVIEW_RECONCILED COMPLETED",
  ]);
}, 60_000);

it("POST /reviews/:id/retry reopens a rejected revision through the audited MANUAL_RETRY override", async () => {
  fake.postBehaviors = ["unprocessable"];
  const { service, app } = await serve(mkdtempSync(join(tmpdir(), "service-retry-")));
  await deliver(service.url, "d-1", "pull_request", { action: "opened", pull_request: { number: 1 } });
  await until(() => app.store.db.prepare("SELECT count(*) AS n FROM review_revisions WHERE state = 'FAILED'").get()?.n === 1);
  await service.idle();
  const id = (app.store.db.prepare("SELECT id FROM review_revisions").get() as { id: string }).id;
  const retry = await fetch(`${service.url}/reviews/${id}/retry`, { method: "POST", headers: { authorization: `Bearer ${ADMIN}` } });
  expect([retry.status, await retry.json()]).toEqual([202, { revisionId: id, state: "COLLECT_CONTEXT" }]);
  await until(() => app.store.revision(id)!.state === "COMPLETED");
  await service.idle();
  expect(app.store.auditEvents(id).find((e) => e.event === "MANUAL_RETRY")!.detail).toBe(`manual override: POST /reviews/${id}/retry (retry 1)`);
  expect(app.store.outboxEvents(app.store.outboxForRevision(id)!.id).map((e) => e.event)).toEqual(["SEND", "REJECTED", "MANUAL_RETRY", "SEND", "SENT"]);
  expect([fake.postAttempts, fake.reviews.length]).toEqual([2, 1]);
}, 60_000);

it("reclaims its own leases at once after a crash: the lease owner is stable per data dir", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "service-lease-"));
  const first = await serve(dataDir);
  const owner = (first.app.store.db.prepare("SELECT value FROM meta WHERE key = 'lease_owner'").get() as { value: string }).value;
  const prId = first.app.store.upsertPull(fixture.pull);
  await first.close();
  services.splice(0);

  // A crash mid-tick leaves the lease behind, five minutes from expiry.
  const crashed = AppStore.open(join(dataDir, "app.sqlite"));
  crashed.db.prepare("INSERT INTO leases (pr_id, owner, expires_at) VALUES (?, ?, ?)").run(prId, owner, Date.now() + 5 * 60_000);
  crashed.close();

  const second = await serve(dataDir);
  await deliver(second.service.url, "d-1", "pull_request", { action: "opened", pull_request: { number: 1 } });
  await until(() => second.app.store.db.prepare("SELECT count(*) AS n FROM review_revisions WHERE state = 'COMPLETED'").get()?.n === 1);
  await second.service.idle();
  expect(fake.postAttempts).toBe(1);
}, 60_000);
