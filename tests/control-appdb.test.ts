import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import { AppDbSource } from "../src/control/appdb.ts";
import { startControlServer } from "../src/control/server.ts";
import { AppStore } from "../src/persistence/store.ts";
import type { ServerMessage } from "../src/protocol/v1.ts";
import { finishReview, startReview } from "../src/runtime/app.ts";
import { syncRepo } from "../src/source/git.ts";
import { produced, TestClient } from "./control-client.ts";
import { createFixture } from "./fixture.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";

const cleanup: (() => Promise<void> | void)[] = [];
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
});

async function daemon(dataDir: string, heartbeatMs?: number) {
  const source = AppDbSource.open(dataDir);
  const server = await startControlServer({ socketPath: join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock"), source, pollMs: 20, ...(heartbeatMs ? { heartbeatMs } : {}) });
  cleanup.push(() => source.close(), () => server.close());
  return server;
}

async function attach(path: string): Promise<TestClient> {
  const c = await TestClient.connect(path);
  cleanup.push(() => c.close());
  await c.hello();
  return c;
}

type Ev = ReturnType<TestClient["events"]>[number];
const describeEvent = (e: Ev) => {
  const d = e.data as Record<string, unknown>;
  switch (e.eventType) {
    case "review.phase_changed":
      return `phase ${d.from} -> ${d.to}`;
    case "tool.activity":
      return `tool ${d.tool}: ${d.summary}`;
    case "finding.updated":
      return `finding ${d.title} @ ${d.file}:${d.line}`;
    case "pr.updated":
      return `pr ${d.repository}#${d.number}`;
    default:
      return `${e.eventType} ${d.phase} ${d.status}${d.failureReason ? ` (${d.failureReason})` : ""}`;
  }
};
const chained = (events: Ev[], from: number) => events.map((e) => e.prevSequence).every((p, i) => p === (i === 0 ? from : events[i - 1]!.sequence));
const journalSeqs = (store: AppStore) => (store.db.prepare("SELECT seq FROM audit_events ORDER BY seq").all() as { seq: number }[]).map((r) => r.seq);

const REVIEW = [
  "pr acme/shop#1",
  "run.started COLLECT_CONTEXT RUNNING",
  "phase COLLECT_CONTEXT -> TRIAGE",
  "phase TRIAGE -> ANALYZE",
  "tool read_diff: src/routes/orders.ts",
  "phase ANALYZE -> VERIFY",
  "finding Order deletion endpoint skips the admin check @ src/routes/orders.ts:5",
  "phase VERIFY -> PREPARE_PUBLICATION",
  "phase PREPARE_PUBLICATION -> PUBLISH",
  "review.completed COMPLETED COMPLETED",
];

it("streams a real review to a client attached from the start, with no gaps", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-e2e-"));
  const server = await daemon(dataDir);
  const c = await attach(server.path);
  c.send({ type: "snapshot.get", requestId: "s" });
  const empty = await c.waitFor((m) => m.type === "snapshot");
  expect(empty).toMatchObject({ sequence: 0, prs: [], runs: [], findings: [] });

  const app = await open(dataDir, fixture, [readDiff(), submitDefect(fixture)]);
  cleanup.push(() => app.close());
  const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
  expect(revision.state).toBe("COMPLETED");
  await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");

  const events = c.events();
  expect(events.map(describeEvent)).toEqual(REVIEW);
  expect(chained(events, 0)).toBe(true);
  expect(events.map((e) => e.sequence)).toEqual(journalSeqs(app.store));

  c.send({ type: "snapshot.get", requestId: "s2" });
  const snapshot = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot" && m.requestId === "s2");
  expect(snapshot.sequence).toBe(events.at(-1)!.sequence);
  expect(snapshot.prs).toEqual([
    { id: revision.pr_id, repository: "acme/shop", number: 1, title: "Add order deletion", headSha: fixture.headSha, baseSha: fixture.baseSha, lifecycle: "ACTIVE" },
  ]);
  expect(snapshot.runs).toEqual([
    {
      id: revision.id,
      prId: revision.pr_id,
      baseSha: fixture.baseSha,
      headSha: fixture.headSha,
      phase: "COMPLETED",
      status: "COMPLETED",
      model: "faux/reviewer",
      startedAt: revision.created_at,
      completedAt: revision.completed_at,
    },
  ]);
  const [finding] = snapshot.findings;
  expect(finding).toMatchObject({ runId: revision.id, title: "Order deletion endpoint skips the admin check", severity: "high", status: "DETECTED", line: 5, evidenceCount: 1, stale: false });

  c.send({ type: "finding.get", requestId: "f", findingId: finding!.id });
  expect(await c.waitFor((m) => m.type === "finding.detail")).toMatchObject({
    finding: {
      id: finding!.id,
      reasoningSummary: "Compared the new route registration with its guarded sibling.",
      evidence: [{ kind: "code_ref", path: "src/routes/orders.ts", start: 4, end: 5, refSha: fixture.headSha, text: "src/routes/orders.ts:4-5" }],
      history: [{ runId: revision.id, disposition: "DETECTED" }],
    },
  });
  c.send({ type: "finding.get", requestId: "f404", findingId: "nope" });
  expect(await c.waitFor((m) => m.type === "error")).toEqual({ type: "error", requestId: "f404", code: "NOT_FOUND", message: "no finding nope" });

  // Timeline pages newest first and chains by nextBeforeSequence.
  const pages: Ev[][] = [];
  for (let before: number | undefined, n = 0; n < 10; n++) {
    c.send({ type: "timeline.get", requestId: `t${n}`, runId: revision.id, limit: 4, ...(before ? { beforeSequence: before } : {}) });
    const page = await c.waitFor<Extract<ServerMessage, { type: "timeline.page" }>>((m) => m.type === "timeline.page" && m.requestId === `t${n}`);
    pages.push(page.events as Ev[]);
    if (page.nextBeforeSequence === undefined) break;
    before = page.nextBeforeSequence;
  }
  expect(pages.map((p) => p.length)).toEqual([4, 4, 2]);
  expect(pages.flat().map(describeEvent).reverse()).toEqual(REVIEW);
  c.send({ type: "timeline.get", requestId: "tf", findingId: finding!.id, limit: 10 });
  expect(((await c.waitFor((m) => m.type === "timeline.page" && m.requestId === "tf")) as { events: Ev[] }).events.map(describeEvent)).toEqual([REVIEW[6]]);

  // Commands change nothing in the engine DB.
  const tables = (app.store.db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all() as { name: string }[]).map((t) => t.name);
  const counts = () => tables.map((t) => [t, (app.store.db.prepare(`SELECT count(*) AS n FROM "${t}"`).get() as { n: number }).n]);
  const before = counts();
  for (const [command, params] of [
    ["review.start", { prId: revision.pr_id, expectedHeadSha: fixture.headSha }],
    ["review.cancel", { runId: revision.id }],
    ["review.retry", { runId: revision.id }],
    ["review.publish", { runId: revision.id, previewId: "p", expectedHeadSha: fixture.headSha }],
  ] as const) {
    c.send({ type: "command", requestId: `cmd-${command}`, command, params, idempotencyKey: `k-${command}` });
    expect(await c.waitFor((m) => m.type === "command.result" && m.requestId === `cmd-${command}`)).toMatchObject({ status: "rejected", code: "CAPABILITY_DENIED" });
  }
  expect(counts()).toEqual(before);
}, 30_000);

it("replays what a disconnected client missed on resume, and the review completes without it", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-e2e-"));
  const server = await daemon(dataDir);
  const first = await attach(server.path);
  first.send({ type: "snapshot.get", requestId: "s" });
  const { streamEpoch } = await first.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  let lastApplied = 0;
  const app = await open(dataDir, fixture, [
    readDiff(),
    async () => {
      await first.waitFor((m) => m.type === "event" && m.eventType === "tool.activity");
      await first.close();
      lastApplied = first.events().at(-1)!.sequence;
      return submitDefect(fixture);
    },
  ]);
  cleanup.push(() => app.close());
  const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
  expect(revision.state).toBe("COMPLETED");

  const second = await attach(server.path);
  second.send({ type: "resume", requestId: "r", streamEpoch, afterSequence: lastApplied });
  await second.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
  expect(chained(first.events(), 0)).toBe(true);
  expect(chained(second.events(), lastApplied)).toBe(true);
  expect([...first.events(), ...second.events()].map(describeEvent)).toEqual(REVIEW);
  expect([...first.events(), ...second.events()].map((e) => e.sequence)).toEqual(journalSeqs(app.store));
  expect(second.messages.some((m) => m.type === "resync.required")).toBe(false);
}, 30_000);

