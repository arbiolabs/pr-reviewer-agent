import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { type FauxResponseStep, fauxAssistantMessage } from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, it } from "vitest";
import { type Config, DEFAULT_CONFIG } from "../src/config/config.ts";
import { COMMANDS_OFF, openSource } from "../src/control/daemon.ts";
import { type EngineOptions, EngineSource } from "../src/control/engine.ts";
import { startControlServer } from "../src/control/server.ts";
import type { ControlSource } from "../src/control/source.ts";
import type { Env } from "../src/github/accounts.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { AppStore } from "../src/persistence/store.ts";
import { type Command, idempotencyKey, type ServerMessage } from "../src/protocol/v1.ts";
import { type App, openApp } from "../src/runtime/app.ts";
import { REVIEW_TASK_NAME } from "../src/runtime/task.ts";
import { produced, TestClient } from "./control-client.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, type Fixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

let fake: FakeGitHub;
let fixture: Fixture;
const cleanup: (() => Promise<void> | void)[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fake.setPull(fixture.pull);
});
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
  await fake.close();
});

const approvalConfig: Config = { ...DEFAULT_CONFIG, review: { ...DEFAULT_CONFIG.review, publish_mode: "approval", retry_backoff_ms: 10 }, github: { ...DEFAULT_CONFIG.github, timeout_ms: 300 } };
const newClient = () => new GitHubClient({ auth: { kind: "token", token: "ghp_test" }, baseUrl: fake.url, maxRetries: 2, backoffMs: 1, concurrency: 2, timeoutMs: 5000 });

type Hosted = { app: App; source: EngineSource; dataDir: string; client: GitHubClient };

/** An App on the fake GitHub, hosted by an EngineSource (not yet served). */
async function host(
  options: { config?: Config; responses?: FauxResponseStep[]; dataDir?: string; client?: GitHubClient; github?: (installationId: number) => GitHubClient; env?: Env; modelIds?: string[]; model?: EngineOptions["model"] } = {},
): Promise<Hosted> {
  const config = options.config ?? approvalConfig;
  const client = options.client ?? newClient();
  const pulls = new GitHubPullSource(() => client);
  const dataDir = options.dataDir ?? mkdtempSync(join(tmpdir(), "engine-"));
  const models = fauxModels(options.responses ?? [readDiff(), submitDefect(fixture)], options.modelIds);
  const app = await openApp({
    dataDir,
    config,
    pulls,
    github: () => client,
    engine: async () => ({ skipped: "test" }),
    models,
    reviewerVersion: "m1+faux/reviewer",
    lockWaitMs: 0,
  });
  // No gh on PATH: journal sequences are pinned, and a detected account would add a row.
  const source = await EngineSource.host(dataDir, app, {
    config,
    pulls,
    github: options.github ?? (() => client),
    env: options.env ?? { PATH: "" },
    account: "github.com/octo",
    models,
    model: options.model ?? { ref: "faux/reviewer", source: "config" },
  });
  cleanup.push(() => source.close());
  return { app, source, dataDir, client };
}

/** A data dir whose startup recovery needs a GitHub client: a failed revision with an outbox GitHub may have received. */
function seedUncertainOutbox(dataDir: string): void {
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  const prId = store.upsertPull(fixture.pull);
  const rev = store.ensureRevision({ prId, baseSha: fixture.pull.baseSha, headSha: fixture.headSha, policyVersion: "1+publish", reviewerVersion: "m1+faux/reviewer" });
  for (const event of ["CONTEXT_PERSISTED", "ROUTING_PERSISTED", "CANDIDATES_PERSISTED", "CANDIDATES_DISPOSED", "HEAD_VERIFIED"] as const) store.advanceFromCurrent(rev.id, event);
  store.advanceOutbox(store.ensureOutbox({ revisionId: rev.id, idempotencyKey: `${rev.id}:1`, payloadHash: "p", payloadJson: "{}", inlineFindingIds: [] }).id, "SEND");
  store.advanceFromCurrent(rev.id, "TERMINAL_ERROR", "daemon stopped mid-POST");
  store.close();
}

