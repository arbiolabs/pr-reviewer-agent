import Type, { type Static, type TSchema } from "typebox";
import { CATEGORIES, SEVERITIES } from "../domain/types.ts";
import { USAGE_BY, USAGE_GROUPS } from "../domain/usage.ts";
import { MAX_TEMPLATE_CHARS } from "../publish/template.ts";
import { COORDINATOR_STATES, FINDING_STATES, PUBLICATION_STATUSES, REVIEW_STATES } from "../domain/machines.ts";

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
/** Engine mode hosts the review engine and may grant the review commands; read-only mode only reads the app DB. */
export const DAEMON_MODES = ["engine", "read-only"] as const;
/** The commands a daemon in engine mode can grant (`welcome.capabilities`), besides the reader's `account.select`, `pr.archive` and `pr.unarchive`. */
export const REVIEW_COMMANDS = ["review.add", "review.start", "review.again", "review.retry", "review.cancel", "review.publish"] as const;
/** `review.publish_mode`: `approval` parks each review until a human posts it. */
export const PUBLISH_MODES = ["comment", "dry_run", "silent_no_findings", "approval"] as const;
export const DIFF_FILE_STATUSES = ["added", "modified", "deleted", "renamed", "binary"] as const;
export const CHECK_STATUSES = ["ok", "warn", "missing"] as const;
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
  archivedAt: Type.Optional(
    Type.String({ description: "RFC 3339 UTC timestamp: when a reader archived the PR; absent unless it is archived, and cleared when a review is started on it" }),
  ),
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
  attempt: Type.Optional(Type.Integer({ minimum: 1, description: "1-based: each manual retry or approval opens the next attempt" })),
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
  unconfirmed: Type.Optional(
    Type.Boolean({ description: "true when the reviewer suspects the finding but could not show it from the cited code alone; absent for a confirmed finding" }),
  ),
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
      basis: Type.Optional(
        Type.String({
          description:
            "why the run observed this state: reverified, located, changed, file_gone, resolution_checked, resolution_refused, reported_unverified, or reopened for the first observation of a finding seeded from a resolved one whose code came back; absent on any other first observation and on a publication",
        }),
      ),
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

/**
 * A GitHub identity the daemon can see. `id` is `<host>/<login>`. `env/GITHUB_TOKEN` or `env/GH_TOKEN` is the daemon's
 * environment token, which overrides every gh account and cannot be selected. A selected account gh no longer lists is still listed, with
 * state error, so the client can show why GitHub reads fail.
 */
export const Account = Type.Object({
  id: Id,
  host: Type.String(),
  login: Type.String(),
  state: Type.Union([Type.Literal("ok"), Type.Literal("error")]),
  ghActive: Type.Boolean({ description: "gh's own active account for this host" }),
  selected: Type.Boolean({ description: "the account this data dir uses for GitHub reads and publishing" }),
  scopes: Type.Array(Type.String()),
});
export type Account = Static<typeof Account>;

/**
 * One readiness check of the daemon's environment, as `pr-review setup --check` reports it: `missing` means reviews
 * cannot run, `warn` that they run with a limitation. `id` is open, so a newer daemon may add checks.
 */
export const SetupCheck = Type.Object({
  id: Type.String(),
  status: oneOf(CHECK_STATUSES),
  detail: Type.String(),
  fix: Type.Optional(Type.String({ description: "what to run or set, for display; never a secret" })),
});
export type SetupCheck = Static<typeof SetupCheck>;

/** Where the model new reviews get was chosen: the daemon's `--model` flag, its environment, its config file, or the first available. */
export const MODEL_SOURCES = ["--model", "PR_REVIEW_MODEL", "config", "default"] as const;

/** A model the built-in engine can review with: a reasoning model whose provider has a credential. */
export const ModelOption = Type.Object({
  ref: Type.String({ minLength: 1, description: "provider/id, what settings.set names" }),
  provider: Type.String(),
  name: Type.String(),
  contextWindow: Type.Integer({ minimum: 0 }),
  authSource: Type.String({ description: "where the provider's credential comes from, as pi-ai reports it; never the credential" }),
  preferred: Type.Boolean({ description: "matched by models.preferred in the config; preferred models are listed first" }),
});
export type ModelOption = Static<typeof ModelOption>;

