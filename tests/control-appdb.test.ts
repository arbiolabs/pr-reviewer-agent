import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { AppDbSource } from "../src/control/appdb.ts";
import { startControlServer } from "../src/control/server.ts";
import { AppStore } from "../src/persistence/store.ts";
import type { CandidateFinding } from "../src/domain/types.ts";
import type { ActivitySpan, ServerMessage } from "../src/protocol/v1.ts";
import { finishReview, startReview } from "../src/runtime/app.ts";
import { syncRepo } from "../src/source/git.ts";
import { produced, TestClient } from "./control-client.ts";
import { FAKE_TOKEN, fakeGh, GH_STATUS } from "./fake-gh.ts";
import { createFixture } from "./fixture.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";

const cleanup: (() => Promise<void> | void)[] = [];
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
});

async function daemon(dataDir: string, heartbeatMs?: number) {
  // No gh on PATH: these tests pin exact journal sequences, and a detected account would add a row.
  const source = AppDbSource.open(dataDir, { PATH: "" });
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
      // Timings and commit SHAs vary from run to run; the words around them do not.
      return `tool ${d.tool}: ${String(d.summary).replace(/\b\d+\.\d+s\b/g, "Ns").replace(/\b[0-9a-f]{7}\b/g, "sha")}`;
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
  "tool phase: collect started",
  "tool github: fetching PR #1",
  "tool github: PR #1: head sha (Ns)",
  "tool git: cloning acme/shop",
  "tool git: cloned acme/shop in Ns",
  "tool git: fetching the head tree's blobs",
  "tool git: head tree already local (Ns)",
  "tool git: diffing sha..sha",
  "tool git: diff: 1 file, +1 -0",
  "phase COLLECT_CONTEXT -> TRIAGE",
  "tool phase: collect done in Ns",
  "tool phase: triage started",
  "tool jev: skipped: Jev disabled by --no-jev",
  "tool route: standard: Jev failed: Jev disabled by --no-jev; using standard",
  "phase TRIAGE -> ANALYZE",
  "tool phase: triage done in Ns",
  "tool phase: analyze started",
  "tool model: turn 1 started",
  "tool model: turn 1: Ns, 11 output tokens, called read_diff",
  "tool read_diff: src/routes/orders.ts",
  "tool model: turn 1: 1 tool result(s) in Ns",
  "tool model: turn 2 started",
  "tool model: turn 2: Ns, 143 output tokens, called submit_candidates",
  "tool submit_candidates: 1 candidate(s), 0 resolution claim(s)",
  "tool model: turn 2: 1 tool result(s) in Ns",
  "phase ANALYZE -> VERIFY",
  "tool phase: analyze done in Ns",
  "tool phase: verify started",
  "tool verify: locating earlier findings at this head",
  "tool verify: 0 earlier finding(s) located (Ns)",
  "tool verify: checking 1 candidate(s)",
  "tool verify: 1 kept, 0 discarded (Ns)",
  "tool verify: kept: Order deletion endpoint skips the admin check",
  "finding Order deletion endpoint skips the admin check @ src/routes/orders.ts:5",
  "phase VERIFY -> PREPARE_PUBLICATION",
  "tool phase: verify done in Ns",
  "tool phase: prepare started",
  "tool prepare: payload: 1 finding(s), 1 inline comment(s), 397-char summary, not posted",
  "tool github: checking the PR head",
  "tool github: head unchanged (Ns)",
  "phase PREPARE_PUBLICATION -> PUBLISH",
  "review.completed COMPLETED COMPLETED",
  "tool phase: prepare done in Ns",
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
  // The waterfall: every span that opens closes, each step sits in its phase, and each tool call in the turn that asked.
  const spans = events.flatMap((e) => (e.eventType === "tool.activity" && (e.data as { span?: ActivitySpan }).span) || []);
  const closed = new Map(spans.filter((s) => s.endedAt).map((s) => [s.id, s]));
  expect(spans.filter((s) => !s.endedAt).every((s) => closed.has(s.id))).toBe(true);
  expect([...closed.values()].map((s) => `${s.kind} ${s.id.replace(/:[0-9a-f]{8}$/, "")} < ${s.parent?.replace(/:[0-9a-f]{8}$/, "") ?? "-"}`)).toEqual([
    "step step:pr < phase:collect",
    "step step:sync < phase:collect",
    "step step:prefetch < phase:collect",
    "step step:diff < phase:collect",
    "phase phase:collect < -",
    "phase phase:triage < -",
    "tool tool < turn:1",
    "model turn:1 < phase:analyze",
    "tool tool < turn:2",
    "model turn:2 < phase:analyze",
    "phase phase:analyze < -",
    "step step:locate < phase:verify",
    "step step:check < phase:verify",
    "phase phase:verify < -",
    "step step:head < phase:prepare",
    "phase phase:prepare < -",
  ]);
  expect(closed.get([...closed.keys()].find((id) => id.startsWith("turn:2"))!)!.outputTokens).toBe(143);
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
      attempt: 1,
    },
  ]);
  const [finding] = snapshot.findings;
  expect(finding).toMatchObject({ runId: revision.id, title: "Order deletion endpoint skips the admin check", severity: "high", status: "DETECTED", line: 5, evidenceCount: 1, stale: false });
  expect(finding).not.toHaveProperty("unconfirmed");

  c.send({ type: "finding.get", requestId: "f", findingId: finding!.id });
  expect(await c.waitFor((m) => m.type === "finding.detail")).toMatchObject({
    finding: {
      id: finding!.id,
      reasoningSummary: "Compared the new route registration with its guarded sibling.",
      evidence: [{ kind: "code_ref", path: "src/routes/orders.ts", start: 4, end: 5, refSha: fixture.headSha, text: '  app.get("/orders", requireAdmin, listOrders);\n  app.delete("/orders/:id", deleteOrder);' }],
      history: [{ runId: revision.id, disposition: "DETECTED" }],
    },
  });
  c.send({ type: "finding.get", requestId: "f404", findingId: "nope" });
  expect(await c.waitFor((m) => m.type === "error")).toEqual({ type: "error", requestId: "f404", code: "NOT_FOUND", message: "no finding nope" });

  // Timeline pages newest first and chains by nextBeforeSequence.
  const pages: Ev[][] = [];
  for (let before: number | undefined, n = 0; n < 10; n++) {
    c.send({ type: "timeline.get", requestId: `t${n}`, runId: revision.id, limit: 10, ...(before ? { beforeSequence: before } : {}) });
    const page = await c.waitFor<Extract<ServerMessage, { type: "timeline.page" }>>((m) => m.type === "timeline.page" && m.requestId === `t${n}`);
    pages.push(page.events as Ev[]);
    if (page.nextBeforeSequence === undefined) break;
    before = page.nextBeforeSequence;
  }
  expect(pages.map((p) => p.length)).toEqual([10, 10, 10, 10, 5]);
  expect(pages.flat().map(describeEvent).reverse()).toEqual(REVIEW);
  c.send({ type: "timeline.get", requestId: "tf", findingId: finding!.id, limit: 10 });
  expect(((await c.waitFor((m) => m.type === "timeline.page" && m.requestId === "tf")) as { events: Ev[] }).events.map(describeEvent)).toEqual(REVIEW.filter((e) => e.startsWith("finding ")));

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