async function serve(source: ControlSource) {
  const server = await startControlServer({ socketPath: join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock"), source, pollMs: 20 });
  cleanup.push(() => server.close());
  const c = await TestClient.connect(server.path);
  cleanup.push(() => c.close());
  const welcome = await c.hello();
  return { c, welcome };
}

type Ev = ReturnType<TestClient["events"]>[number];
const describeEvent = (e: Ev) => {
  const d = e.data as Record<string, unknown>;
  switch (e.eventType) {
    case "review.phase_changed":
      return `phase ${d.from} -> ${d.to}`;
    case "review.publication_changed":
      return `publication ${d.status}${d.reviewId ? ` review ${d.reviewId}` : ""}`;
    case "tool.activity":
      return `tool ${d.tool}`;
    case "finding.updated":
      return `finding ${d.title}`;
    case "pr.updated":
      return `pr ${d.repository}#${d.number}`;
    default:
      return `${e.eventType} ${d.phase}${d.failureReason ? ` (${d.failureReason})` : ""}`;
  }
};
// These tests follow the review's flow; control-appdb.test.ts checks the progress lines between its steps.
const PROGRESS = new Set(["phase", "github", "git", "jev", "route", "model", "verify", "prepare", "submit_candidates"]);
const flow = (events: Ev[]) => events.filter((e) => !(e.eventType === "tool.activity" && PROGRESS.has((e.data as { tool: string }).tool)));
const chained = (events: Ev[], from: number) => events.map((e) => e.prevSequence).every((p, i) => p === (i === 0 ? from : events[i - 1]!.sequence));

let requests = 0;
type Result = Extract<ServerMessage, { type: "command.result" }>;
/** Send a command with the key the protocol fixes for it, and wait for its result. */
async function command(c: TestClient, cmd: Omit<Command, "type" | "requestId" | "idempotencyKey">, key?: string): Promise<Result> {
  const requestId = `cmd-${++requests}`;
  const message = { type: "command", requestId, idempotencyKey: "", ...cmd } as Command;
  message.idempotencyKey = key ?? idempotencyKey(message);
  c.send(message);
  return c.waitFor<Result>((m) => m.type === "command.result" && m.requestId === requestId);
}
/** A replay carries a fresh requestId; the operation is what must match. */
const operation = ({ requestId: _, ...rest }: Result) => rest;
const rejected = (code: string, message: string | RegExp) => expect.objectContaining({ status: "rejected", code, message: typeof message === "string" ? message : expect.stringMatching(message) });
const taskStatuses = (app: App) => app.harness.commit(async (tx) => (await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100)).items.map((t) => t.state.status), BACKGROUND_CONTEXT);

const UP_TO_APPROVAL = [
  "pr acme/shop#1",
  "run.started COLLECT_CONTEXT",
  "phase COLLECT_CONTEXT -> TRIAGE",
  "phase TRIAGE -> ANALYZE",
  "tool read_diff",
  "phase ANALYZE -> VERIFY",
  "finding Order deletion endpoint skips the admin check",
  "phase VERIFY -> PREPARE_PUBLICATION",
  "phase PREPARE_PUBLICATION -> AWAITING_APPROVAL",
  "publication awaiting_approval",
];

it("is read-only while another pr-review holds the data dir, and grants only the reader commands there", async () => {
  const holder = await host();
  const notes: string[] = [];
  const source = await openSource(
    holder.dataDir,
    true,
    { openEngine: async (dataDir) => (await host({ dataDir })).source },
    (t) => notes.push(t),
  );
  cleanup.push(() => source.close());
  expect(source.mode).toBe("read-only");
  expect(notes).toEqual([`pr-review daemon: read-only mode (another pr-review is running on ${holder.dataDir})\n`]);
  const { c, welcome } = await serve(source);
  expect(welcome).toMatchObject({ mode: "read-only", capabilities: ["account.select", "pr.archive", "pr.unarchive"] });
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  expect(await command(c, { command: "review.start", params: { prId: "p", expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("CAPABILITY_DENIED", `review.start is not available: another pr-review is running on ${holder.dataDir}`),
  );
  // The holder is still the engine: its own welcome grants the commands.
  expect((await serve(holder.source)).welcome).toMatchObject({ mode: "engine", capabilities: ["account.select", "pr.archive", "pr.unarchive", "review.add", "review.start", "review.again", "review.retry", "review.cancel", "review.publish", "settings.set"] });
}, 30_000);

it("falls back to read-only with the reason when recovery fails, holding no lock and running no task", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "engine-"));
  seedUncertainOutbox(dataDir);
  const notes: string[] = [];
  const source = await openSource(
    dataDir,
    true,
    {
      openEngine: async (dir) =>
        (
          await host({
            dataDir: dir,
            github: (installationId) => {
              throw new Error(`this PR was recorded without a GitHub App installation (${installationId}); use token auth for it`);
            },
          })
        ).source,
    },
    (t) => notes.push(t),
  );
  cleanup.push(() => source.close());
  const why = "this PR was recorded without a GitHub App installation (0); use token auth for it";
  expect([source.mode, notes]).toEqual(["read-only", [`pr-review daemon: read-only mode (${why})\n`]]);
  const { c, welcome } = await serve(source);
  expect(welcome).toMatchObject({ mode: "read-only", readOnlyReason: why, capabilities: ["account.select", "pr.archive", "pr.unarchive"] });
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  expect(await command(c, { command: "review.retry", params: { runId: "r" } })).toEqual(rejected("CAPABILITY_DENIED", `review.retry is not available: ${why}`));
  // The failed engine released the data dir: a CLI review can open it at once, and finds nothing running.
  const cli = await openApp({
    dataDir,
    config: approvalConfig,
    pulls: new GitHubPullSource(newClient),
    engine: async () => ({ skipped: "test" }),
    models: fauxModels([]),
    reviewerVersion: "m1+faux/reviewer",
    lockWaitMs: 0,
  });
  cleanup.push(() => cli.close());
  expect(await taskStatuses(cli)).toEqual([]);
}, 30_000);

it("is read-only by default: no lock, no engine, the reader commands only; the review commands need --allow-commands, and review.publish approval mode", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "engine-"));
  seedUncertainOutbox(dataDir);
  let engines = 0;
  const notes: string[] = [];
  const off = await openSource(dataDir, false, { openEngine: async (dir) => (engines++, (await host({ dataDir: dir })).source) }, (t) => notes.push(t));
  cleanup.push(() => off.close());
  const { c, welcome } = await serve(off);
  expect([engines, notes, welcome]).toEqual([0, [], expect.objectContaining({ mode: "read-only", readOnlyReason: COMMANDS_OFF, capabilities: ["account.select", "pr.archive", "pr.unarchive"] })]);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  expect(await command(c, { command: "review.cancel", params: { runId: "r" } })).toEqual(rejected("CAPABILITY_DENIED", `review.cancel is not available: ${COMMANDS_OFF}`));
  // Nothing holds the data dir, and the uncertain outbox was not reconciled: a CLI review opens it at once and finds it untouched.
  const cli = await openApp({
    dataDir,
    config: approvalConfig,
    pulls: new GitHubPullSource(newClient),
    engine: async () => ({ skipped: "test" }),
    models: fauxModels([]),
    reviewerVersion: "m1+faux/reviewer",
    lockWaitMs: 0,
  });
  cleanup.push(() => cli.close());
  expect([await taskStatuses(cli), cli.store.uncertainOutboxes().length, fake.listCalls]).toEqual([[], 1, 0]);

  const comment = await host({ config: { ...approvalConfig, review: { ...approvalConfig.review, publish_mode: "comment" } } });
  const served = await serve(comment.source);
  expect(served.welcome).toMatchObject({ mode: "engine", capabilities: ["account.select", "pr.archive", "pr.unarchive", "review.add", "review.start", "review.again", "review.retry", "review.cancel", "settings.set"] });
  expect(served.welcome).not.toHaveProperty("readOnlyReason");
  served.c.send({ type: "snapshot.get", requestId: "s" });
  await served.c.waitFor((m) => m.type === "snapshot");
  expect(await command(served.c, { command: "review.publish", params: { runId: "r", previewId: "p", expectedHeadSha: "h" } })).toEqual(
    rejected("CAPABILITY_DENIED", 'review.publish needs review.publish_mode "approval" (it is "comment")'),
  );
}, 30_000);

