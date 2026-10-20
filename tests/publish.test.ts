import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FauxResponseStep } from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { type App, finishReview, openApp, startReview } from "../src/runtime/app.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, type Fixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

let fake: FakeGitHub;
let fixture: Fixture;
let sleeps: number[];
const apps: App[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fake.setPull(fixture.pull);
  sleeps = [];
});
afterEach(async () => {
  for (const app of apps.splice(0)) await app.close();
  await fake.close();
});

async function openPublishing(responses: FauxResponseStep[] = [readDiff(), submitDefect(fixture)]) {
  const client = new GitHubClient({
    auth: { kind: "token", token: "ghp_test" },
    baseUrl: fake.url,
    maxRetries: 2,
    backoffMs: 1,
    concurrency: 2,
    timeoutMs: 5000,
    sleep: async (ms) => void sleeps.push(ms),
  });
  const app = await openApp({
    dataDir: mkdtempSync(join(tmpdir(), "publish-")),
    config: DEFAULT_CONFIG,
    pulls: new GitHubPullSource(() => client),
    github: () => client,
    engine: { skipped: "Jev disabled by --no-jev" },
    models: fauxModels(responses),
    model: { provider: "faux", modelId: "reviewer" },
    reviewerVersion: "m2+faux/reviewer",
  });
  apps.push(app);
  return app;
}

async function publish(app: App) {
  const revision = await finishReview(app, await startReview(app, fixture.pull, "1", { publish: true }));
  const outbox = app.store.outboxForRevision(revision.id)!;
  return { revision, outbox, outboxEvents: app.store.outboxEvents(outbox.id).map((e) => `${e.from_status} ${e.event} ${e.to_status}`) };
}

const transitions = (app: App, id: string) => app.store.auditEvents(id).map((e) => `${e.from_state} ${e.event} ${e.to_state}`).slice(4);

it("publishes one COMMENT review with the marker and an inline comment on the verified line", async () => {
  const app = await openPublishing();
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, outbox.github_review_id, fake.postAttempts]).toEqual(["COMPLETED", "published", 1000, 1]);
  const review = fake.reviews[0]!;
  expect(review.body).toContain(`Reviewed commit: \`${fixture.headSha}\``);
  expect(review.body).toContain("- High: Order deletion endpoint skips the admin check (`src/routes/orders.ts:5`)");
  expect(review.body).toMatch(new RegExp(`<!-- durable-review:${revision.id}:[0-9a-f]{64} -->\\n$`));
  expect([review.event, review.commit_id]).toEqual(["COMMENT", fixture.headSha]);
  expect(review.comments).toEqual([
    {
      path: "src/routes/orders.ts",
      line: 5,
      side: "RIGHT",
      body: "**High: Order deletion endpoint skips the admin check**\n\nGET /orders requires requireAdmin; the new DELETE /orders/:id has no guard, so any caller can delete orders.",
    },
  ]);
  expect(app.store.findingsForRevision(revision.id).map((f) => f.status)).toEqual(["PUBLISHED"]);
  expect(transitions(app, revision.id)).toEqual(["PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH", "PUBLISH REVIEW_RECONCILED COMPLETED"]);
  const again = await startReview(app, fixture.pull, "1", { publish: true });
  expect([again.taskId, fake.postAttempts]).toEqual([undefined, 1]);
}, 30_000);

it("(b) reconciles a review GitHub stored before dropping the connection, across pages, without a second POST", async () => {
  fake.postBehaviors = ["store-then-drop"];
  fake.pageSize = 2;
  fake.addForeignReviews(3, fixture.headSha);
  const app = await openPublishing();
  const { revision, outbox, outboxEvents } = await publish(app);
  expect([revision.state, outbox.status, outbox.github_review_id, fake.postAttempts, fake.reviews.length, fake.listCalls]).toEqual([
    "COMPLETED",
    "published",
    1003,
    1,
    4,
    2,
  ]);
  expect(outboxEvents).toEqual(["pending SEND sending", "sending UNCERTAIN needs_reconciliation", "needs_reconciliation FOUND published"]);
  expect(transitions(app, revision.id)).toEqual([
    "PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH",
    "PUBLISH WRITE_UNCERTAIN RECONCILE",
    "RECONCILE REVIEW_RECONCILED COMPLETED",
  ]);
}, 30_000);

it("(d) leaves an ambiguous write in needs_reconciliation when the listing fails, and never POSTs again", async () => {
  fake.postBehaviors = ["bad-gateway"];
  fake.listStatus = 500;
  const app = await openPublishing();
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, fake.postAttempts]).toEqual(["FAILED", "needs_reconciliation", 1]);
  expect(app.store.auditEvents(revision.id).at(-1)!.detail).toBe("cannot_safely_decide: GET /repos/acme/shop/pulls/1/reviews?per_page=100: HTTP 500");
}, 30_000);

it("(e) supersedes a revision whose head moved between prepare and publish, with zero POSTs", async () => {
  const app = await openPublishing();
  fake.onGetPull = () => {
    if (app.store.db.prepare("SELECT count(*) AS n FROM publication_outbox").get()?.n === 1) fake.setPull({ ...fixture.pull, headSha: "f".repeat(40) });
  };
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, outbox.last_error, fake.postAttempts]).toEqual(["SUPERSEDED", "failed", "superseded", 0]);
}, 30_000);

it("(f) retries a 429 with Retry-After before the POST is processed and publishes once", async () => {
  fake.postBehaviors = ["rate-limited"];
  const app = await openPublishing();
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, outbox.attempt_count, fake.postAttempts, fake.reviews.length, sleeps]).toEqual(["COMPLETED", "published", 1, 2, 1, [1000]]);
}, 30_000);

it("stops after the attempt budget when every POST is ambiguous and reconciliation proves it absent", async () => {
  fake.postBehaviors = Array(10).fill("bad-gateway");
  const app = await openPublishing();
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, outbox.attempt_count, fake.postAttempts, fake.reviews.length]).toEqual(["FAILED", "failed", 4, 4, 0]);
  expect(app.store.auditEvents(revision.id).at(-1)!.detail).toBe("publish failed: 4 attempts, none reached GitHub");
}, 30_000);

it("fails the revision when GitHub rejects the review outright", async () => {
  fake.postBehaviors = ["unprocessable"];
  const app = await openPublishing();
  const { revision, outbox } = await publish(app);
  expect([revision.state, outbox.status, fake.postAttempts]).toEqual(["FAILED", "failed", 1]);
  expect(app.store.auditEvents(revision.id).at(-1)!.detail).toBe("GitHub rejected the review: POST /repos/acme/shop/pulls/1/reviews: HTTP 422");
}, 30_000);
