import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { TaskId } from "@earendil-works/pi-durable";
import { IDLE_REVIEW_STATES, TERMINAL_REVIEW_STATES } from "../domain/machines.ts";
import type { PullSource } from "../github/pulls.ts";
import type { RevisionRow } from "../persistence/store.ts";
import { type App, ensureTask, retryReview, type StartedReview, startReview } from "./app.ts";

/**
 * The guarded review commands (TUI_SPEC §3C, §3D), shared by the daemon and the CLI. Each one checks the revision's
 * state and GitHub's live head itself, whatever the caller showed; a refusal names the protocol error code.
 */
export class Refused extends Error {
  readonly code: "NOT_FOUND" | "INVALID_TRANSITION" | "REVISION_STALE" | "CONFLICT";

  constructor(code: Refused["code"], message: string) {
    super(message);
    this.code = code;
  }
}

const ctx = BACKGROUND_CONTEXT;

function revisionOf(app: App, revisionId: string): RevisionRow {
  const revision = app.store.revision(revisionId);
  if (!revision) throw new Refused("NOT_FOUND", `no run ${revisionId}`);
  return revision;
}

export async function livePull(app: App, pulls: PullSource, prId: string) {
  const pr = app.store.pullById(prId);
  if (!pr) throw new Refused("NOT_FOUND", `no PR ${prId}`);
  const [owner, repo] = pr.repo_full_name.split("/") as [string, string];
  const live = await pulls.getPull({ owner, repo, number: pr.number, installationId: pr.installation_id });
  app.store.upsertPull(live);
  return live;
}

/** review.start: a tracked PR at the head the caller saw, which must still be GitHub's; a head already reviewed is a conflict. */
export async function startFromHead(
  app: App,
  pulls: PullSource,
  params: { prId: string; expectedHeadSha: string },
  options: { basePolicyVersion: string; publish: boolean },
): Promise<StartedReview> {
  const live = await livePull(app, pulls, params.prId);
  if (live.headSha !== params.expectedHeadSha) throw new Refused("REVISION_STALE", `the head of ${live.fullName}#${live.number} is now ${live.headSha}`);
  if (live.state !== "open") throw new Refused("CONFLICT", `${live.fullName}#${live.number} is ${live.merged ? "merged" : "closed"}`);
  const started = await startReview(app, live, options.basePolicyVersion, { publish: options.publish });
  // No task means an idle revision: an ended one is refused here, and the only other idle state is parked.
  // The message names the PR as #N, as refuseReviewed and the client's own refusals do; the TUI is its only reader.
  if (started.taskId === undefined) {
    refuseReviewed(app, started.revision.id);
    throw new Refused("CONFLICT", `#${live.number} at ${live.headSha.slice(0, 7)} is waiting for your approval`);
  }
  return started;
}

/** A start names a head, so once that head's review has ended it is refused, whatever key it comes under. */
export function refuseReviewed(app: App, revisionId: string): void {
  const revision = app.store.revision(revisionId);
  if (!revision || !TERMINAL_REVIEW_STATES.includes(revision.state)) return;
  const pr = app.store.pullById(revision.pr_id);
  const head = `#${pr?.number ?? revision.pr_id} at ${revision.head_sha.slice(0, 7)}`;
  const why = { COMPLETED: "is already reviewed", FAILED: "failed its review; retry it instead", SUPERSEDED: "is superseded by a newer head" };
  throw new Refused("CONFLICT", `${head} ${why[revision.state as keyof typeof why] ?? "is already reviewed"}`);
}

/** review.retry: only a FAILED revision reopens, through the audited MANUAL_RETRY override. */
export async function retry(app: App, revisionId: string, retrySource: string, onCommit?: () => void): Promise<StartedReview> {
  const revision = revisionOf(app, revisionId);
  if (revision.state !== "FAILED") throw new Refused("INVALID_TRANSITION", `MANUAL_RETRY is not legal in ${revision.state}`);
  return retryReview(app, revision, retrySource, onCommit);
}

