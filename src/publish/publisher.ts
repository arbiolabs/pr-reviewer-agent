import { sha256 } from "../domain/types.ts";
import { capitalize, renderSummary } from "../cli/report.ts";
import { type GitHubClient, GitHubError } from "../github/client.ts";
import type { AppStore, FindingView, OutboxRow } from "../persistence/store.ts";
import type { FileDiff } from "../source/diff.ts";

/** Part of the idempotency key: a publisher that builds a different payload is a different publication. */
export const PUBLISHER_VERSION = "p1";

export type ReviewComment = { path: string; line: number; side: "RIGHT"; body: string };
/** The POST body, exactly as sent. `event` is always COMMENT: the reviewer never approves or requests changes. */
export type ReviewPayload = { commit_id: string; body: string; event: "COMMENT"; comments: ReviewComment[] };
export type PullTarget = { fullName: string; number: number };

export const marker = (revisionId: string, payloadHash: string) => `<!-- durable-review:${revisionId}:${payloadHash} -->`;

// Model text cannot open an HTML comment, so it cannot forge another revision's marker.
const inert = (text: string) => text.replaceAll("<!--", "&lt;!--");

/**
 * §9.3 steps 1 and 3: the canonical payload and its hash. The hash covers everything but the marker, which carries
 * it. Inline comments go only on verified findings whose head line is inside a hunk of the pinned diff.
 */
export function buildPayload(input: {
  revisionId: string;
  headSha: string;
  findings: FindingView[];
  limitations: string[];
  files: Map<string, FileDiff>;
}): { payload: ReviewPayload; hash: string } {
  const comments: ReviewComment[] = input.findings
    .filter(({ candidate: c }) => c.newLine !== null && input.files.get(c.file)?.headLines.has(c.newLine))
    .map(({ candidate: c }) => ({
      path: c.file,
      line: c.newLine!,
      side: "RIGHT",
      body: inert([`**${capitalize(c.severity)}: ${c.title}**`, "", c.explanation, ...(c.suggestedFix ? ["", `Suggested fix: ${c.suggestedFix}`] : [])].join("\n")),
    }));
  const body = inert(renderSummary(input.headSha, input.findings, input.limitations));
  const hash = sha256(JSON.stringify({ body, commit_id: input.headSha, event: "COMMENT", comments }));
  return { payload: { commit_id: input.headSha, body: `${body}\n${marker(input.revisionId, hash)}\n`, event: "COMMENT", comments }, hash };
}

export type PostOutcome = { kind: "posted"; reviewId: number } | { kind: "rejected"; error: string } | { kind: "uncertain"; error: string };

/** §9.3 step 4. Only a definite GitHub refusal is `rejected`; anything that may have landed is `uncertain`. */
export async function postReview(client: GitHubClient, pr: PullTarget, payload: ReviewPayload, signal?: AbortSignal): Promise<PostOutcome> {
  try {
    const { data } = await client.request<{ id: number }>("POST", `/repos/${pr.fullName}/pulls/${pr.number}/reviews`, { body: payload, ...(signal ? { signal } : {}) });
    return { kind: "posted", reviewId: data.id };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return error instanceof GitHubError && error.class === "fatal" ? { kind: "rejected", error: message } : { kind: "uncertain", error: message };
  }
}

export type ReconcileVerdict = { kind: "found"; reviewId: number } | { kind: "absent" } | { kind: "undecided"; reason: string };

type ListedReview = { id: number; body: string | null; commit_id: string | null; user: { login: string } | null };

/**
 * §9.3 step 6: every page of the PR's reviews, matched by marker, author and commit. Absent is only claimed after a
 * complete listing; a failed page or an unknown author is `undecided`.
 */
export async function findReview(client: GitHubClient, pr: PullTarget, outbox: OutboxRow): Promise<ReconcileVerdict> {
  const payload = JSON.parse(outbox.payload_json) as ReviewPayload;
  const wanted = marker(outbox.revision_id, outbox.payload_hash);
  try {
    const author = await client.viewerLogin();
    const reviews = await client.paginate<ListedReview>(`/repos/${pr.fullName}/pulls/${pr.number}/reviews`);
    const match = reviews.find((r) => r.body?.includes(wanted) && r.user?.login === author && r.commit_id === payload.commit_id);
    return match ? { kind: "found", reviewId: match.id } : { kind: "absent" };
  } catch (error) {
    return { kind: "undecided", reason: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * Reconcile one outbox that may have reached GitHub, without ever posting. Found: the outbox and its findings are
 * published, and the revision completes if its state allows. Absent: the outbox goes back to pending. Undecided: it
 * stays in needs_reconciliation.
 */
export async function reconcileOutbox(store: AppStore, client: GitHubClient, pr: PullTarget, outbox: OutboxRow): Promise<ReconcileVerdict> {
  if (outbox.status === "sending") store.advanceOutbox(outbox.id, "UNCERTAIN", "found in sending: the last POST may have reached GitHub");
  const verdict = await findReview(client, pr, outbox);
  store.tx(() => {
    if (verdict.kind === "found") {
      store.advanceOutbox(outbox.id, "FOUND", `review ${verdict.reviewId} found by marker`, verdict.reviewId);
      store.markFindingsPublished(outbox.revision_id);
      store.advanceFromCurrent(outbox.revision_id, "REVIEW_RECONCILED", `review ${verdict.reviewId} found on GitHub`);
    } else if (verdict.kind === "absent") {
      store.advanceOutbox(outbox.id, "ABSENT", "complete listing has no review with the marker");
    } else {
      store.advanceOutbox(outbox.id, "UNDECIDED", `cannot_safely_decide: ${verdict.reason}`);
    }
  });
  return verdict;
}
