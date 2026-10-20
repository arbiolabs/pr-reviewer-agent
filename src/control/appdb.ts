import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { COORDINATOR_STATES, FINDING_STATES, REVIEW_STATES, type ReviewState } from "../domain/machines.ts";
import type { CandidateFinding } from "../domain/types.ts";
import { ACCOUNT_KEY, chooseAccount, detectGh, type Env, type GhDetection, listAccounts } from "../github/accounts.ts";
import { AppStore, type PullRow, type RevisionRow } from "../persistence/store.ts";
import { sanitize } from "../protocol/sanitize.ts";
import type { Account, Finding, FindingDetail, Pr, Run, SequencedEvent } from "../protocol/v1.ts";
import { PinnedRepo } from "../source/git.ts";
import { type DiffPage, pageDiff, parseDiffFiles } from "./diff.ts";
import { ControlError, type ControlSource, type DiffRequest, type Snapshot, type TimelinePage, type TimelineRequest } from "./source.ts";

type JournalRow = { seq: number; prev: number; revision_id: string | null; from_state: string; event: string; to_state: string; detail: string | null; at: string };
type FindingRow = { id: string; pr_id: string; repo_full_name: string; fingerprint: string; status: string; current_severity: string; revision_id: string; evidence_json: string; stale: number };

/** Every string that leaves this source passes here: repository, PR and model text can carry terminal escapes. */
function clean<T>(value: T): T {
  if (typeof value === "string") return sanitize(value) as T;
  if (Array.isArray(value)) return value.map(clean) as T;
  if (value !== null && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, clean(v)])) as T;
  return value;
}

const JOURNAL_SELECT = `SELECT a.seq, a.revision_id, a.from_state, a.event, a.to_state, a.detail, a.at,
  coalesce((SELECT max(b.seq) FROM audit_events b WHERE b.seq < a.seq), 0) AS prev FROM audit_events a`;

// One finding at one observation: the given revision's, or the latest when revision is null.
const FINDING_SELECT = `SELECT f.id, f.pr_id, p.repo_full_name, f.fingerprint, f.status, f.current_severity, o.revision_id, o.evidence_json,
  r.head_sha != p.latest_head_sha AS stale
  FROM findings f JOIN finding_observations o ON o.finding_id = f.id
  JOIN review_revisions r ON r.id = o.revision_id JOIN pull_requests p ON p.id = f.pr_id
  WHERE (:id IS NULL OR f.id = :id)
  AND o.revision_id = coalesce(:revision, (SELECT revision_id FROM finding_observations WHERE finding_id = f.id ORDER BY created_at DESC LIMIT 1))
  ORDER BY f.created_at, f.id`;

const isPhase = (s: string): s is ReviewState => (REVIEW_STATES as readonly string[]).includes(s);
const TERMINAL_EVENT = { COMPLETED: "review.completed", FAILED: "run.failed", SUPERSEDED: "run.superseded" } as const;
const SEEN_KEY = "github.accounts_seen";
const EXCERPT_LINES = 40;
const EXCERPT_BYTES = 8 * 1024;
const BLOB_CACHE_ENTRIES = 64;

/** Lines `start..end` of `content`, capped at EXCERPT_LINES and EXCERPT_BYTES with a closing `… N more lines`; `undefined` past the end. */
export function excerpt(content: string, start: number, end: number): string | undefined {
  const lines = (content.endsWith("\n") ? content.slice(0, -1) : content).split("\n").slice(start - 1, end);
  if (lines.length === 0) return undefined;
  const kept: string[] = [];
  let bytes = 0;
  for (const line of lines.slice(0, EXCERPT_LINES)) {
    bytes += Buffer.byteLength(line) + 1;
    if (bytes > EXCERPT_BYTES) {
      if (kept.length === 0) kept.push(Buffer.from(line).subarray(0, EXCERPT_BYTES).toString());
      break;
    }
    kept.push(line);
  }
  const more = lines.length - kept.length;
  return [...kept, ...(more > 0 ? [`… ${more} more line${more === 1 ? "" : "s"}`] : [])].join("\n");
}

/**
 * Reads the engine's app DB on its own read-only connection; WAL lets it read while the engine writes. The journal is
 * `audit_events`: every row becomes exactly one event at `sequence = seq`, so there are no holes to explain. The only
 * writes are account rows, through `writer`: the selection and a journal row whenever gh's accounts change.
 */
export class AppDbSource implements ControlSource {
  private readonly db: DatabaseSync;
  private readonly writer: AppStore;
  private readonly dataDir: string;
  private readonly env: Env;
  private readonly streamEpoch: string;
  private diffCache: { key: string; files: ReturnType<typeof parseDiffFiles> } | undefined;
  /** File contents by repo, sha and path, least recently used first. Misses are not cached: a later sync may add the blob. */
  private readonly blobs = new Map<string, string>();
  private gh: { at: number; detection: GhDetection } | undefined;