/**
 * review.cancel: refused while a GitHub write may be in flight (PUBLISH, RECONCILE). A parked revision is failed
 * directly; a running one has its durable task aborted, and the abort handler reports `reason`.
 */
export async function cancel(app: App, revisionId: string, reason: string): Promise<void> {
  const revision = revisionOf(app, revisionId);
  if (revision.state === "PUBLISH" || revision.state === "RECONCILE") throw new Refused("CONFLICT", `cannot cancel in ${revision.state}: a GitHub write may be in flight`);
  if (TERMINAL_REVIEW_STATES.includes(revision.state)) throw new Refused("INVALID_TRANSITION", `CANCELLED is not legal in ${revision.state}`);
  if (revision.state === "AWAITING_APPROVAL") {
    app.store.advance(revision.id, "AWAITING_APPROVAL", "CANCELLED", reason);
    return;
  }
  if (!revision.durable_task_id) throw new Refused("CONFLICT", `revision ${revision.id} has no durable task yet`);
  app.store.requestCancel(revision.id, reason);
  await app.harness.abortTask(Number(revision.durable_task_id) as TaskId, ctx);
}

/**
 * review.publish: approve exactly the stored payload (`previewId` is its hash) for exactly the head the caller saw,
 * which must still be GitHub's open head. Then the audited APPROVED edge opens the attempt that runs the existing
 * exactly-once publisher. `onCommit` runs in APPROVED's transaction.
 */
export async function approve(
  app: App,
  pulls: PullSource,
  params: { revisionId: string; previewId: string; expectedHeadSha?: string },
  source: string,
  onCommit?: () => void,
): Promise<StartedReview> {
  const revision = revisionOf(app, params.revisionId);
  if (revision.state !== "AWAITING_APPROVAL") throw new Refused("INVALID_TRANSITION", `APPROVED is not legal in ${revision.state}`);
  const outbox = app.store.outboxForRevision(revision.id);
  if (!outbox || outbox.payload_hash !== params.previewId) throw new Refused("CONFLICT", `preview ${params.previewId} is not the stored payload${outbox ? ` ${outbox.payload_hash}` : ""}`);
  if (params.expectedHeadSha !== undefined && params.expectedHeadSha !== revision.head_sha) {
    throw new Refused("REVISION_STALE", `revision ${revision.id} reviewed ${revision.head_sha}, not ${params.expectedHeadSha}`);
  }
  const live = await livePull(app, pulls, revision.pr_id);
  if (live.state !== "open") {
    app.store.advance(revision.id, "AWAITING_APPROVAL", "PR_NOT_OPEN", `pr_not_open: ${live.merged ? "merged" : "closed"}`);
    throw new Refused("REVISION_STALE", `${live.fullName}#${live.number} is ${live.merged ? "merged" : "closed"}; revision ${revision.id} is superseded`);
  }
  if (live.headSha !== revision.head_sha) {
    app.store.advance(revision.id, "AWAITING_APPROVAL", "NEWER_HEAD", `head moved to ${live.headSha}`);
    throw new Refused("REVISION_STALE", `the head of ${live.fullName}#${live.number} is now ${live.headSha}; revision ${revision.id} is superseded`);
  }
  app.store.advance(revision.id, "AWAITING_APPROVAL", "APPROVED", `approved by ${source} (preview ${outbox.payload_hash})`, onCommit);
  const approved = app.store.revision(revision.id)!;
  return { revision: approved, taskId: await ensureTask(app, approved, true) };
}

/** A revision whose approved attempt is in flight (after a crash, say) resumes it; `undefined` otherwise. */
export async function resumeApproved(app: App, revision: RevisionRow): Promise<StartedReview | undefined> {
  if (IDLE_REVIEW_STATES.includes(revision.state) || app.store.reopenings(revision.id).at(-1) !== "APPROVED") return undefined;
  return { revision, taskId: await ensureTask(app, revision, true) };
}
