import { readdirSync, readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { transition } from "../domain/machine.ts";
import { REVIEW_MACHINE, type ReviewEvent, type ReviewState, TERMINAL_REVIEW_STATES } from "../domain/machines.ts";
import { type CandidateFinding, type ChangedFile, type Disposition, type PullInfo, stableId } from "../domain/types.ts";

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
  created_at: string;
  completed_at: string | null;
};

export type PullRow = {
  id: string;
  repository_id: number;
  number: number;
  status: string;
  latest_base_sha: string;
  latest_head_sha: string;
  repo_full_name: string;
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

export type FindingView = {
  findingId: string;
  fingerprint: string;
  status: string;
  severity: string;
  candidate: CandidateFinding;
};

export type AuditEvent = { seq: number; from_state: string; event: string; to_state: string; detail: string | null; at: string };

const now = () => new Date().toISOString();

export class AppStore {
  readonly db: DatabaseSync;

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

  tx<T>(fn: () => T): T {
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

  upsertPull(pr: PullInfo): string {
    const id = stableId("pr", pr.installationId, pr.repositoryId, pr.number);
    const status = pr.merged ? "MERGED" : pr.state === "closed" ? "CLOSED" : "ACTIVE";
    this.db
      .prepare(
        `INSERT INTO pull_requests (id, installation_id, repository_id, number, status, latest_base_sha, latest_head_sha, updated_at, repo_full_name)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET status = excluded.status, latest_base_sha = excluded.latest_base_sha,
           latest_head_sha = excluded.latest_head_sha, updated_at = excluded.updated_at, repo_full_name = excluded.repo_full_name`,
      )
      .run(id, pr.installationId, pr.repositoryId, pr.number, status, pr.baseSha, pr.headSha, now(), pr.fullName);
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
      if (current !== from) throw new Error(`revision ${revisionId} is ${current}, expected ${from} for ${event}`);
      writes?.();
      this.recordTransition(revisionId, from, event, result.state, detail);
      return result.state;
    });
  }

  /** Apply `event` from whatever state the revision is in; `undefined` when it is not legal there. */
  advanceFromCurrent(revisionId: string, event: ReviewEvent, detail: string | null = null): ReviewState | undefined {
    return this.tx(() => {
      const current = this.revision(revisionId)!.state;
      const result = transition(REVIEW_MACHINE, current, event);
      if (!result.ok) return undefined;
      this.recordTransition(revisionId, current, event, result.state, detail);
      return result.state;
    });
  }

  private recordTransition(revisionId: string, from: ReviewState, event: ReviewEvent, to: ReviewState, detail: string | null): void {
    const at = now();
    this.db
      .prepare("UPDATE review_revisions SET state = ?, completed_at = ? WHERE id = ?")
      .run(to, TERMINAL_REVIEW_STATES.includes(to) ? at : null, revisionId);
    this.db
      .prepare("INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (?, ?, ?, ?, ?, ?)")
      .run(revisionId, from, event, to, detail, at);
  }

  retryCount(revisionId: string): number {
    return (this.db.prepare("SELECT count(*) AS n FROM audit_events WHERE revision_id = ? AND event = 'MANUAL_RETRY'").get(revisionId) as { n: number }).n;
  }

  auditEvents(revisionId: string): AuditEvent[] {
    return this.db
      .prepare("SELECT seq, from_state, event, to_state, detail, at FROM audit_events WHERE revision_id = ? ORDER BY seq")
      .all(revisionId) as AuditEvent[];
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
  saveCandidateSet(revisionId: string, candidates: CandidateFinding[]): { id: string; stored: boolean } {
    const id = stableId("candidates", revisionId);
    const info = this.db
      .prepare("INSERT INTO candidate_sets (id, revision_id, candidates_json, created_at) VALUES (?, ?, ?, ?) ON CONFLICT (id) DO NOTHING")
      .run(id, revisionId, JSON.stringify(candidates), now());
    return { id, stored: info.changes === 1 };
  }

  candidateSet(revisionId: string): { id: string; candidates: CandidateFinding[]; dispositions: Disposition[] | null } | undefined {
    const row = this.db.prepare("SELECT * FROM candidate_sets WHERE revision_id = ?").get(revisionId) as Record<string, string | null> | undefined;
    if (!row) return undefined;
    return {
      id: row.id!,
      candidates: JSON.parse(row.candidates_json!),
      dispositions: row.dispositions_json ? JSON.parse(row.dispositions_json) : null,
    };
  }

  countCandidateSets(revisionId: string): number {
    return (this.db.prepare("SELECT count(*) AS n FROM candidate_sets WHERE revision_id = ?").get(revisionId) as { n: number }).n;
  }

  /** Writes for VERIFY → PREPARE_PUBLICATION; call inside `advance()` so they commit with the transition. */
  writeVerification(
    revision: RevisionRow,
    dispositions: Disposition[],
    verified: { findingId: string; fingerprint: string; candidate: CandidateFinding }[],
  ): void {
    const at = now();
    this.db.prepare("UPDATE candidate_sets SET dispositions_json = ? WHERE revision_id = ?").run(JSON.stringify(dispositions), revision.id);
    for (const { findingId, fingerprint, candidate } of verified) {
      this.db
        .prepare(
          `INSERT INTO findings (id, pr_id, first_revision_id, fingerprint, status, current_severity, created_at, updated_at)
           VALUES (?, ?, ?, ?, 'DETECTED', ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET current_severity = excluded.current_severity, updated_at = excluded.updated_at`,
        )
        .run(findingId, revision.pr_id, revision.id, fingerprint, candidate.severity, at, at);
      this.db
        .prepare(
          `INSERT INTO finding_observations (id, finding_id, revision_id, disposition, evidence_json, location_json, created_at)
           VALUES (?, ?, ?, 'VERIFIED', ?, ?, ?) ON CONFLICT (finding_id, revision_id) DO NOTHING`,
        )
        .run(stableId("observation", findingId, revision.id), findingId, revision.id, JSON.stringify(candidate), JSON.stringify({ file: candidate.file, newLine: candidate.newLine }), at);
    }
  }

  findingsForRevision(revisionId: string): FindingView[] {
    const rows = this.db
      .prepare(
        `SELECT f.id, f.fingerprint, f.status, f.current_severity, o.evidence_json FROM finding_observations o
         JOIN findings f ON f.id = o.finding_id WHERE o.revision_id = ? ORDER BY o.created_at, f.id`,
      )
      .all(revisionId) as Record<string, string>[];
    return rows.map((row) => ({
      findingId: row.id!,
      fingerprint: row.fingerprint!,
      status: row.status!,
      severity: row.current_severity!,
      candidate: JSON.parse(row.evidence_json!),
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
}
