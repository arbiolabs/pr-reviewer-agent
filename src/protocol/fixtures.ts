// Source of protocol/fixtures and protocol/playback. Edit here, then run `pnpm protocol:emit`.
import type { ClientMessage, DiffFile, Finding, FindingDetail, Pr, Run, ServerMessage } from "./v1.ts";

const EPOCH = "epoch-1";
const T0 = Date.parse("2026-10-20T13:00:00Z");
const at = (s: number) => new Date(T0 + s * 1000).toISOString().replace(".000", "");
const sha = (c: string) => c.repeat(40);

const pr = (n: number, extra: Partial<Pr> = {}): Pr => ({
  id: `acme/shop#${n}`,
  repository: "acme/shop",
  number: n,
  title: `PR ${n}`,
  headSha: sha(String(n % 10)),
  baseSha: sha("b"),
  lifecycle: "ACTIVE",
  ...extra,
});

const run = (id: string, p: Pr, extra: Partial<Run> = {}): Run => ({
  id,
  prId: p.id,
  baseSha: p.baseSha,
  headSha: p.headSha,
  phase: "VERIFY",
  status: "RUNNING",
  model: "claude-opus-5-5",
  startedAt: at(0),
  ...extra,
});

const finding = (id: string, r: Run, extra: Partial<Finding> = {}): Finding => ({
  id,
  prId: r.prId,
  runId: r.id,
  fingerprint: `fp-${id}`,
  title: `Finding ${id}`,
  category: "correctness",
  severity: "high",
  status: "DETECTED",
  confidence: 0.82,
  file: "src/booking.ts",
  line: 42,
  evidenceCount: 1,
  stale: false,
  ...extra,
});

type Seq = { epoch?: string; seq: number; prev?: number; t?: number };
type Sequenced = Exclude<Extract<ServerMessage, { type: "event" }>, { eventType: "heartbeat" }>;
const ev = <E extends Sequenced["eventType"]>(
  s: Seq,
  eventType: E,
  data: Extract<Sequenced, { eventType: E }>["data"],
  ids: { prId: string; runId?: string },
): ServerMessage =>
  ({
    type: "event",
    streamEpoch: s.epoch ?? EPOCH,
    sequence: s.seq,
    prevSequence: s.prev ?? s.seq - 1,
    occurredAt: at(s.t ?? s.seq),
    ...ids,
    eventType,
    data,
  }) as ServerMessage;

const phase = (s: Seq, r: Run, from: Run["phase"], to: Run["phase"], event: string) =>
  ev(s, "review.phase_changed", { from, to, event, detail: null }, { prId: r.prId, runId: r.id });
const activity = (s: Seq, r: Run, tool: string, summary: string) =>
  ev(s, "tool.activity", { tool, summary }, { prId: r.prId, runId: r.id });
const findingUpdated = (s: Seq, f: Finding) => ev(s, "finding.updated", f, { prId: f.prId, runId: f.runId });
const runEvent = (s: Seq, type: "run.started" | "review.completed" | "run.superseded" | "run.failed", r: Run) =>
  ev(s, type, r, { prId: r.prId, runId: r.id });

const welcome = (latestSequence: number, epoch = EPOCH): ServerMessage => ({
  type: "welcome",
  requestId: "req-hello",
  protocolVersion: 1,
  serverVersion: "0.1.0",
  sessionId: "session-1",
  capabilities: [],
  streamEpoch: epoch,
  latestSequence,
});
const snapshot = (sequence: number, prs: Pr[], runs: Run[], findings: Finding[], epoch = EPOCH): ServerMessage => ({
  type: "snapshot",
  requestId: "req-snapshot",
  streamEpoch: epoch,
  sequence,
  prs,
  runs,
  findings,
});

// ---------- shared world ----------

