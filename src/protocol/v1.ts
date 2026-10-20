import Type, { type Static, type TSchema } from "typebox";
import { CATEGORIES, SEVERITIES } from "../domain/types.ts";
import { COORDINATOR_STATES, FINDING_STATES, REVIEW_STATES } from "../domain/machines.ts";

export const PROTOCOL_VERSION = 1;
export const MAX_LINE_BYTES = 1024 * 1024;

const Id = Type.String({ minLength: 1 });
const Sha = Id;
const Timestamp = Type.String({ description: "RFC 3339 UTC timestamp" });
const Seq = Type.Integer({ minimum: 0 });
const oneOf = <T extends string[]>(values: readonly [...T]) => Type.Enum(values);

export const ERROR_CODES = [
  "UNAUTHORIZED",
  "CAPABILITY_DENIED",
  "INVALID_TRANSITION",
  "REVISION_STALE",
  "NOT_FOUND",
  "CONFLICT",
  "RATE_LIMITED",
  "TEMPORARILY_UNAVAILABLE",
  "INTERNAL_ERROR",
  "PROTOCOL_MISMATCH",
  "INVALID_REQUEST",
] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

export const RUN_STATUSES = ["RUNNING", "COMPLETED", "FAILED", "SUPERSEDED"] as const;
export const DIFF_FILE_STATUSES = ["added", "modified", "deleted", "renamed", "binary"] as const;
export const EVIDENCE_KINDS = ["code_ref", "tool_summary", "test_result", "scenario", "link"] as const;

// ---------- entities ----------

export const Pr = Type.Object({
  id: Id,
  repository: Type.String(),
  number: Type.Integer({ minimum: 1 }),
  title: Type.String(),
  author: Type.Optional(Type.String()),
  headSha: Sha,
  baseSha: Sha,
  lifecycle: oneOf(COORDINATOR_STATES),
});
export type Pr = Static<typeof Pr>;

export const Run = Type.Object({
  id: Id,
  prId: Id,
  baseSha: Sha,
  headSha: Sha,
  phase: oneOf(REVIEW_STATES),
  status: oneOf(RUN_STATUSES),
  model: Type.String(),
  startedAt: Timestamp,
  completedAt: Type.Optional(Timestamp),
  failureReason: Type.Optional(Type.String()),
});
export type Run = Static<typeof Run>;

const findingFields = {
  id: Id,
  prId: Id,
  runId: Id,
  fingerprint: Type.String(),
  title: Type.String(),
  category: oneOf(CATEGORIES),
  severity: oneOf(SEVERITIES),
  status: oneOf(FINDING_STATES),
  confidence: Type.Number({ minimum: 0, maximum: 1 }),
  file: Type.String(),
  line: Type.Union([Type.Integer({ minimum: 1 }), Type.Null()], { description: "head-revision line; null for a file-level finding" }),
  evidenceCount: Type.Integer({ minimum: 0 }),
  stale: Type.Boolean({ description: "the anchor's head SHA is no longer the PR head; never silently remapped" }),
};
export const Finding = Type.Object(findingFields);
export type Finding = Static<typeof Finding>;

export const Evidence = Type.Object({
  kind: oneOf(EVIDENCE_KINDS),
  path: Type.Optional(Type.String()),
  start: Type.Optional(Type.Integer({ minimum: 1 })),
  end: Type.Optional(Type.Integer({ minimum: 1 })),
  refSha: Type.Optional(Sha),
  text: Type.String(),
});
export type Evidence = Static<typeof Evidence>;

export const FindingDetail = Type.Object({
  ...findingFields,
  explanation: Type.String(),
  reasoningSummary: Type.String(),
  suggestedFix: Type.Optional(Type.String()),
  evidence: Type.Array(Evidence),
  history: Type.Array(
    Type.Object({
      runId: Id,
      disposition: oneOf(FINDING_STATES),
      at: Timestamp,
    }),
  ),
});
export type FindingDetail = Static<typeof FindingDetail>;

