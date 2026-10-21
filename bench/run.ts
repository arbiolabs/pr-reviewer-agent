import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { main } from "../src/cli/main.ts";
import type { Disposition } from "../src/domain/types.ts";
import { GitHubClient } from "../src/github/client.ts";
import { FakeGitHub } from "../tests/fake-github.ts";
import { FakePullSource } from "../tests/fixture.ts";
import { type Commit, type Finding, type InlineComment, type Label, LINE_TOLERANCE, type Match, type Rate, ratio, score, scoreTracking, type Step, type TrackedFinding, type TrackedLabel, type TrackScore } from "./score.ts";

const CASES_DIR = new URL("./cases/", import.meta.url).pathname;

/** A case with `commits` is a multi-commit case, and its labels are `TrackedLabel`s. */
type CaseSpec = { title: string; body: string; labels: Label[]; commits?: Commit[] };
type Review = { state: string; error: string | null; seconds: number; findings: TrackedFinding[]; discarded: Disposition[]; outboxStatus: string | null };
type CaseResult = {
  id: string;
  state: string;
  error: string | null;
  seconds: number;
  labels: number;
  findings: Finding[];
  matched: Match[];
  missed: Label[];
  extra: Finding[];
  discarded: Disposition[];
};
type TrackedStep = Review & Step;
type TrackedResult = { id: string; steps: TrackedStep[] } & TrackScore;
type Rates = Pick<TrackScore, "falseResolution" | "identityKept" | "duplicateComments" | "resolutionRecall" | "newFound">;
type RunResult = { cases: CaseResult[]; tracked: TrackedResult[]; tracking: Rates; tp: number; fn: number; fp: number; precision: number | undefined; recall: number | undefined };

/**
 * A local repository whose base commit is `base/` and whose head commit is `head/` copied over it (so `head/` holds only
 * added and changed files), served by the tests' fake pull source. `commit` adds a later head the same way.
 */
