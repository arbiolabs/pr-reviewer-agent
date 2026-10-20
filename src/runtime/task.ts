import type { Context } from "@earendil-works/chord";
import {
  configure,
  type ConversationId,
  defineTask,
  type Extension,
  type ModelRef,
  type NextTaskState,
  type RunningTask,
  type Task,
  type TaskRuntime,
} from "@earendil-works/pi-durable";
import type { Config } from "../config/config.ts";
import type { ReviewState } from "../domain/machines.ts";
import { type ReviewCheckpoint, type ReviewMode, sha256 } from "../domain/types.ts";
import type { PullSource } from "../github/pulls.ts";
import { askJev, type JevOutcome, type JevRoute, selectReviewMode, type TriageInput, type TriageResult } from "../jev/decision.ts";
import type { AppStore, RevisionRow } from "../persistence/store.ts";
import { buildReviewRequest } from "../reasoning/request.ts";
import { TargetDoc, type Workspace } from "../reasoning/tools.ts";
import { verifyCandidates } from "../reasoning/verify.ts";
import { PinnedRepo, syncRepo } from "../source/git.ts";
import type { GitHubClient } from "../github/client.ts";
import { buildPayload, postReview, PUBLISHER_VERSION, type ReviewPayload, reconcileOutbox } from "../publish/publisher.ts";
import { parseDiff } from "../source/diff.ts";

export const REVIEW_TASK_NAME = "pr-review.revision";

/**
 * `retry` counts the MANUAL_RETRY overrides before this task: each override gets its own durable task. `publish` posts
 * the review through the outbox; without it PUBLISH records the intentional no-publication (dry run).
 */
export type ReviewInput = { revisionId: string; retry?: number; publish?: boolean };
export type ReviewResult = { revisionId: string; state: ReviewState };
export type ReviewTask = Task<ReviewInput, ReviewCheckpoint, ReviewResult, object>;

export type ReviewTaskDeps = {
  store: AppStore;
  pulls: PullSource;
  engine: JevRoute;
  config: Config;
  cacheDir: string;
  /** Sent to git only as a per-command header on clone and fetch, for private repositories. */
  gitToken?: ((installationId: number) => Promise<string | undefined>) | undefined;
  /** The GitHub client for an installation (0 is token auth). Needed only to publish. */
  github?: ((installationId: number) => GitHubClient) | undefined;
  model: ModelRef;
  tools: Extension;
  workspace(revisionId: string): Promise<Workspace>;
};

type Runtime = TaskRuntime<ReviewInput, ReviewCheckpoint, ReviewResult, object>;
type Next = NextTaskState<ReviewCheckpoint, ReviewResult>;
type PhaseBody<P extends ReviewCheckpoint["phase"]> = (
  checkpoint: Extract<ReviewCheckpoint, { phase: P }>,
  revision: RevisionRow,
  runtime: Runtime,
  context: Context,
  input: ReviewInput,
) => Promise<ReviewCheckpoint | Next>;

const finished = (revisionId: string, state: ReviewState): Next => ({ status: "terminal", outcome: { status: "completed", result: { revisionId, state } } });

