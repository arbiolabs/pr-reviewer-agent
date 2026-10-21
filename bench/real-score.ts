import type { Config } from "../src/config/config.ts";
import { LINE_TOLERANCE } from "./score.ts";

/** The parts of a GitHub pull the snapshot filters on. */
export type PullMeta = { number: number; draft: boolean; title: string; author: string; authorType: string; changedFiles: number; additions: number; deletions: number };

export const MAX_CHANGED_LINES = 3000;
export const MAX_CHANGED_FILES = 60;

/** Why the snapshot leaves a pull out, or undefined to keep it. */
export function skipReason(p: PullMeta): string | undefined {
  if (p.draft) return "draft";
  if (p.authorType === "Bot" || /dependabot|\[bot\]$/i.test(p.author)) return "bot author";
  if (p.title.startsWith("Flush:")) return "Flush: title";
  if (p.changedFiles === 0) return "no changed files";
  if (p.changedFiles > MAX_CHANGED_FILES) return `over ${MAX_CHANGED_FILES} files`;
  if (p.additions + p.deletions > MAX_CHANGED_LINES) return `over ${MAX_CHANGED_LINES} changed lines`;
  return undefined;
}

/** Throws unless the eval's argv and loaded config can only produce a dry run. */
export function assertDryRun(argv: string[], config: Pick<Config, "review">): void {
  if (argv.some((a) => a === "--publish" || a.startsWith("--publish="))) throw new Error("refusing to start: --publish is never allowed in an eval");
  if (config.review.publish_mode !== "dry_run") throw new Error(`refusing to start: review.publish_mode is ${config.review.publish_mode}, the eval needs dry_run`);
}

/** A verified finding as the eval stores it. `line` is the head-side line, null for a file-level finding. */
export type EvalFinding = { file: string; line: number | null; severity: string; category: string; title: string; explanation: string };

/** A GitHub inline review comment, as the snapshot stores it. */
export type HumanComment = {
  id: number;
  path: string;
  line: number | null;
  side: string | null;
  original_line: number | null;
  commit_id: string;
  original_commit_id: string;
  in_reply_to_id: number | null;
  user: { login: string; type: string };
  body: string;
};

const near = (a: number | null, b: number | null, tol: number) => (a === null || b === null ? a === b : Math.abs(a - b) <= tol);

export type CommentSplit = { considered: HumanComment[]; excluded: { bot: number; reply: number; olderCommit: number; noHeadLine: number } };

/**
 * The human comments a finding can agree with: top-level, written by a user, positioned on the snapshot head, on a
 * head-side line. Every other comment is counted under the first rule it fails.
 */
export function splitComments(comments: HumanComment[], headSha: string): CommentSplit {
  const excluded = { bot: 0, reply: 0, olderCommit: 0, noHeadLine: 0 };
  const considered: HumanComment[] = [];
  for (const c of comments) {
    if (c.user.type !== "User") excluded.bot++;
    else if (c.in_reply_to_id !== null) excluded.reply++;
    else if (c.commit_id !== headSha) excluded.olderCommit++;
    else if (c.line === null || c.side !== "RIGHT") excluded.noHeadLine++;
    else considered.push(c);
  }
  return { considered, excluded };
}

/** Findings within `tol` lines of a considered comment on the same path, and comments with such a finding. Not one-to-one. */
export function humanOverlap(findings: EvalFinding[], comments: HumanComment[], tol = LINE_TOLERANCE): { agree: number; matched: number } {
  const hit = (f: EvalFinding, c: HumanComment) => f.file === c.path && f.line !== null && near(f.line, c.line, tol);
  return { agree: findings.filter((f) => comments.some((c) => hit(f, c))).length, matched: comments.filter((c) => findings.some((f) => hit(f, c))).length };
}

const same = (a: EvalFinding, b: EvalFinding, tol: number) => a.file === b.file && a.category === b.category && near(a.line, b.line, tol);

/**
 * Jaccard of two runs' findings, matched one-to-one by path, category and line within `tol`. Undefined when both runs
 * found nothing, so an empty pair never reads as perfect agreement.
 */
export function jaccard(a: EvalFinding[], b: EvalFinding[], tol = LINE_TOLERANCE): number | undefined {
  if (a.length === 0 && b.length === 0) return undefined;
  const left = [...b];
  let matched = 0;
  for (const f of a) {
    const i = left.findIndex((g) => same(f, g, tol));
    if (i === -1) continue;
    left.splice(i, 1);
    matched++;
  }
  return matched / (a.length + b.length - matched);
}

/** Mean Jaccard over every pair of runs; undefined when no pair has a finding. */
export function stability(runs: EvalFinding[][], tol = LINE_TOLERANCE): number | undefined {
  const scores: number[] = [];
  for (let i = 0; i < runs.length; i++) for (let j = i + 1; j < runs.length; j++) {
    const s = jaccard(runs[i]!, runs[j]!, tol);
    if (s !== undefined) scores.push(s);
  }
  return mean(scores);
}

export const mean = (xs: number[]) => (xs.length === 0 ? undefined : xs.reduce((a, b) => a + b, 0) / xs.length);