it("approves a review end to end over the socket: parked, previewed, published once, replay-safe, streamed without gaps", async () => {
  const h = await host();
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  const snap = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  // The title comes from GitHub with the PR, before any review has collected context.
  expect(snap.prs.map((p) => p.title)).toEqual([fixture.pull.title]);

  expect(await command(c, { command: "review.start", params: { prId: "nope", expectedHeadSha: fixture.headSha } })).toEqual(rejected("NOT_FOUND", "no PR nope"));
  expect(await command(c, { command: "review.start", params: { prId, expectedHeadSha: "0".repeat(40) } })).toEqual(rejected("REVISION_STALE", `the head of acme/shop#1 is now ${fixture.headSha}`));
  expect(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }, "k")).toEqual(rejected("INVALID_REQUEST", `idempotencyKey must be review.start:${prId}:${fixture.headSha}`));
  expect(await command(c, { command: "review.retry", params: { runId: "nope" } })).toEqual(rejected("NOT_FOUND", "no run nope"));
  expect(await command(c, { command: "review.publish", params: { runId: "nope", previewId: "p", expectedHeadSha: fixture.headSha } })).toEqual(rejected("NOT_FOUND", "no run nope"));
  expect(await taskStatuses(h.app)).toEqual([]);

  const started = await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } });
  expect(started).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.start:[0-9a-f]{32}$/) });
  const runId = (started as { operationId: string }).operationId.slice("review.start:".length);
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.publication_changed" && (m.data as { status: string }).status === "awaiting_approval");
  expect(flow(c.events()).map(describeEvent)).toEqual(UP_TO_APPROVAL);
  expect(chained(c.events(), snap.sequence)).toBe(true);
  expect(c.events().filter((e) => e.eventType === "pr.updated").map((e) => (e.data as { title: string }).title)).toEqual([fixture.pull.title]);
  expect([h.app.store.revision(runId)!.state, fake.postAttempts]).toEqual(["AWAITING_APPROVAL", 0]);

  // A replayed start answers the same operation; a running or parked head cannot be started, retried or cancelled wrongly.
  expect(operation(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }))).toEqual(operation(started));
  expect(await taskStatuses(h.app)).toEqual(["terminal"]);
  expect(await command(c, { command: "review.retry", params: { runId } })).toEqual(rejected("INVALID_TRANSITION", "MANUAL_RETRY is not legal in AWAITING_APPROVAL"));

  c.send({ type: "publication.get", requestId: "pub", runId });
  const preview = await c.waitFor<Extract<ServerMessage, { type: "publication.preview" }>>((m) => m.type === "publication.preview");
  const outbox = h.app.store.outboxForRevision(runId)!;
  const payload = JSON.parse(outbox.payload_json) as { body: string; comments: unknown[] };
  expect(preview).toEqual({
    type: "publication.preview",
    requestId: "pub",
    runId,
    previewId: outbox.payload_hash,
    headSha: fixture.headSha,
    body: payload.body,
    comments: payload.comments,
    event: "COMMENT",
    status: "awaiting_approval",
  });
  expect(preview.comments).toEqual([{ path: "src/routes/orders.ts", line: 5, side: "RIGHT", body: expect.stringContaining("**High: Order deletion endpoint skips the admin check**") }]);
  c.send({ type: "publication.get", requestId: "pub404", runId: "nope" });
  expect(await c.waitFor((m) => m.type === "error" && m.requestId === "pub404")).toMatchObject({ code: "NOT_FOUND", message: "no stored publication for run nope" });

  expect(await command(c, { command: "review.publish", params: { runId, previewId: "stale-preview", expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("CONFLICT", `preview stale-preview is not the stored payload ${outbox.payload_hash}`),
  );
  expect(await command(c, { command: "review.publish", params: { runId, previewId: outbox.payload_hash, expectedHeadSha: "1".repeat(40) } })).toEqual(
    rejected("REVISION_STALE", `revision ${runId} reviewed ${fixture.headSha}, not ${"1".repeat(40)}`),
  );
  expect(fake.postAttempts).toBe(0);

  const approvalSeq = h.app.store.db.prepare("SELECT max(seq) AS n FROM audit_events").get() as { n: number };
  const approved = await command(c, { command: "review.publish", params: { runId, previewId: outbox.payload_hash, expectedHeadSha: fixture.headSha } });
  expect(approved).toMatchObject({ status: "accepted", operationId: `review.publish:${runId}` });
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
  expect(flow(c.events()).map(describeEvent).slice(UP_TO_APPROVAL.length)).toEqual([
    "phase AWAITING_APPROVAL -> PUBLISH",
    "publication publishing",
    "publication published review 1000",
    "finding Order deletion endpoint skips the admin check",
    "review.completed COMPLETED",
  ]);
  expect(chained(c.events(), snap.sequence)).toBe(true);
  const published = c.events().find((e) => e.eventType === "review.publication_changed" && (e.data as { status: string }).status === "published")!;
  expect(published.data).toEqual({ runId, status: "published", reviewId: 1000, url: "https://github.com/acme/shop/pull/1#pullrequestreview-1000" });
  expect([fake.postAttempts, fake.reviews.length, h.app.store.revision(runId)!.state]).toEqual([1, 1, "COMPLETED"]);
  // A replay after publication carries what was true at each row: no review URL before the review existed.
  expect(
    h.source
      .eventsAfter(snap.sequence, 100)
      .filter((e) => e.eventType === "review.publication_changed")
      .map((e) => e.data),
  ).toEqual([
    { runId, status: "awaiting_approval" },
    { runId, status: "publishing" },
    { runId, status: "published", reviewId: 1000, url: "https://github.com/acme/shop/pull/1#pullrequestreview-1000" },
  ]);
  const findingId = (c.events().findLast((e) => e.eventType === "finding.updated")!.data as { id: string }).id;
  // Each finding row carries the status as of that row: detected at VERIFY, published at PUBLISH.
  expect(
    h.source
      .eventsAfter(0, 1000)
      .filter((e) => e.eventType === "finding.updated" && (e.data as { id: string }).id === findingId)
      .map((e) => (e.data as { status: string }).status),
  ).toEqual(["DETECTED", "PUBLISHED"]);
  expect((await h.source.finding(findingId)).history.map(({ runId: run, disposition }) => [run, disposition])).toEqual([
    [runId, "DETECTED"],
    [runId, "PUBLISHED"],
  ]);
  // The journal follows causality: the publisher's rows, then the run's end (its progress lines aside).
  expect(h.app.store.db.prepare("SELECT event, to_state, detail FROM audit_events WHERE revision_id = ? AND seq > ? AND event != 'TOOL_ACTIVITY' ORDER BY seq").all(runId, approvalSeq.n)).toEqual([
    { event: "APPROVED", to_state: "PUBLISH", detail: expect.stringMatching(/^approved by /) },
    { event: "PUBLICATION_CHANGED", to_state: "PUBLISH", detail: "publishing" },
    { event: "PUBLICATION_CHANGED", to_state: "PUBLISH", detail: "published" },
    { event: "FINDING_UPDATED", to_state: "PUBLISH", detail: expect.any(String) },
    { event: "REVIEW_RECONCILED", to_state: "COMPLETED", detail: "review 1000 posted" },
  ]);

  // The replayed approval is the recorded one: same answer, no second attempt, no second POST.
  expect(operation(await command(c, { command: "review.publish", params: { runId, previewId: outbox.payload_hash, expectedHeadSha: fixture.headSha } }))).toEqual(operation(approved));
  expect([fake.postAttempts, await taskStatuses(h.app)]).toEqual([1, ["terminal", "terminal"]]);
  expect(h.app.store.db.prepare("SELECT idempotency_key, command FROM control_commands ORDER BY created_at").all()).toEqual([
    { idempotency_key: `review.start:${prId}:${fixture.headSha}`, command: "review.start" },
    { idempotency_key: `review.publish:${runId}:${outbox.payload_hash}`, command: "review.publish" },
  ]);
  // The start's own key replays its recorded result after the run finished: a true replay, not a new start.
  expect(operation(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }))).toEqual(operation(started));
  expect(await taskStatuses(h.app)).toEqual(["terminal", "terminal"]);
  // With GitHub unreachable the replay still answers with its recorded result.
  fake.pulls.delete(`${fixture.pull.fullName}#${fixture.pull.number}`);
  expect(operation(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }))).toEqual(operation(started));
  fake.setPull(fixture.pull);
  // Once the base moves, the same key starts a new revision on it instead of replaying the reviewed one.
  fake.setPull({ ...fixture.pull, baseSha: "e".repeat(40) });
  const rebased = await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } });
  expect(rebased).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.start:[0-9a-f]{32}$/) });
  expect((rebased as { operationId: string }).operationId).not.toBe((started as { operationId: string }).operationId);
  const rebasedId = (rebased as { operationId: string }).operationId.slice("review.start:".length);
  expect(h.app.store.revision(rebasedId)?.base_sha).toBe("e".repeat(40));
  expect(await command(c, { command: "review.cancel", params: { runId: rebasedId } })).toMatchObject({ status: "accepted" });
  fake.setPull(fixture.pull);
  // A key that never started this revision (the head was reviewed some other way) is refused in plain words.
  h.app.store.db.prepare("DELETE FROM control_commands WHERE command = 'review.start'").run();
  expect(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("CONFLICT", `#1 at ${fixture.headSha.slice(0, 7)} is already reviewed`),
  );
  c.send({ type: "publication.get", requestId: "pub2", runId });
  // A client that lost the publication events to a resync gets the review back from the preview.
  expect(await c.waitFor((m) => m.type === "publication.preview" && m.requestId === "pub2")).toMatchObject({
    status: "published",
    reviewId: 1000,
    url: "https://github.com/acme/shop/pull/1#pullrequestreview-1000",
  });
}, 60_000);

