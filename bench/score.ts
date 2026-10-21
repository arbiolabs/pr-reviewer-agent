import type { Category, Severity } from "../src/domain/types.ts";
import { SEVERITIES } from "../src/domain/types.ts";

/** How many lines outside a label's range a finding may sit and still match it. */
export const LINE_TOLERANCE = 3;

export type Label = { file: string; line_range: [number, number]; severity_at_least: Severity; category: Category; note: string };
export type Finding = { file: string; newLine: number; severity: Severity; category: Category; title: string };
export type Match<L extends Label = Label, F extends Finding = Finding> = { label: L; finding: F; severityOk: boolean; categoryOk: boolean };
export type Score<L extends Label = Label, F extends Finding = Finding> = { matched: Match<L, F>[]; missed: L[]; extra: F[] };

const distance = (label: Label, line: number) => Math.max(0, label.line_range[0] - line, line - label.line_range[1]);
const near = (label: Label, file: string, line: number) => file === label.file && distance(label, line) <= LINE_TOLERANCE;

/**
 * TP = matched labels, FN = `missed`, FP = `extra`. A finding matches a label when the file is equal and its line is
 * within LINE_TOLERANCE of `line_range`. Each label takes the nearest unmatched finding, so each side matches at most once.
 * Severity and category are reported on the match, never required.
 */
export function score<L extends Label, F extends Finding>(labels: L[], findings: F[]): Score<L, F> {
  const extra = [...findings];
  const matched: Match<L, F>[] = [];
  const missed: L[] = [];
  for (const label of labels) {
    const candidates = extra.filter((f) => near(label, f.file, f.newLine));
    const finding = candidates.sort((a, b) => distance(label, a.newLine) - distance(label, b.newLine))[0];
    if (!finding) {
      missed.push(label);
      continue;
    }
    extra.splice(extra.indexOf(finding), 1);
    matched.push({ label, finding, severityOk: SEVERITIES.indexOf(finding.severity) <= SEVERITIES.indexOf(label.severity_at_least), categoryOk: finding.category === label.category });
  }
  return { matched, missed, extra };
}

/** Undefined when the denominator is zero: no findings has no precision, no labels has no recall. */
export const ratio = (tp: number, other: number) => (tp + other === 0 ? undefined : tp / (tp + other));

/** A label of a multi-commit case. Later commits name it by `id`. */
export type TrackedLabel = Label & { id: string };
/** `resolved` needs RESOLVED, `open` needs STILL_PRESENT, and `not_resolved` takes any status but RESOLVED. */
export type ExpectedStatus = "resolved" | "open" | "not_resolved";
/** `file` and `line_range` say where the label sits after the commit, when the commit moved it. */
export type Expectation = { label: string; status: ExpectedStatus; file?: string; line_range?: [number, number] };
/** `labels` are bugs the commit adds. */
export type Commit = { dir: string; delete: string[]; expect: Expectation[]; labels: TrackedLabel[] };
export type TrackedFinding = Finding & { findingId: string };
export type InlineComment = { path: string; line: number };
/** One review of one head: its JSON findings, the stored status of every finding of the PR, and the inline comments it posted. */
export type Step = { findings: TrackedFinding[]; statuses: Record<string, string>; comments: InlineComment[] };

/** `findingId` is undefined when the first review missed the label. Such an outcome counts in no rate. */
export type Outcome = { step: number; label: string; expected: ExpectedStatus; findingId: string | undefined; status: string | undefined; ok: boolean; idKept: boolean | undefined };
export type Rate = { n: number; of: number };
export type TrackScore = {
  outcomes: Outcome[];
  falseResolution: Rate;
  identityKept: Rate;
  duplicateComments: Rate;
  resolutionRecall: Rate;
  /** Labels added by a later commit that a finding with a new ID matched. */
  newFound: Rate;
};

const meets = (expected: ExpectedStatus, status: string | undefined) =>
  expected === "resolved" ? status === "RESOLVED" : expected === "open" ? status === "STILL_PRESENT" : status !== undefined && status !== "RESOLVED";

/**
 * Scores a multi-commit case. `steps[0]` reviews the first head and `steps[i]` the head after `commits[i - 1]`. The
 * first step's line match gives each label a finding ID, and later steps read that ID's status. An inline comment after
 * the first step is a duplicate when its label already had one. The duplicate rate's denominator is every inline comment.
 */
export function scoreTracking(labels: TrackedLabel[], commits: Commit[], steps: Step[]): TrackScore {
  const ids = new Map(score(labels, steps[0]!.findings).matched.map((m) => [m.label.id, m.finding.findingId]));
  const byId = new Map(labels.map((l) => [l.id, l]));
  const commented = new Set<string>();
  const outcomes: Outcome[] = [];
  const rate = () => ({ n: 0, of: 0 });
  const rates = { falseResolution: rate(), identityKept: rate(), duplicateComments: rate(), resolutionRecall: rate(), newFound: rate() };
  const count = (r: Rate, hit: boolean) => {
    r.of++;
    if (hit) r.n++;
  };
  const tally = (step: number, active: TrackedLabel[], comments: InlineComment[]) => {
    for (const c of comments) {
      const label = active.find((l) => near(l, c.path, c.line));
      count(rates.duplicateComments, step > 0 && label !== undefined && commented.has(label.id));
      if (label) commented.add(label.id);
    }
  };
  tally(0, labels, steps[0]!.comments);
  for (const [i, commit] of commits.entries()) {
    const step = steps[i + 1]!;
    const known = new Set(ids.values());
    const fresh = score(commit.labels, step.findings.filter((f) => !known.has(f.findingId)));
    for (const m of fresh.matched) ids.set(m.label.id, m.finding.findingId);
    rates.newFound.n += fresh.matched.length;
    rates.newFound.of += commit.labels.length;
    for (const l of commit.labels) byId.set(l.id, l);
    const active: TrackedLabel[] = [...commit.labels];
    for (const e of commit.expect) {
      const label = { ...byId.get(e.label)!, ...(e.file ? { file: e.file } : {}), ...(e.line_range ? { line_range: e.line_range } : {}) };
      active.push(label);
      const findingId = ids.get(e.label);
      const status = findingId === undefined ? undefined : step.statuses[findingId];
      let idKept: boolean | undefined;
      if (findingId !== undefined) {
        count(e.status === "resolved" ? rates.resolutionRecall : rates.falseResolution, status === "RESOLVED");
        if (e.status === "open") {
          idKept = score([label], step.findings).matched[0]?.finding.findingId === findingId;
          count(rates.identityKept, idKept);
        }
      }
      outcomes.push({ step: i + 1, label: e.label, expected: e.status, findingId, status, ok: findingId !== undefined && meets(e.status, status), idKept });
    }
    tally(i + 1, active, step.comments);
  }
  return { outcomes, ...rates };
}