  private constructor(dataDir: string, writer: AppStore, env: Env) {
    this.dataDir = dataDir;
    this.writer = writer;
    this.env = env;
    this.db = new DatabaseSync(join(dataDir, "app.sqlite"), { readOnly: true });
    this.db.exec("PRAGMA busy_timeout = 5000");
    this.streamEpoch = (this.db.prepare("SELECT value FROM meta WHERE key = 'stream_epoch'").get() as { value: string }).value;
  }

  /** Creates or migrates the app DB first, so the epoch exists before any review ran. `env` is where gh and GITHUB_TOKEN are looked up. */
  static open(dataDir: string, env: Env = process.env): AppDbSource {
    mkdirSync(dataDir, { recursive: true });
    return new AppDbSource(dataDir, AppStore.open(join(dataDir, "app.sqlite")), env);
  }

  /** Re-run gh detection when the cached one is older than `maxAgeMs`; a change from what this data dir last saw is journaled. */
  async refreshAccounts(maxAgeMs: number): Promise<void> {
    if (this.gh && Date.now() - this.gh.at < maxAgeMs) return;
    const detection = await detectGh(this.env);
    this.gh = { at: Date.now(), detection };
    const seen = JSON.stringify(detection.installed ? detection.accounts : null);
    if ((this.writer.getMeta(SEEN_KEY) ?? "null") === seen) return;
    const n = detection.installed ? detection.accounts.length : 0;
    this.writer.journalMeta("ACCOUNTS_DETECTED", `${n} gh account${n === 1 ? "" : "s"}`, { [SEEN_KEY]: seen });
  }

  /** Validated against a detection made now, so a client cannot select an account gh dropped since the last refresh. */
  async selectAccount(accountId: string): Promise<string> {
    await this.refreshAccounts(0);
    const choice = chooseAccount(this.gh!.detection, accountId, this.env);
    if (!choice.ok) throw new ControlError(choice.code, choice.message);
    return `account.select:${this.writer.journalMeta("ACCOUNT_SELECTED", choice.account.id, { [ACCOUNT_KEY]: choice.account.id })}`;
  }

  private accounts(): Account[] {
    const selected = this.db.prepare("SELECT value FROM meta WHERE key = ?").get(ACCOUNT_KEY) as { value: string } | undefined;
    return listAccounts(this.gh?.detection ?? { installed: false }, selected?.value, this.env);
  }

  private read<T>(fn: () => T): T {
    this.db.exec("BEGIN");
    try {
      return clean(fn());
    } finally {
      this.db.exec("COMMIT");
    }
  }

  epoch(): string {
    return this.streamEpoch;
  }

  latestSequence(): number {
    return (this.db.prepare("SELECT coalesce(max(seq), 0) AS n FROM audit_events").get() as { n: number }).n;
  }

  snapshot(): Snapshot {
    return this.read(() => ({
      streamEpoch: this.streamEpoch,
      sequence: this.latestSequence(),
      prs: (this.db.prepare("SELECT * FROM pull_requests ORDER BY updated_at DESC, id").all() as PullRow[]).map((p) => this.pr(p)),
      runs: (this.db.prepare("SELECT * FROM review_revisions ORDER BY created_at, id").all() as RevisionRow[]).map((r) =>
        this.run(r, r.state, r.completed_at, r.state === "FAILED" ? this.failureReason(r.id) : null),
      ),
      findings: (this.db.prepare(FINDING_SELECT).all({ id: null, revision: null }) as FindingRow[]).map(toFinding),
      accounts: this.accounts(),
    }));
  }

  eventsAfter(sequence: number, limit: number): SequencedEvent[] {
    return this.read(() =>
      (this.db.prepare(`${JOURNAL_SELECT} WHERE a.seq > ? ORDER BY a.seq LIMIT ?`).all(sequence, limit) as JournalRow[]).map((row) => this.event(row)),
    );
  }

  timeline(request: TimelineRequest): TimelinePage {
    return this.read(() => {
      const rows = this.db
        .prepare(
          `${JOURNAL_SELECT} LEFT JOIN review_revisions r ON r.id = a.revision_id
           WHERE (:pr IS NULL OR r.pr_id = :pr) AND (:run IS NULL OR a.revision_id = :run)
           AND (:finding IS NULL OR (a.event IN ('FINDING_OBSERVED', 'FINDING_UPDATED') AND a.detail = :finding))
           AND (:before IS NULL OR a.seq < :before) ORDER BY a.seq DESC LIMIT :limit`,
        )
        .all({
          pr: request.prId ?? null,
          run: request.runId ?? null,
          finding: request.findingId ?? null,
          before: request.beforeSequence ?? null,
          limit: request.limit + 1,
        }) as JournalRow[];
      const events = rows.slice(0, request.limit).map((row) => this.event(row));
      return rows.length > request.limit ? { events, nextBeforeSequence: events.at(-1)!.sequence } : { events };
    });
  }