it("marks a suspected finding unconfirmed in the snapshot, its detail and its update event", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-e2e-"));
  const server = await daemon(dataDir);
  const c = await attach(server.path);
  const app = await open(dataDir, fixture, [readDiff(), submitDefect(fixture, "suspected")]);
  cleanup.push(() => app.close());
  expect((await finishReview(app, await startReview(app, fixture.pull, "1"))).state).toBe("COMPLETED");
  c.send({ type: "snapshot.get", requestId: "s" });
  const { findings } = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  expect(findings).toMatchObject([{ title: "Order deletion endpoint skips the admin check", unconfirmed: true }]);
  c.send({ type: "finding.get", requestId: "f", findingId: findings[0]!.id });
  expect(await c.waitFor((m) => m.type === "finding.detail")).toMatchObject({ finding: { unconfirmed: true } });
  c.send({ type: "timeline.get", requestId: "t", findingId: findings[0]!.id, limit: 10 });
  const page = (await c.waitFor((m) => m.type === "timeline.page")) as { events: Ev[] };
  expect(page.events.map((e) => (e.data as { unconfirmed?: boolean }).unconfirmed)).toEqual([true]);
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
  // GitHub's title is what the PR reads as; the reviewed context's copy only stands in for rows that predate it.
  const prId = store.upsertPull({ ...createFixture().pull, title: "\u001b[31mevil\u001b[0m\u009b2J title" });
  const rev = store.ensureRevision({ prId, baseSha: "b".repeat(40), headSha: "h".repeat(40), policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });
  store.saveSnapshot({ revisionId: rev.id, pr: { title: "an older title", body: "", fullName: "acme/shop", number: 1, cloneUrl: "" }, changedFiles: [], diff: "", limitations: [] });
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
  // Each run event carries the attempt it happened in; the retry opened the second.
  expect(c.events().filter((e) => e.eventType.startsWith("run.")).map((e) => (e.data as { attempt?: number }).attempt)).toEqual([1, 1, 2, 2]);

  c.send({ type: "snapshot.get", requestId: "s2" });
  const snapshot = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot" && m.requestId === "s2");
  expect(snapshot.prs[0]!.title).toBe("evil title");
  expect(snapshot.runs.map((r) => r.attempt)).toEqual([2]);
  c.send({ type: "resume", requestId: "r", streamEpoch: "other", afterSequence: 1 });
  expect(await c.waitFor((m) => m.type === "resync.required")).toMatchObject({ requestId: "r" });
});

