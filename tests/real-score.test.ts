import { expect, it } from "vitest";
import {
  assertDryRun,
  discardKind,
  type EvalFinding,
  type GradeEntry,
  gradedPrecision,
  type HumanComment,
  humanOverlap,
  jaccard,
  mergeSheet,
  type PullMeta,
  routing,
  skipReason,
  splitComments,
  stability,
  uniqueFindings,
} from "../bench/real-score.ts";

const HEAD = "a".repeat(40);
const f = (file: string, line: number | null, category = "correctness"): EvalFinding => ({ file, line, severity: "high", category, title: `${file}:${line}`, explanation: "x" });
const comment = (id: number, path: string, line: number | null, over: Partial<HumanComment> = {}): HumanComment => ({
  id,
  path,
  line,
  side: "RIGHT",
  original_line: line,
  commit_id: HEAD,
  original_commit_id: HEAD,
  in_reply_to_id: null,
  user: { login: "dev", type: "User" },
  body: "why?",
  ...over,
});

it("skips drafts, bots, Flush: titles, empty and oversized pulls, and keeps the rest", () => {
  const base: PullMeta = { number: 1, draft: false, title: "Add export", author: "dev", authorType: "User", changedFiles: 3, additions: 100, deletions: 20 };
  const cases: Partial<PullMeta>[] = [{}, { draft: true }, { author: "dependabot[bot]", authorType: "Bot" }, { author: "renovate[bot]" }, { title: "Flush: staging" }, { changedFiles: 0 }, { changedFiles: 61 }, { additions: 2990, deletions: 11 }, { changedFiles: 60, additions: 2999, deletions: 1 }];
  expect(cases.map((c) => skipReason({ ...base, ...c }))).toEqual([undefined, "draft", "bot author", "bot author", "Flush: title", "no changed files", "over 60 files", "over 3000 changed lines", undefined]);
});

it("refuses --publish and any publish mode but dry_run, and lets a dry run through", () => {
  const dry = { review: { publish_mode: "dry_run" as const } } as Parameters<typeof assertDryRun>[1];
  expect(() => assertDryRun(["review", "u", "--publish"], dry)).toThrow("refusing to start: --publish is never allowed in an eval");
  expect(() => assertDryRun(["review", "u", "--publish=true"], dry)).toThrow("--publish is never allowed");
  expect(() => assertDryRun(["review", "u", "--dry-run"], { review: { ...dry.review, publish_mode: "comment" } })).toThrow("review.publish_mode is comment, the eval needs dry_run");
  expect(assertDryRun(["review", "u", "--dry-run"], dry)).toBe(undefined);
});

it("uses only top-level user comments on the head's right side and counts the rest by reason", () => {
  const comments = [
    comment(1, "a.ts", 10),
    comment(2, "a.ts", 12, { user: { login: "bot", type: "Bot" } }),
    comment(3, "a.ts", 10, { in_reply_to_id: 1 }),
    comment(4, "a.ts", 20, { commit_id: "b".repeat(40) }),
    comment(5, "a.ts", null),
    comment(6, "a.ts", 30, { side: "LEFT" }),
  ];
  const { considered, excluded } = splitComments(comments, HEAD);
  expect(considered.map((c) => c.id)).toEqual([1]);
  expect(excluded).toEqual({ bot: 1, reply: 1, olderCommit: 1, noHeadLine: 2 });
});

it("counts findings near a human comment and comments near a finding, on the same path within three lines", () => {
  const comments = [comment(1, "a.ts", 10), comment(2, "b.ts", 50)];
  const findings = [f("a.ts", 13), f("a.ts", 7), f("a.ts", 14), f("b.ts", 10), f("a.ts", null)];
  expect(humanOverlap(findings, comments)).toEqual({ agree: 2, matched: 1 });
});

it("scores run agreement as a one-to-one Jaccard and leaves two empty runs unscored", () => {
  const a = [f("a.ts", 10), f("a.ts", 40), f("b.ts", null)];
  const b = [f("a.ts", 12), f("a.ts", 40, "security"), f("b.ts", null), f("c.ts", 1)];
  expect(jaccard(a, b)).toBe(2 / 5);
  expect(jaccard([f("a.ts", 10)], [f("a.ts", 10), f("a.ts", 11)])).toBe(1 / 2);
  expect(jaccard([], [])).toBe(undefined);
  expect(jaccard([f("a.ts", 1)], [])).toBe(0);
  expect(stability([[f("a.ts", 1)], [f("a.ts", 1)], []])).toBe((1 + 0 + 0) / 3);
  expect(stability([[], []])).toBe(undefined);
});

it("groups findings across runs into a grading sheet that keeps verdicts when regenerated, and scores precision", () => {
  const sheet = uniqueFindings(7, HEAD, [[f("a.ts", 10), f("a.ts", 11)], [f("a.ts", 12), f("b.ts", 3)]]);
  expect(sheet.map((e) => [e.id, e.runs, e.reference, e.verdict])).toEqual([
    ["7:a.ts:10:correctness", [1, 2], `PR 7 at ${HEAD.slice(0, 12)}, a.ts:10`, null],
    ["7:a.ts:11:correctness", [1], `PR 7 at ${HEAD.slice(0, 12)}, a.ts:11`, null],
    ["7:b.ts:3:correctness", [2], `PR 7 at ${HEAD.slice(0, 12)}, b.ts:3`, null],
  ]);
  const graded: GradeEntry[] = [{ ...sheet[0]!, verdict: "real" }, { ...sheet[2]!, verdict: "not_real" }, { ...sheet[1]!, id: "gone", verdict: "real" }];
  const merged = mergeSheet(sheet, graded);
  expect(merged.map((e) => e.verdict)).toEqual(["real", null, "not_real"]);
  expect(gradedPrecision([...merged, { ...sheet[1]!, id: "u", verdict: "unsure" }])).toEqual({ graded: 3, real: 1, notReal: 1, unsure: 1, precision: 0.5 });
  expect(gradedPrecision(sheet).precision).toBe(undefined);
});

it("groups the verifier's discard reasons without their paths and numbers", () => {
  const reasons = ["src/x.ts is not in the changed set", "src/x.ts:9 is outside every hunk of the head-side diff", "evidence is suspected", "severity low is below medium", "confidence 0.3 is below 0.5", "duplicate of candidate c1", "something new", null];
  expect(reasons.map(discardKind)).toEqual(["file not in the changed set", "line outside every hunk", "evidence is suspected", "severity low below threshold", "confidence below threshold", "duplicate candidate", "other", "no reason"]);
});

it("reads who raised the review depth from the routing reasons", () => {
  expect(routing(["Jev needs-deep-review probability 0.85 suggests deep", "mandatory deep path rule matched: db/migrations/1.sql"])).toEqual({ jevSuggested: "deep", jevFailed: false, pathRule: true });
  expect(routing(["Jev disabled by --no-jev; using standard"])).toEqual({ jevSuggested: null, jevFailed: true, pathRule: false });
});

it("keeps discarded candidates apart from reported findings on the sheet, with the verifier's reason", () => {
  const dropped = { ...f("a.ts", 10), discardReason: "evidence is suspected" };
  const sheet = uniqueFindings(7, HEAD, [[dropped], [dropped]], undefined, "discarded");
  expect(sheet.map((e) => [e.id, e.kind, e.discardReason, e.runs])).toEqual([["discarded:7:a.ts:10:correctness", "discarded", "evidence is suspected", [1, 2]]]);
  expect(uniqueFindings(7, HEAD, [[f("a.ts", 10)]])[0]!.kind).toBe("reported");
});