export type Verdict = "real" | "not_real" | "unsure";
/** `reported` reached the review; `discarded` is a candidate the verifier dropped, graded to see what the verifier costs. */
export type GradeKind = "reported" | "discarded";
export type DiscardedCandidate = EvalFinding & { discardReason: string | null };
/** One row of the grading sheet: a finding across runs, keyed so a regenerated sheet keeps its verdicts. */
export type GradeEntry = EvalFinding & { id: string; kind: GradeKind; discardReason?: string | null; pr: number; runs: number[]; reference: string; verdict: Verdict | null };

/** Groups one PR's findings across runs into unique findings: same path and category, line within `tol` of the first. */
export function uniqueFindings(pr: number, headSha: string, runs: (EvalFinding | DiscardedCandidate)[][], tol = LINE_TOLERANCE, kind: GradeKind = "reported"): GradeEntry[] {
  const entries: GradeEntry[] = [];
  for (const [run, findings] of runs.entries()) {
    for (const f of findings) {
      const entry = entries.find((e) => same(e, f, tol) && !e.runs.includes(run + 1));
      if (entry) entry.runs.push(run + 1);
      else {
        const id = `${kind === "discarded" ? "discarded:" : ""}${pr}:${f.file}:${f.line ?? "file"}:${f.category}`;
        entries.push({ id, kind, pr, ...f, runs: [run + 1], reference: `PR ${pr} at ${headSha.slice(0, 12)}, ${f.file}:${f.line ?? "file"}`, verdict: null });
      }
    }
  }
  return entries;
}

/** The fresh sheet, with each entry's verdict taken from `previous` when its id is there. */
export function mergeSheet(fresh: GradeEntry[], previous: GradeEntry[]): GradeEntry[] {
  const verdicts = new Map(previous.map((e) => [e.id, e.verdict]));
  return fresh.map((e) => ({ ...e, verdict: verdicts.get(e.id) ?? null }));
}

export type Graded = { graded: number; real: number; notReal: number; unsure: number; precision: number | undefined };

/** Precision over entries graded real or not_real; unsure entries are counted but left out of it. */
export function gradedPrecision(sheet: GradeEntry[]): Graded {
  const count = (v: Verdict) => sheet.filter((e) => e.verdict === v).length;
  const [real, notReal, unsure] = [count("real"), count("not_real"), count("unsure")];
  return { graded: real + notReal + unsure, real, notReal, unsure, precision: real + notReal === 0 ? undefined : real / (real + notReal) };
}

/** The verifier's discard reasons with paths, lines and numbers taken out, so they group into a histogram. */
const DISCARD_KINDS: [RegExp, string][] = [
  [/is not in the changed set/, "file not in the changed set"],
  [/^no line anchor/, "no line anchor"],
  [/outside every hunk/, "line outside every hunk"],
  [/^no code evidence/, "no code evidence"],
  [/cites a SHA outside/, "evidence cites another SHA"],
  [/does not exist$/, "evidence file missing"],
  [/is outside 1-/, "evidence lines out of range"],
  [/^evidence is (\w+)/, "evidence is $1"],
  [/^severity (\w+) is below/, "severity $1 below threshold"],
  [/^confidence .* is below/, "confidence below threshold"],
  [/^duplicate of candidate/, "duplicate candidate"],
];

export function discardKind(reason: string | null): string {
  if (reason === null) return "no reason";
  for (const [pattern, kind] of DISCARD_KINDS) {
    const m = pattern.exec(reason);
    if (m) return kind.replace("$1", m[1] ?? "");
  }
  return "other";
}

/** How the review mode was reached, read from the policy's own reasons in `selectReviewMode`. */
export type Routing = { jevSuggested: string | null; jevFailed: boolean; pathRule: boolean };

/** `prs/<N>/pull.json` in a dataset. */
export type StoredPull = PullMeta & { fullName: string; repositoryId: number; private: boolean; body: string; baseSha: string; headSha: string; baseRef: string; headRef: string };
export type Manifest = { repo: string; snapshotAt: string; prs: { number: number; baseSha: string; headSha: string; changedFiles: number; additions: number; deletions: number }[]; skipped: { number: number; reason: string }[] };
/** One review of one PR. `jev` is the judgment Jev returned, null when it was skipped or failed. */
export type RunRecord = {
  pr: number;
  run: number;
  state: string;
  error: string | null;
  seconds: number;
  mode: string | null;
  modeReasons: string[];
  routing: Routing;
  jev: { category: string; risk: number; needsDeep: number; confidence: number | null } | null;
  jevError: string | null;
  findings: EvalFinding[];
  discarded: { candidateId: string; reason: string | null }[];
};
export type Results = { dataset: string; model: string; commit: string; jev: boolean; date: string; records: RunRecord[] };

export function routing(reasons: string[]): Routing {
  const suggested = reasons.map((r) => /^Jev needs-deep-review probability [\d.]+ suggests (\w+)/.exec(r)?.[1]).find((s) => s !== undefined);
  return { jevSuggested: suggested ?? null, jevFailed: suggested === undefined, pathRule: reasons.some((r) => r.startsWith("mandatory deep path rule matched")) };
}
