import { sha256 } from "../domain/types.ts";
import { findingLabel, renderSummary, type SummaryFacts, unseen } from "../cli/report.ts";
import { type GitHubClient, GitHubError } from "../github/client.ts";
import type { AppStore, FindingView, OutboxRow } from "../persistence/store.ts";
import type { FileDiff } from "../source/diff.ts";

/**
 * Part of the idempotency key: a publisher that builds a different payload is a different publication. `p2` comments
 * only on findings never posted and lists carried ones in the body. A `p1` outbox already stored still posts as it is.
 */
export const PUBLISHER_VERSION = "p2";

export type ReviewComment = { path: string; line: number; side: "RIGHT"; body: string };
const UNCONFIRMED_NOTE = "Unconfirmed: the reviewer suspects this defect but could not show it from the cited code alone.";

/** The POST body, exactly as sent. `event` is always COMMENT: the reviewer never approves or requests changes. */
export type ReviewPayload = { commit_id: string; body: string; event: "COMMENT"; comments: ReviewComment[] };
export type PullTarget = { fullName: string; number: number };

export const marker = (revisionId: string, payloadHash: string) => `<!-- durable-review:${revisionId}:${payloadHash} -->`;
/** What every payload of one revision shares: a rerun builds a different payload, but it is the same review. */
const revisionMarker = (revisionId: string) => `<!-- durable-review:${revisionId}:`;

// Model text cannot open an HTML comment, so it cannot forge another revision's marker.
const inert = (text: string) => text.replaceAll("<!--", "&lt;!--");

/** GitHub rejects a review body over this many characters, and would then reject every review of the revision. */
export const MAX_BODY_CHARS = 65_536;
const TRUNCATION_NOTICE = "\n\n[The rest of this review body was cut to fit GitHub's size limit.]";

/** The body, cut at a line end with a notice when it and its marker line would exceed MAX_BODY_CHARS. The marker stays whole and last. */
function fitBody(body: string, revisionId: string): string {
  const room = MAX_BODY_CHARS - `\n${marker(revisionId, sha256(""))}\n`.length;
  if (body.length <= room) return body;
  return `${body.slice(0, room - TRUNCATION_NOTICE.length).replace(/\n?[^\n]*$/, "")}${TRUNCATION_NOTICE}`;
}

/**
 * §9.3 steps 1 and 3: the canonical payload and its hash. The hash covers everything but the marker, which carries
 * it. Inline comments go only on findings never posted before, anchored at this head inside a hunk of the pinned diff;
 * a finding carried from an earlier review is listed in the body instead.
 */
export function buildPayload(input: {
  revisionId: string;
  headSha: string;
  findings: FindingView[];
  limitations: string[];
  files: Map<string, FileDiff>;
  /** The summary template; empty or absent is the default. */
  template?: string;
  facts?: SummaryFacts;
}): { payload: ReviewPayload; hash: string; inlineFindingIds: string[] } {
  const inline = input.findings.filter((f) => !f.posted && f.anchored && f.candidate.newLine !== null && input.files.get(f.candidate.file)?.headLines.has(f.candidate.newLine));
  const comments: ReviewComment[] = inline.map(({ candidate: c }) => ({
    path: c.file,
    line: c.newLine!,
    side: "RIGHT",
    body: inert([`**${findingLabel(c)}: ${c.title}**`, ...(c.evidenceKind === "suspected" ? ["", UNCONFIRMED_NOTE] : []), "", c.explanation, ...(c.suggestedFix ? ["", `Suggested fix: ${c.suggestedFix}`] : [])].join("\n")),
  }));
  const body = fitBody(inert(renderSummary(input.headSha, input.findings, input.limitations, undefined, { template: input.template, facts: input.facts })), input.revisionId);
  const hash = sha256(JSON.stringify({ body, commit_id: input.headSha, event: "COMMENT", comments }));
  return { payload: { commit_id: input.headSha, body: `${body}\n${marker(input.revisionId, hash)}\n`, event: "COMMENT", comments }, hash, inlineFindingIds: inline.map((f) => f.findingId) };
}

/** Whether a review says something the reader has not seen: an inline comment, a finding new to them (`unseen`), or a newly resolved one. */
export const hasNews = (findings: FindingView[], inlineFindingIds: string[]) => inlineFindingIds.length > 0 || findings.some((f) => unseen(f) || f.status === "RESOLVED");

export type PostOutcome =
  | { kind: "posted"; reviewId: number }
  | { kind: "rejected"; error: string }
  | { kind: "not_sent"; error: string }
  | { kind: "uncertain"; error: string };

/**
 * §9.3 step 4. Only a definite GitHub refusal is `rejected`. A retryable write error is one GitHub never processed (a
 * rate limit, or a request that never left), so it is `not_sent`. Anything that may have landed is `uncertain`.
 */
export async function postReview(client: GitHubClient, pr: PullTarget, payload: ReviewPayload, signal?: AbortSignal): Promise<PostOutcome> {
  try {
    const { data } = await client.request<{ id: number }>("POST", `/repos/${pr.fullName}/pulls/${pr.number}/reviews`, { body: payload, ...(signal ? { signal } : {}) });
    return { kind: "posted", reviewId: data.id };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (error instanceof GitHubError && error.class === "fatal") return { kind: "rejected", error: message };
    if (error instanceof GitHubError && error.class === "retryable") return { kind: "not_sent", error: message };
    return { kind: "uncertain", error: message };
  }
}

export type ReconcileVerdict = { kind: "found"; reviewId: number } | { kind: "absent" } | { kind: "undecided"; reason: string };

type ListedReview = { id: number; body: string | null; commit_id: string | null; user: { login: string } | null };

/**
 * §9.3 step 6: every page of the PR's reviews, matched by the revision's marker (any payload hash), author and commit.
 * Absent is only claimed after a complete listing; a failed page or an unknown author is `undecided`.
 */
export async function findReview(client: GitHubClient, pr: PullTarget, outbox: OutboxRow): Promise<ReconcileVerdict> {
  const payload = JSON.parse(outbox.payload_json) as ReviewPayload;
  const wanted = revisionMarker(outbox.revision_id);
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
  return recordVerdict(store, outbox, await findReview(client, pr, outbox));
}

/** Write a reconciliation verdict onto an outbox in needs_reconciliation, and onto its revision when found. */
export function recordVerdict(store: AppStore, outbox: OutboxRow, verdict: ReconcileVerdict): ReconcileVerdict {
  store.tx(() => {
    if (verdict.kind === "found") {
      store.advanceOutbox(outbox.id, "FOUND", `review ${verdict.reviewId} found by marker`, verdict.reviewId);
      store.markFindingsPublished(outbox.id);
      store.advanceFromCurrent(outbox.revision_id, "REVIEW_RECONCILED", `review ${verdict.reviewId} found on GitHub`);
    } else if (verdict.kind === "absent") {
      store.advanceOutbox(outbox.id, "ABSENT", "complete listing has no review with the marker");
    } else {
      store.advanceOutbox(outbox.id, "UNDECIDED", `cannot_safely_decide: ${verdict.reason}`);
    }
  });
  return verdict;
}
