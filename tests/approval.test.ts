import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { type FauxResponseStep, fauxAssistantMessage } from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { AppStore } from "../src/persistence/store.ts";
import { type App, finishReview, openApp, startReview } from "../src/runtime/app.ts";
import { approve, cancel, Refused, retry } from "../src/runtime/commands.ts";
import { REVIEW_TASK_NAME } from "../src/runtime/task.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, type Fixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

let fake: FakeGitHub;
let fixture: Fixture;
const apps: App[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fake.setPull(fixture.pull);
});
afterEach(async () => {
  for (const app of apps.splice(0)) await app.close();
  await fake.close();
});

const config = { ...DEFAULT_CONFIG, review: { ...DEFAULT_CONFIG.review, publish_mode: "approval" as const, retry_backoff_ms: 10 }, github: { ...DEFAULT_CONFIG.github, timeout_ms: 300 } };
const client = () => new GitHubClient({ auth: { kind: "token", token: "ghp_test" }, baseUrl: fake.url, maxRetries: 2, backoffMs: 1, concurrency: 2, timeoutMs: 5000 });

async function openApproving(responses: FauxResponseStep[] = [readDiff(), submitDefect(fixture)], dataDir = mkdtempSync(join(tmpdir(), "approval-"))) {
  const github = client();
  const pulls = new GitHubPullSource(() => github);
  const app = await openApp({ dataDir, config, pulls, github: () => github, engine: { skipped: "test" }, models: fauxModels(responses), model: { provider: "faux", modelId: "reviewer" }, reviewerVersion: "m1+faux/reviewer" });
  apps.push(app);
  app.harness.resume();
  return { app, pulls, dataDir };
}

/** Review the fixture PR up to AWAITING_APPROVAL. */
async function park(h: Awaited<ReturnType<typeof openApproving>>) {
  const revision = await finishReview(h.app, await startReview(h.app, fixture.pull, "1", { publish: true }));
  expect([revision.state, fake.postAttempts]).toEqual(["AWAITING_APPROVAL", 0]);
  return { revision, outbox: h.app.store.outboxForRevision(revision.id)! };
}

const transitions = (app: App, id: string) => app.store.auditEvents(id).map((e) => `${e.from_state} ${e.event} ${e.to_state}`);
const refused = async (work: Promise<unknown>) => {
  try {
    await work;
  } catch (error) {
    if (error instanceof Refused) return [error.code, error.message];
    throw error;
  }
  throw new Error("not refused");
};
const taskCount = (app: App) => app.harness.commit(async (tx) => (await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100)).items.map((t) => t.state.status), BACKGROUND_CONTEXT);

it("parks a review in AWAITING_APPROVAL with the payload stored and nothing posted, then posts once on approval", async () => {
  const h = await openApproving();
  const { revision, outbox } = await park(h);
  expect(outbox.status).toBe("pending");
  expect(JSON.parse(outbox.payload_json)).toMatchObject({ commit_id: fixture.headSha, event: "COMMENT", comments: [{ path: "src/routes/orders.ts", line: 5 }] });
  expect(transitions(h.app, revision.id).slice(4)).toEqual(["PREPARE_PUBLICATION APPROVAL_REQUIRED AWAITING_APPROVAL"]);
  // Starting the same head again is a no-op: the parked revision is the answer.
  expect((await startReview(h.app, fixture.pull, "1", { publish: true })).taskId).toBe(undefined);

  expect(await refused(approve(h.app, h.pulls, { revisionId: revision.id, previewId: "not-the-hash" }, "test"))).toEqual(["CONFLICT", `preview not-the-hash is not the stored payload ${outbox.payload_hash}`]);
  expect(await refused(approve(h.app, h.pulls, { revisionId: "nope", previewId: outbox.payload_hash }, "test"))).toEqual(["NOT_FOUND", "no run nope"]);
  expect(await refused(approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash, expectedHeadSha: "0".repeat(40) }, "test"))).toEqual([
    "REVISION_STALE",
    `revision ${revision.id} reviewed ${fixture.headSha}, not ${"0".repeat(40)}`,
  ]);
  expect(fake.postAttempts).toBe(0);

  const started = await approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash, expectedHeadSha: fixture.headSha }, "test");
  const done = await finishReview(h.app, started);
  expect([done.state, h.app.store.outboxForRevision(revision.id)!.status, fake.postAttempts, fake.reviews.length]).toEqual(["COMPLETED", "published", 1, 1]);
  expect(fake.reviews[0]!.body).toContain(`<!-- durable-review:${revision.id}:${outbox.payload_hash} -->`);
  expect(transitions(h.app, revision.id).slice(4)).toEqual([
    "PREPARE_PUBLICATION APPROVAL_REQUIRED AWAITING_APPROVAL",
    "AWAITING_APPROVAL APPROVED PUBLISH",
    "PUBLISH REVIEW_RECONCILED COMPLETED",
  ]);
  expect(h.app.store.auditEvents(revision.id).find((e) => e.event === "APPROVED")!.detail).toBe(`approved by test (preview ${outbox.payload_hash})`);
  expect(await taskCount(h.app)).toEqual(["terminal", "terminal"]);
  // A second approval finds nothing to approve, and never a second POST.
  expect(await refused(approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash }, "test"))).toEqual(["INVALID_TRANSITION", "APPROVED is not legal in COMPLETED"]);
  expect(fake.postAttempts).toBe(1);
}, 30_000);