  async finding(findingId: string): Promise<FindingDetail> {
    const { repo, refs, detail } = this.read(() => {
      const row = this.db.prepare(FINDING_SELECT).get({ id: findingId, revision: null }) as FindingRow | undefined;
      if (!row) throw new ControlError("NOT_FOUND", `no finding ${findingId}`);
      const candidate = JSON.parse(row.evidence_json) as CandidateFinding;
      const history = this.db
        .prepare("SELECT revision_id, disposition, created_at FROM finding_observations WHERE finding_id = ? ORDER BY created_at")
        .all(findingId) as { revision_id: string; disposition: string; created_at: string }[];
      const finding: Omit<FindingDetail, "evidence"> = {
        ...toFinding(row),
        explanation: candidate.explanation,
        reasoningSummary: candidate.reasoningSummary,
        ...(candidate.suggestedFix ? { suggestedFix: candidate.suggestedFix } : {}),
        // An observation records VERIFIED; until publication tracks more, a verified observation is a DETECTED finding.
        history: history.map((h) => ({
          runId: h.revision_id,
          disposition: (FINDING_STATES as readonly string[]).includes(h.disposition) ? (h.disposition as Finding["status"]) : "DETECTED",
          at: h.created_at,
        })),
      };
      return { repo: row.repo_full_name, refs: candidate.codeEvidence, detail: finding };
    });
    const evidence: FindingDetail["evidence"] = [];
    for (const { path, start, end, refSha } of refs) {
      const content = await this.blob(repo, refSha, path);
      const code = content === undefined ? undefined : excerpt(content, start, end);
      evidence.push({ kind: "code_ref", path, start, end, refSha, text: code ?? `${path}:${start}-${end} (code unavailable offline)` });
    }
    return clean({ ...detail, evidence });
  }

  /** Read from the local bare repo only: the daemon never fetches. Any failure reads as unavailable. */
  private async blob(repo: string, sha: string, path: string): Promise<string | undefined> {
    const key = `${repo}\0${sha}\0${path}`;
    let content = this.blobs.get(key);
    if (content === undefined) {
      content = await new PinnedRepo(join(this.dataDir, "repos", `${repo}.git`), sha, sha).show(sha, path, { offline: true }).catch(() => undefined);
      if (content === undefined) return undefined;
    }
    this.blobs.delete(key);
    this.blobs.set(key, content);
    if (this.blobs.size > BLOB_CACHE_ENTRIES) this.blobs.delete(this.blobs.keys().next().value!);
    return content;
  }

  async diff(request: DiffRequest): Promise<DiffPage> {
    const found = this.db
      .prepare("SELECT p.repo_full_name FROM review_revisions r JOIN pull_requests p ON p.id = r.pr_id WHERE r.pr_id = ? AND r.base_sha = ? AND r.head_sha = ? LIMIT 1")
      .get(request.prId, request.baseSha, request.headSha) as { repo_full_name: string } | undefined;
    if (!found) throw new ControlError("NOT_FOUND", `no review of ${request.prId} at ${request.baseSha}...${request.headSha}`);
    const key = `${request.prId}\0${request.baseSha}\0${request.headSha}`;
    if (this.diffCache?.key !== key) {
      const repo = new PinnedRepo(join(this.dataDir, "repos", `${found.repo_full_name}.git`), request.baseSha, request.headSha);
      this.diffCache = { key, files: parseDiffFiles(await repo.diff()) };
    }
    return clean(pageDiff(this.diffCache.files, request));
  }

  close(): void {
    this.db.close();
    this.writer.close();
  }

  private revision(id: string): RevisionRow {
    return this.db.prepare("SELECT * FROM review_revisions WHERE id = ?").get(id) as RevisionRow;
  }

  private pr(pull: PullRow, at?: RevisionRow): Pr {
    const latest = this.db
      .prepare("SELECT c.pr_json FROM context_snapshots c JOIN review_revisions r ON r.id = c.revision_id WHERE r.pr_id = ? ORDER BY c.created_at DESC LIMIT 1")
      .get(pull.id) as { pr_json: string } | undefined;
    return {
      id: pull.id,
      repository: pull.repo_full_name,
      number: pull.number,
      title: latest ? (JSON.parse(latest.pr_json) as { title: string }).title : `${pull.repo_full_name}#${pull.number}`,
      headSha: at?.head_sha ?? pull.latest_head_sha,
      baseSha: at?.base_sha ?? pull.latest_base_sha,
      lifecycle: (COORDINATOR_STATES as readonly string[]).includes(pull.status) ? (pull.status as Pr["lifecycle"]) : "ACTIVE",
    };
  }

