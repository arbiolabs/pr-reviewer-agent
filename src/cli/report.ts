import { type Disposition, type ResolutionVerdict, SEVERITIES } from "../domain/types.ts";
import type { AppStore, AuditEvent, FindingView, OutboxRow, RevisionRow } from "../persistence/store.ts";

export type ReviewReport = {
  revision: RevisionRow;
  fullName: string;
  number: number;
  mode: string | null;
  modeReasons: string[];
  limitations: string[];
  findings: FindingView[];
  dispositions: Disposition[];
  resolutions: ResolutionVerdict[];
  audit: AuditEvent[];
  outbox: OutboxRow | undefined;
};

export function loadReport(store: AppStore, revisionId: string): ReviewReport {
  const revision = store.revision(revisionId)!;
  const pr = store.pullById(revision.pr_id)!;
  const decision = store.decision(revisionId);
  return {
    revision,
    fullName: pr.repo_full_name,
    number: pr.number,
    mode: decision?.selectedMode ?? null,
    modeReasons: decision?.reasons ?? [],
    limitations: store.snapshot(revisionId)?.limitations ?? [],
    findings: store
      .findingsForRevision(revisionId)
      .sort((a, b) => SEVERITIES.indexOf(a.candidate.severity) - SEVERITIES.indexOf(b.candidate.severity)),
    dispositions: store.candidateSet(revisionId)?.dispositions ?? [],
    resolutions: store.candidateSet(revisionId)?.verdicts ?? [],
    audit: store.auditEvents(revisionId),
    outbox: store.outboxForRevision(revisionId),
  };
}

export const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Spec §12 summary format. A run that did not complete says so; it is never reported as a clean review. */
export function renderMarkdown(report: ReviewReport): string {
  const last = report.audit.at(-1);
  const incomplete = report.revision.state === "COMPLETED" ? undefined : `Review did not complete (${report.revision.state})${last?.detail ? `: ${last.detail}` : ""}.`;
  return renderSummary(report.revision.head_sha, report.findings, report.limitations, incomplete);
}

/**
 * The body section a carried finding is listed under, by the state this revision observed it in; any other state goes
 * last. "Looks resolved" is the only word on resolution: the bot never resolves or replies in a GitHub thread.
 */
const CARRIED_SECTIONS: [heading: string, states: string[]][] = [
  ["Still present from an earlier commit:", ["STILL_PRESENT"]],
  ["Looks resolved since the last review:", ["RESOLVED"]],
  ["Possibly fixed, the cited code changed:", ["POTENTIALLY_RESOLVED"]],
  ["Outdated, the cited file is gone:", ["OUTDATED"]],
  ["Uncertain, reported again without verifiable evidence:", ["UNCERTAIN"]],
  ["Carried from an earlier commit:", []],
];

/** Carried findings listed per section before "and N more": a long PR's body stays readable and bounded. */
const MAX_CARRIED_PER_SECTION = 20;

/**
 * A finding the reader has not seen: first found by this revision, or reopened by a revision whose review never reached
 * GitHub, as when a newer push superseded it. It is listed as new, with the commit it looked resolved at, so the
 * reopening is shown once whichever review carries it first.
 */
export const unseen = (f: FindingView) => !f.carried || (f.reopens !== null && !f.listed);

/** The §12 body for a head; `incomplete` replaces the findings line for a run that did not finish. */
export function renderSummary(headSha: string, findings: FindingView[], limitations: string[], incomplete?: string): string {
  const lines = ["## Automated PR review", "", `Reviewed commit: \`${headSha}\``, ""];
  const fresh = findings.filter(unseen);
  const carried = findings.filter((f) => !unseen(f));
  const where = (f: FindingView) => (f.anchored ? `${f.candidate.file}:${f.candidate.newLine}` : f.candidate.file);
  if (incomplete !== undefined) {
    lines.push(incomplete);
  } else if (fresh.length === 0) {
    lines.push(`No ${carried.length === 0 ? "" : "new "}actionable issues found in the reviewed scope.`);
  } else {
    const n = fresh.length;
    lines.push(`Found ${n} actionable ${n === 1 ? "issue" : "issues"}.`, "");
    for (const { candidate: c, reopens } of fresh) {
      const back = reopens ? `; looked resolved at \`${reopens.resolvedAt.slice(0, 7)}\`, but its code is still here` : "";
      lines.push(`- ${capitalize(c.severity)}: ${c.title} (\`${c.file}:${c.newLine}\`${back})`);
    }
  }
  const sectioned = new Set(CARRIED_SECTIONS.flatMap(([, states]) => states));
  for (const [heading, states] of CARRIED_SECTIONS) {
    const group = carried.filter((f) => (states.length === 0 ? !sectioned.has(f.status) : states.includes(f.status)));
    if (group.length === 0) continue;
    lines.push("", heading, "");
    for (const f of group.slice(0, MAX_CARRIED_PER_SECTION)) lines.push(`- ${capitalize(f.candidate.severity)}: ${f.candidate.title} (\`${where(f)}\`, first reported on \`${f.firstHeadSha.slice(0, 7)}\`)`);
    if (group.length > MAX_CARRIED_PER_SECTION) lines.push(`- and ${group.length - MAX_CARRIED_PER_SECTION} more`);
  }
  for (const limitation of limitations) lines.push("", `Scope limited: ${limitation}.`);
  lines.push("", "Review performed with isolated read-only tools. Human review is still required.");
  return `${lines.join("\n")}\n`;
}

export function renderJson(report: ReviewReport) {
  return {
    pullRequest: `${report.fullName}#${report.number}`,
    revisionId: report.revision.id,
    baseSha: report.revision.base_sha,
    headSha: report.revision.head_sha,
    state: report.revision.state,
    reviewMode: report.mode,
    reviewModeReasons: report.modeReasons,
    limitations: report.limitations,
    findings: report.findings.map((f) => ({ findingId: f.findingId, fingerprint: f.fingerprint, status: f.status, reopens: f.reopens, ...f.candidate })),
    discarded: report.dispositions.filter((d) => d.state === "DISCARDED"),
    resolutions: report.resolutions,
    published: report.outbox?.status === "published",
    githubReviewId: report.outbox?.github_review_id ?? null,
    outboxStatus: report.outbox?.status ?? null,
  };
}