const RepoName = Type.String({ pattern: "^[\\w.-]+/[\\w.-]+$", maxLength: 200, description: "owner/name" });
const RepoList = Type.Array(RepoName, { maxItems: 200 });
const IntervalMinutes = Type.Union([Type.Literal(5), Type.Literal(15), Type.Literal(30)]);
const DailyCap = Type.Integer({ minimum: 0, description: "automatic starts allowed per local day of the daemon; 0 means no cap" });

/** What the daemon's last poll for review requests saw. `error` says why it failed; it is retried with backoff. */
export const AutoReviewPoll = Type.Object({
  at: Timestamp,
  found: Type.Integer({ minimum: 0, description: "open, non-draft PRs requesting the account's review" }),
  started: Type.Integer({ minimum: 0, description: "reviews it started; at most 5 a poll, the rest wait for the next" }),
  skipped: Type.Integer({ minimum: 0, description: "found but not started: reviewed at their head, archived, excluded, closed, or past a cap" }),
  startedToday: Type.Integer({ minimum: 0, description: "automatic starts since local midnight, this poll's included" }),
  capReached: Type.Optional(Type.Boolean({ description: "true when startedToday has reached a nonzero dailyCap: no more start until the next local day" })),
  error: Type.Optional(Type.String()),
});
export type AutoReviewPoll = Static<typeof AutoReviewPoll>;

/** A repository the daemon's account can reach, as `repos.list` lists it. */
export const AccessibleRepo = Type.Object({ fullName: RepoName, private: Type.Boolean(), archived: Type.Boolean(), pushedAt: Type.Optional(Timestamp) });
export type AccessibleRepo = Static<typeof AccessibleRepo>;

/** A `{{name}}` the review summary's template may use, and what it looks like filled in. */
export const TemplateVariable = Type.Object({ name: Type.String({ minLength: 1 }), description: Type.String(), sample: Type.String() });

/** One inline comment of the stored review payload, exactly as it will be posted. */
export const ReviewComment = Type.Object({ path: Type.String(), line: Type.Integer({ minimum: 1 }), side: Type.Literal("RIGHT"), body: Type.String() });

/**
 * What a run's publication looks like now: `awaiting_approval` holds the exact payload for a human, `publishing`
 * means the exactly-once publisher owns it, `uncertain` means a write may have landed and reconciliation decides.
 */
export const PublicationChange = Type.Object({
  runId: Id,
  status: oneOf(PUBLICATION_STATUSES),
  reviewId: Type.Optional(Type.Integer({ minimum: 1, description: "the GitHub review, once one is known" })),
  url: Type.Optional(Type.String()),
});
export type PublicationChange = Static<typeof PublicationChange>;

const Count = Type.Integer({ minimum: 0 });
const usageTotals = {
  reviews: Type.Integer({ minimum: 0, description: "distinct runs with at least one recorded model response" }),
  responses: Type.Integer({ minimum: 0, description: "model responses, each retry and error answer included" }),
  input: Count,
  output: Count,
  cacheRead: Count,
  cacheWrite: Count,
  cost: Type.Number({ minimum: 0, description: "USD at the model's catalog price, rounded to 6 decimals" }),
};
const LocalDate = Type.String({ pattern: "^\\d{4}-\\d{2}-\\d{2}$", description: "a calendar date in the report's timeZone" });

/** One day, ISO week (Monday first) or month of model usage; `start` and `end` are both inside it. */
export const UsageBucket = Type.Object({ start: LocalDate, end: LocalDate, ...usageTotals });
export const UsageTotals = Type.Object(usageTotals);
export const UsageModel = Type.Object({ model: Type.String({ minLength: 1, description: "provider/id" }), ...usageTotals });
/** One repository, PR author or PR of a usage breakdown. */
export const UsageGroup = Type.Object({
  key: Type.String({ minLength: 1, description: "unique within the breakdown" }),
  label: Type.String({ description: "display text: the repository, the login, the PR's title, or Unknown author / Unknown PR" }),
  repository: Type.Optional(Type.String()),
  number: Type.Optional(Type.Integer({ minimum: 1 })),
  title: Type.Optional(Type.String()),
  prId: Type.Optional(Id),
  author: Type.Optional(Type.String()),
  ...usageTotals,
  share: Type.Number({ minimum: 0, maximum: 1, description: "this row's part of the range's cost, rounded to 6 decimals; 0 when the range cost nothing" }),
});

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
  "review.publication_changed",
  "accounts.updated",
  "heartbeat",
] as const;