it("supersedes a parked review when the head moves before approval, with nothing posted", async () => {
  const h = await host();
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const runId = started.operationId.slice("review.start:".length);
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL");
  const previewId = h.app.store.outboxForRevision(runId)!.payload_hash;
  // A start for the parked head: its own key replays the run, any other key is refused in plain words.
  expect(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })).toMatchObject({ operationId: started.operationId });
  const moveStartKey = (from: string, to: string) => h.app.store.db.prepare("UPDATE control_commands SET idempotency_key = ? WHERE idempotency_key = ?").run(to, from);
  moveStartKey(`review.start:${prId}:${fixture.headSha}`, "elsewhere");
  expect(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("CONFLICT", `#1 at ${fixture.headSha.slice(0, 7)} is waiting for your approval`),
  );
  moveStartKey("elsewhere", `review.start:${prId}:${fixture.headSha}`);

  fake.setPull({ ...fixture.pull, headSha: "f".repeat(40) });
  expect(await command(c, { command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("REVISION_STALE", `the head of acme/shop#1 is now ${"f".repeat(40)}; revision ${runId} is superseded`),
  );
  await c.waitFor((m) => m.type === "event" && m.eventType === "run.superseded");
  expect(flow(c.events()).map(describeEvent).slice(-2)).toEqual(["publication failed", "run.superseded SUPERSEDED"]);
  expect(await command(c, { command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } })).toEqual(rejected("INVALID_TRANSITION", "APPROVED is not legal in SUPERSEDED"));
  expect([fake.postAttempts, h.app.store.outboxForRevision(runId)!.status]).toEqual([0, "failed"]);
  // The old head's start replays as recorded; the new head starts a new run.
  expect(operation(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }))).toEqual(operation(started as Result));
  const next = await command(c, { command: "review.start", params: { prId, expectedHeadSha: "f".repeat(40) } });
  expect(next).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.start:[0-9a-f]{32}$/) });
  expect((next as { operationId: string }).operationId).not.toBe(started.operationId);
}, 60_000);

