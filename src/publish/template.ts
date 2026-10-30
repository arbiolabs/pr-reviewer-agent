/**
 * The review summary's template: `{{name}}` placeholders and nothing else. One table names every variable; rendering,
 * validation and the settings answer all read it.
 */
export const TEMPLATE_VARIABLES = [
  { name: "repo", description: "The repository, owner/name.", sample: "acme/shop" },
  { name: "pr_number", description: "The pull request's number.", sample: "1291" },
  { name: "pr_title", description: "The pull request's title.", sample: "Capture payment after the order is confirmed" },
  { name: "pr_author", description: "The pull request's author; empty when GitHub did not say.", sample: "octocat" },
  { name: "head_sha", description: "The reviewed commit, in full.", sample: "f86859b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7" },
  { name: "short_sha", description: "The reviewed commit, first 7 characters.", sample: "f86859b" },
  { name: "model", description: "The model that reviewed it, provider/id.", sample: "anthropic/claude-opus-5-5" },
  { name: "summary_line", description: "One sentence: how many new issues, none, or that the run did not complete.", sample: "Found 2 actionable issues." },
  {
    name: "findings_list",
    description: "One line per new finding: severity, title and file:line; empty when there are none.",
    sample: "- High: Payment captured before the order is confirmed (`src/checkout/capture.ts:42`)\n- Medium (unconfirmed): Retry loop has no upper bound (`src/checkout/retry.ts:17`)",
  },
  { name: "findings_count", description: "How many new findings this review lists.", sample: "2" },
  { name: "critical_count", description: "New critical findings.", sample: "0" },
  { name: "high_count", description: "New high findings.", sample: "1" },
  { name: "medium_count", description: "New medium findings.", sample: "1" },
  { name: "low_count", description: "New low findings.", sample: "0" },
  { name: "unconfirmed_count", description: "New findings the reviewer suspects but could not show from the code.", sample: "1" },
  {
    name: "carried",
    description: "Findings from earlier commits, under a heading per state; empty when there are none.",
    sample: "Still present from an earlier commit:\n\n- Low: Log line prints the card's last four digits (`src/checkout/log.ts:9`, first reported on `b131fd0`)",
  },
  {
    name: "focus_files",
    description: "Up to three changed files the reviewer says to read first, each with why; empty when it named none.",
    sample: "Where to start reading:\n\n- `src/checkout/capture.ts`: the capture now runs before the order's state is saved",
  },
  { name: "limitations", description: "One “Scope limited: …” line per limit on what was reviewed; empty when none.", sample: "" },
] as const;

export type TemplateVariable = (typeof TEMPLATE_VARIABLES)[number]["name"];
export type TemplateValues = Record<TemplateVariable, string>;

/** Today's body, line for line: an empty list, carried block or limitation leaves no gap (see `fillTemplate`). */
export const DEFAULT_SUMMARY_TEMPLATE = `## Automated PR review

Reviewed commit: \`{{head_sha}}\`

{{summary_line}}

{{findings_list}}

{{carried}}

{{focus_files}}

{{limitations}}

Review performed with isolated read-only tools. Human review is still required.
`;

export const MAX_TEMPLATE_CHARS = 4000;

const PLACEHOLDER = /\{\{([^{}]*)\}\}/g;
const KNOWN = new Set<string>(TEMPLATE_VARIABLES.map((v) => v.name));

/** Why `template` cannot be saved, or undefined when it can. The empty string is valid: it means the default. */
export function templateProblem(template: string): string | undefined {
  if (template.length > MAX_TEMPLATE_CHARS) return `the template is ${template.length} characters; the limit is ${MAX_TEMPLATE_CHARS}`;
  const unknown = [...new Set([...template.matchAll(PLACEHOLDER)].map((m) => m[1]!).filter((n) => !KNOWN.has(n)))];
  if (unknown.length > 0) return `unknown ${unknown.length === 1 ? "variable" : "variables"} ${unknown.map((n) => `{{${n}}}`).join(", ")}; known: ${[...KNOWN].join(", ")}`;
  return undefined;
}

/** The template in force: a saved one, or the default when none is saved (the empty string). */
export const effectiveTemplate = (saved: string) => saved || DEFAULT_SUMMARY_TEMPLATE;

/**
 * Empty variables go first and blank-line runs collapse to one blank line, then the rest are filled in. So an empty
 * variable alone on its line leaves no gap, and a value's own text is never touched. A placeholder inside a value is
 * not expanded again.
 */
export function fillTemplate(template: string, values: TemplateValues): string {
  const value = (name: string) => (KNOWN.has(name) ? values[name as TemplateVariable] : undefined);
  const shaped = template.replace(PLACEHOLDER, (whole, name: string) => (value(name) === "" ? "" : whole)).replace(/\n{3,}/g, "\n\n");
  return shaped.replace(PLACEHOLDER, (whole, name: string) => value(name) ?? whole);
}

export const templateSamples = (): TemplateValues => Object.fromEntries(TEMPLATE_VARIABLES.map((v) => [v.name, v.sample])) as TemplateValues;