const Phase = oneOf(REVIEW_STATES);

export const SPAN_KINDS = ["phase", "step", "model", "tool"] as const;
/**
 * The timed piece of work a `tool.activity` line belongs to, for the waterfall. A span opens with a line that has no
 * `endedAt` and closes with a later line of the same `id` that has one; a line with both is a span that opened and
 * closed at once. `parent` is the id of the span it ran inside. Counts and timings only, never model text.
 */
export const ActivitySpan = Type.Object({
  id: Type.String({ minLength: 1, maxLength: 64 }),
  parent: Type.Optional(Type.String({ minLength: 1, maxLength: 64 })),
  kind: oneOf(SPAN_KINDS),
  startedAt: Timestamp,
  endedAt: Type.Optional(Timestamp),
  outputTokens: Type.Optional(Type.Integer({ minimum: 0 })),
});
export type ActivitySpan = Static<typeof ActivitySpan>;

export const KnownEvent = Type.Union([
  event("pr.updated", Pr, { prId: Id }),
  event("run.started", Run, { prId: Id, runId: Id }),
  event(
    "review.phase_changed",
    Type.Object({ from: Phase, to: Phase, event: Type.String(), detail: Type.Union([Type.String(), Type.Null()]) }),
    { prId: Id, runId: Id },
  ),
  event("finding.updated", Finding, { prId: Id, runId: Id }),
  event("tool.activity", Type.Object({ tool: Type.String(), summary: Type.String({ maxLength: 200 }), span: Type.Optional(ActivitySpan) }), { prId: Id, runId: Id }),
  event("review.completed", Run, { prId: Id, runId: Id }),
  event("run.superseded", Run, { prId: Id, runId: Id }),
  event("run.failed", Run, { prId: Id, runId: Id }),
  event("review.publication_changed", PublicationChange, { prId: Id, runId: Id }),
  event("accounts.updated", Type.Object({ accounts: Type.Array(Account) }), {}),
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
  command("review.add", { url: Type.String({ minLength: 1, maxLength: 512, description: "a GitHub pull request URL or owner/repo#N" }) }),
  command("review.start", { prId: Id, expectedHeadSha: Sha }),
  command("review.again", { prId: Id, expectedHeadSha: Sha }),
  command("review.cancel", { runId: Id }),
  command("review.retry", { runId: Id }),
  command("review.publish", { runId: Id, previewId: Id, expectedHeadSha: Sha }),
  command("account.select", { accountId: Id }),
  command("pr.archive", { prId: Id }),
  command("pr.unarchive", { prId: Id }),
  command("settings.set", {
    model: Type.Optional(
      Type.String({
        minLength: 1,
        maxLength: 256,
        description: "one of settings.models; saved as models.default in the daemon's config and used for reviews started after it",
      }),
    ),
    summaryTemplate: Type.Optional(
      Type.Union([Type.String({ maxLength: MAX_TEMPLATE_CHARS }), Type.Null()], {
        description: "saved as review.summary_template and used for review bodies prepared after it; null or empty restores the default",
      }),
    ),
    autoReview: Type.Optional(
      Type.Object(
        {
          enabled: Type.Optional(Type.Boolean()),
          intervalMinutes: Type.Optional(IntervalMinutes),
          includeTeamRequests: Type.Optional(Type.Boolean()),
          repos: Type.Optional(Type.Object({ include: Type.Optional(RepoList), exclude: Type.Optional(RepoList) }, { additionalProperties: false })),
          dailyCap: Type.Optional(DailyCap),
        },
        { additionalProperties: false, description: "saved under review.auto_review; only the fields given change" },
      ),
    ),
  }),
  command("daemon.stop", {
    pauseReviews: Type.Boolean({ description: "stop even while reviews run: they pause and resume on the next engine start; without it a running review makes the stop CONFLICT" }),
  }),
]);
export type Command = Static<typeof Command>;
export type ReviewCommand = Exclude<Command, { command: "account.select" | "pr.archive" | "pr.unarchive" | "daemon.stop" | "settings.set" }>;