export function createReviewTask(deps: ReviewTaskDeps): ReviewTask {
  const { store, config } = deps;

  /**
   * Each phase does its idempotent app-DB write, then commits the next checkpoint. A throw retries the phase from the
   * same checkpoint with a backoff, up to `max_retries_per_phase`, then fails the revision with the reason.
   */
  function phase<P extends ReviewCheckpoint["phase"]>(body: PhaseBody<P>) {
    return async (task: RunningTask<ReviewInput, Extract<ReviewCheckpoint, { phase: P }>, ReviewResult>, runtime: Runtime, context: Context) => {
      const checkpoint = task.state.checkpoint;
      let next: ReviewCheckpoint | Next;
      try {
        next = await body(checkpoint, store.revision(checkpoint.revisionId)!, runtime, context, task.input);
      } catch (error) {
        if (runtime.signal.aborted) throw error;
        const message = error instanceof Error ? error.message : String(error);
        const attempt = (checkpoint.attempt ?? 0) + 1;
        if (attempt > config.review.max_retries_per_phase) {
          const state = store.advanceFromCurrent(checkpoint.revisionId, "TERMINAL_ERROR", `${checkpoint.phase} failed after ${attempt} attempts: ${message}`);
          await runtime.commit(() => ({ status: "terminal", outcome: { status: "failed", error: { message, detail: { phase: checkpoint.phase, state: state ?? null } } } }), context);
          return;
        }
        runtime.report(new Error(`${checkpoint.phase} attempt ${attempt} failed: ${message}`));
        await runtime.sleep(runtime.now() + config.review.retry_backoff_ms * 2 ** (attempt - 1), context);
        next = { ...checkpoint, attempt };
      }
      await runtime.commit(() => ("phase" in next ? { status: "running", checkpoint: next } : next), context);
    };
  }

  /** Re-read the authoritative PR (§2 invariant 5); a head other than the revision's means it is superseded. */
  async function livePull(revision: RevisionRow) {
    const pr = store.pullById(revision.pr_id)!;
    const [owner, repo] = pr.repo_full_name.split("/") as [string, string];
    const live = await deps.pulls.getPull({ owner, repo, number: pr.number, installationId: pr.installation_id });
    store.upsertPull(live);
    return live;
  }

  const collect: PhaseBody<"collect"> = async (cp, revision) => {
    // After a crash this converges without a second audit row; after --retry it re-enters TRIAGE on the pinned context.
    const existing = store.snapshot(revision.id);
    if (existing) {
      store.advance(revision.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED", "reused the stored context snapshot");
      return { phase: "triage", revisionId: revision.id, contextSnapshotId: existing.id };
    }
    const pull = await livePull(revision);
    if (pull.headSha !== revision.head_sha) {
      store.advance(revision.id, "COLLECT_CONTEXT", "NEWER_HEAD", `head moved to ${pull.headSha}`);
      return finished(revision.id, "SUPERSEDED");
    }
    const pr = store.pullById(revision.pr_id)!;
    const repo = new PinnedRepo(await syncRepo(deps.cacheDir, pr.repo_full_name, pull.cloneUrl, pr.number, await deps.gitToken?.(pr.installation_id)), revision.base_sha, revision.head_sha);
    await repo.assertCommitsPresent();
    const [diff, changedFiles] = [await repo.diff(), await repo.changedFiles()];
    const limitations: string[] = [];
    if (changedFiles.length > config.review.max_changed_files) {
      limitations.push(`${changedFiles.length} changed files exceed max_changed_files ${config.review.max_changed_files}; review is not complete`);
    }
    if (Buffer.byteLength(diff) > config.review.max_diff_bytes) {
      limitations.push(`diff of ${Buffer.byteLength(diff)} bytes exceeds max_diff_bytes ${config.review.max_diff_bytes}; review is not complete`);
    }
    store.advance(revision.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED", `${changedFiles.length} changed file(s)`, () => {
      store.saveSnapshot({
        revisionId: revision.id,
        pr: { title: pull.title, body: pull.body, fullName: pr.repo_full_name, number: pr.number, cloneUrl: pull.cloneUrl },
        changedFiles,
        diff,
        limitations,
      });
    });
    return { phase: "triage", revisionId: revision.id, contextSnapshotId: store.snapshot(revision.id)!.id };
  };

  const triage: PhaseBody<"triage"> = async (cp, revision) => {
    const existing = store.decision(revision.id);
    if (existing) {
      store.advance(revision.id, "TRIAGE", "ROUTING_PERSISTED", `reused routing decision: ${existing.selectedMode}`);
      return { phase: "analyze", revisionId: revision.id, triageDecisionId: existing.id };
    }
    const snapshot = store.snapshot(revision.id)!;
    const input: TriageInput = { title: snapshot.pr.title, body: snapshot.pr.body, changedFiles: snapshot.changedFiles, diffExcerpt: snapshot.diff };
    const route = deps.engine;
    const engine = "skipped" in route ? undefined : route;
    const { outcome, raw }: { outcome: JevOutcome; raw?: unknown } =
      "skipped" in route ? { outcome: { ok: false, error: route.skipped } } : await askJev(route, input, config.review.jev_timeout_ms);
    const { mode, reasons } = selectReviewMode(outcome, snapshot.changedFiles.map((f) => f.path), config);
    const normalized: TriageResult | undefined = outcome.ok ? { ...outcome.judgment, selectedReviewMode: mode } : undefined;
    store.advance(revision.id, "TRIAGE", "ROUTING_PERSISTED", `${mode}: ${reasons.join("; ")}`, () => {
      store.saveDecision({
        revisionId: revision.id,
        inputHash: sha256(JSON.stringify(input)),
        provider: engine?.provider ?? "none",
        modelId: engine?.modelId ?? "none",
        rawResponse: raw,
        error: outcome.ok ? null : outcome.error,
        normalized,
        selectedMode: mode,
        reasons,
      });
    });
    return { phase: "analyze", revisionId: revision.id, triageDecisionId: store.decision(revision.id)!.id };
  };

  const analyze: PhaseBody<"analyze"> = async (cp, revision, runtime, context) => {
    if (!store.candidateSet(revision.id)) await converse(cp, revision, runtime, context);
    const set = store.candidateSet(revision.id);
    if (!set) throw new Error("the model answered without calling submit_candidates");
    store.advance(revision.id, "ANALYZE", "CANDIDATES_PERSISTED", `${set.candidates.length} candidate(s) from ${deps.model.provider}/${deps.model.modelId}`);
    return { phase: "verify", revisionId: revision.id, candidateSetId: set.id };
  };

  /** Ask the review model; its `submit_candidates` call stores the candidate set. */
  async function converse(cp: Extract<ReviewCheckpoint, { phase: "analyze" }>, revision: RevisionRow, runtime: Runtime, context: Context) {
    let child: ConversationId | undefined;
    // A rerun after a crash finds the conversation it created through the ownership index instead of making another.
    await runtime.commit(async (tx) => {
      const existing = (await tx.scanConversations({ ownerTaskId: runtime.taskId }, 1)).items[0];
      if (existing) {
        child = existing.id;
        return undefined;
      }
      const created = await tx.createConversation({ ownership: { kind: "task", taskId: runtime.taskId } });
      await configure(tx, created.id, { model: deps.model, extensions: [deps.tools] });
      (await tx.doc(TargetDoc, created.id)).revisionId = revision.id;
      child = created.id;
      return undefined;
    }, context);
    const conversation = (await runtime.conversation(child!, context))!;
    const decision = store.decision(revision.id)!;
    const content = buildReviewRequest({
      snapshot: store.snapshot(revision.id)!,
      baseSha: revision.base_sha,
      headSha: revision.head_sha,
      mode: decision.selectedMode as ReviewMode,
      reasons: decision.reasons,
    });
    // The same request ID finds the same submission after a restart; a retry asks again under a new one.
    const requestId = cp.attempt ? `analyze:${revision.id}:${cp.attempt}` : `analyze:${revision.id}`;
    const settled = await (await conversation.submit({ type: "input", content, requestId }, context)).wait(context);
    if (settled.status !== "done") throw new Error(`analysis ${settled.status}: ${settled.reason}${settled.detail ? ` ${JSON.stringify(settled.detail)}` : ""}`);
  }

  const verify: PhaseBody<"verify"> = async (cp, revision) => {
    const set = store.candidateSet(revision.id)!;
    const ws = await deps.workspace(revision.id);
    const { dispositions, verified } = await verifyCandidates(set.candidates, {
      prId: revision.pr_id,
      baseSha: revision.base_sha,
      headSha: revision.head_sha,
      files: ws.files,
      lineCount: (sha, path) => ws.repo.lineCount(sha, path),
      thresholds: config.verification,
    });
    store.advance(revision.id, "VERIFY", "CANDIDATES_DISPOSED", `${verified.length} verified, ${dispositions.length - verified.length} discarded`, () =>
      store.writeVerification(revision, dispositions, verified),
    );
    return { phase: "prepare", revisionId: revision.id, verifiedSetId: set.id };
  };

  const prepare: PhaseBody<"prepare"> = async (cp, revision, _runtime, _context, input) => {
    const findings = store.findingsForRevision(revision.id);
    const publish = input.publish === true && !(config.review.publish_mode === "silent_no_findings" && findings.length === 0);
    if (revision.state === "PREPARE_PUBLICATION") {
      const { headSha } = await livePull(revision);
      if (headSha !== revision.head_sha) {
        store.advance(revision.id, "PREPARE_PUBLICATION", "NEWER_HEAD", `head moved to ${headSha}`);
        return finished(revision.id, "SUPERSEDED");
      }
      // §9.3 step 1: the canonical payload is persisted with the transition into PUBLISH.
      store.advance(revision.id, "PREPARE_PUBLICATION", "HEAD_VERIFIED", `head still ${revision.head_sha}`, () => {
        if (!publish) return;
        const snapshot = store.snapshot(revision.id)!;
        const { payload, hash } = buildPayload({ revisionId: revision.id, headSha: revision.head_sha, findings, limitations: snapshot.limitations, files: parseDiff(snapshot.diff) });
        store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:${PUBLISHER_VERSION}`, payloadHash: hash, payloadJson: JSON.stringify(payload) });
      });
    }
    const outbox = store.outboxForRevision(revision.id);
    if (outbox) return { phase: "publish", revisionId: revision.id, outboxId: outbox.id };
    store.advance(revision.id, "PUBLISH", "NO_PUBLICATION_RECORDED", input.publish ? "no findings: silent_no_findings" : "dry run: nothing published");
    return finished(revision.id, "COMPLETED");
  };

  const target = (revision: RevisionRow) => {
    const pr = store.pullById(revision.pr_id)!;
    if (!deps.github) throw new Error("publishing needs a GitHub client");
    return { client: deps.github(pr.installation_id), pr: { fullName: pr.repo_full_name, number: pr.number } };
  };
  const toReconcile = (revision: RevisionRow, outboxId: string): ReviewCheckpoint => ({ phase: "reconcile", revisionId: revision.id, outboxId });

  /**
   * §9.3 steps 2, 4 and 5. What to do is decided by the outbox status alone, so a rerun after a crash at any point
   * converges: `sending` means a POST may have happened and goes to reconciliation, never to a second POST.
   */
  const publishPhase: PhaseBody<"publish"> = async (cp, revision, runtime) => {
    const outbox = store.outbox(cp.outboxId)!;
    switch (outbox.status) {
      case "published":
        store.advance(revision.id, "PUBLISH", "REVIEW_RECONCILED", `review ${outbox.github_review_id}`);
        return finished(revision.id, "COMPLETED");
      case "failed":
        return finished(revision.id, revision.state);
      case "sending":
      case "needs_reconciliation":
        store.tx(() => {
          if (outbox.status === "sending") store.advanceOutbox(outbox.id, "UNCERTAIN", "found in sending: the last POST may have reached GitHub");
          store.advance(revision.id, "PUBLISH", "WRITE_UNCERTAIN", "outbox needs reconciliation");
        });
        return toReconcile(revision, outbox.id);
      case "pending": {
        const { headSha } = await livePull(revision);
        if (headSha !== revision.head_sha) {
          store.advance(revision.id, "PUBLISH", "NEWER_HEAD", `head moved to ${headSha} before publication`);
          return finished(revision.id, "SUPERSEDED");
        }
        const { client, pr } = target(revision);
        store.advanceOutbox(outbox.id, "SEND", `attempt ${outbox.attempt_count + 1}`);
        const result = await postReview(client, pr, JSON.parse(outbox.payload_json) as ReviewPayload, runtime.signal);
        if (result.kind === "posted") {
          store.tx(() => {
            store.advanceOutbox(outbox.id, "SENT", `review ${result.reviewId}`, result.reviewId);
            store.markFindingsPublished(revision.id);
            store.advance(revision.id, "PUBLISH", "REVIEW_RECONCILED", `review ${result.reviewId} posted`);
          });
          return finished(revision.id, "COMPLETED");
        }
        if (result.kind === "rejected") {
          store.tx(() => {
            store.advanceOutbox(outbox.id, "REJECTED", result.error);
            store.advance(revision.id, "PUBLISH", "TERMINAL_ERROR", `GitHub rejected the review: ${result.error}`);
          });
          return finished(revision.id, "FAILED");
        }
        store.tx(() => {
          store.advanceOutbox(outbox.id, "UNCERTAIN", result.error);
          store.advance(revision.id, "PUBLISH", "WRITE_UNCERTAIN", result.error);
        });
        return toReconcile(revision, outbox.id);
      }
    }
  };

  /** §9.3 steps 6 and 7. Never posts; absence leads back to PUBLISH only while the attempt budget lasts. */
  const reconcile: PhaseBody<"reconcile"> = async (cp, revision) => {
    let outbox = store.outbox(cp.outboxId)!;
    if (outbox.status === "published") {
      store.advance(revision.id, "RECONCILE", "REVIEW_RECONCILED", `review ${outbox.github_review_id}`);
      return finished(revision.id, "COMPLETED");
    }
    if (outbox.status !== "pending") {
      const { client, pr } = target(revision);
      const verdict = await reconcileOutbox(store, client, pr, outbox);
      if (verdict.kind === "found") return finished(revision.id, "COMPLETED");
      if (verdict.kind === "undecided") {
        store.advance(revision.id, "RECONCILE", "TERMINAL_ERROR", `cannot_safely_decide: ${verdict.reason}`);
        return finished(revision.id, "FAILED");
      }
      outbox = store.outbox(cp.outboxId)!;
    }
    if (outbox.attempt_count > config.review.max_retries_per_phase) {
      store.tx(() => {
        store.advanceOutbox(outbox.id, "ABANDON", `confirmed absent after ${outbox.attempt_count} attempts`);
        store.advance(revision.id, "RECONCILE", "TERMINAL_ERROR", `publish failed: ${outbox.attempt_count} attempts, none reached GitHub`);
      });
      return finished(revision.id, "FAILED");
    }
    store.advance(revision.id, "RECONCILE", "CONFIRMED_NOT_POSTED", "complete listing has no review with the marker");
    return { phase: "publish", revisionId: revision.id, outboxId: outbox.id };
  };

  return defineTask<ReviewInput, ReviewCheckpoint, ReviewResult>({
    name: REVIEW_TASK_NAME,
    version: 1,
    initial: (input) => ({ phase: "collect", revisionId: input.revisionId }),
    phases: { collect: phase(collect), triage: phase(triage), analyze: phase(analyze), verify: phase(verify), prepare: phase(prepare), publish: phase(publishPhase), reconcile: phase(reconcile) },
    // An abort is SUPERSEDED only when the live head really moved; otherwise the revision FAILED for that reason.
    abort: async (task, runtime, context) => {
      const id = task.input.revisionId;
      const reason = `durable task aborted${task.abortReason ? ` (${task.abortReason})` : ""}`;
      let event: "NEWER_HEAD" | "TERMINAL_ERROR" = "TERMINAL_ERROR";
      let detail = reason;
      try {
        const revision = store.revision(id)!;
        const { headSha } = await livePull(revision);
        if (headSha !== revision.head_sha) [event, detail] = ["NEWER_HEAD", `${reason}; head moved to ${headSha}`];
      } catch (error) {
        detail = `${reason}; head check failed: ${error instanceof Error ? error.message : String(error)}`;
      }
      const state = store.advanceFromCurrent(id, event, detail) ?? store.advanceFromCurrent(id, "TERMINAL_ERROR", detail) ?? store.revision(id)!.state;
      await runtime.commit(() => ({ status: "terminal", outcome: { status: "aborted", reason: `revision ${state}` } }), context);
    },
  });
}