it("keeps a failed run's reason after its publication is reconciled", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-reason-"));
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  const prId = store.upsertPull(createFixture().pull);
  const rev = store.ensureRevision({ prId, baseSha: "b".repeat(40), headSha: "h".repeat(40), policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });
  for (const event of ["CONTEXT_PERSISTED", "ROUTING_PERSISTED", "CANDIDATES_PERSISTED", "CANDIDATES_DISPOSED", "HEAD_VERIFIED"] as const) store.advanceFromCurrent(rev.id, event);
  const outbox = store.ensureOutbox({ revisionId: rev.id, idempotencyKey: `${rev.id}:1`, payloadHash: "p", payloadJson: "{}", inlineFindingIds: [] });
  store.advanceOutbox(outbox.id, "SEND");
  store.advanceFromCurrent(rev.id, "TERMINAL_ERROR", "GitHub timed out");
  // Reconciliation finds nothing on GitHub: the outbox returns to pending, and the journal notes the publication failed.
  store.advanceOutbox(outbox.id, "ABSENT");
  expect(store.db.prepare("SELECT event, detail FROM audit_events WHERE revision_id = ? AND to_state = 'FAILED' ORDER BY seq").all(rev.id)).toEqual([
    { event: "TERMINAL_ERROR", detail: "GitHub timed out" },
    { event: "PUBLICATION_CHANGED", detail: "failed" },
  ]);

  const server = await daemon(dataDir);
  const c = await attach(server.path);
  c.send({ type: "snapshot.get", requestId: "s" });
  const snapshot = await c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
  expect(snapshot.runs.map((r) => [r.status, r.failureReason])).toEqual([["FAILED", "GitHub timed out"]]);
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

it("sends the cited lines as evidence text, capped, sanitized, and never fetched", async () => {
  const fixture = createFixture();
  const git = (dir: string, ...args: string[]) => execFileSync("git", ["-C", dir, "-c", "user.name=F", "-c", "user.email=f@x", ...args], { encoding: "utf8" }).trim();
  const lines = [
    ...Array.from({ length: 100 }, (_, i) => `line ${i + 1}`),
    ...Array.from({ length: 100 }, (_, i) => `long ${i + 101} ${"x".repeat(500)}`),
    'const s = "\u001b[31mred\u001b[0m\u0007\u009b2J";',
  ];
  writeFileSync(join(fixture.repoDir, "app.ts"), lines.join("\n") + "\n");
  git(fixture.repoDir, "add", ".");
  git(fixture.repoDir, "commit", "-q", "-m", "app");
  const sha = git(fixture.repoDir, "rev-parse", "HEAD");
  git(fixture.repoDir, "config", "uploadpack.allowFilter", "true");

  const dataDir = mkdtempSync(join(tmpdir(), "ctl-evidence-"));
  const server = await daemon(dataDir);
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  // A local-path clone ignores --filter, so it has every blob. A file:// one honours blob:none and has none.
  await syncRepo(join(dataDir, "repos"), "acme/shop", fixture.repoDir, 1);
  const lazy = await syncRepo(join(dataDir, "repos"), "acme/lazy", `file://${fixture.repoDir}`, 1);
  const ref = (start: number, end: number, path = "app.ts") => ({ path, start, end, refSha: sha });
  const save = (repositoryId: number, fullName: string, codeEvidence: CandidateFinding["codeEvidence"]) => {
    const prId = store.upsertPull({ ...fixture.pull, repositoryId, fullName, baseSha: sha, headSha: sha });
    const revision = store.ensureRevision({ prId, baseSha: sha, headSha: sha, policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });
    const candidate: CandidateFinding = {
      id: "c1",
      category: "security",
      severity: "high",
      title: "t",
      explanation: "e",
      file: "app.ts",
      newLine: 3,
      codeEvidence,
      reasoningSummary: "r",
      confidence: 0.9,
      evidenceKind: "confirmed_by_static_trace",
    };
    store.writeVerification(revision, [], [{ findingId: `f-${fullName}`, fingerprint: fullName, candidate }], [{ findingId: `f-${fullName}`, path: ["DETECTED"], basis: null, candidate, location: { file: candidate.file, newLine: candidate.newLine, headSha: sha, evidence: [] } }]);
    return `f-${fullName}`;
  };
  const shop = save(1, "acme/shop", [ref(3, 7), ref(1, 100), ref(101, 200), ref(201, 201), ref(3, 7, "gone.ts")]);
  const offline = save(2, "acme/lazy", [ref(3, 7)]);

  const c = await attach(server.path);
  const detail = async (findingId: string) => {
    c.send({ type: "finding.get", requestId: findingId, findingId });
    const m = await c.waitFor<Extract<ServerMessage, { type: "finding.detail" }>>((m) => m.type === "finding.detail" && m.requestId === findingId);
    return m.finding.evidence.map((e) => e.text);
  };
  const [five, short, long, hostile, gone] = await detail(shop);
  expect(five).toBe("line 3\nline 4\nline 5\nline 6\nline 7");
  expect(short!.split("\n")).toEqual([...lines.slice(0, 40), "… 60 more lines"]);
  const kept = long!.split("\n").slice(0, -1);
  expect(kept).toEqual(lines.slice(100, 100 + kept.length));
  expect(Buffer.byteLength(kept.join("\n"))).toBeLessThanOrEqual(8 * 1024);
  expect(long!.split("\n").at(-1)).toBe(`… ${100 - kept.length} more lines`);
  expect(hostile).toBe('const s = "red";');
  expect(gone).toBe("gone.ts:3-7 (code unavailable offline)");

  expect(await detail(offline)).toEqual(["app.ts:3-7 (code unavailable offline)"]);
  // The origin was reachable all along: the fallback is the daemon refusing to fetch, and the blob is still absent.
  const blob = git(fixture.repoDir, "rev-parse", `${sha}:app.ts`);
  expect(() => execFileSync("git", ["-C", lazy, "cat-file", "-e", blob], { env: { ...process.env, GIT_NO_LAZY_FETCH: "1" }, stdio: "ignore" })).toThrow();
}, 30_000);

it("answers a diff whose blobs the clone lacks with TEMPORARILY_UNAVAILABLE instead of fetching them", async () => {
  const fixture = createFixture();
  const git = (dir: string, ...args: string[]) => execFileSync("git", ["-C", dir, "-c", "user.name=F", "-c", "user.email=f@x", ...args], { encoding: "utf8" }).trim();
  const baseSha = git(fixture.repoDir, "rev-parse", "HEAD");
  writeFileSync(join(fixture.repoDir, "app.ts"), "export const changed = true;\n");
  git(fixture.repoDir, "add", ".");
  git(fixture.repoDir, "commit", "-q", "-m", "change");
  const headSha = git(fixture.repoDir, "rev-parse", "HEAD");
  git(fixture.repoDir, "config", "uploadpack.allowFilter", "true");

  const dataDir = mkdtempSync(join(tmpdir(), "ctl-lazydiff-"));
  const server = await daemon(dataDir);
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  const lazy = await syncRepo(join(dataDir, "repos"), "acme/lazy", `file://${fixture.repoDir}`, 1);
  const prId = store.upsertPull({ ...fixture.pull, repositoryId: 2, fullName: "acme/lazy", baseSha, headSha });
  store.ensureRevision({ prId, baseSha, headSha, policyVersion: "1", reviewerVersion: "m1+faux/reviewer" });

  const c = await attach(server.path);
  c.send({ type: "diff.get", requestId: "lazy", prId, baseSha, headSha });
  expect(await c.waitFor((m) => m.type === "error" && m.requestId === "lazy")).toMatchObject({ code: "TEMPORARILY_UNAVAILABLE" });
  const blob = git(fixture.repoDir, "rev-parse", `${headSha}:app.ts`);
  expect(() => execFileSync("git", ["-C", lazy, "cat-file", "-e", blob], { env: { ...process.env, GIT_NO_LAZY_FETCH: "1" }, stdio: "ignore" })).toThrow();
}, 30_000);

describe("account.select", () => {
  async function accountsDaemon(env: Record<string, string>) {
    const dataDir = mkdtempSync(join(tmpdir(), "ctl-accounts-"));
    const source = AppDbSource.open(dataDir, env);
    const server = await startControlServer({ socketPath: join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock"), source, pollMs: 20 });
    cleanup.push(() => source.close(), () => server.close());
    const c = await attach(server.path);
    let raw = "";
    c.socket.on("data", (chunk) => (raw += chunk));
    const select = (accountId: string, requestId = `sel-${accountId}`) => {
      c.send({ type: "command", requestId, command: "account.select", params: { accountId }, idempotencyKey: `account.select:${accountId}` });
      return c.waitFor<Extract<ServerMessage, { type: "command.result" }>>((m) => m.type === "command.result" && m.requestId === requestId);
    };
    const dbDump = () => {
      const store = AppStore.open(join(dataDir, "app.sqlite"));
      const tables = (store.db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all() as { name: string }[]).map((t) => t.name);
      const dump = JSON.stringify(tables.map((t) => store.db.prepare(`SELECT * FROM "${t}"`).all()));
      store.close();
      return dump;
    };
    return { dataDir, source, c, select, raw: () => raw, dbDump };
  }

  it("rejects before streaming, then selects through the journal; the client sees the selection only in accounts.updated", async () => {
    const gh = fakeGh();
    const d = await accountsDaemon({ PATH: gh.dir });
    expect(d.c.messages[0]).toMatchObject({ type: "welcome", capabilities: ["account.select"] });
    expect(await d.select("github.com/octo-work", "early")).toMatchObject({ status: "rejected", code: "CONFLICT" });

    d.c.send({ type: "snapshot.get", requestId: "s" });
    const snap = await d.c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
    // The first detection in a fresh data dir is journaled once (seq 1), so resuming clients learn the accounts too.
    expect(snap.sequence).toBe(1);
    expect(snap.accounts!.map((a) => [a.id, a.state, a.ghActive, a.selected])).toEqual([
      ["github.com/diego-arbio-dev", "ok", true, false],
      ["github.com/octo-work", "ok", false, false],
      ["github.com/old-bot", "error", false, false],
      ["ghe.acme.com/diego", "ok", true, false],
    ]);

    expect(await d.select("github.com/nobody")).toMatchObject({ status: "rejected", code: "NOT_FOUND" });
    expect(await d.select("github.com/old-bot")).toMatchObject({ status: "rejected", code: "CONFLICT" });
    expect(await d.select("ghe.acme.com/diego")).toMatchObject({ status: "rejected", code: "CONFLICT" });
    d.c.send({ type: "command", requestId: "bad-key", command: "account.select", params: { accountId: "github.com/octo-work" }, idempotencyKey: "k" });
    expect(await d.c.waitFor((m) => m.type === "command.result" && m.requestId === "bad-key")).toMatchObject({ status: "rejected", code: "INVALID_REQUEST" });

    const from = d.c.messages.length;
    expect(await d.select("github.com/octo-work")).toEqual({ type: "command.result", requestId: "sel-github.com/octo-work", status: "accepted", operationId: "account.select:2" });
    const updated = await d.c.waitFor((m) => m.type === "event" && m.eventType === "accounts.updated", from);
    expect(d.c.messages.indexOf(updated)).toBeGreaterThan(d.c.messages.findIndex((m) => m.type === "command.result" && m.status === "accepted"));
    expect(updated).toMatchObject({ sequence: 2, prevSequence: 1 });
    expect((updated as { data: { accounts: { id: string; selected: boolean }[] } }).data.accounts.filter((a) => a.selected).map((a) => a.id)).toEqual(["github.com/octo-work"]);

    const store = AppStore.open(join(d.dataDir, "app.sqlite"));
    expect(store.getMeta("github.account")).toBe("github.com/octo-work");
    expect(store.db.prepare("SELECT seq, revision_id, from_state, event, to_state, detail FROM audit_events ORDER BY seq").all()).toEqual([
      { seq: 1, revision_id: null, from_state: "-", event: "ACCOUNTS_DETECTED", to_state: "-", detail: "4 gh accounts" },
      { seq: 2, revision_id: null, from_state: "-", event: "ACCOUNT_SELECTED", to_state: "-", detail: "github.com/octo-work" },
    ]);
    store.close();

    // gh drops the selected account: the change is journaled, and the selection stays listed, as an error.
    gh.set({ hosts: { "github.com": [GH_STATUS.hosts["github.com"][0]] } });
    await d.source.refreshAccounts(0);
    const dropped = await d.c.waitFor((m) => m.type === "event" && m.eventType === "accounts.updated" && m.sequence === 3);
    expect((dropped as { data: { accounts: unknown[] } }).data.accounts).toEqual([
      { id: "github.com/diego-arbio-dev", host: "github.com", login: "diego-arbio-dev", state: "ok", ghActive: true, selected: false, scopes: ["gist", "read:org", "repo", "workflow"] },
      { id: "github.com/octo-work", host: "github.com", login: "octo-work", state: "error", ghActive: false, selected: true, scopes: [] },
    ]);

    // The daemon never asks gh for a token, so none can reach the socket or the DB.
    expect(gh.calls().every((call) => call === "auth status --json hosts")).toBe(true);
    expect(d.raw()).not.toContain(FAKE_TOKEN);
    expect(d.dbDump()).not.toContain(FAKE_TOKEN);
    expect(d.raw()).not.toContain("hosts.yml");
  }, 30_000);

  it("shows account events in the unscoped timeline, and not in a PR-, run- or finding-scoped one", async () => {
    const d = await accountsDaemon({ PATH: fakeGh().dir });
    d.c.send({ type: "snapshot.get", requestId: "s" });
    await d.c.waitFor((m) => m.type === "snapshot");
    expect(await d.select("github.com/octo-work")).toMatchObject({ status: "accepted" });
    const timeline = async (requestId: string, scope: Record<string, string>) => {
      d.c.send({ type: "timeline.get", requestId, limit: 10, ...scope });
      const page = await d.c.waitFor<Extract<ServerMessage, { type: "timeline.page" }>>((m) => m.type === "timeline.page" && m.requestId === requestId);
      return page.events.map((e) => [e.sequence, e.eventType]);
    };
    expect(await timeline("all", {})).toEqual([[2, "accounts.updated"], [1, "accounts.updated"]]);
    expect(await timeline("pr", { prId: "acme/shop#184" })).toEqual([]);
    expect(await timeline("run", { runId: "rev-184-a" })).toEqual([]);
    expect(await timeline("finding", { findingId: "f-1" })).toEqual([]);
  }, 30_000);

  it("lists GITHUB_TOKEN as env/GITHUB_TOKEN, refuses to select over it, and never sends its value", async () => {
    const secret = "ghp_ENVsecret987654321";
    const d = await accountsDaemon({ PATH: fakeGh().dir, GITHUB_TOKEN: secret });
    d.c.send({ type: "snapshot.get", requestId: "s" });
    const snap = await d.c.waitFor<Extract<ServerMessage, { type: "snapshot" }>>((m) => m.type === "snapshot");
    expect(snap.accounts![0]).toEqual({ id: "env/GITHUB_TOKEN", host: "env", login: "GITHUB_TOKEN", state: "ok", ghActive: false, selected: false, scopes: [] });
    expect(await d.select("github.com/octo-work")).toMatchObject({ status: "rejected", code: "CONFLICT", message: expect.stringContaining("GITHUB_TOKEN") });
    expect(await d.select("env/GITHUB_TOKEN")).toMatchObject({ status: "rejected", code: "NOT_FOUND" });
    expect(d.raw()).not.toContain(secret);
    expect(d.dbDump()).not.toContain(secret);
  }, 30_000);
});

it("produced every server message and event type, each passing the v1 schema", () => {
  expect([...produced].sort()).toEqual(
    [
      "command.result",
      "diff.page",
      "error",
      "event:accounts.updated",
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

it("shows an observation from before migration 055 as DETECTED until the finding's publication, then PUBLISHED, in replay and history", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "ctl-legacy-"));
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  cleanup.push(() => store.close());
  const prId = store.upsertPull(createFixture().pull);
  const key = { prId, baseSha: "b".repeat(40), policyVersion: "1", reviewerVersion: "m1+faux/reviewer" };
  const [r1, r2] = [store.ensureRevision({ ...key, headSha: "1".repeat(40) }), store.ensureRevision({ ...key, headSha: "2".repeat(40) })];
  const candidate = JSON.stringify({ id: "c1", category: "security", severity: "high", title: "t", explanation: "e", file: "src/x.ts", newLine: 5, codeEvidence: [], reasoningSummary: "r", confidence: 0.9, evidenceKind: "confirmed_by_static_trace" });
  const t = (n: number) => `2026-10-20T00:00:0${n}Z`;
  store.db.prepare("INSERT INTO findings (id, pr_id, first_revision_id, fingerprint, status, current_severity, created_at, updated_at) VALUES ('f', ?, ?, 'fp', 'DETECTED', 'high', ?, ?)").run(prId, r1.id, t(1), t(1));
  const observe = store.db.prepare(`INSERT INTO finding_observations (id, finding_id, revision_id, disposition, evidence_json, location_json, created_at) VALUES (?, 'f', ?, 'VERIFIED', ?, '{"file":"src/x.ts","newLine":5}', ?)`);
  observe.run("o1", r1.id, candidate, t(1));
  store.db.prepare("UPDATE findings SET status = 'PUBLISHED', updated_at = ? WHERE id = 'f'").run(t(2));
  observe.run("o2", r2.id, candidate, t(3));
  const source = AppDbSource.open(dataDir, { PATH: "" });
  cleanup.push(() => source.close());
  expect(source.eventsAfter(0, 1000).filter((e) => e.eventType === "finding.updated").map((e) => (e.data as { status: string }).status)).toEqual(["DETECTED", "PUBLISHED", "PUBLISHED"]);
  expect((await source.finding("f")).history.map((h) => h.disposition)).toEqual(["DETECTED", "PUBLISHED", "PUBLISHED"]);
});