it("supersedes a parked revision whose head moved or whose PR closed, with zero POSTs", async () => {
  const h = await openApproving();
  const { revision, outbox } = await park(h);
  fake.setPull({ ...fixture.pull, headSha: "f".repeat(40) });
  expect(await refused(approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash, expectedHeadSha: fixture.headSha }, "test"))).toEqual([
    "REVISION_STALE",
    `the head of acme/shop#1 is now ${"f".repeat(40)}; revision ${revision.id} is superseded`,
  ]);
  expect([h.app.store.revision(revision.id)!.state, h.app.store.outbox(outbox.id)!.status, fake.postAttempts]).toEqual(["SUPERSEDED", "failed", 0]);
  expect(await refused(approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash }, "test"))).toEqual(["INVALID_TRANSITION", "APPROVED is not legal in SUPERSEDED"]);

  const other = await openApproving(undefined, mkdtempSync(join(tmpdir(), "approval-closed-")));
  fake.setPull(fixture.pull);
  const parked = await park(other);
  fake.setPull({ ...fixture.pull, state: "closed", merged: true });
  expect(await refused(approve(other.app, other.pulls, { revisionId: parked.revision.id, previewId: parked.outbox.payload_hash }, "test"))).toEqual([
    "REVISION_STALE",
    `acme/shop#1 is merged; revision ${parked.revision.id} is superseded`,
  ]);
  expect(other.app.store.auditEvents(parked.revision.id).at(-1)).toMatchObject({ event: "PR_NOT_OPEN", to_state: "SUPERSEDED", detail: "pr_not_open: merged" });
  expect(fake.postAttempts).toBe(0);
}, 30_000);

it("cancels a parked revision, reopens it with retry as a fresh attempt, and approves that attempt once", async () => {
  const h = await openApproving([readDiff(), submitDefect(fixture), readDiff(), submitDefect(fixture)]);
  const { revision, outbox } = await park(h);
  await cancel(h.app, revision.id, "cancelled from the TUI");
  expect(h.app.store.auditEvents(revision.id).at(-1)).toMatchObject({ event: "CANCELLED", to_state: "FAILED", detail: "cancelled from the TUI" });
  expect(h.app.store.outbox(outbox.id)!.status).toBe("failed");
  expect(await refused(cancel(h.app, revision.id, "again"))).toEqual(["INVALID_TRANSITION", "CANCELLED is not legal in FAILED"]);

  const reopened = await finishReview(h.app, await retry(h.app, revision.id, "tui via daemon"));
  expect([reopened.state, h.app.store.outbox(outbox.id)!.status, h.app.store.reopenings(revision.id)]).toEqual(["AWAITING_APPROVAL", "pending", ["MANUAL_RETRY"]]);
  expect(h.app.store.auditEvents(revision.id).find((e) => e.event === "MANUAL_RETRY")!.detail).toBe("manual override: tui via daemon (retry 1)");
  expect(await refused(retry(h.app, revision.id, "tui via daemon"))).toEqual(["INVALID_TRANSITION", "MANUAL_RETRY is not legal in AWAITING_APPROVAL"]);

  const done = await finishReview(h.app, await approve(h.app, h.pulls, { revisionId: revision.id, previewId: outbox.payload_hash }, "test"));
  expect([done.state, fake.postAttempts, h.app.store.reopenings(revision.id)]).toEqual(["COMPLETED", 1, ["MANUAL_RETRY", "APPROVED"]]);
  expect(await taskCount(h.app)).toEqual(["terminal", "terminal", "terminal"]);
}, 30_000);

