import { mkdirSync, readdirSync, readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { transition } from "../domain/machine.ts";
import {
  type CoordinatorState,
  type FindingState,
  OUTBOX_MACHINE,
  type OutboxEvent,
  type OutboxStatus,
  publicationStatus,
  REVIEW_EVENTS,
  REVIEW_MACHINE,
  type ReviewEvent,
  type ReviewState,
  TERMINAL_REVIEW_STATES,
} from "../domain/machines.ts";
import type { Usage } from "@earendil-works/pi-ai";
import type { UsageRow } from "../domain/usage.ts";
import { type CandidateFinding, type ChangedFile, type Disposition, type PullInfo, type ResolutionClaim, type ResolutionVerdict, stableId } from "../domain/types.ts";
import { sanitize } from "../protocol/sanitize.ts";
import type { ActivitySpan } from "../protocol/v1.ts";
import type { Observation, ObservedLocation, Reopened } from "../reasoning/track.ts";

/** How many tool calls and plain activity lines one revision journals a second; see `appendActivity`. */
export const ACTIVITY_PER_SECOND = 10;

const MIGRATIONS_DIR = new URL("../../migrations/", import.meta.url);

export type RevisionRow = {
  id: string;
  pr_id: string;
  base_sha: string;
  head_sha: string;
  policy_version: string;
  reviewer_version: string;
  state: ReviewState;
  durable_task_id: string | null;
  code_read_at: string | null;
  created_at: string;
  completed_at: string | null;
  /** Set by a TUI cancel before its abort; the abort handler reports it as the failure reason. */
  cancel_reason: string | null;
};

/** The audited events that reopen a revision; each starts its own durable task attempt. */
export const REOPENINGS = ["MANUAL_RETRY", "APPROVED"] as const;
export type Reopening = (typeof REOPENINGS)[number];

/** `attempt` is the run attempt the key is bound to (migration 053); null when the key names a head, not an attempt. */
export type CommandRecord = { command: string; operationId: string; attempt: number | null };

export type PullRow = {
  id: string;
  installation_id: number;
  repository_id: number;
  number: number;
  status: CoordinatorState;
  review_due_at: number | null;
  latest_base_sha: string;
  latest_head_sha: string;
  repo_full_name: string;
  /** NULL on rows written before migration 054 until GitHub reports the PR again. */
  title: string | null;
  /** When a reader archived the PR (migration 059); NULL when it is not archived. */
  archived_at: string | null;
};

export type Snapshot = {
  id: string;
  revisionId: string;
  pr: { title: string; body: string; fullName: string; number: number; cloneUrl: string };
  changedFiles: ChangedFile[];
  diff: string;
  limitations: string[];
};

export type DecisionRecord = {
  id: string;
  revisionId: string;
  inputHash: string;
  provider: string;
  modelId: string;
  rawResponse: unknown;
  error: string | null;
  normalized: unknown;
  selectedMode: string;
  reasons: string[];
};

/** A finding as one revision observed it. `candidate` is the evidence as of that observation, relocated when the code moved. */
export type FindingView = {
  findingId: string;
  fingerprint: string;
  status: string;
  severity: string;
  candidate: CandidateFinding;
  /** First reported by an earlier revision. */
  carried: boolean;
  firstHeadSha: string;
  /** An inline comment is on GitHub, or in an outbox that may still reach it; such a finding is never commented again. */
  posted: boolean;
  /** An earlier review that carries it in its body is on GitHub, or in an outbox that may still reach it: the reader has seen it. */
  listed: boolean;
  /** `candidate.file` and `newLine` are valid at this revision's head, not carried from an older one. */
  anchored: boolean;
  /** The resolved finding this one reopens, when its code came back after a resolution, and the head it was resolved on. */
  reopens: Reopened | null;
};

/** An open finding at its latest observation before `excludeRevisionId`, with the head that observation was made on. */
export type OpenFinding = {
  findingId: string;
  status: FindingState;
  candidate: CandidateFinding;
  /** Rows written before migration 055 have no `headSha` and no `evidence`. */
  location: ObservedLocation | Pick<ObservedLocation, "file" | "newLine">;
  observedAt: string;
};

export type OutboxRow = {
  id: string;
  revision_id: string;
  idempotency_key: string;
  payload_hash: string;
  payload_json: string;
  status: OutboxStatus;
  github_review_id: number | null;
  attempt_count: number;
  last_error: string | null;
  updated_at: string;
  /** The findings the payload comments on; NULL on a `p1` outbox, whose payload commented on every finding of its revision. */
  inline_finding_ids_json: string | null;
};

export type OutboxEventRow = { from_status: OutboxStatus; event: OutboxEvent; to_status: OutboxStatus; detail: string | null };

export type WebhookRow = { delivery_id: string; event_type: string; payload_json: string; attempts: number };
export type DeadWebhookRow = { delivery_id: string; event_type: string; attempts: number; last_error: string | null; processed_at: string };

export type AuditEvent = { seq: number; from_state: string; event: string; to_state: string; detail: string | null; at: string };

/** The revision is not where the caller believed: another actor moved it, or a publication guard refused the step. */
export class StateConflict extends Error {}

const now = () => new Date().toISOString();

/** Outbox events whose detail is why publication is not done; `last_error` keeps the latest one. */
const OUTBOX_ERRORS: readonly OutboxEvent[] = ["NOT_SENT", "UNCERTAIN", "REJECTED", "ABANDON", "UNDECIDED"];
/** The journal also holds stream-only rows (tool activity, publication status); a revision's audit trail is its transitions. */
export const TRANSITIONS = `event IN (${REVIEW_EVENTS.map((e) => `'${e}'`).join(", ")})`;

export class AppStore {
  readonly db: DatabaseSync;
  private readonly recentActivity = new Map<string, number[]>();

  private constructor(db: DatabaseSync) {
    this.db = db;
  }

  static open(path: string): AppStore {
    const db = new DatabaseSync(path);
    db.exec("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
    db.exec("CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL)");
    const store = new AppStore(db);
    for (const name of readdirSync(MIGRATIONS_DIR).filter((f) => f.endsWith(".sql")).sort()) {
      store.tx(() => {
        if (db.prepare("SELECT 1 FROM schema_migrations WHERE name = ?").get(name)) return;
        db.exec(readFileSync(new URL(name, MIGRATIONS_DIR), "utf8"));
        db.prepare("INSERT INTO schema_migrations (name, applied_at) VALUES (?, ?)").run(name, now());
      });
    }
    return store;
  }

  close(): void {
    this.db.close();
  }

  /** One transaction; nested calls join the outer one. */
  tx<T>(fn: () => T): T {
    if (this.db.isTransaction) return fn();
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const result = fn();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }

  /** Insert or refresh a PR. `status` is the coordinator machine's state: set from GitHub on insert, then the coordinator's. */
  upsertPull(pr: PullInfo): string {
    const id = stableId("pr", pr.installationId, pr.repositoryId, pr.number);
    const status = pr.merged ? "MERGED" : pr.state === "closed" ? "CLOSED" : "ACTIVE";
    this.db
      .prepare(
        `INSERT INTO pull_requests (id, installation_id, repository_id, number, status, latest_base_sha, latest_head_sha, updated_at, repo_full_name, title)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET latest_base_sha = excluded.latest_base_sha,
           latest_head_sha = excluded.latest_head_sha, updated_at = excluded.updated_at, repo_full_name = excluded.repo_full_name,
           title = excluded.title`,
      )
      .run(id, pr.installationId, pr.repositoryId, pr.number, status, pr.baseSha, pr.headSha, now(), pr.fullName, pr.title);
    return id;
  }

  pullById(id: string): PullRow | undefined {
    return this.db.prepare("SELECT * FROM pull_requests WHERE id = ?").get(id) as PullRow | undefined;
  }

  pullByName(fullName: string, number: number): PullRow | undefined {
    return this.db
      .prepare("SELECT * FROM pull_requests WHERE lower(repo_full_name) = lower(?) AND number = ?")
      .get(fullName, number) as PullRow | undefined;
  }

  ensureRevision(key: { prId: string; baseSha: string; headSha: string; policyVersion: string; reviewerVersion: string }): RevisionRow {
    const id = stableId("revision", key.prId, key.baseSha, key.headSha, key.policyVersion, key.reviewerVersion);
    this.db
      .prepare(
        `INSERT INTO review_revisions (id, pr_id, base_sha, head_sha, policy_version, reviewer_version, state, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 'COLLECT_CONTEXT', ?) ON CONFLICT (id) DO NOTHING`,
      )
      .run(id, key.prId, key.baseSha, key.headSha, key.policyVersion, key.reviewerVersion, now());
    return this.revision(id)!;
  }

  revision(id: string): RevisionRow | undefined {
    return this.db.prepare("SELECT * FROM review_revisions WHERE id = ?").get(id) as RevisionRow | undefined;
  }

  revisionsForPr(prId: string): RevisionRow[] {
    return this.db.prepare("SELECT * FROM review_revisions WHERE pr_id = ? ORDER BY created_at, id").all(prId) as RevisionRow[];
  }

  markCodeRead(revisionId: string): void {
    this.db.prepare("UPDATE review_revisions SET code_read_at = ? WHERE id = ? AND code_read_at IS NULL").run(now(), revisionId);
  }

  setDurableTask(revisionId: string, taskId: string): void {
    this.db.prepare("UPDATE review_revisions SET durable_task_id = ? WHERE id = ?").run(taskId, revisionId);
  }

  /**
   * Apply `event` to a revision that is in `from`, with `writes` in the same transaction. A revision already in the
   * target state means an earlier attempt committed this step before crashing, so it converges without writing again.
   */
  advance(revisionId: string, from: ReviewState, event: ReviewEvent, detail: string | null = null, writes?: () => void): ReviewState {
    const result = transition(REVIEW_MACHINE, from, event);
    if (!result.ok) throw new Error(result.reason);
    return this.tx(() => {
      const current = this.revision(revisionId)?.state;
      if (current === result.state) return current;
      if (current !== from) throw new StateConflict(`revision ${revisionId} is ${current}, expected ${from} for ${event}`);
      writes?.();
      if (!this.recordTransition(revisionId, from, event, result.state, detail)) throw new StateConflict(`${event} refused in ${from}: a review write may have reached GitHub`);
      return result.state;
    });
  }

  /** Apply `event` from whatever state the revision is in; `undefined` when it is not legal there. */
  advanceFromCurrent(revisionId: string, event: ReviewEvent, detail: string | null = null): ReviewState | undefined {
    return this.tx(() => {
      const current = this.revision(revisionId)!.state;
      const result = transition(REVIEW_MACHINE, current, event);
      if (!result.ok || !this.recordTransition(revisionId, current, event, result.state, detail)) return undefined;
      return result.state;
    });
  }

  /** False when the publication guard refuses the transition; the caller's transaction then writes nothing. */
  private recordTransition(revisionId: string, from: ReviewState, event: ReviewEvent, to: ReviewState, detail: string | null): boolean {
    if (!this.guardPublication(revisionId, from, event)) return false;
    const at = now();
    this.db
      .prepare("UPDATE review_revisions SET state = ?, completed_at = ? WHERE id = ?")
      .run(to, TERMINAL_REVIEW_STATES.includes(to) ? at : null, revisionId);
    // The new attempt's abort must report its own reason, not the cancel that ended the last one.
    if ((REOPENINGS as readonly string[]).includes(event)) this.db.prepare("UPDATE review_revisions SET cancel_reason = NULL WHERE id = ?").run(revisionId);
    this.db
      .prepare("INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (?, ?, ?, ?, ?, ?)")
      .run(revisionId, from, event, to, detail, at);
    const outbox = this.outboxForRevision(revisionId);
    if (outbox) this.journalPublication(revisionId, publicationStatus(to, outbox.status), at);
    return true;
  }

  /**
   * §4.4 guards between a revision and its outbox. A superseded or cancelled publication is only legal while nothing
   * was sent, and then the outbox is abandoned with it. A revision that fails while a POST is in flight leaves the
   * outbox to reconciliation. A manual retry reopens an outbox GitHub definitely rejected.
   */
  private guardPublication(revisionId: string, from: ReviewState, event: ReviewEvent): boolean {
    const outbox = this.outboxForRevision(revisionId);
    if (!outbox) return true;
    const unsent = from === "PUBLISH" || from === "AWAITING_APPROVAL";
    if (unsent && (event === "NEWER_HEAD" || event === "PR_NOT_OPEN" || event === "CANCELLED")) {
      if (outbox.status !== "pending") return false;
      this.advanceOutbox(outbox.id, "ABANDON", event === "CANCELLED" ? "cancelled" : "superseded");
    }
    if (event === "TERMINAL_ERROR" && outbox.status === "sending") this.advanceOutbox(outbox.id, "UNCERTAIN", "revision failed while the POST was in flight");
    if (event === "MANUAL_RETRY" && outbox.status === "failed") this.advanceOutbox(outbox.id, "MANUAL_RETRY", "manual retry of the revision");
    // A review again never sets aside a publication that may have reached GitHub; an unsent one is abandoned with it.
    if (event === "REVIEW_AGAIN") {
      if (outbox.status === "sending" || outbox.status === "needs_reconciliation") return false;
      if (outbox.status === "pending") this.advanceOutbox(outbox.id, "ABANDON", "set aside by a review again");
    }
    return true;
  }

  /**
   * One journal row (`PUBLICATION_CHANGED`, detail = the derived status) per change of a revision's publication status,
   * for the control stream's `review.publication_changed`. Written from every outbox write and revision transition, and
   * deduplicated against the last row, so a replay of either converges.
   */
  private journalPublication(revisionId: string, status: string | null, at: string): void {
    if (status === null) return;
    const last = this.db
      .prepare("SELECT detail FROM audit_events WHERE revision_id = ? AND event = 'PUBLICATION_CHANGED' ORDER BY seq DESC LIMIT 1")
      .get(revisionId) as { detail: string } | undefined;
    if (last?.detail === status) return;
    this.db
      .prepare("INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) SELECT id, state, 'PUBLICATION_CHANGED', state, ?, ? FROM review_revisions WHERE id = ?")
      .run(status, at, revisionId);
  }

  /**
   * Insert the canonical payload once per idempotency key; a rerun gets the row that won. Called inside the transition
   * out of PREPARE_PUBLICATION, which journals the publication status once the revision is in its new state.
   */
  ensureOutbox(row: { revisionId: string; idempotencyKey: string; payloadHash: string; payloadJson: string; inlineFindingIds: string[] }): OutboxRow {
    this.db
      .prepare(
        `INSERT INTO publication_outbox (id, revision_id, idempotency_key, payload_hash, payload_json, status, updated_at, inline_finding_ids_json)
         VALUES (?, ?, ?, ?, ?, 'pending', ?, ?) ON CONFLICT (idempotency_key) DO NOTHING`,
      )
      .run(stableId("outbox", row.idempotencyKey), row.revisionId, row.idempotencyKey, row.payloadHash, row.payloadJson, now(), JSON.stringify(row.inlineFindingIds));
    return this.db.prepare("SELECT * FROM publication_outbox WHERE idempotency_key = ?").get(row.idempotencyKey) as OutboxRow;
  }

  outbox(id: string): OutboxRow | undefined {
    return this.db.prepare("SELECT * FROM publication_outbox WHERE id = ?").get(id) as OutboxRow | undefined;
  }

  outboxForRevision(revisionId: string): OutboxRow | undefined {
    return this.db.prepare("SELECT * FROM publication_outbox WHERE revision_id = ? ORDER BY updated_at DESC LIMIT 1").get(revisionId) as OutboxRow | undefined;
  }

  /** Outboxes where a write may have reached GitHub: recovery reconciles these before anything posts again (§11). */
  uncertainOutboxes(): OutboxRow[] {
    return this.db.prepare("SELECT * FROM publication_outbox WHERE status IN ('sending', 'needs_reconciliation') ORDER BY updated_at").all() as OutboxRow[];
  }

  /** Apply an outbox event in its own audited transaction (or the caller's). `SEND` counts the attempt in the same write. */
  advanceOutbox(id: string, event: OutboxEvent, detail: string | null = null, githubReviewId?: number): OutboxStatus {
    return this.tx(() => {
      const current = this.outbox(id)!.status;
      const result = transition(OUTBOX_MACHINE, current, event);
      if (!result.ok) throw new Error(`outbox ${id}: ${result.reason}`);
      const at = now();
      this.db
        .prepare(
          `UPDATE publication_outbox SET status = ?, updated_at = ?, attempt_count = attempt_count + ?,
             github_review_id = coalesce(?, github_review_id), last_error = ? WHERE id = ?`,
        )
        .run(result.state, at, event === "SEND" ? 1 : 0, githubReviewId ?? null, OUTBOX_ERRORS.includes(event) ? detail : null, id);
      this.db
        .prepare("INSERT INTO outbox_events (outbox_id, from_status, event, to_status, detail, at) VALUES (?, ?, ?, ?, ?, ?)")
        .run(id, current, event, result.state, detail, at);
      const revisionId = this.outbox(id)!.revision_id;
      this.journalPublication(revisionId, publicationStatus(this.revision(revisionId)!.state, result.state), at);
      return result.state;
    });
  }

  outboxEvents(id: string): OutboxEventRow[] {
    return this.db.prepare("SELECT from_status, event, to_status, detail FROM outbox_events WHERE outbox_id = ? ORDER BY seq").all(id) as OutboxEventRow[];
  }

  /**
   * A published outbox's review is on GitHub: it becomes the review of record for the findings its payload commented
   * on (first comment wins), and the revision's findings still DETECTED are PUBLISHED (§4.3). A finding past DETECTED
   * keeps its tracked state; the review column, not the status, says it was posted.
   */
  markFindingsPublished(outboxId: string): void {
    const outbox = this.outbox(outboxId)!;
    for (const id of this.inlineFindingIds(outbox)) this.db.prepare("UPDATE findings SET github_review_id = ? WHERE id = ? AND github_review_id IS NULL").run(outbox.github_review_id, id);
    this.db
      .prepare(
        `UPDATE findings SET status = 'PUBLISHED', updated_at = ? WHERE status = 'DETECTED'
         AND id IN (SELECT finding_id FROM finding_observations WHERE revision_id = ?)`,
      )
      .run(now(), outbox.revision_id);
  }

  /**
   * The findings an outbox's payload comments on. A `p1` outbox recorded none, so they are read off its payload: the
   * revision's findings observed at a file and line one of its comments sits on.
   */
  private inlineFindingIds(outbox: Pick<OutboxRow, "revision_id" | "payload_json" | "inline_finding_ids_json">): string[] {
    if (outbox.inline_finding_ids_json !== null) return JSON.parse(outbox.inline_finding_ids_json) as string[];
    const rows = this.db
      .prepare(
        `SELECT fo.finding_id FROM finding_observations fo, json_each(?, '$.comments') c
         WHERE fo.revision_id = ? AND json_extract(c.value, '$.path') = json_extract(fo.location_json, '$.file')
         AND json_extract(c.value, '$.line') = json_extract(fo.location_json, '$.newLine')`,
      )
      .all(outbox.payload_json, outbox.revision_id) as { finding_id: string }[];
    return rows.map((r) => r.finding_id);
  }

  retryCount(revisionId: string): number {
    return (this.db.prepare("SELECT count(*) AS n FROM audit_events WHERE revision_id = ? AND event = 'MANUAL_RETRY'").get(revisionId) as { n: number }).n;
  }

  /** The revision's reopenings in order; their count is the durable task attempt, and the last one says where it starts. */
  reopenings(revisionId: string): Reopening[] {
    return (this.db.prepare(`SELECT event FROM audit_events WHERE revision_id = ? AND event IN (${REOPENINGS.map((e) => `'${e}'`).join(", ")}) ORDER BY seq`).all(revisionId) as { event: Reopening }[]).map((r) => r.event);
  }

  /** Mark a revision as cancelled by a human before its durable task is aborted; the abort handler reports `reason`. */
  requestCancel(revisionId: string, reason: string): void {
    this.db.prepare("UPDATE review_revisions SET cancel_reason = ? WHERE id = ?").run(reason, revisionId);
  }

  /** The accepted result recorded under a control command's idempotency key, if any. */
  commandResult(idempotencyKey: string): CommandRecord | undefined {
    const row = this.db.prepare("SELECT command, result_json, attempt FROM control_commands WHERE idempotency_key = ?").get(idempotencyKey) as
      | { command: string; result_json: string; attempt: number | null }
      | undefined;
    return row ? { command: row.command, operationId: (JSON.parse(row.result_json) as { operationId: string }).operationId, attempt: row.attempt } : undefined;
  }

  /** Record an accepted control command for its attempt; a replay of the key on that attempt answers this result and does nothing else. */
  recordCommand(idempotencyKey: string, record: CommandRecord): void {
    this.db
      .prepare(
        `INSERT INTO control_commands (idempotency_key, command, result_json, attempt, created_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (idempotency_key) DO UPDATE SET result_json = excluded.result_json, attempt = excluded.attempt, created_at = excluded.created_at`,
      )
      .run(idempotencyKey, record.command, JSON.stringify({ operationId: record.operationId }), record.attempt, now());
  }

  auditEvents(revisionId: string): AuditEvent[] {
    return this.db
      .prepare(`SELECT seq, from_state, event, to_state, detail, at FROM audit_events WHERE revision_id = ? AND ${TRANSITIONS} ORDER BY seq`)
      .all(revisionId) as AuditEvent[];
  }

  getMeta(key: string): string | undefined {
    return (this.db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined)?.value;
  }

  /** Set `meta` entries and journal one revision-less row (from_state = to_state = '-') in one transaction; returns its seq. */
  journalMeta(event: "ACCOUNT_SELECTED" | "ACCOUNTS_DETECTED", detail: string, meta: Record<string, string>): number {
    return this.tx(() => {
      for (const [key, value] of Object.entries(meta)) this.db.prepare("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value").run(key, value);
      const row = this.db.prepare("INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (NULL, '-', ?, '-', ?, ?) RETURNING seq").get(event, detail, now()) as { seq: number };
      return row.seq;
    });
  }

  /**
   * Archive or unarchive a PR, journaling a revision-less PR_ARCHIVED or PR_UNARCHIVED row whose detail is the PR id.
   * Returns the seq of the row that put the PR in that state, so a repeat answers it and journals nothing (0 for a PR
   * never archived), or undefined for an unknown PR.
   */
  setArchived(prId: string, archived: boolean): number | undefined {
    return this.tx(() => {
      const pull = this.pullById(prId);
      if (!pull) return undefined;
      const event = archived ? "PR_ARCHIVED" : "PR_UNARCHIVED";
      if ((pull.archived_at !== null) === archived) {
        const last = this.db.prepare("SELECT max(seq) AS seq FROM audit_events WHERE revision_id IS NULL AND event = ? AND detail = ?").get(event, prId) as { seq: number | null };
        return last.seq ?? 0;
      }
      const at = now();
      this.db.prepare("UPDATE pull_requests SET archived_at = ? WHERE id = ?").run(archived ? at : null, prId);
      return (this.db.prepare("INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (NULL, '-', ?, '-', ?, ?) RETURNING seq").get(event, prId, at) as { seq: number }).seq;
    });
  }

  /**
   * Journal one progress line for the control stream: its source word (a tool name, or `github`, `git`, `model`...)
   * plus a path, query, count or timing, never file contents or model text. `span` places it on the waterfall.
   * Tool calls and plain lines past ACTIVITY_PER_SECOND a second for one revision are dropped; phase, step and model
   * spans always pass, so every span that opens also closes.
   */
  appendActivity(revisionId: string, tool: string, summary: string, span?: ActivitySpan): void {
    if (!span || span.kind === "tool") {
      const t = Date.now();
      const recent = (this.recentActivity.get(revisionId) ?? []).filter((at) => t - at < 1000);
      if (recent.length >= ACTIVITY_PER_SECOND) return;
      this.recentActivity.set(revisionId, [...recent, t]);
    }
    const detail = Array.from(sanitize(`${tool} ${summary}`).replace(/\s+/g, " ")).slice(0, 200).join("");
    this.db
      .prepare(
        "INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at, span_json) SELECT id, state, 'TOOL_ACTIVITY', state, ?, ?, ? FROM review_revisions WHERE id = ?",
      )
      .run(detail, now(), span ? JSON.stringify(span) : null, revisionId);
  }

  /** One model response of a review: its tokens and the cost pi-ai priced it at. */
  recordUsage(revisionId: string, model: string, usage: Usage): void {
    const { input, output, cacheRead, cacheWrite, cost } = usage;
    this.db
      .prepare(
        `INSERT INTO model_usage (revision_id, pr_id, at, model, input, output, cache_read, cache_write, cost_input, cost_output, cost_cache_read, cost_cache_write, cost_total)
         SELECT id, pr_id, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ? FROM review_revisions WHERE id = ?`,
      )
      .run(now(), model, input, output, cacheRead, cacheWrite, cost.input, cost.output, cost.cacheRead, cost.cacheWrite, cost.total, revisionId);
  }

  saveSnapshot(snapshot: Omit<Snapshot, "id">): string {
    const id = stableId("snapshot", snapshot.revisionId);
    this.db
      .prepare(
        `INSERT INTO context_snapshots (id, revision_id, pr_json, changed_files_json, diff, limitations_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING`,
      )
      .run(id, snapshot.revisionId, JSON.stringify(snapshot.pr), JSON.stringify(snapshot.changedFiles), snapshot.diff, JSON.stringify(snapshot.limitations), now());
    return id;
  }

  snapshot(revisionId: string): Snapshot | undefined {
    const row = this.db.prepare("SELECT * FROM context_snapshots WHERE revision_id = ?").get(revisionId) as Record<string, string> | undefined;
    if (!row) return undefined;
    return {
      id: row.id!,
      revisionId,
      pr: JSON.parse(row.pr_json!),
      changedFiles: JSON.parse(row.changed_files_json!),
      diff: row.diff!,
      limitations: JSON.parse(row.limitations_json!),
    };
  }

  saveDecision(record: Omit<DecisionRecord, "id">): string {
    const id = stableId("decision", record.revisionId);
    this.db
      .prepare(
        `INSERT INTO decision_records (id, revision_id, input_hash, provider, model_id, raw_response_json, error, normalized_json, selected_mode, reasons_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING`,
      )
      .run(
        id,
        record.revisionId,
        record.inputHash,
        record.provider,
        record.modelId,
        record.rawResponse === undefined ? null : JSON.stringify(record.rawResponse),
        record.error,
        record.normalized === undefined ? null : JSON.stringify(record.normalized),
        record.selectedMode,
        JSON.stringify(record.reasons),
        now(),
      );
    return id;
  }

  decision(revisionId: string): DecisionRecord | undefined {
    const row = this.db.prepare("SELECT * FROM decision_records WHERE revision_id = ?").get(revisionId) as Record<string, string | null> | undefined;
    if (!row) return undefined;
    return {
      id: row.id!,
      revisionId,
      inputHash: row.input_hash!,
      provider: row.provider!,
      modelId: row.model_id!,
      rawResponse: row.raw_response_json ? JSON.parse(row.raw_response_json) : undefined,
      error: row.error ?? null,
      normalized: row.normalized_json ? JSON.parse(row.normalized_json) : undefined,
      selectedMode: row.selected_mode!,
      reasons: JSON.parse(row.reasons_json!),
    };
  }

  /** First submission wins: a replayed or repeated submit_candidates call cannot replace the stored set. */
  saveCandidateSet(revisionId: string, candidates: CandidateFinding[], resolutions: ResolutionClaim[] = []): { id: string; stored: boolean } {
    const id = stableId("candidates", revisionId);
    const info = this.db
      .prepare("INSERT INTO candidate_sets (id, revision_id, candidates_json, resolutions_json, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING")
      .run(id, revisionId, JSON.stringify(candidates), JSON.stringify(resolutions), now());
    return { id, stored: info.changes === 1 };
  }

  candidateSet(revisionId: string): { id: string; candidates: CandidateFinding[]; resolutions: ResolutionClaim[]; dispositions: Disposition[] | null; verdicts: ResolutionVerdict[] | null } | undefined {
    const row = this.db.prepare("SELECT * FROM candidate_sets WHERE revision_id = ?").get(revisionId) as Record<string, string | null> | undefined;
    if (!row) return undefined;
    return {
      id: row.id!,
      candidates: JSON.parse(row.candidates_json!),
      resolutions: row.resolutions_json ? JSON.parse(row.resolutions_json) : [],
      dispositions: row.dispositions_json ? JSON.parse(row.dispositions_json) : null,
      verdicts: row.resolution_verdicts_json ? JSON.parse(row.resolution_verdicts_json) : null,
    };
  }

  countCandidateSets(revisionId: string): number {
    return (this.db.prepare("SELECT count(*) AS n FROM candidate_sets WHERE revision_id = ?").get(revisionId) as { n: number }).n;
  }

  /**
   * Writes for VERIFY → PREPARE_PUBLICATION; call inside `advance()` so they commit with the transition. `verified`
   * upserts the finding rows (a re-report keeps its ID and refreshes the severity); `observations` writes one row per
   * finding this revision saw, with the state it ended in, and moves the finding there. The observation goes in before
   * the status changes, so the journal trigger charges the change to this revision. `verdicts` judge the submission's
   * resolution claims; the observations already carry what each accepted one did.
   */
  writeVerification(
    revision: RevisionRow,
    dispositions: Disposition[],
    verified: { findingId: string; fingerprint: string; candidate: CandidateFinding }[],
    observations: Observation[],
    verdicts: ResolutionVerdict[] = [],
  ): void {
    const at = now();
    this.db.prepare("UPDATE candidate_sets SET dispositions_json = ?, resolution_verdicts_json = ? WHERE revision_id = ?").run(JSON.stringify(dispositions), JSON.stringify(verdicts), revision.id);
    for (const { findingId, fingerprint, candidate } of verified) {
      this.db
        .prepare(
          `INSERT INTO findings (id, pr_id, first_revision_id, fingerprint, status, current_severity, created_at, updated_at)
           VALUES (?, ?, ?, ?, 'DETECTED', ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET current_severity = excluded.current_severity, updated_at = excluded.updated_at`,
        )
        .run(findingId, revision.pr_id, revision.id, fingerprint, candidate.severity, at, at);
    }
    for (const { findingId, path, basis, candidate, location, reopens } of observations) {
      const state = path.at(-1)!;
      // A retried VERIFY meets its first attempt's observation and leaves it, status included: the observation that
      // was journaled is the one that stands, so the retry adds no status change the history would call a publication.
      const { changes } = this.db
        .prepare(
          `INSERT INTO finding_observations (id, finding_id, revision_id, disposition, evidence_json, location_json, created_at, path_json, basis)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (finding_id, revision_id) DO NOTHING`,
        )
        .run(stableId("observation", findingId, revision.id), findingId, revision.id, state, JSON.stringify(candidate), JSON.stringify(location), at, JSON.stringify(path), basis);
      if (Number(changes) === 1) this.db.prepare("UPDATE findings SET status = ?, updated_at = ? WHERE id = ? AND status != ?").run(state, at, findingId, state);
      if (reopens) this.db.prepare("UPDATE findings SET reopens = ? WHERE id = ? AND reopens IS NULL").run(reopens.findingId, findingId);
    }
  }

  findingsForRevision(revisionId: string): FindingView[] {
    const revision = this.revision(revisionId)!;
    const rows = this.db
      .prepare(
        `SELECT f.id, f.fingerprint, f.status, f.current_severity, f.first_revision_id, f.github_review_id, f.reopens, o.evidence_json, o.location_json, fr.head_sha AS first_head_sha,
           (SELECT rr.head_sha FROM finding_observations ro JOIN review_revisions rr ON rr.id = ro.revision_id
            WHERE ro.finding_id = f.reopens AND ro.disposition = 'RESOLVED' ORDER BY ro.created_at DESC LIMIT 1) AS resolved_at
         FROM finding_observations o JOIN findings f ON f.id = o.finding_id JOIN review_revisions fr ON fr.id = f.first_revision_id
         WHERE o.revision_id = ? ORDER BY o.created_at, f.id`,
      )
      .all(revisionId) as Record<string, string | null>[];
    // A finding counts as posted while an outbox may still put it on GitHub: its POST may have landed (`sending`,
    // `needs_reconciliation`), or a live revision is about to send it. A missed comment is cheaper than a duplicate.
    // A `pending` outbox of a finished revision was never sent or was proven absent, so its findings are not posted.
    // It counts as listed once such an outbox, or a published one, belongs to a revision that observed it.
    const terminal = TERMINAL_REVIEW_STATES.map((s) => `'${s}'`).join(", ");
    const outboxes = this.db
      .prepare(
        `SELECT o.revision_id, o.status, o.payload_json, o.inline_finding_ids_json FROM publication_outbox o JOIN review_revisions r ON r.id = o.revision_id
         WHERE r.pr_id = ? AND o.revision_id != ?
         AND (o.status IN ('published', 'sending', 'needs_reconciliation') OR (o.status = 'pending' AND r.state NOT IN (${terminal})))`,
      )
      .all(revision.pr_id, revisionId) as Pick<OutboxRow, "revision_id" | "status" | "payload_json" | "inline_finding_ids_json">[];
    const inFlight = new Set(outboxes.filter((o) => o.status !== "published").flatMap((o) => this.inlineFindingIds(o)));
    const listed = new Set(
      outboxes.length === 0
        ? []
        : (this.db.prepare(`SELECT DISTINCT finding_id FROM finding_observations WHERE revision_id IN (${outboxes.map(() => "?").join(", ")})`).all(...outboxes.map((o) => o.revision_id)) as { finding_id: string }[]).map((r) => r.finding_id),
    );
    return rows.map((row) => {
      const location = JSON.parse(row.location_json!) as Partial<ObservedLocation>;
      return {
        findingId: row.id!,
        fingerprint: row.fingerprint!,
        status: row.status!,
        severity: row.current_severity!,
        candidate: JSON.parse(row.evidence_json!),
        carried: row.first_revision_id !== revisionId,
        firstHeadSha: row.first_head_sha!,
        posted: row.github_review_id !== null || inFlight.has(row.id!),
        listed: listed.has(row.id!),
        anchored: location.headSha === undefined || location.headSha === revision.head_sha,
        reopens: row.reopens == null ? null : { findingId: row.reopens, resolvedAt: row.resolved_at! },
      };
    });
  }

  /** Every finding of the PR not yet RESOLVED, as last observed by a revision other than `excludeRevisionId`. */
  openFindings(prId: string, excludeRevisionId: string): OpenFinding[] {
    return this.findingsAsLastObserved("f.status != 'RESOLVED'", { pr: prId, exclude: excludeRevisionId, limit: -1 });
  }

  /**
   * The RESOLVED findings of the PR that no revision other than `excludeRevisionId` has reopened, most recently
   * resolved first and at most `limit` of them, as last observed: the location is where the code was last seen and
   * `observedAt` the head it was resolved on. The revision that reopened one still sees it, so its retry derives the
   * same successor.
   */
  resolvedFindings(prId: string, excludeRevisionId: string, limit: number): OpenFinding[] {
    return this.findingsAsLastObserved("f.status = 'RESOLVED' AND NOT EXISTS (SELECT 1 FROM findings n WHERE n.reopens = f.id AND n.first_revision_id != :exclude)", { pr: prId, exclude: excludeRevisionId, limit }, "f.updated_at DESC, f.id");
  }

  private findingsAsLastObserved(where: string, params: { pr: string; exclude: string; limit: number }, order = "f.created_at, f.id"): OpenFinding[] {
    const rows = this.db
      .prepare(
        `SELECT f.id, f.status, o.evidence_json, o.location_json, r.head_sha FROM findings f
         JOIN finding_observations o ON o.id = (SELECT id FROM finding_observations WHERE finding_id = f.id AND revision_id != :exclude ORDER BY created_at DESC, id LIMIT 1)
         JOIN review_revisions r ON r.id = o.revision_id
         WHERE f.pr_id = :pr AND ${where} ORDER BY ${order} LIMIT :limit`,
      )
      .all(params) as Record<string, string>[];
    return rows.map((row) => ({
      findingId: row.id!,
      status: row.status as FindingState,
      candidate: JSON.parse(row.evidence_json!),
      location: JSON.parse(row.location_json!),
      observedAt: row.head_sha!,
    }));
  }

  previousFindings(prId: string, excludeRevisionId: string): { findingId: string; status: string; severity: string; title: string; file: string }[] {
    const rows = this.db
      .prepare(
        `SELECT f.id, f.status, f.current_severity, o.evidence_json FROM findings f
         JOIN finding_observations o ON o.finding_id = f.id
         WHERE f.pr_id = ? AND o.revision_id != ?
         GROUP BY f.id HAVING o.created_at = max(o.created_at) ORDER BY f.created_at`,
      )
      .all(prId, excludeRevisionId) as Record<string, string>[];
    return rows.map((row) => {
      const candidate = JSON.parse(row.evidence_json!) as CandidateFinding;
      return { findingId: row.id!, status: row.status!, severity: row.current_severity!, title: candidate.title, file: candidate.file };
    });
  }

  countFindings(prId: string): number {
    return (this.db.prepare("SELECT count(*) AS n FROM findings WHERE pr_id = ?").get(prId) as { n: number }).n;
  }

  /** §9.2 step 2: false for a delivery already in the inbox, which makes a redelivery a no-op. */
  recordWebhook(event: { deliveryId: string; eventType: string; payloadHash: string; payloadJson: string }): boolean {
    const info = this.db
      .prepare(
        `INSERT INTO webhook_events (delivery_id, event_type, payload_hash, payload_json, received_at)
         VALUES (?, ?, ?, ?, ?) ON CONFLICT (delivery_id) DO NOTHING`,
      )
      .run(event.deliveryId, event.eventType, event.payloadHash, event.payloadJson, now());
    return info.changes === 1;
  }

  /** Unprocessed deliveries whose backoff has elapsed; one that keeps failing waits and never fills the window. */
  dueWebhooks(nowMs: number, limit = 100): WebhookRow[] {
    return this.db
      .prepare(
        `SELECT delivery_id, event_type, payload_json, attempts FROM webhook_events
         WHERE processed_at IS NULL AND (next_attempt_at IS NULL OR next_attempt_at <= ?) ORDER BY received_at, delivery_id LIMIT ?`,
      )
      .all(nowMs, limit) as WebhookRow[];
  }

  countUnprocessedWebhooks(): number {
    return (this.db.prepare("SELECT count(*) AS n FROM webhook_events WHERE processed_at IS NULL").get() as { n: number }).n;
  }

  /** Count a failed attempt: retry at `retryAtMs`, or with `null` the delivery is dead and processed with its reason. */
  failWebhook(deliveryId: string, error: string, retryAtMs: number | null): void {
    this.db
      .prepare(
        `UPDATE webhook_events SET attempts = attempts + 1, last_error = ?, next_attempt_at = ?,
           processed_at = CASE WHEN ? IS NULL THEN ? END, outcome = CASE WHEN ? IS NULL THEN ? END WHERE delivery_id = ?`,
      )
      .run(error, retryAtMs, retryAtMs, now(), retryAtMs, `dead: ${error}`, deliveryId);
  }

  deadWebhooks(): DeadWebhookRow[] {
    return this.db
      .prepare("SELECT delivery_id, event_type, attempts, last_error, processed_at FROM webhook_events WHERE outcome LIKE 'dead: %' ORDER BY processed_at DESC")
      .all() as DeadWebhookRow[];
  }

  markWebhookProcessed(deliveryId: string, outcome: string): void {
    this.db.prepare("UPDATE webhook_events SET processed_at = ?, outcome = ? WHERE delivery_id = ?").run(now(), outcome, deliveryId);
  }

  setPullSchedule(prId: string, status: CoordinatorState, reviewDueAt: number | null): void {
    this.db.prepare("UPDATE pull_requests SET status = ?, review_due_at = ?, updated_at = ? WHERE id = ?").run(status, reviewDueAt, now(), prId);
  }

  duePulls(nowMs: number): PullRow[] {
    return this.db.prepare("SELECT * FROM pull_requests WHERE review_due_at IS NOT NULL AND review_due_at <= ? ORDER BY review_due_at").all(nowMs) as PullRow[];
  }

  /** Revisions with no terminal state yet, for supersede requests and startup recovery. */
  unfinishedRevisions(prId?: string): RevisionRow[] {
    const terminal = TERMINAL_REVIEW_STATES.map((s) => `'${s}'`).join(", ");
    const sql = `SELECT * FROM review_revisions WHERE state NOT IN (${terminal})${prId ? " AND pr_id = ?" : ""} ORDER BY created_at`;
    return (prId ? this.db.prepare(sql).all(prId) : this.db.prepare(sql).all()) as RevisionRow[];
  }

  requestSupersede(revisionId: string): void {
    this.db.prepare("UPDATE review_revisions SET superseded_requested_at = ? WHERE id = ? AND superseded_requested_at IS NULL").run(now(), revisionId);
  }

  /** §11: one owner per PR. A lease past `expires_at` (its owner crashed) can be taken over. */
  acquireLease(prId: string, owner: string, nowMs: number, ttlMs: number): boolean {
    const info = this.db
      .prepare(
        `INSERT INTO leases (pr_id, owner, expires_at) VALUES (?, ?, ?)
         ON CONFLICT (pr_id) DO UPDATE SET owner = excluded.owner, expires_at = excluded.expires_at
         WHERE leases.owner = excluded.owner OR leases.expires_at <= ?`,
      )
      .run(prId, owner, nowMs + ttlMs, nowMs);
    return info.changes === 1;
  }

  /** This data dir's lease owner, stable across restarts. */
  leaseOwner(): string {
    return (this.db.prepare("SELECT value FROM meta WHERE key = 'lease_owner'").get() as { value: string }).value;
  }

  releaseLease(prId: string, owner: string): void {
    this.db.prepare("DELETE FROM leases WHERE pr_id = ? AND owner = ?").run(prId, owner);
  }
}

/** The recorded model responses from `since` (an ISO instant) on; `db` may be a read-only connection. */
export function usageRows(db: DatabaseSync, since: string): UsageRow[] {
  return db
    .prepare(
      `SELECT revision_id AS revisionId, at, model, input, output, cache_read AS cacheRead, cache_write AS cacheWrite, cost_total AS cost
       FROM model_usage WHERE at >= ? ORDER BY at, id`,
    )
    .all(since) as UsageRow[];
}

/** Run `fn` on the data dir's app DB, creating the dir and DB if needed. Takes no data-dir lock: WAL lets it share the DB. */
export async function withStore<T>(dataDir: string, fn: (store: AppStore) => Promise<T> | T): Promise<T> {
  mkdirSync(dataDir, { recursive: true });
  const store = AppStore.open(`${dataDir}/app.sqlite`);
  try {
    return await fn(store);
  } finally {
    store.close();
  }
}