  /** A run as of one journal row: everything but phase, completion and failure is fixed when the revision is created. */
  private run(r: RevisionRow, phase: ReviewState, completedAt: string | null, failureReason: string | null): Run {
    const status = phase === "COMPLETED" || phase === "FAILED" || phase === "SUPERSEDED" ? phase : "RUNNING";
    return {
      id: r.id,
      prId: r.pr_id,
      baseSha: r.base_sha,
      headSha: r.head_sha,
      phase,
      status,
      model: r.reviewer_version.slice(r.reviewer_version.indexOf("+") + 1),
      startedAt: r.created_at,
      ...(status !== "RUNNING" && completedAt ? { completedAt } : {}),
      ...(failureReason ? { failureReason } : {}),
    };
  }

  private failureReason(revisionId: string): string | null {
    const row = this.db.prepare("SELECT detail FROM audit_events WHERE revision_id = ? AND to_state = 'FAILED' ORDER BY seq DESC LIMIT 1").get(revisionId) as
      | { detail: string | null }
      | undefined;
    return row?.detail ?? null;
  }

  private event(row: JournalRow): SequencedEvent {
    if (row.revision_id === null) {
      const head = { type: "event" as const, streamEpoch: this.streamEpoch, sequence: row.seq, prevSequence: row.prev, occurredAt: row.at };
      // The list is the current one, not the one at row.seq: accounts.updated replaces the whole list, so a replay converges.
      if (row.event === "ACCOUNT_SELECTED" || row.event === "ACCOUNTS_DETECTED") return { ...head, eventType: "accounts.updated", data: { accounts: this.accounts() } };
      return { ...head, eventType: `journal.${row.event}`, data: { event: row.event, detail: row.detail } };
    }
    const rev = this.revision(row.revision_id);
    const head = { type: "event" as const, streamEpoch: this.streamEpoch, sequence: row.seq, prevSequence: row.prev, occurredAt: row.at, prId: rev.pr_id };
    const ids = { ...head, runId: rev.id };
    const findingAt = (revision: string | null) => toFinding(this.db.prepare(FINDING_SELECT).get({ id: row.detail, revision }) as FindingRow);
    switch (row.event) {
      case "TOOL_ACTIVITY": {
        const detail = row.detail ?? "";
        const space = detail.indexOf(" ");
        return { ...ids, eventType: "tool.activity", data: space === -1 ? { tool: detail, summary: "" } : { tool: detail.slice(0, space), summary: detail.slice(space + 1) } };
      }
      case "PR_UPDATED":
        return { ...head, eventType: "pr.updated", data: this.pr(this.db.prepare("SELECT * FROM pull_requests WHERE id = ?").get(rev.pr_id) as PullRow, rev) };
      case "FINDING_OBSERVED":
        return { ...ids, eventType: "finding.updated", data: findingAt(rev.id) };
      case "FINDING_UPDATED":
        return { ...ids, eventType: "finding.updated", data: findingAt(null) };
    }
    const { from_state: from, to_state: to, event } = row;
    if (!isPhase(from) || !isPhase(to)) return { ...ids, eventType: `journal.${event}`, data: { from, to, event, detail: row.detail } };
    if (event === "REVISION_CREATED" || event === "MANUAL_RETRY") return { ...ids, eventType: "run.started", data: this.run(rev, to, null, null) };
    if (to === "COMPLETED" || to === "FAILED" || to === "SUPERSEDED") {
      return { ...ids, eventType: TERMINAL_EVENT[to], data: this.run(rev, to, row.at, to === "FAILED" ? row.detail : null) };
    }
    return { ...ids, eventType: "review.phase_changed", data: { from, to, event, detail: row.detail } };
  }
}

function toFinding(row: FindingRow): Finding {
  const c = JSON.parse(row.evidence_json) as CandidateFinding;
  return {
    id: row.id,
    prId: row.pr_id,
    runId: row.revision_id,
    fingerprint: row.fingerprint,
    title: c.title,
    category: c.category,
    severity: row.current_severity as Finding["severity"],
    status: row.status as Finding["status"],
    confidence: c.confidence,
    file: c.file,
    line: c.newLine !== null && c.newLine >= 1 ? c.newLine : null,
    evidenceCount: c.codeEvidence.length,
    stale: row.stale === 1,
  };
}
