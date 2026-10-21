import { expect, it } from "vitest";
import { type Finding, type Label, ratio, score } from "../bench/score.ts";

const label: Label = { file: "src/a.ts", line_range: [10, 12], severity_at_least: "high", category: "security", note: "missing guard" };
const at = (file: string, newLine: number, severity: Finding["severity"] = "high"): Finding => ({ file, newLine, severity, category: "security", title: `${file}:${newLine}` });

it("matches a finding inside the range and reports severity and category without requiring them", () => {
  const finding = at("src/a.ts", 11, "low");
  expect(score([label], [finding])).toEqual({ matched: [{ label, finding, severityOk: false, categoryOk: true }], missed: [], extra: [] });
});

it("widens the range by three lines and no further, and needs the same file", () => {
  const near = [at("src/a.ts", 7), at("src/a.ts", 15)];
  expect(score([label], [near[0]!]).matched.map((m) => m.finding.newLine)).toEqual([7]);
  expect(score([label], [near[1]!]).matched.map((m) => m.finding.newLine)).toEqual([15]);
  const far = [at("src/a.ts", 6), at("src/a.ts", 16), at("src/b.ts", 11)];
  expect(score([label], far)).toEqual({ matched: [], missed: [label], extra: far });
});

it("matches a label at most once, to the nearest finding, and counts the rest as false positives", () => {
  const result = score([label], [at("src/a.ts", 14), at("src/a.ts", 12)]);
  expect(result.matched.map((m) => m.finding.newLine)).toEqual([12]);
  expect(result.extra).toEqual([at("src/a.ts", 14)]);
});

it("counts every finding on a clean case as a false positive, and leaves precision and recall undefined at 0/0", () => {
  expect(score([], [at("src/a.ts", 3)])).toEqual({ matched: [], missed: [], extra: [at("src/a.ts", 3)] });
  expect([ratio(0, 0), ratio(1, 3), ratio(0, 2)]).toEqual([undefined, 0.25, 0]);
});
