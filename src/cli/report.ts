import { type Disposition, SEVERITIES } from "../domain/types.ts";
import type { AppStore, AuditEvent, FindingView, RevisionRow } from "../persistence/store.ts";

export type ReviewReport = {
  revision: RevisionRow;
  fullName: string;
  number: number;
  mode: string | null;
  modeReasons: string[];
  limitations: string[];
  findings: FindingView[];
  dispositions: Disposition[];
  audit: AuditEvent[];
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
    audit: store.auditEvents(revisionId),
  };
}

const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Spec §12 summary format. A run that did not complete says so; it is never reported as a clean review. */
export function renderMarkdown(report: ReviewReport): string {
  const lines = ["## Automated PR review", "", `Reviewed commit: \`${report.revision.head_sha}\``, ""];
  if (report.revision.state !== "COMPLETED") {
    const last = report.audit.at(-1);
    lines.push(`Review did not complete (${report.revision.state})${last?.detail ? `: ${last.detail}` : ""}.`);
  } else if (report.findings.length === 0) {
    lines.push("No actionable issues found in the reviewed scope.");
  } else {
    const n = report.findings.length;
    lines.push(`Found ${n} actionable ${n === 1 ? "issue" : "issues"}.`, "");
    for (const { candidate: c } of report.findings) lines.push(`- ${capitalize(c.severity)}: ${c.title} (\`${c.file}:${c.newLine}\`)`);
  }
  for (const limitation of report.limitations) lines.push("", `Scope limited: ${limitation}.`);
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
    findings: report.findings.map((f) => ({ findingId: f.findingId, fingerprint: f.fingerprint, status: f.status, ...f.candidate })),
    discarded: report.dispositions.filter((d) => d.state === "DISCARDED"),
    published: false,
  };
}