it("review.again sets a parked review aside and reviews the head anew; replay-safe, refused while a review runs or a publication settles", async () => {
  const reached: (() => void)[] = [];
  let release = () => {};
  // The second run holds in ANALYZE until released, then reports the same defect.
  const held: FauxResponseStep = () =>
    new Promise((resolve) => {
      reached.shift()!();
      release = () => resolve(submitDefect(fixture));
    });
  const h = await host({ responses: [readDiff(), submitDefect(fixture), readDiff(), held, readDiff(), submitDefect(fixture)] });
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const parked = (from: number) => c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL", from);
  const again = () => command(c, { command: "review.again", params: { prId, expectedHeadSha: fixture.headSha } });
  const head = fixture.headSha.slice(0, 7);

  expect(await command(c, { command: "review.again", params: { prId, expectedHeadSha: "0".repeat(40) } })).toEqual(rejected("REVISION_STALE", `the head of acme/shop#1 is now ${fixture.headSha}`));
  expect(await again()).toEqual(rejected("CONFLICT", `#1 at ${head} has no review yet; start it instead`));
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const first = started.operationId.slice("review.start:".length);
  await parked(0);
  const outbox = h.app.store.outboxForRevision(first)!;

  const inAnalyze = new Promise<void>((resolve) => reached.push(resolve));
  const mark = c.messages.length;
  const accepted = await again();
  expect(accepted).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.again:[0-9a-f]{32}$/) });
  const second = (accepted as { operationId: string }).operationId.slice("review.again:".length);
  expect(second).not.toBe(first);
  await inAnalyze;
  // The parked payload was never sent: it is abandoned with its run, which reports superseded before the new run starts.
  expect([h.app.store.revision(first)!.state, h.app.store.outboxForRevision(first)!.status, fake.postAttempts]).toEqual(["SUPERSEDED", "failed", 0]);
  const lifecycle = c.messages.slice(mark).filter((m) => m.type === "event" && ["run.superseded", "run.started"].includes(m.eventType));
  expect(lifecycle.map((m) => `${(m as { eventType: string }).eventType} ${(m as { data: { id: string } }).data.id}`)).toEqual([`run.superseded ${first}`, `run.started ${second}`]);
  expect(h.app.store.auditEvents(first).at(-1)).toMatchObject({ event: "REVIEW_AGAIN", detail: "manual override: tui via daemon (review again 1)" });
  // Its own key replays while the new run is in flight; under another key it is refused, as is a start's approval of the old run.
  expect(operation(await again())).toEqual(operation(accepted));
  expect((await taskStatuses(h.app)).length).toBe(2);
  h.app.store.db.prepare("DELETE FROM control_commands WHERE command = 'review.again'").run();
  expect(await again()).toEqual(rejected("CONFLICT", "#1 is being reviewed (ANALYZE); wait for it or cancel it"));
  expect(await command(c, { command: "review.publish", params: { runId: first, previewId: outbox.payload_hash, expectedHeadSha: fixture.headSha } })).toEqual(
    rejected("INVALID_TRANSITION", "APPROVED is not legal in SUPERSEDED"),
  );

  release();
  await parked(mark);
  const rerun = h.app.store.revision(second)!;
  expect([rerun.head_sha, rerun.policy_version]).toEqual([fixture.headSha, "1+again1+publish"]);
  expect(h.app.store.auditEvents(second).map((e) => e.event)).toEqual(["CONTEXT_PERSISTED", "ROUTING_PERSISTED", "CANDIDATES_PERSISTED", "CANDIDATES_DISPOSED", "APPROVAL_REQUIRED"]);
  expect(h.app.store.outboxForRevision(second)!.id).not.toBe(outbox.id);
  expect(h.app.store.countFindings(prId)).toBe(1);

  // A publication that may have reached GitHub holds a review again until it settles.
  const setOutbox = (status: string) => h.app.store.db.prepare("UPDATE publication_outbox SET status = ? WHERE revision_id = ?").run(status, second);
  h.app.store.db.prepare("UPDATE review_revisions SET state = 'PUBLISH' WHERE id = ?").run(second);
  setOutbox("sending");
  expect(await again()).toEqual(rejected("CONFLICT", "#1 is being reviewed (PUBLISH); wait for it or cancel it"));
  h.app.store.db.prepare("UPDATE review_revisions SET state = 'FAILED' WHERE id = ?").run(second);
  setOutbox("needs_reconciliation");
  expect(await again()).toEqual(rejected("CONFLICT", `#1 at ${head} has a publication that may have reached GitHub; reconcile revision ${second} first`));
  h.app.store.db.prepare("UPDATE review_revisions SET state = 'AWAITING_APPROVAL' WHERE id = ?").run(second);
  setOutbox("pending");

  // Once the run it opened is idle, the same key is a new request: the head is reviewed a third time.
  const before = c.messages.length;
  const third = await again();
  expect(third).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.again:[0-9a-f]{32}$/) });
  await parked(before);
  const thirdId = (third as { operationId: string }).operationId.slice("review.again:".length);
  expect([second, thirdId].map((id) => h.app.store.revision(id)!.state)).toEqual(["SUPERSEDED", "AWAITING_APPROVAL"]);
  expect([h.app.store.revision(thirdId)!.policy_version, h.app.store.countFindings(prId), fake.postAttempts]).toEqual(["1+again2+publish", 1, 0]);
}, 60_000);