const pr1 = pr(184, { title: "Booking API", author: "octocat" });
const pr2 = pr(185, { title: "Fix cache eviction" });
const pr3 = pr(186, { title: "Bump deps", lifecycle: "QUIESCING" });
const run1 = run("rev-184-a", pr1);
const run2 = run("rev-185-a", pr2, { phase: "COMPLETED", status: "COMPLETED", completedAt: at(-60) });
const f1 = finding("f-1", run1);
const f2 = finding("f-2", run2, { severity: "low", category: "performance", line: null, file: "src/cache.ts" });

// ---------- message fixtures ----------

const diffFiles: DiffFile[] = [
  {
    path: "src/booking.ts",
    status: "modified",
    additions: 2,
    deletions: 1,
    hunks: [
      {
        header: "@@ -40,3 +40,4 @@ export function book()",
        oldStart: 40,
        newStart: 40,
        lines: [
          { kind: "context", old: 40, new: 40, text: "  const slot = await lock(id);" },
          { kind: "del", old: 41, text: "  if (slot.free) {" },
          { kind: "add", new: 41, text: "  if (slot.free && !slot.held) {" },
          { kind: "add", new: 42, text: "    slot.held = true;" },
          { kind: "context", old: 42, new: 43, text: "  }" },
        ],
      },
    ],
    truncated: false,
  },
  { path: "assets/logo.png", status: "binary", additions: 0, deletions: 0, hunks: [], truncated: false },
  { path: "src/slots.ts", oldPath: "src/slot.ts", status: "renamed", additions: 0, deletions: 0, hunks: [], truncated: false },
  {
    path: "src/generated/schema.ts",
    status: "added",
    additions: 4000,
    deletions: 0,
    hunks: [{ header: "@@ -0,0 +1,4000 @@", oldStart: 0, newStart: 1, lines: [{ kind: "add", new: 1, text: "// generated" }] }],
    truncated: true,
  },
];

const detail: FindingDetail = {
  ...f1,
  explanation: "Two concurrent bookings can both observe `slot.free` before either sets `held`.",
  reasoningSummary: "Traced book() -> lock(); lock() is advisory and not awaited by the second caller.",
  suggestedFix: "Take the row lock with SELECT ... FOR UPDATE before checking `free`.",
  evidence: [
    { kind: "code_ref", path: "src/booking.ts", start: 40, end: 43, refSha: pr1.headSha, text: "if (slot.free && !slot.held) {" },
    { kind: "tool_summary", text: "grep lock( src/ -> 3 matches" },
    { kind: "test_result", text: "booking.race.test.ts: 1 failed" },
    { kind: "scenario", text: "Two clients POST /book for slot 9 within 5 ms." },
    { kind: "link", text: "https://example.com/docs/locking" },
  ],
  history: [{ runId: run1.id, disposition: "DETECTED", at: at(30) }],
};

const hostileTitle = "\u001b[31mred\u001b[0m \u001b]8;;https://evil.example\u0007click\u001b]8;;\u0007 \u009b2J\u0085done";

