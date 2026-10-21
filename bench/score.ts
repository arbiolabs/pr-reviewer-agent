import type { Category, Severity } from "../src/domain/types.ts";
import { SEVERITIES } from "../src/domain/types.ts";

/** How many lines outside a label's range a finding may sit and still match it. */
export const LINE_TOLERANCE = 3;

export type Label = { file: string; line_range: [number, number]; severity_at_least: Severity; category: Category; note: string };
export type Finding = { file: string; newLine: number; severity: Severity; category: Category; title: string };
export type Match = { label: Label; finding: Finding; severityOk: boolean; categoryOk: boolean };
export type Score = { matched: Match[]; missed: Label[]; extra: Finding[] };

const distance = (label: Label, line: number) => Math.max(0, label.line_range[0] - line, line - label.line_range[1]);

/**
 * TP = matched labels, FN = `missed`, FP = `extra`. A finding matches a label when the file is equal and its line is
 * within LINE_TOLERANCE of `line_range`. Each label takes the nearest unmatched finding, so each side matches at most once.
 * Severity and category are reported on the match, never required.
 */
export function score(labels: Label[], findings: Finding[]): Score {
  const extra = [...findings];
  const matched: Match[] = [];
  const missed: Label[] = [];
  for (const label of labels) {
    const candidates = extra.filter((f) => f.file === label.file && distance(label, f.newLine) <= LINE_TOLERANCE);
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
