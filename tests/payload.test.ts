import { expect, it } from "vitest";
import { renderSummary } from "../src/cli/report.ts";
import type { FindingView } from "../src/persistence/store.ts";
import { buildPayload, hasNews, MAX_BODY_CHARS, marker } from "../src/publish/publisher.ts";
import { parseDiff } from "../src/source/diff.ts";

const view = (i: number, over: Partial<FindingView> = {}): FindingView => ({
  findingId: `f-${i}`,
  fingerprint: `fp-${i}`,
  status: "STILL_PRESENT",
  severity: "high",
  candidate: {
    id: `c-${i}`,
    category: "security",
    severity: "high",
    title: `Finding ${i} ${"x".repeat(200)}`,
    explanation: "e",
    file: "src/x.ts",
    newLine: i + 1,
    codeEvidence: [],
    reasoningSummary: "r",
    confidence: 0.9,
    evidenceKind: "confirmed_by_static_trace",
  },
  carried: true,
  firstHeadSha: "a".repeat(40),
  posted: true,
  listed: true,
  anchored: true,
  reopens: null,
  ...over,
});
const many = (n: number, over: Partial<FindingView> = {}) => Array.from({ length: n }, (_, i) => view(i, over));

it("lists a reopened finding as new, with the commit it looked resolved at, until a review that carries it is on GitHub, and counts it as news even off the diff", () => {
  const reopens = { findingId: "f-old", resolvedAt: "b".repeat(40) };
  const back = view(1, { carried: true, posted: false, listed: false, reopens });
  const unseen = renderSummary("h".repeat(40), [back], []);
  expect(unseen).toContain(`Found 1 actionable issue.\n\n- High: Finding 1 ${"x".repeat(200)} (\`src/x.ts:2\`; looked resolved at \`${"b".repeat(7)}\`, but its code is still here)`);
  expect(unseen).not.toContain("Still present");
  expect(hasNews([back], [])).toBe(true);
  // The review that reopened it posted: from then on it is carried like any other finding.
  const seen = { ...back, listed: true };
  expect(renderSummary("h".repeat(40), [seen], [])).toContain(`No new actionable issues found in the reviewed scope.\n\nStill present from an earlier commit:\n\n- High: Finding 1 ${"x".repeat(200)} (\`src/x.ts:2\`, first reported on \`${"a".repeat(7)}\`)`);
  expect(hasNews([seen], [])).toBe(false);
  expect(hasNews([view(2)], ["f-2"])).toBe(true);
  expect(hasNews([view(3, { status: "RESOLVED" })], [])).toBe(true);
});

it("lists at most 20 carried findings per section, then says how many more", () => {
  const body = renderSummary("h".repeat(40), many(25), []);
  expect(body.match(/^- High: Finding/gm)?.length).toBe(20);
  expect(body).toContain("- and 5 more");
  expect(renderSummary("h".repeat(40), many(20), [])).not.toContain("more");
});

it("cuts a body that would exceed GitHub's limit at a line end and keeps the marker whole and last", () => {
  const { payload, hash } = buildPayload({ revisionId: "rev-1", headSha: "h".repeat(40), findings: many(400, { carried: false, posted: false }), limitations: [], files: parseDiff("") });
  expect(payload.body.length).toBeLessThanOrEqual(MAX_BODY_CHARS);
  expect(payload.body.endsWith(`\n${marker("rev-1", hash)}\n`)).toBe(true);
  const notice = payload.body.indexOf("\n\n[The rest of this review body was cut");
  expect(notice).toBeGreaterThan(0);
  expect(payload.body.slice(0, notice).split("\n").at(-1)).toMatch(/^- High: Finding \d+ x+ \(`src\/x\.ts:\d+`\)$/);
});