it("binds each key to one attempt: cancel, retry and approve act again on a later attempt and replay within one", async () => {
  const reached: (() => void)[] = [];
  const blocked: FauxResponseStep = (_context, options) =>
    new Promise((resolve) => {
      reached.shift()!();
      options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
    });
  const analyzing = () => new Promise<void>((resolve) => reached.push(resolve));
  // Attempts 0 and 1 are cancelled mid-ANALYZE; attempt 2 submits; the retries after that reuse its candidates.
  const h = await host({ responses: [readDiff(), blocked, readDiff(), blocked, readDiff(), submitDefect(fixture)] });
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const failures = () => c.events().filter((e) => e.eventType === "run.failed").map((e) => (e.data as { failureReason: string }).failureReason);
  const failed = (n: number) => c.waitFor(() => failures().length >= n, 0, 5000);
  const parked = (from: number) => c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL", from);
  const cancel = () => command(c, { command: "review.cancel", params: { runId } });
  const retry = () => command(c, { command: "review.retry", params: { runId } });
  const publish = () => command(c, { command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } });

  let inAnalyze = analyzing();
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const runId = started.operationId.slice("review.start:".length);
  let previewId = "";
  await inAnalyze;
  // A start for the running head under a key that never started it attaches to the run: no second task.
  const startKey = `review.start:${prId}:${fixture.headSha}`;
  h.app.store.db.prepare("UPDATE control_commands SET idempotency_key = 'elsewhere' WHERE idempotency_key = ?").run(startKey);
  expect(operation(await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } }))).toEqual(operation(started as Result));
  expect((await taskStatuses(h.app)).length).toBe(1);
  const cancelled = await cancel();
  expect(cancelled).toEqual({ type: "command.result", requestId: expect.any(String), status: "accepted", operationId: `review.cancel:${runId}` });
  await failed(1);
  expect([failures(), await taskStatuses(h.app)]).toEqual([["cancelled from the TUI"], ["terminal"]]);
  expect(operation(await cancel())).toEqual(operation(cancelled));
  expect(await command(c, { command: "review.cancel", params: { runId: "nope" } })).toEqual(rejected("NOT_FOUND", "no run nope"));

  inAnalyze = analyzing();
  const retried = await retry();
  expect(retried).toMatchObject({ status: "accepted", operationId: `review.retry:${runId}` });
  await inAnalyze;
  expect(h.app.store.auditEvents(runId).find((e) => e.event === "MANUAL_RETRY")!.detail).toBe("manual override: tui via daemon (retry 1)");
  expect(operation(await retry())).toEqual(operation(retried));
  // The same cancel key now names the retried attempt: it cancels again instead of replaying.
  expect(await cancel()).toMatchObject({ status: "accepted", operationId: `review.cancel:${runId}` });
  await failed(2);
  expect(failures()).toEqual(["cancelled from the TUI", "cancelled from the TUI"]);
  // And the same retry key reopens the twice-failed revision once more.
  let mark = c.messages.length;
  expect(await retry()).toMatchObject({ status: "accepted", operationId: `review.retry:${runId}` });
  await parked(mark);
  expect([h.app.store.reopenings(runId), await taskStatuses(h.app)]).toEqual([["MANUAL_RETRY", "MANUAL_RETRY"], ["terminal", "terminal", "terminal"]]);

  // GitHub rejects the approved POST; the retried attempt parks again on the same payload, and the same approval key posts it.
  previewId = h.app.store.outboxForRevision(runId)!.payload_hash;
  fake.postBehaviors.push("unprocessable");
  const approved = await publish();
  expect(approved).toMatchObject({ status: "accepted", operationId: `review.publish:${runId}` });
  await failed(3);
  expect([fake.postAttempts, fake.reviews.length, h.app.store.outboxForRevision(runId)!.status, failures().at(-1)]).toEqual([1, 0, "failed", expect.stringMatching(/^GitHub rejected the review: /)]);
  expect(operation(await publish())).toEqual(operation(approved));
  mark = c.messages.length;
  expect(await retry()).toMatchObject({ status: "accepted", operationId: `review.retry:${runId}` });
  await parked(mark);
  expect(h.app.store.outboxForRevision(runId)!.payload_hash).toBe(previewId);
  expect(await publish()).toMatchObject({ status: "accepted", operationId: `review.publish:${runId}` });
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
  expect([fake.postAttempts, fake.reviews.length, h.app.store.revision(runId)!.state]).toEqual([2, 1, "COMPLETED"]);
  expect(h.app.store.reopenings(runId)).toEqual(["MANUAL_RETRY", "MANUAL_RETRY", "APPROVED", "MANUAL_RETRY", "APPROVED"]);

  h.app.store.db.prepare("UPDATE review_revisions SET state = 'PUBLISH' WHERE id = ?").run(runId);
  expect(await cancel()).toEqual(rejected("CONFLICT", "cannot cancel in PUBLISH: a GitHub write may be in flight"));
}, 90_000);

it("answers an approval that lost a race to a cancel with command.result CONFLICT naming the current state", async () => {
  const client = newClient();
  const request = client.request.bind(client);
  let armed = false;
  let release: () => void = () => {};
  const blocked = new Promise<void>((resolve) => {
    client.request = (async (method: string, path: string, options?: Parameters<typeof request>[2]) => {
      if (armed && method === "GET" && path.includes("/pulls/")) {
        armed = false;
        resolve();
        await new Promise<void>((r) => (release = r));
      }
      return request(method, path, options);
    }) as typeof client.request;
  });
  const h = await host({ client });
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const runId = started.operationId.slice("review.start:".length);
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL");
  const previewId = h.app.store.outboxForRevision(runId)!.payload_hash;

  armed = true;
  const publish = { type: "command", requestId: "race", command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } } as Command;
  publish.idempotencyKey = idempotencyKey(publish);
  c.send(publish);
  await blocked;
  const second = await serve(h.source);
  second.c.send({ type: "snapshot.get", requestId: "s" });
  await second.c.waitFor((m) => m.type === "snapshot");
  expect(await command(second.c, { command: "review.cancel", params: { runId } })).toMatchObject({ status: "accepted" });
  await second.c.waitFor((m) => m.type === "event" && m.eventType === "run.failed");
  release();
  expect(await c.waitFor((m) => "requestId" in m && m.requestId === "race")).toEqual({
    type: "command.result",
    requestId: "race",
    status: "rejected",
    code: "CONFLICT",
    message: `revision ${runId} is FAILED, expected AWAITING_APPROVAL for APPROVED`,
  });
  expect([fake.postAttempts, h.app.store.revision(runId)!.state]).toEqual([0, "FAILED"]);
  // The connection is still in the protocol: the next command is answered, and so is one the engine crashes on.
  expect(await command(c, { command: "review.retry", params: { runId } })).toMatchObject({ status: "accepted", operationId: `review.retry:${runId}` });
  h.source.command = async () => {
    throw new Error("durable storage is closed");
  };
  expect(await command(c, { command: "review.cancel", params: { runId } })).toEqual(rejected("INTERNAL_ERROR", "durable storage is closed"));
}, 60_000);

it("recovers an approval the daemon was stopped in the middle of, with one POST", async () => {
  const client = newClient();
  const request = client.request.bind(client);
  let armed = false;
  let release: () => void = () => {};
  const blocked = new Promise<void>((resolve) => {
    client.request = (async (method: string, path: string, options?: Parameters<typeof request>[2]) => {
      if (armed && method === "GET" && path.includes("/reviews")) {
        armed = false;
        resolve();
        await new Promise<void>((_, reject) => (release = () => reject(new Error("daemon stopping"))));
      }
      return request(method, path, options);
    }) as typeof client.request;
  });
  const slow: Config = { ...approvalConfig, review: { ...approvalConfig.review, retry_backoff_ms: 3000 } };
  const first = await host({ config: slow, client });
  const { c } = await serve(first.source);
  const prId = first.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const runId = started.operationId.slice("review.start:".length);
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL");
  const previewId = first.app.store.outboxForRevision(runId)!.payload_hash;

  armed = true;
  expect(await command(c, { command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } })).toMatchObject({ status: "accepted" });
  await blocked;
  // The daemon stops while the publisher waits on GitHub: APPROVED is recorded, nothing was posted.
  release();
  for (const fn of cleanup.splice(0).reverse()) await fn();
  const stopped = AppStore.open(join(first.dataDir, "app.sqlite"));
  expect([stopped.revision(runId)!.state, stopped.outboxForRevision(runId)!.status, fake.postAttempts]).toEqual(["PUBLISH", "pending", 0]);
  stopped.close();

  const second = await host({ config: slow, dataDir: first.dataDir, responses: [] });
  const again = await serve(second.source);
  again.c.send({ type: "snapshot.get", requestId: "s" });
  const snap = await again.c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  expect(snap.runs.map((r) => [r.id, r.phase])).toEqual([[runId, "PUBLISH"]]);
  await again.c.waitFor((m) => m.type === "event" && m.eventType === "review.completed", 0, 20_000);
  expect(flow(again.c.events()).map(describeEvent)).toEqual(["publication published review 1000", "finding Order deletion endpoint skips the admin check", "review.completed COMPLETED"]);
  expect([fake.postAttempts, fake.reviews.length, second.app.store.outboxForRevision(runId)!.status]).toEqual([1, 1, "published"]);
  // The replayed approval still answers the recorded operation.
  expect(await command(again.c, { command: "review.publish", params: { runId, previewId, expectedHeadSha: fixture.headSha } })).toMatchObject({ status: "accepted", operationId: `review.publish:${runId}` });
  expect(fake.postAttempts).toBe(1);
}, 60_000);