function loadCase(id: string) {
  const dir = join(CASES_DIR, id);
  const spec = JSON.parse(readFileSync(join(dir, "case.json"), "utf8")) as CaseSpec;
  const repoDir = mkdtempSync(join(tmpdir(), `bench-${id}-`));
  const git = (...args: string[]) => execFileSync("git", ["-C", repoDir, "-c", "user.name=Bench", "-c", "user.email=bench@example.com", ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  cpSync(join(dir, "base"), repoDir, { recursive: true });
  git("add", "-A");
  git("commit", "-q", "-m", "base");
  const baseSha = git("rev-parse", "HEAD");
  git("branch", "pr");
  const commit = (overlay: string, remove: string[], message: string) => {
    git("checkout", "-q", "pr");
    if (remove.length > 0) git("rm", "-q", "--", ...remove);
    cpSync(join(dir, overlay), repoDir, { recursive: true });
    git("add", "-A");
    git("commit", "-q", "-m", message);
    const sha = git("rev-parse", "HEAD");
    git("update-ref", "refs/pull/1/head", sha);
    git("checkout", "-q", "main");
    return sha;
  };
  const headSha = commit("head", [], spec.title);
  const pull = { installationId: 0, repositoryId: 1, fullName: `bench/${id}`, number: 1, title: spec.title, body: spec.body, baseSha, headSha, state: "open" as const, merged: false, private: false, cloneUrl: repoDir };
  return { spec, pulls: new FakePullSource(pull), commit };
}

/** One `pr-review review` of the pull's current head, with `--publish` to `github` when given, else `--dry-run`. */
async function review(id: string, model: string, pulls: FakePullSource, github?: GitHubClient): Promise<Review> {
  let stdout = "";
  let stderr = "";
  const started = Date.now();
  const code = await main(["review", `https://github.com/bench/${id}/pull/1`, github ? "--publish" : "--dry-run", "--json", "--no-jev", "--model", model], {
    pulls,
    ...(github ? { github } : {}),
    isTTY: false,
    out: (t) => (stdout += t),
    err: (t) => (stderr += t),
  });
  const seconds = Math.round((Date.now() - started) / 1000);
  const report = stdout ? (JSON.parse(stdout) as { state: string; findings: TrackedFinding[]; discarded: Disposition[]; outboxStatus: string | null }) : undefined;
  const state = report?.state ?? "ERROR";
  const error = state === "COMPLETED" ? null : (stderr.trim().split("\n").at(-1) ?? `exit ${code}`);
  return { state, error, seconds, findings: report?.findings ?? [], discarded: report?.discarded ?? [], outboxStatus: report?.outboxStatus ?? null };
}

async function runCase(id: string, model: string): Promise<CaseResult> {
  const { spec, pulls } = loadCase(id);
  process.env.PR_REVIEW_DATA_DIR = mkdtempSync(join(tmpdir(), "bench-data-"));
  const { state, error, seconds, findings, discarded } = await review(id, model, pulls);
  return { id, state, error, seconds, labels: spec.labels.length, findings, ...score(spec.labels, findings), discarded };
}

/** The stored status of every finding of the PR, read through `pr-review inspect`, which covers findings this head did not report. */
async function statuses(id: string): Promise<Record<string, string>> {
  let stdout = "";
  await main(["inspect", "--pr", `bench/${id}#1`, "--json"], { out: (t) => (stdout += t) });
  const reports = JSON.parse(stdout) as { findings: { findingId: string; status: string }[] }[];
  return Object.fromEntries(reports.flatMap((r) => r.findings.map((f) => [f.findingId, f.status])));
}

/**
 * Reviews the first head, then each commit's head, on one data directory so the store carries findings between them.
 * Every review publishes to an in-process fake GitHub, so the inline comments counted are the ones the outbox really sent.
 */
async function runTracked(id: string, model: string): Promise<TrackedResult> {
  const { spec, pulls, commit } = loadCase(id);
  const commits = spec.commits!;
  process.env.PR_REVIEW_DATA_DIR = mkdtempSync(join(tmpdir(), "bench-data-"));
  const fake = await FakeGitHub.start();
  const github = new GitHubClient({ auth: { kind: "token", token: "bench" }, baseUrl: fake.url, maxRetries: 2, backoffMs: 1, concurrency: 2, timeoutMs: 15_000 });
  try {
    const steps: TrackedStep[] = [];
    for (let i = 0; i <= commits.length; i++) {
      const c = commits[i - 1];
      if (c) pulls.pull = { ...pulls.pull, headSha: commit(c.dir, c.delete, c.dir) };
      const posted = fake.reviews.length;
      const r = await review(id, model, pulls, github);
      const comments = fake.reviews.slice(posted).flatMap((rv) => (rv.comments as InlineComment[]).map(({ path, line }) => ({ path, line })));
      steps.push({ ...r, statuses: await statuses(id), comments });
    }
    return { id, steps, ...scoreTracking(spec.labels as TrackedLabel[], commits, steps) };
  } finally {
    await fake.close();
  }
}

const sumRates = (results: Rates[]): Rates => {
  const keys = ["falseResolution", "identityKept", "duplicateComments", "resolutionRecall", "newFound"] as const;
  return Object.fromEntries(keys.map((k) => [k, results.reduce((acc, r) => ({ n: acc.n + r[k].n, of: acc.of + r[k].of }), { n: 0, of: 0 })])) as Rates;
};

function totals(cases: CaseResult[], tracked: TrackedResult[]): RunResult {
  const sum = (pick: (c: CaseResult) => unknown[]) => cases.reduce((n, c) => n + pick(c).length, 0);
  const tp = sum((c) => c.matched);
  const fn = sum((c) => c.missed);
  const fp = sum((c) => c.extra);
  return { cases, tracked, tracking: sumRates(tracked), tp, fn, fp, precision: ratio(tp, fp), recall: ratio(tp, fn) };
}

const pct = (x: number | undefined) => (x === undefined ? "n/a" : `${(x * 100).toFixed(0)}%`);
const frac = (r: Rate) => `${r.n}/${r.of}${r.of > 0 ? ` (${pct(r.n / r.of)})` : ""}`;

function spread(values: (number | undefined)[]) {
  const xs = values.filter((x): x is number => x !== undefined);
  if (xs.length === 0) return undefined;
  return { mean: xs.reduce((a, b) => a + b, 0) / xs.length, min: Math.min(...xs), max: Math.max(...xs) };
}

function table(rows: string[][]) {
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)));
  for (const r of rows) console.log(r.map((cell, i) => cell.padEnd(widths[i]!)).join("  "));
  console.log("");
}