it("maps failure, manual retry and supersession, and sanitizes repository text", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-journal-"));
  const server = await daemon(dataDir, 40);
  const c = await attach(server.path);
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");

  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  const prId = store.upsertPull({ ...createFixture().pull, title: "ignored" });
  const rev = store.ensureRevision({ prId, baseSha: "b".repeat(40), headSha: "h".repeat(40), policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });
  store.saveSnapshot({ revisionId: rev.id, pr: { title: "\u001b[31mevil\u001b[0m\u009b2J title", body: "", fullName: "acme/shop", number: 1, cloneUrl: "" }, changedFiles: [], diff: "", limitations: [] });
  store.advance(rev.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED");
  store.appendActivity(rev.id, "search_code", "\u001b]52;c;ZXZpbA==\u0007token");
  store.advanceFromCurrent(rev.id, "TERMINAL_ERROR", "model \u001b[2Kunavailable");
  store.advance(rev.id, "FAILED", "MANUAL_RETRY", "retry 1");
  store.advanceFromCurrent(rev.id, "NEWER_HEAD", "head moved");
  await c.waitFor((m) => m.type === "event" && m.eventType === "run.superseded");
  expect(c.events().map(describeEvent)).toEqual([
    "pr acme/shop#1",
    "run.started COLLECT_CONTEXT RUNNING",
    "phase COLLECT_CONTEXT -> TRIAGE",
    "tool search_code: token",
    "run.failed FAILED FAILED (model unavailable)",
    "run.started COLLECT_CONTEXT RUNNING",
    "run.superseded SUPERSEDED SUPERSEDED",
  ]);
  await c.waitFor((m) => m.type === "event" && m.eventType === "heartbeat");

  c.send({ type: "snapshot.get", requestId: "s2" });
  const snapshot = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot" && m.requestId === "s2");
  expect(snapshot.prs[0]!.title).toBe("evil title");
  c.send({ type: "resume", requestId: "r", streamEpoch: "other", afterSequence: 1 });
  expect(await c.waitFor((m) => m.type === "resync.required")).toMatchObject({ requestId: "r" });
});

