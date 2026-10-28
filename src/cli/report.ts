import { type CandidateFinding, type Disposition, type ResolutionVerdict, SEVERITIES } from "../domain/types.ts";
import type { AppStore, AuditEvent, FindingView, OutboxRow, PullRow, RevisionRow, Snapshot } from "../persistence/store.ts";
import { effectiveTemplate, fillTemplate, type TemplateValues } from "../publish/template.ts";

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
  facts: SummaryFacts;
};

export function loadReport(store: AppStore, revisionId: string): ReviewReport {
  const revision = store.revision(revisionId)!;
  const pr = store.pullById(revision.pr_id)!;
  const decision = store.decision(revisionId);
  const snapshot = store.snapshot(revisionId);
  return {
    revision,
    fullName: pr.repo_full_name,
    number: pr.number,
    mode: decision?.selectedMode ?? null,
    modeReasons: decision?.reasons ?? [],
    limitations: snapshot?.limitations ?? [],
    findings: store
      .findingsForRevision(revisionId)
      .sort((a, b) => SEVERITIES.indexOf(a.candidate.severity) - SEVERITIES.indexOf(b.candidate.severity)),
    dispositions: store.candidateSet(revisionId)?.dispositions ?? [],
    resolutions: store.candidateSet(revisionId)?.verdicts ?? [],
    audit: store.auditEvents(revisionId),
    outbox: store.outboxForRevision(revisionId),
    facts: summaryFacts(revision, pr, snapshot),
  };
}

export const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** `High`, or `High (unconfirmed)` for a finding the model only suspected. */
export const findingLabel = (c: CandidateFinding) => capitalize(c.severity) + (c.evidenceKind === "suspected" ? " (unconfirmed)" : "");

/** Spec §12 summary format. A run that did not complete says so; it is never reported as a clean review. */
export function renderMarkdown(report: ReviewReport, template = ""): string {
  const last = report.audit.at(-1);
  const incomplete = report.revision.state === "COMPLETED" ? undefined : `Review did not complete (${report.revision.state})${last?.detail ? `: ${last.detail}` : ""}.`;
  return renderSummary(report.revision.head_sha, report.findings, report.limitations, incomplete, { template, facts: report.facts });
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

/** What the summary says about the PR beside its findings; a caller that does not know a part leaves it empty. */
export type SummaryFacts = { repo: string; prNumber: number | null; prTitle: string; prAuthor: string; model: string };

const NO_FACTS: SummaryFacts = { repo: "", prNumber: null, prTitle: "", prAuthor: "", model: "" };

/** A revision's facts: the title its context recorded, else the PR's; the model is the part of its reviewer version after `m1+`. */
export const summaryFacts = (revision: RevisionRow, pr: PullRow, snapshot: Snapshot | undefined): SummaryFacts => ({
  repo: pr.repo_full_name,
  prNumber: pr.number,
  prTitle: snapshot?.pr.title ?? pr.title ?? "",
  prAuthor: pr.author ?? "",
  model: revision.reviewer_version.slice(revision.reviewer_version.indexOf("+") + 1),
});

/**
 * The §12 body for a head, through `template` (the default when empty); `incomplete` replaces the findings line for a
 * run that did not finish.
 */
export function renderSummary(headSha: string, findings: FindingView[], limitations: string[], incomplete?: string, options: { template?: string; facts?: SummaryFacts } = {}): string {
  return fillTemplate(effectiveTemplate(options.template ?? ""), summaryValues(headSha, findings, limitations, incomplete, options.facts ?? NO_FACTS));
}

function summaryValues(headSha: string, findings: FindingView[], limitations: string[], incomplete: string | undefined, facts: SummaryFacts): TemplateValues {
  const fresh = findings.filter(unseen);
  const carried = findings.filter((f) => !unseen(f));
  const where = (f: FindingView) => (f.anchored ? `${f.candidate.file}:${f.candidate.newLine}` : f.candidate.file);
  const n = fresh.length;
  const summaryLine =
    incomplete ?? (n === 0 ? `No ${carried.length === 0 ? "" : "new "}actionable issues found in the reviewed scope.` : `Found ${n} actionable ${n === 1 ? "issue" : "issues"}.`);
  const findingsList = (incomplete === undefined ? fresh : []).map(({ candidate: c, reopens }) => {
    const back = reopens ? `; looked resolved at \`${reopens.resolvedAt.slice(0, 7)}\`, but its code is still here` : "";
    return `- ${findingLabel(c)}: ${c.title} (\`${c.file}:${c.newLine}\`${back})`;
  });
  const sectioned = new Set(CARRIED_SECTIONS.flatMap(([, states]) => states));
  const sections: string[] = [];
  for (const [heading, states] of CARRIED_SECTIONS) {
    const group = carried.filter((f) => (states.length === 0 ? !sectioned.has(f.status) : states.includes(f.status)));
    if (group.length === 0) continue;
    const lines = group.slice(0, MAX_CARRIED_PER_SECTION).map((f) => `- ${findingLabel(f.candidate)}: ${f.candidate.title} (\`${where(f)}\`, first reported on \`${f.firstHeadSha.slice(0, 7)}\`)`);
    if (group.length > MAX_CARRIED_PER_SECTION) lines.push(`- and ${group.length - MAX_CARRIED_PER_SECTION} more`);
    sections.push([heading, "", ...lines].join("\n"));
  }
  const count = (keep: (f: FindingView) => boolean) => String(fresh.filter(keep).length);
  return {
    repo: facts.repo,
    pr_number: facts.prNumber === null ? "" : String(facts.prNumber),
    pr_title: facts.prTitle,
    pr_author: facts.prAuthor,
    head_sha: headSha,
    short_sha: headSha.slice(0, 7),
    model: facts.model,
    summary_line: summaryLine,
    findings_list: findingsList.join("\n"),
    findings_count: String(n),
    critical_count: count((f) => f.candidate.severity === "critical"),
    high_count: count((f) => f.candidate.severity === "high"),
    medium_count: count((f) => f.candidate.severity === "medium"),
    low_count: count((f) => f.candidate.severity === "low"),
    unconfirmed_count: count((f) => f.candidate.evidenceKind === "suspected"),
    carried: sections.join("\n\n"),
    limitations: limitations.map((l) => `Scope limited: ${l}.`).join("\n\n"),
  };
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
