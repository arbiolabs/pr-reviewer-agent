import { expect, it } from "vitest";
import { type Commit, type Finding, type Label, ratio, score, scoreTracking, type Step, type TrackedFinding, type TrackedLabel } from "../bench/score.ts";

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

const auth: TrackedLabel = { ...label, id: "auth" };
const tracked = (findingId: string, file: string, newLine: number): TrackedFinding => ({ ...at(file, newLine), findingId });
const commit = (expect: Commit["expect"], labels: TrackedLabel[] = []): Commit => ({ dir: "commit2", delete: [], expect, labels });

it("scores a re-posted finding that stays PUBLISHED: identity kept, one duplicate comment after the first review, the open expectation unmet", () => {
  const steps: Step[] = [
    { findings: [tracked("f1", "src/a.ts", 11)], statuses: { f1: "PUBLISHED" }, comments: [{ path: "src/a.ts", line: 11 }, { path: "src/a.ts", line: 12 }] },
    { findings: [tracked("f1", "src/a.ts", 11)], statuses: { f1: "PUBLISHED" }, comments: [{ path: "src/a.ts", line: 11 }] },
  ];
  expect(scoreTracking([auth], [commit([{ label: "auth", status: "open" }])], steps)).toEqual({
    outcomes: [{ step: 1, label: "auth", expected: "open", findingId: "f1", status: "PUBLISHED", ok: false, idKept: true }],
    falseResolution: { n: 0, of: 1 },
    identityKept: { n: 1, of: 1 },
    duplicateComments: { n: 1, of: 3 },
    resolutionRecall: { n: 0, of: 0 },
    newFound: { n: 0, of: 0 },
  });
});

it("follows a moved label, and counts a false resolution and a lost identity", () => {
  const steps: Step[] = [
    { findings: [tracked("f1", "src/a.ts", 11)], statuses: { f1: "PUBLISHED" }, comments: [] },
    { findings: [tracked("f2", "src/b.ts", 31)], statuses: { f1: "RESOLVED", f2: "DETECTED" }, comments: [{ path: "src/b.ts", line: 31 }] },
  ];
  expect(scoreTracking([auth], [commit([{ label: "auth", status: "open", file: "src/b.ts", line_range: [30, 32] }])], steps)).toEqual({
    outcomes: [{ step: 1, label: "auth", expected: "open", findingId: "f1", status: "RESOLVED", ok: false, idKept: false }],
    falseResolution: { n: 1, of: 1 },
    identityKept: { n: 0, of: 1 },
    duplicateComments: { n: 0, of: 1 },
    resolutionRecall: { n: 0, of: 0 },
    newFound: { n: 0, of: 0 },
  });
});

it("needs a new finding ID for a label a later commit adds, and leaves a label the first review missed out of every rate", () => {
  const missed: TrackedLabel = { ...label, id: "missed", file: "src/c.ts" };
  const again: TrackedLabel = { ...label, id: "again" };
  const other: TrackedLabel = { ...label, id: "other", file: "src/d.ts" };
  const steps: Step[] = [
    { findings: [tracked("f1", "src/a.ts", 11)], statuses: { f1: "PUBLISHED" }, comments: [{ path: "src/a.ts", line: 11 }] },
    { findings: [], statuses: { f1: "RESOLVED" }, comments: [] },
    {
      findings: [tracked("f1", "src/a.ts", 11), tracked("f3", "src/d.ts", 10)],
      statuses: { f1: "RESOLVED", f3: "DETECTED" },
      comments: [{ path: "src/a.ts", line: 11 }, { path: "src/d.ts", line: 10 }, { path: "src/e.ts", line: 1 }],
    },
  ];
  const commits = [commit([{ label: "auth", status: "resolved" }, { label: "missed", status: "not_resolved" }]), commit([], [again, other])];
  expect(scoreTracking([auth, missed], commits, steps)).toEqual({
    outcomes: [
      { step: 1, label: "auth", expected: "resolved", findingId: "f1", status: "RESOLVED", ok: true, idKept: undefined },
      { step: 1, label: "missed", expected: "not_resolved", findingId: undefined, status: undefined, ok: false, idKept: undefined },
    ],
    falseResolution: { n: 0, of: 0 },
    identityKept: { n: 0, of: 0 },
    duplicateComments: { n: 0, of: 4 },
    resolutionRecall: { n: 1, of: 1 },
    newFound: { n: 1, of: 2 },
  });
});