function print(runs: RunResult[], model: string) {
  if (runs[0]!.cases.length > 0) {
    const per = (id: string, f: (c: CaseResult) => string) => runs.map((r) => f(r.cases.find((c) => c.id === id)!)).join(",");
    const rows = [["case", "labels", "TP", "FN", "FP", "discarded", "state", "sec"]];
    for (const { id, labels } of runs[0]!.cases) {
      rows.push([
        id,
        String(labels),
        per(id, (c) => String(c.matched.length)),
        per(id, (c) => String(c.missed.length)),
        per(id, (c) => String(c.extra.length)),
        per(id, (c) => String(c.discarded.length)),
        per(id, (c) => c.state),
        per(id, (c) => String(c.seconds)),
      ]);
    }
    table(rows);
    for (const [i, r] of runs.entries()) console.log(`run ${i + 1}: TP ${r.tp}  FN ${r.fn}  FP ${r.fp}  precision ${pct(r.precision)}  recall ${pct(r.recall)}  errors ${r.cases.filter((c) => c.error).length}`);
    for (const [name, s] of [["precision", spread(runs.map((r) => r.precision))], ["recall", spread(runs.map((r) => r.recall))]] as const) {
      console.log(`${name}: mean ${pct(s?.mean)}  min ${pct(s?.min)}  max ${pct(s?.max)}  (${runs.length} runs, ${model})`);
    }
    console.log(`\nA finding matches a label on the same file within ${LINE_TOLERANCE} lines of its range. Severity and category are reported, not required.`);
    for (const [i, r] of runs.entries()) {
      for (const c of r.cases) {
        const at = `  run ${i + 1} ${c.id}:`;
        for (const { finding: f, label: l, severityOk, categoryOk } of c.matched) {
          if (!severityOk) console.log(`${at} TP with severity ${f.severity}, label wants at least ${l.severity_at_least}`);
          if (!categoryOk) console.log(`${at} TP with category ${f.category}, label says ${l.category}`);
        }
        for (const f of c.extra) console.log(`${at} FP ${f.severity} ${f.category} ${f.file}:${f.newLine} ${f.title}`);
        for (const d of c.discarded) console.log(`${at} verifier discarded ${d.candidateId}: ${d.reason}`);
        if (c.error) console.log(`${at} ERROR ${c.error}`);
      }
    }
  }
  if (runs[0]!.tracked.length === 0) return;
  console.log("");
  const rows = [["case", "run", "expected -> status (id kept)", "findings per step", "inline comments per step", "duplicates", "states"]];
  for (const [i, r] of runs.entries()) {
    for (const t of r.tracked) {
      const outcomes = t.outcomes.map((o) => `${o.label}@${o.step} ${o.expected}->${o.status ?? "unmatched"}${o.idKept === undefined ? "" : o.idKept ? " (kept)" : " (lost)"}`).join("; ");
      const states = [...new Set(t.steps.map((s) => `${s.state}/${s.outboxStatus ?? "-"}`))].join(",");
      rows.push([t.id, String(i + 1), outcomes, t.steps.map((s) => s.findings.length).join(","), t.steps.map((s) => s.comments.length).join(","), String(t.duplicateComments.n), states]);
    }
  }
  table(rows);
  for (const [i, r] of runs.entries()) {
    const t = r.tracking;
    console.log(`run ${i + 1}: false resolution ${frac(t.falseResolution)}  identity kept ${frac(t.identityKept)}  duplicate comments ${frac(t.duplicateComments)}  resolution recall ${frac(t.resolutionRecall)}  new labels found ${frac(t.newFound)}`);
  }
  for (const [i, r] of runs.entries()) {
    for (const t of r.tracked) {
      for (const [n, s] of t.steps.entries()) {
        const at = `  run ${i + 1} ${t.id} step ${n}:`;
        for (const f of s.findings) console.log(`${at} ${f.findingId.slice(0, 8)} ${s.statuses[f.findingId]} ${f.severity} ${f.file}:${f.newLine} ${f.title}`);
        if (s.error) console.log(`${at} ERROR ${s.error}`);
      }
    }
  }
}

const argv = process.argv.slice(2);
const { values } = parseArgs({
  args: argv[0] === "--" ? argv.slice(1) : argv,
  options: { model: { type: "string" }, case: { type: "string", multiple: true }, runs: { type: "string", default: "1" }, json: { type: "string" } },
});
if (!values.model) {
  console.error("usage: pnpm bench -- --model provider/id [--case id]... [--runs N] [--json out.json]");
  process.exit(2);
}
const ids = values.case ?? readdirSync(CASES_DIR).sort();
const multi = new Set(ids.filter((id) => (JSON.parse(readFileSync(join(CASES_DIR, id, "case.json"), "utf8")) as CaseSpec).commits));
const runs: RunResult[] = [];
for (let i = 0; i < Number(values.runs); i++) {
  const cases: CaseResult[] = [];
  const tracked: TrackedResult[] = [];
  for (const id of ids) {
    console.error(`run ${i + 1} ${id}...`);
    if (multi.has(id)) tracked.push(await runTracked(id, values.model));
    else cases.push(await runCase(id, values.model));
  }
  runs.push(totals(cases, tracked));
}
print(runs, values.model);
if (values.json) {
  const commit = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const summary = { precision: spread(runs.map((r) => r.precision)), recall: spread(runs.map((r) => r.recall)), tracking: sumRates(runs.map((r) => r.tracking)) };
  writeFileSync(values.json, `${JSON.stringify({ model: values.model, commit, date: new Date().toISOString(), lineTolerance: LINE_TOLERANCE, summary, runs }, null, 2)}\n`);
}