export const DiffLine = Type.Object({
  kind: Type.Union([Type.Literal("context"), Type.Literal("add"), Type.Literal("del")]),
  old: Type.Optional(Type.Integer({ minimum: 1 })),
  new: Type.Optional(Type.Integer({ minimum: 1 })),
  text: Type.String(),
});

export const DiffFile = Type.Object({
  path: Type.String(),
  oldPath: Type.Optional(Type.String({ description: "set when status is renamed" })),
  status: oneOf(DIFF_FILE_STATUSES),
  additions: Type.Integer({ minimum: 0 }),
  deletions: Type.Integer({ minimum: 0 }),
  hunks: Type.Array(
    Type.Object({
      header: Type.String(),
      oldStart: Type.Integer({ minimum: 0 }),
      newStart: Type.Integer({ minimum: 0 }),
      lines: Type.Array(DiffLine),
    }),
  ),
  truncated: Type.Boolean(),
});
export type DiffFile = Static<typeof DiffFile>;

// ---------- events ----------

const sequenced = {
  type: Type.Literal("event"),
  streamEpoch: Id,
  sequence: Type.Integer({ minimum: 1 }),
  prevSequence: Type.Integer({ minimum: 0, description: "sequence of the previous event this server emitted, or the snapshot's sequence" }),
  occurredAt: Timestamp,
};

const event = <E extends string, D extends TSchema, X extends Record<string, TSchema>>(eventType: E, data: D, extra: X) =>
  Type.Object({ ...sequenced, ...extra, eventType: Type.Literal(eventType), data });

export const KNOWN_EVENT_TYPES = [
  "pr.updated",
  "run.started",
  "review.phase_changed",
  "finding.updated",
  "tool.activity",
  "review.completed",
  "run.superseded",
  "run.failed",
  "heartbeat",
] as const;

const Phase = oneOf(REVIEW_STATES);

export const KnownEvent = Type.Union([
  event("pr.updated", Pr, { prId: Id }),
  event("run.started", Run, { prId: Id, runId: Id }),
  event(
    "review.phase_changed",
    Type.Object({ from: Phase, to: Phase, event: Type.String(), detail: Type.Union([Type.String(), Type.Null()]) }),
    { prId: Id, runId: Id },
  ),
  event("finding.updated", Finding, { prId: Id, runId: Id }),
  event("tool.activity", Type.Object({ tool: Type.String(), summary: Type.String({ maxLength: 200 }) }), { prId: Id, runId: Id }),
  event("review.completed", Run, { prId: Id, runId: Id }),
  event("run.superseded", Run, { prId: Id, runId: Id }),
  event("run.failed", Run, { prId: Id, runId: Id }),
]);

// Never durable, never sequenced: carries no sequence, so it cannot advance or break the gap check.
export const Heartbeat = Type.Object({
  type: Type.Literal("event"),
  eventType: Type.Literal("heartbeat"),
  streamEpoch: Id,
  occurredAt: Timestamp,
});

// A newer server may emit event types this version does not know. They still carry a sequence and still advance
// lastApplied; the client ignores their data. Known names are excluded so a malformed known event fails validation.
export const UnknownEvent = Type.Object({
  ...sequenced,
  prId: Type.Optional(Id),
  runId: Type.Optional(Id),
  eventType: Type.String({ not: { enum: [...KNOWN_EVENT_TYPES] } }),
  data: Type.Unknown(),
});

export const Event = Type.Union([KnownEvent, Heartbeat, UnknownEvent]);
export type Event = Static<typeof Event>;
export type SequencedEvent = Exclude<Event, Static<typeof Heartbeat>>;

// ---------- client -> server ----------

const req = <T extends string, F extends Record<string, TSchema> = {}>(type: T, fields = {} as F) =>
  Type.Object({ type: Type.Literal(type), requestId: Id, ...fields });

