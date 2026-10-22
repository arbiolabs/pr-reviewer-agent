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
import type { AppStore, OpenFinding, RevisionRow } from "../persistence/store.ts";
import { buildReviewRequest } from "../reasoning/request.ts";
import { TargetDoc, type Workspace } from "../reasoning/tools.ts";
import { evidenceTexts, type HeadReader, headReader, locate, observe, type ObservedLocation, type PriorFinding, reopen } from "../reasoning/track.ts";
import { fingerprint, verifyCandidates } from "../reasoning/verify.ts";
import { PinnedRepo, syncRepo } from "../source/git.ts";
import type { GitHubClient } from "../github/client.ts";
import { buildPayload, findReview, hasNews, postReview, PUBLISHER_VERSION, type ReviewPayload, recordVerdict } from "../publish/publisher.ts";
import { parseDiff } from "../source/diff.ts";

export const REVIEW_TASK_NAME = "pr-review.revision";

/**
 * Resolved findings checked for their code on each later head, most recently resolved first. The check costs a file
 * read per finding, so the bound keeps a long PR's review from growing with every resolution; a finding past it stays
 * resolved unchecked, which is the only way this bound fails, and it fails toward the status already shown.
 */
export const MAX_WATCHED = 50;

/**
 * `retry` counts the reopenings (MANUAL_RETRY or APPROVED) before this task: each gets its own durable task. `publish`
 * posts the review through the outbox; without it PUBLISH records the intentional no-publication (dry run).
 * `approvedOutboxId` is the attempt a human approval opens: it starts at the publish phase of that outbox.
 */