it("pages a diff by nextCursor, caps it per page, and gives binary files metadata only", async () => {
  const fixture = createFixture();
  const git = (...args: string[]) => execFileSync("git", ["-C", fixture.repoDir, "-c", "user.name=F", "-c", "user.email=f@x", ...args], { encoding: "utf8" }).trim();
  const row = (i: number) => `line ${String(i).padStart(5, "0")} ${"x".repeat(90)}`;
  git("checkout", "-q", "main");
  writeFileSync(join(fixture.repoDir, "big.txt"), Array.from({ length: 20_000 }, (_, i) => row(i)).join("\n") + "\n");
  git("add", ".");
  git("commit", "-q", "-m", "big");
  const baseSha = git("rev-parse", "HEAD");
  git("checkout", "-q", "-b", "pr");
  writeFileSync(join(fixture.repoDir, "big.txt"), Array.from({ length: 20_000 }, (_, i) => (i % 20 === 10 ? `${row(i)} changed` : row(i))).join("\n") + "\n");
  writeFileSync(join(fixture.repoDir, "generated.txt"), Array.from({ length: 8_000 }, (_, i) => row(i)).join("\n") + "\n");
  writeFileSync(join(fixture.repoDir, "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 0, 0, 255]));
  git("add", ".");
  git("commit", "-q", "-m", "pr");
  const headSha = git("rev-parse", "HEAD");
  git("update-ref", "refs/pull/1/head", headSha);

  const dataDir = mkdtempSync(join(tmpdir(), "ctl-diff-"));
  const server = await daemon(dataDir);
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  const prId = store.upsertPull({ ...fixture.pull, baseSha, headSha });
  store.ensureRevision({ prId, baseSha, headSha, policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });
  await syncRepo(join(dataDir, "repos"), "acme/shop", fixture.repoDir, 1);

  const c = await attach(server.path);
  const raw: string[] = [];
  c.socket.on("data", (chunk: string) => raw.push(chunk));
  type Page = Extract<ServerMessage, { type: "diff.page" }>;
  const pages: Page[] = [];
  for (let cursor: string | undefined, n = 0; n < 20; n++) {
    c.send({ type: "diff.get", requestId: `d${n}`, prId, baseSha, headSha, ...(cursor ? { cursor } : {}) });
    const page = await c.waitFor<Page>((m) => m.type === "diff.page" && m.requestId === `d${n}`);
    pages.push(page);
    expect(page.truncated).toBe(page.nextCursor !== undefined);
    if (page.nextCursor === undefined) break;
    cursor = page.nextCursor;
  }
  expect(Math.max(...raw.join("").split("\n").map((l) => Buffer.byteLength(l)))).toBeLessThan(1024 * 1024);
  expect(pages.length).toBeGreaterThan(2);

  const parts = (path: string) => pages.flatMap((p) => p.files.filter((f) => f.path === path));
  const big = parts("big.txt");
  expect(big.length).toBeGreaterThan(1);
  expect(big.slice(0, -1).every((f) => f.truncated)).toBe(true);
  expect(big.at(-1)!.truncated).toBe(false);
  const hunks = big.flatMap((f) => f.hunks);
  expect(hunks.length).toBe(1000);
  expect(hunks.map((h) => h.header)).toEqual(git("diff", `${baseSha}...${headSha}`, "--", "big.txt").split("\n").filter((l) => l.startsWith("@@")));
  expect(hunks[0]!.lines.find((l) => l.kind === "add")).toEqual({ kind: "add", new: 11, text: `${row(10)} changed` });
  expect(big[0]!).toMatchObject({ status: "modified", additions: 1000, deletions: 1000 });

  const [generated] = parts("generated.txt");
  expect(generated).toMatchObject({ status: "added", additions: 8000, truncated: true });
  expect(generated!.hunks[0]!.lines.length).toBeLessThan(8000);
  expect(parts("logo.png")).toEqual([{ path: "logo.png", status: "binary", additions: 0, deletions: 0, hunks: [], truncated: false }]);

  c.send({ type: "diff.get", requestId: "one", prId, baseSha, headSha, file: "logo.png", limit: 1 });
  expect(await c.waitFor((m) => m.type === "diff.page" && m.requestId === "one")).toMatchObject({ files: [{ path: "logo.png" }], truncated: false });
  c.send({ type: "diff.get", requestId: "lim", prId, baseSha, headSha, file: "src/routes/orders.ts", limit: 1 });
  expect(await c.waitFor((m) => m.type === "diff.page" && m.requestId === "lim")).toMatchObject({ files: [], truncated: false });
  c.send({ type: "diff.get", requestId: "gone", prId, baseSha, headSha: "0".repeat(40) });
  expect(await c.waitFor((m) => m.type === "error" && m.requestId === "gone")).toMatchObject({ code: "NOT_FOUND" });
}, 60_000);

it("produced every server message and event type, each passing the v1 schema", () => {
  expect([...produced].sort()).toEqual(
    [
      "command.result",
      "diff.page",
      "error",
      "event:finding.updated",
      "event:heartbeat",
      "event:pr.updated",
      "event:review.completed",
      "event:review.phase_changed",
      "event:run.failed",
      "event:run.started",
      "event:run.superseded",
      "event:tool.activity",
      "finding.detail",
      "resync.required",
      "snapshot",
      "timeline.page",
      "welcome",
    ].sort(),
  );
});