export const fixtures: Record<string, ClientMessage | ServerMessage | Record<string, unknown>> = {
  "client.hello": { type: "hello", requestId: "req-hello", protocolVersion: 1, client: "ratatui", capabilities: ["snapshots", "events"] },
  "client.hello.future-version": { type: "hello", requestId: "req-hello", protocolVersion: 2, client: "ratatui", capabilities: [] },
  "client.snapshot.get": { type: "snapshot.get", requestId: "req-snapshot" },
  "client.resume": { type: "resume", requestId: "req-resume", streamEpoch: EPOCH, afterSequence: 108 },
  "client.diff.get": {
    type: "diff.get",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    file: "src/booking.ts",
    cursor: "c-2",
    limit: 100,
  },
  "client.diff.get.minimal": { type: "diff.get", requestId: "req-diff", prId: pr1.id, baseSha: pr1.baseSha, headSha: pr1.headSha },
  "client.finding.get": { type: "finding.get", requestId: "req-finding", findingId: "f-1" },
  "client.timeline.get": { type: "timeline.get", requestId: "req-tl", prId: pr1.id, runId: run1.id, findingId: "f-1", beforeSequence: 100, limit: 50 },
  "client.timeline.get.minimal": { type: "timeline.get", requestId: "req-tl", limit: 50 },
  "client.command.review.start": {
    type: "command",
    requestId: "req-cmd",
    command: "review.start",
    params: { prId: pr1.id, expectedHeadSha: pr1.headSha },
    idempotencyKey: `review.start:${pr1.id}:${pr1.headSha}`,
  },
  "client.command.review.cancel": { type: "command", requestId: "req-cmd", command: "review.cancel", params: { runId: run1.id }, idempotencyKey: "k1" },
  "client.command.review.retry": { type: "command", requestId: "req-cmd", command: "review.retry", params: { runId: run1.id }, idempotencyKey: "k2" },
  "client.command.review.publish": {
    type: "command",
    requestId: "req-cmd",
    command: "review.publish",
    params: { runId: run1.id, previewId: "preview-1", expectedHeadSha: pr1.headSha },
    idempotencyKey: "k3",
  },

  "server.welcome": welcome(108),
  "server.snapshot": snapshot(108, [pr1, pr2, pr3], [run1, run2], [f1, f2]),
  "server.snapshot.empty": { type: "snapshot", streamEpoch: EPOCH, sequence: 0, prs: [], runs: [], findings: [] },
  "server.snapshot.hostile": snapshot(5, [pr(1, { title: hostileTitle })], [], [finding("f-h", run1, { title: "safe\rEVIL\u001bc" })]),
  "server.event.pr.updated": ev({ seq: 109 }, "pr.updated", pr1, { prId: pr1.id }),
  "server.event.run.started": runEvent({ seq: 109 }, "run.started", run(run1.id, pr1, { phase: "COLLECT_CONTEXT" })),
  "server.event.review.phase_changed": phase({ seq: 109 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  "server.event.finding.updated": findingUpdated({ seq: 109 }, f1),
  "server.event.tool.activity": activity({ seq: 109 }, run1, "read_file", "src/booking.ts:1-120"),
  "server.event.review.completed": runEvent({ seq: 109 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(109) }),
  "server.event.run.superseded": runEvent({ seq: 109 }, "run.superseded", { ...run1, phase: "SUPERSEDED", status: "SUPERSEDED", completedAt: at(109) }),
  "server.event.run.failed": runEvent({ seq: 109 }, "run.failed", {
    ...run1,
    phase: "FAILED",
    status: "FAILED",
    completedAt: at(109),
    failureReason: "aborted by operator",
  }),
  "server.event.heartbeat": { type: "event", eventType: "heartbeat", streamEpoch: EPOCH, occurredAt: at(110) },
  "server.event.unknown-type": {
    type: "event",
    streamEpoch: EPOCH,
    sequence: 109,
    prevSequence: 108,
    occurredAt: at(109),
    prId: pr1.id,
    eventType: "review.publication_changed",
    data: { state: "PENDING", whatever: [1, 2] },
  },
  "server.event.extra-fields": {
    ...(findingUpdated({ seq: 109 }, { ...f1, futureField: { nested: true } } as Finding) as object),
    traceId: "abc",
  },
  "server.resync.required": { type: "resync.required", requestId: "req-resume", reason: "epoch epoch-0 is no longer available" },
  "server.resync.required.unsolicited": { type: "resync.required", reason: "client queue overflow" },
  "server.diff.page": {
    type: "diff.page",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    files: diffFiles,
    nextCursor: "c-2",
    truncated: true,
  },
  "server.diff.page.last": {
    type: "diff.page",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    files: [],
    truncated: false,
  },
  "server.finding.detail": { type: "finding.detail", requestId: "req-finding", finding: detail },
  "server.finding.detail.minimal": {
    type: "finding.detail",
    requestId: "req-finding",
    finding: { ...f2, explanation: "e", reasoningSummary: "r", evidence: [], history: [] },
  },
  "server.timeline.page": {
    type: "timeline.page",
    requestId: "req-tl",
    events: [
      phase({ seq: 100 }, run1, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED"),
      activity({ seq: 99 }, run1, "search", "lock("),
    ] as Sequenced[],
    nextBeforeSequence: 99,
  },
  "server.timeline.page.end": { type: "timeline.page", requestId: "req-tl", events: [] },
  "server.command.result.accepted": { type: "command.result", requestId: "req-cmd", status: "accepted", operationId: "op-1" },
  "server.command.result.rejected": {
    type: "command.result",
    requestId: "req-cmd",
    status: "rejected",
    code: "CAPABILITY_DENIED",
    message: "review.start is not available in this version",
  },
  "server.error": { type: "error", requestId: "req-finding", code: "NOT_FOUND", message: "no finding f-9" },
  "server.error.protocol-mismatch": { type: "error", requestId: "req-hello", code: "PROTOCOL_MISMATCH", message: "unsupported protocolVersion 2", supported: [1] },
  "server.error.no-request-id": { type: "error", code: "INVALID_REQUEST", message: "line is not valid JSON" },
};

// ---------- negative fixtures: name -> [message, why] ----------

export const invalidFixtures: Record<string, [unknown, string]> = {
  "client.command.unknown-param": [
    { ...(fixtures["client.command.review.start"] as object), params: { prId: pr1.id, expectedHeadSha: pr1.headSha, force: true } },
    "Command params are closed: an unknown param is rejected rather than ignored.",
  ],
  "client.command.extra-field": [
    { ...(fixtures["client.command.review.cancel"] as object), actor: "admin" },
    "Command messages are closed: identity comes from the connection, never from a caller-chosen field.",
  ],
  "client.command.unknown-command": [
    { type: "command", requestId: "r", command: "review.delete", params: {}, idempotencyKey: "k" },
    "Only the four reserved command names exist.",
  ],
  "client.hello.no-request-id": [{ type: "hello", protocolVersion: 1, client: "ratatui", capabilities: [] }, "Every request carries a requestId."],
  "client.diff.get.limit-zero": [{ ...(fixtures["client.diff.get.minimal"] as object), limit: 0 }, "limit is 1..500."],
  "client.unknown-type": [{ type: "subscribe", requestId: "r" }, "Unknown client message types are invalid requests."],
  "server.welcome.version-2": [{ ...(fixtures["server.welcome"] as object), protocolVersion: 2 }, "A v1 welcome always says protocolVersion 1."],
  "server.event.missing-prev-sequence": [
    (({ prevSequence: _, ...rest }) => rest)(fixtures["server.event.tool.activity"] as Record<string, unknown>),
    "Sequenced events carry prevSequence; the client detects gaps with it.",
  ],
  "server.event.sequence-zero": [{ ...(fixtures["server.event.tool.activity"] as object), sequence: 0 }, "Journal sequences start at 1."],
  "server.event.finding.bad-severity": [
    { ...(fixtures["server.event.finding.updated"] as object), data: { ...f1, severity: "blocker" } },
    "A known eventType with malformed data does not fall through to the unknown-event shape.",
  ],
  "server.event.phase.cancelled": [
    { ...(fixtures["server.event.review.phase_changed"] as object), data: { from: "VERIFY", to: "CANCELLED", event: "X", detail: null } },
    "Phases are the engine's states; CANCELLED does not exist (an abort is FAILED with a reason).",
  ],
  "server.event.tool.activity.long-summary": [
    { ...(fixtures["server.event.tool.activity"] as object), data: { tool: "read_file", summary: "x".repeat(201) } },
    "tool.activity summaries are capped at 200 chars.",
  ],
  "server.event.heartbeat.no-epoch": [{ type: "event", eventType: "heartbeat", occurredAt: at(1) }, "Heartbeats still name the stream epoch."],
  "server.command.result.accepted-no-operation": [
    { type: "command.result", requestId: "r", status: "accepted" },
    "An accepted command names the operation that later events confirm.",
  ],
  "server.error.unknown-code": [{ type: "error", code: "TEAPOT", message: "m" }, "Error codes are a closed list."],
  "server.finding.line-zero": [
    { ...(fixtures["server.finding.detail"] as Record<string, unknown>), finding: { ...detail, line: 0 } },
    "Lines are 1-based; a file-level finding uses null.",
  ],
};

// ---------- playback scripts ----------

export type PlaybackLine = { delayMs: number; message: ServerMessage };
export type Expect = {
  streamEpoch: string;
  lastSequence: number;
  resyncRequested: boolean;
  toolActivityCount: number;
  prs: { id: string; headSha: string; title: string }[];
  runs: { id: string; phase: string; status: string }[];
  findings: { id: string; title: string; status: string; stale: boolean }[];
};

const lines = (delayMs: number, messages: ServerMessage[]): PlaybackLine[] => messages.map((message) => ({ delayMs, message }));
const prRows = (...ps: Pr[]) => ps.map((p) => ({ id: p.id, headSha: p.headSha, title: p.title }));
const baseSnapshot = snapshot(10, [pr1, pr2, pr3], [run1, run2], [f2]);

const normal = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "src/booking.ts:1-120"),
  { type: "event", eventType: "heartbeat", streamEpoch: EPOCH, occurredAt: at(11) },
  findingUpdated({ seq: 12 }, f1),
  phase({ seq: 13 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  findingUpdated({ seq: 14 }, { ...f1, confidence: 0.9, evidenceCount: 2 }),
  {
    type: "event",
    streamEpoch: EPOCH,
    sequence: 15,
    prevSequence: 14,
    occurredAt: at(15),
    eventType: "review.publication_changed",
    data: { reserved: true },
  } as ServerMessage,
  phase({ seq: 16 }, run1, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED"),
  phase({ seq: 17 }, run1, "PUBLISH", "COMPLETED", "NO_PUBLICATION_RECORDED"),
  runEvent({ seq: 18 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(18) }),
]);

const gap = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  phase({ seq: 13, prev: 12 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  activity({ seq: 14 }, run1, "read_file", "after the gap; must not apply"),
]);

const duplicate = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  findingUpdated({ seq: 12 }, f1),
  findingUpdated({ seq: 12 }, f1),
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  phase({ seq: 13 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
]);

const epochChange = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  activity({ epoch: "epoch-2", seq: 3 }, run1, "read_file", "new epoch; must not apply"),
  snapshot(3, [pr1, pr2], [run(run1.id, pr1, { phase: "ANALYZE" })], [], "epoch-2"),
  phase({ epoch: "epoch-2", seq: 4 }, run1, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED"),
]);

const pr1New = { ...pr1, headSha: sha("c") };
const run1New = run("rev-184-b", pr1New, { phase: "COLLECT_CONTEXT", startedAt: at(13) });
const superseded = lines(40, [
  welcome(10),
  snapshot(10, [pr1, pr2, pr3], [run1, run2], [f1, f2]),
  ev({ seq: 11 }, "pr.updated", pr1New, { prId: pr1.id }),
  runEvent({ seq: 12 }, "run.superseded", { ...run1, phase: "SUPERSEDED", status: "SUPERSEDED", completedAt: at(12) }),
  findingUpdated({ seq: 13 }, { ...f1, stale: true }),
  runEvent({ seq: 14 }, "run.started", run1New),
]);

const longTitle = "Refactor booking ".repeat(118).slice(0, 2000);
const cjk = "修复竞态条件 🔥 👩‍💻 ｗｉｄｅ";
const hostilePr1 = pr(1, { title: hostileTitle });
const hostilePr2 = pr(2, { title: longTitle });
const hostilePr3 = pr(3, { title: cjk });
const hostileRun = run("rev-1", hostilePr1);
const hostile = lines(40, [
  welcome(1),
  snapshot(1, [hostilePr1, hostilePr2, hostilePr3], [hostileRun], []),
  activity({ seq: 2 }, hostileRun, "read_file", "\u001b]52;c;ZXZpbA==\u0007src/a.ts \u001b[2K\u001b[1A"),
  findingUpdated({ seq: 3 }, finding("f-h", hostileRun, { title: "safe\rEVIL\u001bc \u0090q\u009c tail" })),
  ev({ seq: 4 }, "pr.updated", { ...hostilePr3, title: `${cjk}\u001b[5m!` }, { prId: hostilePr3.id }),
]);

const burst: PlaybackLine[] = [
  ...lines(0, [welcome(10), baseSnapshot]),
  ...Array.from({ length: 5000 }, (_, i) => ({
    delayMs: 0,
    message: activity({ seq: 11 + i, t: 11 }, run1, "read_file", `src/file-${i}.ts`),
  })),
];

const runRow = (r: Run, phase = r.phase, status = r.status) => ({ id: r.id, phase, status });
const baseExpect = (over: Partial<Expect>): Expect => ({
  streamEpoch: EPOCH,
  lastSequence: 10,
  resyncRequested: false,
  toolActivityCount: 0,
  prs: prRows(pr1, pr2, pr3),
  runs: [runRow(run1), runRow(run2)],
  findings: [{ id: f2.id, title: f2.title, status: f2.status, stale: false }],
  ...over,
});

export const playback: Record<string, { script: PlaybackLine[]; expect: Expect }> = {
  normal: {
    script: normal,
    expect: baseExpect({
      lastSequence: 18,
      toolActivityCount: 1,
      runs: [runRow(run1, "COMPLETED", "COMPLETED"), runRow(run2)],
      findings: [
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
        { id: f1.id, title: f1.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  gap: { script: gap, expect: baseExpect({ lastSequence: 11, resyncRequested: true, toolActivityCount: 1 }) },
  duplicate: {
    script: duplicate,
    expect: baseExpect({
      lastSequence: 13,
      toolActivityCount: 1,
      runs: [runRow(run1, "PREPARE_PUBLICATION"), runRow(run2)],
      findings: [
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
        { id: f1.id, title: f1.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  "epoch-change": {
    script: epochChange,
    expect: {
      streamEpoch: "epoch-2",
      lastSequence: 4,
      resyncRequested: true,
      toolActivityCount: 1,
      prs: prRows(pr1, pr2),
      runs: [runRow(run1, "VERIFY", "RUNNING")],
      findings: [],
    },
  },
  superseded: {
    script: superseded,
    expect: baseExpect({
      lastSequence: 14,
      prs: prRows(pr1New, pr2, pr3),
      runs: [runRow(run1, "SUPERSEDED", "SUPERSEDED"), runRow(run2), runRow(run1New)],
      findings: [
        { id: f1.id, title: f1.title, status: "DETECTED", stale: true },
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  hostile: {
    script: hostile,
    expect: {
      streamEpoch: EPOCH,
      lastSequence: 4,
      resyncRequested: false,
      toolActivityCount: 1,
      prs: [
        { id: hostilePr1.id, headSha: hostilePr1.headSha, title: "red click done" },
        { id: hostilePr2.id, headSha: hostilePr2.headSha, title: longTitle },
        { id: hostilePr3.id, headSha: hostilePr3.headSha, title: `${cjk}!` },
      ],
      runs: [runRow(hostileRun)],
      findings: [{ id: "f-h", title: "safeEVIL  tail", status: "DETECTED", stale: false }],
    },
  },
  burst: { script: burst, expect: baseExpect({ lastSequence: 5010, toolActivityCount: 5000 }) },
};