it("cancels a running review with the TUI's reason, and refuses while a GitHub write may be in flight", async () => {
  let inFlight!: () => void;
  const reachedAnalyze = new Promise<void>((resolve) => (inFlight = resolve));
  const blocked: FauxResponseStep = (_context, options) =>
    new Promise((resolve) => {
      inFlight();
      options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
    });
  const h = await openApproving([readDiff(), blocked]);
  const started = await startReview(h.app, fixture.pull, "1", { publish: true });
  await reachedAnalyze;
  await cancel(h.app, started.revision.id, "cancelled from the TUI");
  const revision = await finishReview(h.app, started);
  expect([revision.state, h.app.store.auditEvents(revision.id).at(-1)]).toEqual(["FAILED", expect.objectContaining({ from_state: "ANALYZE", event: "TERMINAL_ERROR", detail: "cancelled from the TUI" })]);
  expect(await taskCount(h.app)).toEqual(["terminal"]);

  h.app.store.db.prepare("UPDATE review_revisions SET state = 'PUBLISH' WHERE id = ?").run(revision.id);
  expect(await refused(cancel(h.app, revision.id, "x"))).toEqual(["CONFLICT", "cannot cancel in PUBLISH: a GitHub write may be in flight"]);
  expect(await refused(cancel(h.app, "nope", "x"))).toEqual(["NOT_FOUND", "no run nope"]);
}, 30_000);

const SCRIPT = new URL("./fixtures/cli-publish.ts", import.meta.url).pathname;
function run(env: Record<string, string>): Promise<{ code: number | null; signal: string | null; stderr: string; stdout: string }> {
  const child = spawn(process.execPath, [SCRIPT], { env: { ...process.env, ...env } });
  let stderr = "";
  let stdout = "";
  child.stderr.on("data", (d) => (stderr += d));
  child.stdout.on("data", (d) => (stdout += d));
  return new Promise((resolve) => child.on("exit", (code, signal) => resolve({ code, signal, stderr, stdout })));
}

it("headless: review parks, approve posts once, and a SIGKILL after approval but before the POST recovers with one POST", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "approval-cli-"));
  const configPath = join(dataDir, "config.json");
  writeFileSync(configPath, JSON.stringify({ review: { publish_mode: "approval" }, github: { timeout_ms: 300 } }));
  const env = { FIXTURE_JSON: JSON.stringify(fixture), PR_REVIEW_DATA_DIR: dataDir, FAKE_URL: fake.url, PR_REVIEW_CONFIG: configPath };
  const reviewed = await run(env);
  const hint = /pr-review approve --revision (\w+) --preview (\w+)/.exec(reviewed.stderr);
  expect([reviewed.code, Boolean(hint), fake.postAttempts]).toEqual([0, true, 0]);
  const [, revisionId, preview] = hint!;

  const wrong = await run({ ...env, CLI_ARGS: JSON.stringify(["approve", "--revision", revisionId!, "--preview", "bad"]) });
  expect([wrong.code, wrong.stderr.trim().endsWith(`CONFLICT: preview bad is not the stored payload ${preview}`), fake.postAttempts]).toEqual([1, true, 0]);

  const killed = await run({ ...env, KILL: "before-post", CLI_ARGS: JSON.stringify(["approve", "--revision", revisionId!, "--preview", preview!]) });
  expect(killed.signal).toBe("SIGKILL");
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  expect(store.db.prepare("SELECT r.state, o.status FROM publication_outbox o JOIN review_revisions r ON r.id = o.revision_id").all()).toEqual([{ state: "PUBLISH", status: "sending" }]);
  store.close();
  rmSync(join(dataDir, "lock"), { recursive: true });

  const again = await run({ ...env, CLI_ARGS: JSON.stringify(["approve", "--revision", revisionId!, "--preview", preview!]) });
  expect([again.code, again.stderr]).toEqual([0, expect.stringContaining("was already approved; waiting for its publication")]);
  expect(again.stdout.trim()).toBe("publication published: review 1000; revision COMPLETED");
  const after = AppStore.open(join(dataDir, "app.sqlite"));
  const outbox = after.outboxForRevision(revisionId!)!;
  expect(after.outboxEvents(outbox.id).map((e) => e.event)).toEqual(["SEND", "UNCERTAIN", "ABSENT", "SEND", "SENT"]);
  expect(after.reopenings(revisionId!)).toEqual(["APPROVED"]);
  after.close();
  expect([fake.postAttempts, fake.reviews.map((r) => r.id)]).toEqual([1, [1000]]);

  // The CLI's own review of that head, started afterwards, shows the stored result instead of starting over.
  const errors: string[] = [];
  process.env.PR_REVIEW_DATA_DIR = dataDir;
  process.env.PR_REVIEW_CONFIG = configPath;
  try {
    const deps = { github: client(), models: fauxModels([]), isTTY: false, out: () => {}, err: (t: string) => errors.push(t) };
    expect(await main(["review", "https://github.com/acme/shop/pull/1", "--publish", "--no-jev", "--model", "faux/reviewer"], deps)).toBe(0);
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
    delete process.env.PR_REVIEW_CONFIG;
  }
  expect(errors.join("")).toContain(`revision ${revisionId} already COMPLETED; showing the stored result`);
  expect(fake.postAttempts).toBe(1);
}, 90_000);