it("starts a head again under the same key once approval mode makes it a new revision", async () => {
  const dryRun: Config = { ...approvalConfig, review: { ...approvalConfig.review, publish_mode: "dry_run" } };
  const first = await host({ config: dryRun });
  const { c } = await serve(first.source);
  const prId = first.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const dry = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed", 0, 20_000);
  for (const fn of cleanup.splice(0).reverse()) await fn();

  const second = await host({ dataDir: first.dataDir });
  const again = await serve(second.source);
  again.c.send({ type: "snapshot.get", requestId: "s" });
  await again.c.waitFor((m) => m.type === "snapshot");
  const started = await command(again.c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } });
  expect(started).toMatchObject({ status: "accepted", operationId: expect.stringMatching(/^review\.start:[0-9a-f]{32}$/) });
  expect((started as { operationId: string }).operationId).not.toBe(dry.operationId);
  await again.c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL", 0, 20_000);
  expect(fake.postAttempts).toBe(0);
}, 60_000);

it("produced the M3 server messages, each passing the v1 schema", () => {
  for (const type of ["publication.preview", "event:review.publication_changed", "event:run.failed", "event:run.superseded", "command.result"]) expect(produced.has(type), type).toBe(true);
});

it("review.add tracks a new PR from its URL and starts its review; a replay answers the same run and starts nothing", async () => {
  const h = await host({ config: { ...approvalConfig, review: { ...approvalConfig.review, publish_mode: "dry_run" } } });
  const { c, welcome } = await serve(h.source);
  expect(welcome).toMatchObject({ capabilities: ["account.select", "pr.archive", "pr.unarchive", "review.add", "review.start", "review.again", "review.retry", "review.cancel", "settings.set"] });
  c.send({ type: "snapshot.get", requestId: "s" });
  const snap = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  expect(snap.prs).toEqual([]);

  const added = await command(c, { command: "review.add", params: { url: "https://github.com/acme/shop/pull/1" } });
  const prId = h.app.store.pullByName("acme/shop", 1)!.id;
  const runId = h.app.store.revisionsForPr(prId)[0]!.id;
  expect(operation(added)).toEqual({ type: "command.result", status: "accepted", operationId: `review.add:${runId}`, prId, runId });
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
  expect(c.events().map(describeEvent).slice(0, 2)).toEqual(["pr acme/shop#1", "run.started COLLECT_CONTEXT"]);
  // dry_run: the review completes and never posts.
  expect([h.app.store.revision(runId)!.state, fake.postAttempts]).toEqual(["COMPLETED", 0]);

  expect(operation(await command(c, { command: "review.add", params: { url: "https://github.com/acme/shop/pull/1" } }))).toEqual(operation(added));
  expect(h.app.store.revisionsForPr(prId)).toHaveLength(1);
  expect(await taskStatuses(h.app)).toEqual(["terminal"]);
}, 30_000);

it("review.add refuses an already tracked PR naming it, an invalid URL, a PR GitHub won't serve, and a closed one", async () => {
  const h = await host();
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");

  expect(await command(c, { command: "review.add", params: { url: "ACME/shop#1" } })).toEqual(
    expect.objectContaining({ status: "rejected", code: "CONFLICT", message: "acme/shop#1 is already tracked", prId }),
  );
  expect(await command(c, { command: "review.add", params: { url: "https://gitlab.com/acme/shop/pull/1" } })).toEqual(
    rejected("INVALID_REQUEST", "https://gitlab.com/acme/shop/pull/1 is not a GitHub pull request; paste its URL (https://github.com/owner/repo/pull/123) or owner/repo#123"),
  );
  expect(await command(c, { command: "review.add", params: { url: "acme/ghost#9" } })).toEqual(
    rejected("NOT_FOUND", "cannot read acme/ghost#9: it does not exist, or github.com/octo has no access to it (HTTP 404)"),
  );
  fake.setPull({ ...fixture.pull, number: 2, state: "closed", merged: true });
  expect(await command(c, { command: "review.add", params: { url: "acme/shop#2" } })).toEqual(rejected("CONFLICT", "acme/shop#2 is merged; only open PRs are reviewed"));
  expect([h.app.store.pullByName("acme/shop", 2), await taskStatuses(h.app)]).toEqual([undefined, []]);
}, 30_000);