export type ReviewInput = { revisionId: string; retry?: number; publish?: boolean; approvedOutboxId?: string };
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
    // The open findings are located here for the request and again in VERIFY: each phase stands alone after a crash.
    const { prior } = await priorFindings(revision, await deps.workspace(revision.id));
    const content = buildReviewRequest({
      snapshot: store.snapshot(revision.id)!,
      baseSha: revision.base_sha,
      headSha: revision.head_sha,
      mode: decision.selectedMode as ReviewMode,
      reasons: decision.reasons,
      prior,
    });
    // The same request ID finds the same submission after a restart; a retry asks again under a new one.
    const requestId = cp.attempt ? `analyze:${revision.id}:${cp.attempt}` : `analyze:${revision.id}`;
    const settled = await (await conversation.submit({ type: "input", content, requestId }, context)).wait(context);
    if (settled.status !== "done") throw new Error(`analysis ${settled.status}: ${settled.reason}${settled.detail ? ` ${JSON.stringify(settled.detail)}` : ""}`);
  }

  /**
   * Every open finding of the PR, located at this revision's head, plus every resolved finding whose code is back
   * (`reopen`). Rows from before migration 055 get their evidence text from the head they were seen on. A finding
   * already doubted was searched for tree-wide by the head that doubted it, so only its own file is read again: a
   * defect that comes back elsewhere is a new finding.
   */
  async function priorFindings(revision: RevisionRow, ws: Workspace): Promise<{ prior: PriorFinding[]; head: HeadReader }> {
    const read = (sha: string, path: string) => ws.repo.show(sha, path);
    const head = headReader(ws.repo, ws.snapshot.changedFiles);
    const locationOf = async (f: OpenFinding): Promise<ObservedLocation> => ("evidence" in f.location ? f.location : { ...f.location, headSha: f.observedAt, evidence: await evidenceTexts(f.candidate, f.observedAt, read) });
    const prior: PriorFinding[] = [];
    for (const f of store.openFindings(revision.pr_id, revision.id)) {
      const location = await locationOf(f);
      const treeWide = f.status !== "POTENTIALLY_RESOLVED" && f.status !== "OUTDATED";
      prior.push({ findingId: f.findingId, status: f.status, candidate: f.candidate, location, located: await locate(location, revision.head_sha, head, { treeWide }) });
    }
    for (const f of store.resolvedFindings(revision.pr_id, revision.id, MAX_WATCHED)) {
      const reopened = await reopen({ ...f, location: await locationOf(f) }, revision.pr_id, revision.id, revision.head_sha, head);
      if (reopened) prior.push(reopened);
    }
    return { prior, head };
  }

  const verify: PhaseBody<"verify"> = async (cp, revision) => {
    const set = store.candidateSet(revision.id)!;
    const ws = await deps.workspace(revision.id);
    const { prior, head } = await priorFindings(revision, ws);
    const { dispositions, verified, unverified, verdicts } = await verifyCandidates(
      set.candidates,
      {
        prId: revision.pr_id,
        revisionId: revision.id,
        baseSha: revision.base_sha,
        headSha: revision.head_sha,
        files: ws.files,
        read: (sha, path) => ws.repo.show(sha, path),
        head,
        thresholds: config.verification,
        prior,
      },
      set.resolutions,
    );
    const observations = observe(prior, verified, { verdicts, unverified });
    // A reopened finding is new to the store: it gets a row like a verified candidate's, unless the model's re-report already linked to it.
    const reopens = prior.filter((f) => f.reopens);
    const reopened = reopens.filter((f) => !verified.some((v) => v.findingId === f.findingId)).map((f) => ({ findingId: f.findingId, fingerprint: fingerprint(f.candidate), candidate: f.candidate }));
    const resolved = observations.filter((o) => o.path.at(-1) === "RESOLVED").length;
    const detail = `${verified.length} verified, ${dispositions.length - verified.length} discarded, ${prior.length - reopens.length} carried, ${resolved} resolved, ${reopens.length} reopened`;
    store.advance(revision.id, "VERIFY", "CANDIDATES_DISPOSED", detail, () => store.writeVerification(revision, dispositions, [...verified, ...reopened], observations, verdicts));
    return { phase: "prepare", revisionId: revision.id, verifiedSetId: set.id };
  };

  const prepare: PhaseBody<"prepare"> = async (cp, revision, _runtime, _context, input) => {
    const findings = store.findingsForRevision(revision.id);
    const snapshot = store.snapshot(revision.id)!;
    const { payload, hash, inlineFindingIds } = buildPayload({ revisionId: revision.id, headSha: revision.head_sha, findings, limitations: snapshot.limitations, files: parseDiff(snapshot.diff) });
    // silent_no_findings posts only a review that says something new: a new finding, a first comment on a carried one, or a newly resolved one.
    const publish = input.publish === true && !(config.review.publish_mode === "silent_no_findings" && !hasNews(findings, inlineFindingIds));
    // Approval mode stores the payload the same way, then parks the revision for a human instead of posting.
    const approval = publish && config.review.publish_mode === "approval";
    if (revision.state === "PREPARE_PUBLICATION") {
      const { headSha } = await livePull(revision);
      if (headSha !== revision.head_sha) {
        store.advance(revision.id, "PREPARE_PUBLICATION", "NEWER_HEAD", `head moved to ${headSha}`);
        return finished(revision.id, "SUPERSEDED");
      }
      // §9.3 step 1: the canonical payload is persisted with the transition into PUBLISH (or AWAITING_APPROVAL).
      store.advance(revision.id, "PREPARE_PUBLICATION", approval ? "APPROVAL_REQUIRED" : "HEAD_VERIFIED", `head still ${revision.head_sha}`, () => {
        if (!publish) return;
        store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:${PUBLISHER_VERSION}`, payloadHash: hash, payloadJson: JSON.stringify(payload), inlineFindingIds });
      });
    }
    // The decision to park is the revision's state, not the live config: a rerun after a crash (even under another
    // publish_mode) ends this attempt here, and the approved attempt owns the rest.
    if (store.revision(revision.id)!.state === "AWAITING_APPROVAL") return finished(revision.id, "AWAITING_APPROVAL");
    const outbox = store.outboxForRevision(revision.id);
    if (outbox) return { phase: "publish", revisionId: revision.id, outboxId: outbox.id };
    store.advance(revision.id, "PUBLISH", "NO_PUBLICATION_RECORDED", input.publish ? "nothing new to post: silent_no_findings" : "dry run: nothing published");
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
   * converges: `sending` means a POST may have happened and goes to reconciliation, never to a second POST. Before
   * any POST, a complete listing must show no review of this revision; a failed listing never leads to a POST.
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
        // Only an attempt whose revision is in PUBLISH may post; a stale one (resumed for a parked revision) ends with no POST.
        if (revision.state !== "PUBLISH") return finished(revision.id, revision.state);
        const { headSha, state, merged } = await livePull(revision);
        if (state !== "open") {
          store.advance(revision.id, "PUBLISH", "PR_NOT_OPEN", `pr_not_open: ${merged ? "merged" : "closed"}`);
          return finished(revision.id, "SUPERSEDED");
        }
        if (headSha !== revision.head_sha) {
          store.advance(revision.id, "PUBLISH", "NEWER_HEAD", `head moved to ${headSha} before publication`);
          return finished(revision.id, "SUPERSEDED");
        }
        const { client, pr } = target(revision);
        // The revision's review may already be on GitHub (posted from another data dir, or by a lost run): adopt it.
        const existing = await findReview(client, pr, outbox);
        if (existing.kind === "found") {
          store.tx(() => {
            store.advanceOutbox(outbox.id, "FOUND", `review ${existing.reviewId} already on GitHub`, existing.reviewId);
            store.markFindingsPublished(outbox.id);
            store.advance(revision.id, "PUBLISH", "REVIEW_RECONCILED", `review ${existing.reviewId} already on GitHub`);
          });
          return finished(revision.id, "COMPLETED");
        }
        if (existing.kind === "undecided") {
          if ((cp.attempt ?? 0) < config.review.max_retries_per_phase) throw new Error(`cannot list reviews before posting: ${existing.reason}`);
          store.tx(() => {
            store.advanceOutbox(outbox.id, "UNDECIDED", `cannot_safely_decide: ${existing.reason}`);
            store.advance(revision.id, "PUBLISH", "TERMINAL_ERROR", `cannot_safely_decide before posting: ${existing.reason}`);
          });
          return finished(revision.id, "FAILED");
        }
        store.advanceOutbox(outbox.id, "SEND", `attempt ${outbox.attempt_count + 1}`);
        const result = await postReview(client, pr, JSON.parse(outbox.payload_json) as ReviewPayload, runtime.signal);
        if (result.kind === "posted") {
          store.tx(() => {
            store.advanceOutbox(outbox.id, "SENT", `review ${result.reviewId}`, result.reviewId);
            store.markFindingsPublished(outbox.id);
            store.advance(revision.id, "PUBLISH", "REVIEW_RECONCILED", `review ${result.reviewId} posted`);
          });
          return finished(revision.id, "COMPLETED");
        }
        if (result.kind === "not_sent") {
          // GitHub never saw it: the outbox is pending again, and the phase retries (or the abort takes over).
          store.advanceOutbox(outbox.id, "NOT_SENT", result.error);
          throw new Error(result.error);
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

  /**
   * §9.3 steps 6 and 7. Never posts; absence leads back to PUBLISH only while the attempt budget lasts. A write that
   * timed out can still land, so the first listing waits a client timeout, and absence needs two complete listings
   * that far apart.
   */
  const reconcile: PhaseBody<"reconcile"> = async (cp, revision, runtime, context) => {
    let outbox = store.outbox(cp.outboxId)!;
    if (outbox.status === "published") {
      store.advance(revision.id, "RECONCILE", "REVIEW_RECONCILED", `review ${outbox.github_review_id}`);
      return finished(revision.id, "COMPLETED");
    }
    if (outbox.status !== "pending") {
      const { client, pr } = target(revision);
      const gap = config.github.timeout_ms;
      await runtime.sleep(runtime.now() + gap, context);
      let verdict = await findReview(client, pr, outbox);
      if (verdict.kind === "absent") {
        await runtime.sleep(runtime.now() + gap, context);
        verdict = await findReview(client, pr, outbox);
      }
      recordVerdict(store, outbox, verdict);
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
    store.advance(revision.id, "RECONCILE", "CONFIRMED_NOT_POSTED", `two complete listings, ${config.github.timeout_ms} ms apart, have no review with the marker`);
    return { phase: "publish", revisionId: revision.id, outboxId: outbox.id };
  };

  return defineTask<ReviewInput, ReviewCheckpoint, ReviewResult>({
    name: REVIEW_TASK_NAME,
    version: 1,
    initial: (input) =>
      input.approvedOutboxId ? { phase: "publish", revisionId: input.revisionId, outboxId: input.approvedOutboxId } : { phase: "collect", revisionId: input.revisionId },
    phases: { collect: phase(collect), triage: phase(triage), analyze: phase(analyze), verify: phase(verify), prepare: phase(prepare), publish: phase(publishPhase), reconcile: phase(reconcile) },
    // An abort is SUPERSEDED only when the live head really moved or the PR closed; otherwise the revision FAILED.
    abort: async (task, runtime, context) => {
      const id = task.input.revisionId;
      const reason = store.revision(id)?.cancel_reason ?? `durable task aborted${task.abortReason ? ` (${task.abortReason})` : ""}`;
      let event: "NEWER_HEAD" | "PR_NOT_OPEN" | "TERMINAL_ERROR" = "TERMINAL_ERROR";
      let detail = reason;
      try {
        const revision = store.revision(id)!;
        const { headSha, state, merged } = await livePull(revision);
        if (state !== "open") [event, detail] = ["PR_NOT_OPEN", `${reason}; pr_not_open: ${merged ? "merged" : "closed"}`];
        else if (headSha !== revision.head_sha) [event, detail] = ["NEWER_HEAD", `${reason}; head moved to ${headSha}`];
      } catch (error) {
        detail = `${reason}; head check failed: ${error instanceof Error ? error.message : String(error)}`;
      }
      const state = store.advanceFromCurrent(id, event, detail) ?? store.advanceFromCurrent(id, "TERMINAL_ERROR", detail) ?? store.revision(id)!.state;
      await runtime.commit(() => ({ status: "terminal", outcome: { status: "aborted", reason: `revision ${state}` } }), context);
    },
  });
}