export type AutoReviewParams = NonNullable<Extract<Command, { command: "settings.set" }>["params"]["autoReview"]>;

/**
 * `settings.set:<model>`, then `:template:<hash>` when it sets the template: different texts are different keys.
 * The hash is FNV-1a 64 of the template's UTF-8 bytes in hex, or `default` for a reset; a client computes it too.
 * Then `:auto:<hash>` when it sets autoReview: FNV-1a 64 of its compact JSON with the keys in schema order (enabled,
 * intervalMinutes, includeTeamRequests, repos: include, exclude, dailyCap) and absent ones left out.
 */
export function settingsKey(p: { model?: string; summaryTemplate?: string | null; autoReview?: AutoReviewParams }): string {
  const template = p.summaryTemplate === undefined ? "" : `:template:${p.summaryTemplate ? fnv1a64(p.summaryTemplate) : "default"}`;
  const a = p.autoReview;
  const canonical = a && { enabled: a.enabled, intervalMinutes: a.intervalMinutes, includeTeamRequests: a.includeTeamRequests, repos: a.repos && { include: a.repos.include, exclude: a.repos.exclude }, dailyCap: a.dailyCap };
  const auto = canonical === undefined ? "" : `:auto:${fnv1a64(JSON.stringify(canonical))}`;
  return `settings.set:${p.model ?? ""}${template}${auto}`;
}

export function fnv1a64(text: string): string {
  let h = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) h = BigInt.asUintN(64, (h ^ BigInt(byte)) * 0x100000001b3n);
  return h.toString(16).padStart(16, "0");
}