it("archiving leaves a running review alone; starting, retrying, reviewing again or re-adding the PR brings it back", async () => {
  const reached: (() => void)[] = [];
  const blocked: FauxResponseStep = (_context, options) =>
    new Promise((resolve) => {
      reached.shift()!();
      options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
    });
  const h = await host({ responses: [readDiff(), blocked, readDiff(), submitDefect(fixture), readDiff(), submitDefect(fixture)] });
  const { c } = await serve(h.source);
  const prId = h.app.store.upsertPull(fixture.pull);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const archive = () => command(c, { command: "pr.archive", params: { prId } });
  const archived = () => h.source.snapshot().prs.find((p) => p.id === prId)!.archivedAt !== undefined;
  const parked = (from: number) => c.waitFor((m) => m.type === "event" && m.eventType === "review.phase_changed" && (m.data as { to: string }).to === "AWAITING_APPROVAL", from);

  await archive();
  const inAnalyze = new Promise<void>((resolve) => reached.push(resolve));
  const started = (await command(c, { command: "review.start", params: { prId, expectedHeadSha: fixture.headSha } })) as { operationId: string };
  const runId = started.operationId.slice("review.start:".length);
  expect(archived()).toBe(false);
  await inAnalyze;

  expect(await archive()).toMatchObject({ status: "accepted" });
  expect([archived(), h.app.store.revision(runId)!.state, await taskStatuses(h.app)]).toEqual([true, "ANALYZE", ["running"]]);
  await command(c, { command: "review.cancel", params: { runId } });
  await c.waitFor((m) => m.type === "event" && m.eventType === "run.failed");
  expect(archived()).toBe(true);

  let mark = c.messages.length;
  expect(await command(c, { command: "review.retry", params: { runId } })).toMatchObject({ status: "accepted" });
  expect(archived()).toBe(false);
  await parked(mark);

  await archive();
  mark = c.messages.length;
  expect(await command(c, { command: "review.again", params: { prId, expectedHeadSha: fixture.headSha } })).toMatchObject({ status: "accepted" });
  expect(archived()).toBe(false);
  await parked(mark);

  await archive();
  expect(await command(c, { command: "review.add", params: { url: "acme/shop#1" } })).toEqual(expect.objectContaining({ status: "rejected", code: "CONFLICT", prId }));
  expect(archived()).toBe(false);
  await c.waitFor(() => h.source.latestSequence() === c.events().at(-1)?.sequence);
  const updates = c.events().filter((e) => e.eventType === "pr.updated" && e.prId === prId);
  expect(updates.some((e) => "archivedAt" in (e.data as object))).toBe(true);
  expect("archivedAt" in (updates.at(-1)!.data as object)).toBe(false);
}, 60_000);

it("a read-only daemon refuses review.add", async () => {
  const off = await openSource(mkdtempSync(join(tmpdir(), "engine-")), false, {}, () => {});
  cleanup.push(() => off.close());
  const { c } = await serve(off);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  expect(await command(c, { command: "review.add", params: { url: "acme/shop#1" } })).toEqual(rejected("CAPABILITY_DENIED", `review.add is not available: ${COMMANDS_OFF}`));
}, 30_000);

it("settings.get lists the models and the one in use; settings.set saves a listed one, keeps the other keys, and the next review uses it", async () => {
  const dir = mkdtempSync(join(tmpdir(), "settings-"));
  const configPath = join(dir, "pr-review.config.json");
  writeFileSync(configPath, JSON.stringify({ models: { default: "faux/reviewer" }, review: { publish_mode: "dry_run" } }));
  const asked: string[] = [];
  const spy =
    (step: FauxResponseStep): FauxResponseStep =>
    (context, options, state, model) => (asked.push(model.id), typeof step === "function" ? step(context, options, state, model) : step);
  const config = { ...approvalConfig, review: { ...approvalConfig.review, publish_mode: "dry_run" as const }, models: { ...approvalConfig.models, preferred: ["faux/other"] } };
  const h = await host({ config, env: { PATH: "", PR_REVIEW_CONFIG: configPath }, modelIds: ["reviewer", "other"], responses: [spy(readDiff()), spy(submitDefect(fixture))] });
  const { c } = await serve(h.source);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  const settings = async () => {
    const requestId = `get-${++requests}`;
    c.send({ type: "settings.get", requestId });
    const { type: _, requestId: __, ...reply } = await c.waitFor<Extract<ServerMessage, { type: "settings" }>>((m) => m.type === "settings" && m.requestId === requestId);
    return reply;
  };
  const option = (id: string, preferred: boolean) => ({ ref: `faux/${id}`, provider: "faux", name: id, contextWindow: expect.any(Number), authSource: expect.any(String), preferred });
  expect(await settings()).toEqual({ model: { ref: "faux/reviewer", source: "config" }, models: [option("other", true), option("reviewer", false)], configPath });

  expect(await command(c, { command: "settings.set", params: { model: "faux/missing" } })).toEqual(rejected("NOT_FOUND", "model faux/missing is not available; choose one of faux/other, faux/reviewer"));
  expect(JSON.parse(readFileSync(configPath, "utf8")).models).toEqual({ default: "faux/reviewer" });

  expect(operation(await command(c, { command: "settings.set", params: { model: "faux/other" } }))).toEqual({ type: "command.result", status: "accepted", operationId: "settings.set:faux/other" });
  expect(JSON.parse(readFileSync(configPath, "utf8"))).toEqual({ models: { default: "faux/other" }, review: { publish_mode: "dry_run" } });
  expect((await settings()).model).toEqual({ ref: "faux/other", source: "config" });

  await command(c, { command: "review.add", params: { url: "https://github.com/acme/shop/pull/1" } });
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
  const revision = h.app.store.revisionsForPr(h.app.store.pullByName("acme/shop", 1)!.id)[0]!;
  expect([revision.reviewer_version, asked]).toEqual(["m1+faux/other", ["other", "other"]]);
}, 30_000);

it("refuses settings.set while the daemon's PR_REVIEW_MODEL outranks the config, and a read-only daemon grants neither settings request", async () => {
  const h = await host({ model: { ref: "faux/reviewer", source: "PR_REVIEW_MODEL" } });
  const engine = await serve(h.source);
  engine.c.send({ type: "snapshot.get", requestId: "s" });
  await engine.c.waitFor((m) => m.type === "snapshot");
  expect(await command(engine.c, { command: "settings.set", params: { model: "faux/reviewer" } })).toEqual(
    rejected("CONFLICT", "the daemon's PR_REVIEW_MODEL (faux/reviewer) overrides the config; remove it and restart the daemon to choose the model here"),
  );

  const off = await openSource(mkdtempSync(join(tmpdir(), "engine-")), false, {}, () => {});
  cleanup.push(() => off.close());
  const { c, welcome } = await serve(off);
  expect(welcome.capabilities).not.toContain("settings.set");
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  expect(await command(c, { command: "settings.set", params: { model: "faux/reviewer" } })).toEqual(rejected("CAPABILITY_DENIED", `settings.set is not available: ${COMMANDS_OFF}`));
  c.send({ type: "settings.get", requestId: "get" });
  expect(await c.waitFor((m) => "requestId" in m && m.requestId === "get")).toEqual({ type: "error", requestId: "get", code: "CAPABILITY_DENIED", message: `settings.get is not available: ${COMMANDS_OFF}` });
}, 30_000);