// Mutating commands are closed: an unknown param is a rejected request, not a silently ignored one.
const command = <C extends string, P extends Record<string, TSchema>>(name: C, params: P) =>
  Type.Object(
    {
      type: Type.Literal("command"),
      requestId: Id,
      command: Type.Literal(name),
      params: Type.Object(params, { additionalProperties: false }),
      idempotencyKey: Id,
    },
    { additionalProperties: false },
  );

export const Command = Type.Union([
  command("review.start", { prId: Id, expectedHeadSha: Sha }),
  command("review.cancel", { runId: Id }),
  command("review.retry", { runId: Id }),
  command("review.publish", { runId: Id, previewId: Id, expectedHeadSha: Sha }),
]);
export type Command = Static<typeof Command>;

export const ClientMessage = Type.Union([
  req("hello", {
    protocolVersion: Type.Integer({ description: "any integer, so a mismatch is parseable and answered with PROTOCOL_MISMATCH" }),
    client: Type.String(),
    capabilities: Type.Array(Type.String()),
  }),
  req("snapshot.get"),
  req("resume", { streamEpoch: Id, afterSequence: Seq }),
  req("diff.get", {
    prId: Id,
    baseSha: Sha,
    headSha: Sha,
    file: Type.Optional(Type.String()),
    cursor: Type.Optional(Type.String()),
    limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 500 })),
  }),
  req("finding.get", { findingId: Id }),
  req("timeline.get", {
    prId: Type.Optional(Id),
    runId: Type.Optional(Id),
    findingId: Type.Optional(Id),
    beforeSequence: Type.Optional(Type.Integer({ minimum: 1 })),
    limit: Type.Integer({ minimum: 1, maximum: 500 }),
  }),
  Command,
]);
export type ClientMessage = Static<typeof ClientMessage>;

// ---------- server -> client ----------

const reply = <T extends string, F extends Record<string, TSchema>>(type: T, fields: F) =>
  Type.Object({ type: Type.Literal(type), requestId: Id, ...fields });
const OptionalRequestId = Type.Optional(Id);

export const ServerMessage = Type.Union([
  reply("welcome", {
    protocolVersion: Type.Literal(PROTOCOL_VERSION),
    serverVersion: Type.String(),
    sessionId: Id,
    capabilities: Type.Array(Type.String()),
    streamEpoch: Id,
    latestSequence: Seq,
  }),
  Type.Object({
    type: Type.Literal("snapshot"),
    requestId: OptionalRequestId,
    streamEpoch: Id,
    sequence: Seq,
    prs: Type.Array(Pr),
    runs: Type.Array(Run),
    findings: Type.Array(Finding),
  }),
  Event,
  Type.Object({ type: Type.Literal("resync.required"), requestId: OptionalRequestId, reason: Type.String() }),
  reply("diff.page", {
    prId: Id,
    baseSha: Sha,
    headSha: Sha,
    files: Type.Array(DiffFile),
    nextCursor: Type.Optional(Type.String()),
    truncated: Type.Boolean(),
  }),
  reply("finding.detail", { finding: FindingDetail }),
  reply("timeline.page", {
    events: Type.Array(Type.Union([KnownEvent, UnknownEvent]), { description: "newest first" }),
    nextBeforeSequence: Type.Optional(Type.Integer({ minimum: 1 })),
  }),
  reply("command.result", { status: Type.Literal("accepted"), operationId: Id }),
  reply("command.result", { status: Type.Literal("rejected"), code: oneOf(ERROR_CODES), message: Type.String() }),
  Type.Object({
    type: Type.Literal("error"),
    requestId: OptionalRequestId,
    code: oneOf(ERROR_CODES),
    message: Type.String(),
    supported: Type.Optional(Type.Array(Type.Integer(), { description: "set with PROTOCOL_MISMATCH" })),
  }),
]);
export type ServerMessage = Static<typeof ServerMessage>;

export const ENTITIES = { pr: Pr, run: Run, finding: Finding, "finding-detail": FindingDetail, evidence: Evidence, "diff-file": DiffFile, event: Event } as const;