/** The one key a command may carry: it binds the key to the target, so a replayed key is the same request. */
export function idempotencyKey(c: Command): string {
  switch (c.command) {
    case "review.add":
      return `review.add:${c.params.url}`;
    case "review.start":
      return `review.start:${c.params.prId}:${c.params.expectedHeadSha}`;
    case "review.again":
      return `review.again:${c.params.prId}:${c.params.expectedHeadSha}`;
    case "review.cancel":
      return `review.cancel:${c.params.runId}`;
    case "review.retry":
      return `review.retry:${c.params.runId}`;
    case "review.publish":
      return `review.publish:${c.params.runId}:${c.params.previewId}`;
    case "account.select":
      return `account.select:${c.params.accountId}`;
    case "pr.archive":
    case "pr.unarchive":
      return `${c.command}:${c.params.prId}`;
    case "settings.set":
      return settingsKey(c.params);
    case "daemon.stop":
      return "daemon.stop";
  }
}

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
  req("publication.get", { runId: Id }),
  req("settings.get"),
  req("repos.list"),
  req("usage.get", {
    by: oneOf(USAGE_BY),
    periods: Type.Optional(Type.Integer({ minimum: 1, maximum: 400, description: "how many buckets up to the current one; 14 days, 12 weeks or 12 months when absent" })),
    group: Type.Optional(oneOf(USAGE_GROUPS)),
  }),
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
    mode: Type.Optional(oneOf(DAEMON_MODES)),
    readOnlyReason: Type.Optional(Type.String({ description: "why a read-only daemon is read-only (commands off, another pr-review holds the data dir, or the engine failed to open), for display" })),
    checks: Type.Optional(Type.Array(SetupCheck, { description: "the daemon's readiness checks, computed for this hello; absent from older servers and fixture servers" })),
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
    accounts: Type.Optional(Type.Array(Account, { description: "absent from servers that predate account selection" })),
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
  // TUI_SPEC §5.5: the exact summary and inline comments, the target SHA, and the hash the approval must name.
  reply("publication.preview", {
    runId: Id,
    previewId: Id,
    headSha: Sha,
    body: Type.String(),
    comments: Type.Array(ReviewComment),
    event: Type.Literal("COMMENT"),
    status: oneOf(PUBLICATION_STATUSES),
    reviewId: Type.Optional(Type.Integer({ minimum: 1, description: "the GitHub review, once published" })),
    url: Type.Optional(Type.String()),
  }),
  // The answer to settings.get, which only a daemon in engine mode gives: the model new reviews get and the choices.
  reply("settings", {
    model: Type.Object({ ref: Type.String({ minLength: 1 }), source: oneOf(MODEL_SOURCES) }),
    models: Type.Array(ModelOption),
    configPath: Type.String({ description: "the config file settings.set writes" }),
    summaryTemplate: Type.Optional(
      Type.Object({
        value: Type.String({ description: "the template in force: the saved one, or the default" }),
        isDefault: Type.Boolean(),
        default: Type.String(),
        variables: Type.Array(TemplateVariable, { description: "every {{name}} a template may use; any other name is refused" }),
      }),
    ),
    publishMode: Type.Optional(oneOf(PUBLISH_MODES)),
    autoReview: Type.Optional(
      Type.Object({
        enabled: Type.Boolean(),
        intervalMinutes: IntervalMinutes,
        includeTeamRequests: Type.Boolean(),
        repos: Type.Object({ include: RepoList, exclude: RepoList }, { description: "an empty include list means every repository" }),
        dailyCap: DailyCap,
        lastPoll: Type.Optional(AutoReviewPoll),
      }),
    ),
  }),
  // The answer to repos.list, which only a daemon in engine mode gives: the repositories its account can reach.
  reply("repos.list.result", {
    repos: Type.Array(AccessibleRepo, { maxItems: 1000, description: "most recently pushed first, archived ones left out" }),
    fetchedAt: Timestamp,
    error: Type.Optional(Type.String({ description: "why GitHub could not be read; repos is then empty" })),
  }),
  // The answer to usage.get, which every daemon gives: recorded model usage, oldest bucket first, empty ones included.
  reply("usage.report", {
    by: oneOf(USAGE_BY),
    timeZone: Type.String({ description: "the daemon's IANA time zone; buckets follow its calendar" }),
    currency: Type.Literal("USD"),
    priceSource: Type.Literal("catalog", { description: "costs are the model's list price as pi-ai's catalog has it, not what a subscription is billed" }),
    buckets: Type.Array(UsageBucket),
    totals: UsageTotals,
    models: Type.Array(UsageModel, { description: "the whole range split by model, costliest first" }),
    group: Type.Optional(oneOf(USAGE_GROUPS)),
    groups: Type.Optional(Type.Array(UsageGroup, { description: "the whole range split by group, costliest first; absent when group is time" })),
  }),
  reply("timeline.page", {
    events: Type.Array(Type.Union([KnownEvent, UnknownEvent]), { description: "newest first" }),
    nextBeforeSequence: Type.Optional(Type.Integer({ minimum: 1 })),
  }),
  reply("command.result", {
    status: Type.Literal("accepted"),
    operationId: Id,
    prId: Type.Optional(Id),
    runId: Type.Optional(Type.String({ minLength: 1, description: "with prId, set by review.add: the PR it tracks and the run it started" })),
  }),
  reply("command.result", {
    status: Type.Literal("rejected"),
    code: oneOf(ERROR_CODES),
    message: Type.String(),
    prId: Type.Optional(Type.String({ minLength: 1, description: "set with CONFLICT when review.add names a PR the daemon already tracks" })),
  }),
  Type.Object({
    type: Type.Literal("error"),
    requestId: OptionalRequestId,
    code: oneOf(ERROR_CODES),
    message: Type.String(),
    supported: Type.Optional(Type.Array(Type.Integer(), { description: "set with PROTOCOL_MISMATCH" })),
  }),
]);
export type ServerMessage = Static<typeof ServerMessage>;
export type ReposAnswer = Omit<Extract<ServerMessage, { type: "repos.list.result" }>, "type" | "requestId">;
export type UsageAnswer = Omit<Extract<ServerMessage, { type: "usage.report" }>, "type" | "requestId">;
export type PublicationPreview = Omit<Extract<ServerMessage, { type: "publication.preview" }>, "type" | "requestId">;

export const ENTITIES = { pr: Pr, run: Run, finding: Finding, "finding-detail": FindingDetail, evidence: Evidence, "diff-file": DiffFile, account: Account, check: SetupCheck, "model-option": ModelOption, event: Event } as const;
