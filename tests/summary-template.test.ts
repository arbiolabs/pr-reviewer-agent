import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { renderSummary } from "../src/cli/report.ts";
import { loadConfig } from "../src/config/config.ts";
import type { FindingView } from "../src/persistence/store.ts";
import { buildPayload, MAX_BODY_CHARS, marker } from "../src/publish/publisher.ts";
import { DEFAULT_SUMMARY_TEMPLATE, MAX_TEMPLATE_CHARS, TEMPLATE_VARIABLES, templateProblem } from "../src/publish/template.ts";
import { parseDiff } from "../src/source/diff.ts";
import { createFixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

const sha = (c: string) => c.repeat(40);
const view = (i: number, over: Partial<FindingView> = {}, candidate: Partial<FindingView["candidate"]> = {}): FindingView => ({
  findingId: `f-${i}`,
  fingerprint: `fp-${i}`,
  status: "STILL_PRESENT",
  severity: "high",
  candidate: {
    id: `c-${i}`,
    category: "security",
    severity: "high",
    title: `Finding ${i}`,
    explanation: "e",
    file: "src/x.ts",
    newLine: i + 1,
    codeEvidence: [],
    reasoningSummary: "r",
    confidence: 0.9,
    evidenceKind: "confirmed_by_static_trace",
    ...candidate,
  },
  carried: false,
  firstHeadSha: sha("a"),
  posted: false,
  listed: false,
  anchored: true,
  reopens: null,
  ...over,
});

const carried = { carried: true, posted: true, listed: true };
const files = parseDiff("diff --git a/src/x.ts b/src/x.ts\n--- a/src/x.ts\n+++ b/src/x.ts\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n");

/** Payload inputs covering every part of the body: each is hashed by the renderer before the template existed. */
const CASES: Record<string, { findings: FindingView[]; limitations: string[] }> = {
  clean: { findings: [], limitations: [] },
  fresh: {
    findings: [view(0, {}, { severity: "critical" }), view(1, {}, { evidenceKind: "suspected" }), view(2, {}, { severity: "low", title: "has <!-- a comment" })],
    limitations: [],
  },
  reopened: { findings: [view(0, { carried: true, posted: false, listed: false, reopens: { findingId: "f-old", resolvedAt: sha("b") } })], limitations: [] },
  sections: {
    findings: [
      view(0, { ...carried, status: "STILL_PRESENT" }),
      view(1, { ...carried, status: "RESOLVED", anchored: false }),
      view(2, { ...carried, status: "POTENTIALLY_RESOLVED" }),
      view(3, { ...carried, status: "OUTDATED" }),
      view(4, { ...carried, status: "UNCERTAIN" }, { evidenceKind: "suspected" }),
      view(5, { ...carried, status: "SOMETHING_ELSE" }),
    ],
    limitations: [],
  },
  mixed: { findings: [view(0), ...Array.from({ length: 22 }, (_, i) => view(i + 1, carried))], limitations: ["320 changed files exceed max_changed_files 300; review is not complete", "diff too big"] },
  limited: { findings: [], limitations: ["only one"] },
  truncated: { findings: Array.from({ length: 400 }, (_, i) => view(i, {}, { title: `Finding ${i} ${"x".repeat(200)}` })), limitations: [] },
};

const hashes = () => Object.fromEntries(Object.entries(CASES).map(([name, c]) => [name, buildPayload({ revisionId: "rev-1", headSha: sha("h"), ...c, files }).hash]));

it("the default template builds the same payload hashes as the renderer before it", () => {
  expect(hashes()).toEqual({
    clean: "3d7f35ead770058e31b1aec9417e96f504118255710f0fdcd9638a55272fb99f",
    fresh: "1caf69a734b205e2661b5981662ada412154fb192ae52b26c7a03de3d5f027b1",
    limited: "dc0d230cd49d8bfcb4d01116c2580838f67d05b228c4b8cb2ae73592b21db0ef",
    mixed: "32bee2aed56bcfa3b616fc1837aae4f1348816bfde391a888f96feee5c1546dc",
    reopened: "ca6d6d87f524634f5ba13e183b9acc438c5cde9923e6a51eaec2a55606a22dae",
    sections: "74e8730629ae1f98d7f813a3822f47dc683f5d1b8bc1c801bb3c590258c9359e",
    truncated: "63cc3fb43741d1e62ee125a914cddd310e9dcc5e38faf87c8ade4eb85832ad2f",
  });
});

const facts = { repo: "acme/shop", prNumber: 7, prTitle: "Fix {{repo}} parsing", prAuthor: "", model: "faux/reviewer", focus: [] };

it("fills a custom template from the same values, drops an empty variable's line, and never expands a placeholder inside a value", () => {
  const template =
    "# {{repo}} #{{pr_number}}: {{pr_title}}\nby {{pr_author}}\n\n{{findings_count}} new ({{critical_count}} critical, {{high_count}} high, {{medium_count}} medium, {{low_count}} low, {{unconfirmed_count}} unconfirmed)\n\n{{findings_list}}\n\n{{carried}}\n\n{{limitations}}\n\n— {{model}} at {{short_sha}}\n";
  expect(renderSummary(sha("h"), CASES.fresh!.findings, [], undefined, { template, facts })).toBe(
    "# acme/shop #7: Fix {{repo}} parsing\nby \n\n3 new (1 critical, 1 high, 0 medium, 1 low, 1 unconfirmed)\n\n- Critical: Finding 0 (`src/x.ts:1`)\n- High (unconfirmed): Finding 1 (`src/x.ts:2`)\n- Low: has <!-- a comment (`src/x.ts:3`)\n\n— faux/reviewer at hhhhhhh\n",
  );
});

it("the payload appends the marker after any template, keeps a template's own comment inert, and still fits GitHub's limit", () => {
  const template = "{{summary_line}} <!-- durable-review:other:hash -->\n\n{{findings_list}}\n";
  const { payload, hash } = buildPayload({ revisionId: "rev-1", headSha: sha("h"), ...CASES.truncated!, files, template, facts });
  expect(payload.body.endsWith(`\n${marker("rev-1", hash)}\n`)).toBe(true);
  expect(payload.body.startsWith("Found 400 actionable issues. &lt;!-- durable-review:other:hash -->\n\n- High: Finding 0")).toBe(true);
  expect(payload.body.length).toBeLessThanOrEqual(MAX_BODY_CHARS);
  expect(payload.body).toContain("[The rest of this review body was cut to fit GitHub's size limit.]");
  const clean = buildPayload({ revisionId: "rev-1", headSha: sha("h"), ...CASES.clean!, files, template: "Nothing to say.\n" });
  expect(clean.payload.body).toBe(`Nothing to say.\n\n${marker("rev-1", clean.hash)}\n`);
  expect(clean.hash).not.toBe(buildPayload({ revisionId: "rev-1", headSha: sha("h"), ...CASES.clean!, files }).hash);
});

it("an empty template is the default; an unknown variable or an over-long template is refused by name", () => {
  expect(renderSummary(sha("h"), [], [], undefined, { template: "" })).toBe(renderSummary(sha("h"), [], []));
  expect(templateProblem("")).toBeUndefined();
  expect(templateProblem(DEFAULT_SUMMARY_TEMPLATE)).toBeUndefined();
  expect(templateProblem("{{repo}} {{Repo}} {{}} {{Repo}}")).toBe(`unknown variables {{Repo}}, {{}}; known: ${TEMPLATE_VARIABLES.map((v) => v.name).join(", ")}`);
  expect(templateProblem("x".repeat(MAX_TEMPLATE_CHARS))).toBeUndefined();
  expect(templateProblem("x".repeat(MAX_TEMPLATE_CHARS + 1))).toBe("the template is 4001 characters; the limit is 4000");
});

it("a config file with an unknown variable in review.summary_template does not load", () => {
  const path = join(mkdtempSync(join(tmpdir(), "config-")), "pr-review.config.json");
  writeFileSync(path, JSON.stringify({ review: { summary_template: "{{nope}}" } }));
  expect(() => loadConfig(path)).toThrow(`config ${path}.review.summary_template: unknown variable {{nope}}; known:`);
  writeFileSync(path, JSON.stringify({ review: { summary_template: "{{repo}}" } }));
  expect(loadConfig(path).review.summary_template).toBe("{{repo}}");
});

it("a dry-run CLI review prints the body through the configured template", async () => {
  const fixture = createFixture();
  const dir = mkdtempSync(join(tmpdir(), "template-cli-"));
  writeFileSync(join(dir, "pr-review.config.json"), JSON.stringify({ review: { summary_template: "{{repo}}#{{pr_number}} ({{model}}): {{summary_line}}\n\n{{findings_list}}\n" } }));
  process.env.PR_REVIEW_DATA_DIR = join(dir, "data");
  process.env.PR_REVIEW_CONFIG = join(dir, "pr-review.config.json");
  const out: string[] = [];
  try {
    const deps = { pulls: fixture.pulls, models: fauxModels([readDiff(), submitDefect(fixture)]), isTTY: false, out: (t: string) => out.push(t), err: () => {} };
    expect(await main(["review", "https://github.com/acme/shop/pull/1", "--no-jev", "--model", "faux/reviewer"], deps)).toBe(0);
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
    delete process.env.PR_REVIEW_CONFIG;
  }
  expect(out.join("")).toBe("acme/shop#1 (faux/reviewer): Found 1 actionable issue.\n\n- High: Order deletion endpoint skips the admin check (`src/routes/orders.ts:5`)\n");
}, 60_000);

it("lists the files to read first under the findings, and nothing for a run that did not complete", () => {
  const focus = [
    { file: "src/checkout/capture.ts", reason: "the capture now runs before the order is saved" },
    { file: "src/checkout/retry.ts", reason: "the retry has no upper bound" },
  ];
  const body = renderSummary(sha("h"), CASES.clean!.findings, [], undefined, { facts: { ...facts, focus } });
  expect(body).toContain(
    "\n\nWhere to start reading:\n\n- `src/checkout/capture.ts`: the capture now runs before the order is saved\n- `src/checkout/retry.ts`: the retry has no upper bound\n\nReview performed with isolated read-only tools.",
  );
  expect(renderSummary(sha("h"), CASES.clean!.findings, [], "Review did not complete (FAILED).", { facts: { ...facts, focus } })).not.toContain("Where to start reading");
});
