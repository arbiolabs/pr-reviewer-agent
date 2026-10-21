import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { main } from "../src/cli/main.ts";
import type { Disposition } from "../src/domain/types.ts";
import { FakePullSource } from "../tests/fixture.ts";
import { type Finding, type Label, LINE_TOLERANCE, type Match, ratio, score } from "./score.ts";

const CASES_DIR = new URL("./cases/", import.meta.url).pathname;

type CaseSpec = { title: string; body: string; labels: Label[] };
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
type RunResult = { cases: CaseResult[]; tp: number; fn: number; fp: number; precision: number | undefined; recall: number | undefined };

/**
 * A local repository whose base commit is `base/` and whose head commit is `head/` copied over it (so `head/` holds only
 * added and changed files), served by the tests' fake pull source.
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
  git("checkout", "-q", "-b", "pr");
  cpSync(join(dir, "head"), repoDir, { recursive: true });
  git("add", "-A");
  git("commit", "-q", "-m", spec.title);
  const headSha = git("rev-parse", "HEAD");
  git("update-ref", "refs/pull/1/head", headSha);
  git("checkout", "-q", "main");
  const pull = { installationId: 0, repositoryId: 1, fullName: `bench/${id}`, number: 1, title: spec.title, body: spec.body, baseSha, headSha, state: "open" as const, merged: false, private: false, cloneUrl: repoDir };
  return { spec, pulls: new FakePullSource(pull) };
}

async function runCase(id: string, model: string): Promise<CaseResult> {
  const { spec, pulls } = loadCase(id);
  process.env.PR_REVIEW_DATA_DIR = mkdtempSync(join(tmpdir(), "bench-data-"));
  let stdout = "";
  let stderr = "";
  const started = Date.now();
  const code = await main(["review", `https://github.com/bench/${id}/pull/1`, "--dry-run", "--json", "--no-jev", "--model", model], {
    pulls,
    isTTY: false,
    out: (t) => (stdout += t),
    err: (t) => (stderr += t),
  });
  const seconds = Math.round((Date.now() - started) / 1000);
  const report = stdout ? (JSON.parse(stdout) as { state: string; findings: Finding[]; discarded: Disposition[] }) : undefined;
  const findings = report?.findings ?? [];
  const state = report?.state ?? "ERROR";
  const error = state === "COMPLETED" ? null : (stderr.trim().split("\n").at(-1) ?? `exit ${code}`);
  return { id, state, error, seconds, labels: spec.labels.length, findings, ...score(spec.labels, findings), discarded: report?.discarded ?? [] };
}

function totals(cases: CaseResult[]): RunResult {
  const sum = (pick: (c: CaseResult) => unknown[]) => cases.reduce((n, c) => n + pick(c).length, 0);
  const tp = sum((c) => c.matched);
  const fn = sum((c) => c.missed);
  const fp = sum((c) => c.extra);
  return { cases, tp, fn, fp, precision: ratio(tp, fp), recall: ratio(tp, fn) };
}

const pct = (x: number | undefined) => (x === undefined ? "n/a" : `${(x * 100).toFixed(0)}%`);

function spread(values: (number | undefined)[]) {
  const xs = values.filter((x): x is number => x !== undefined);
  if (xs.length === 0) return undefined;
  return { mean: xs.reduce((a, b) => a + b, 0) / xs.length, min: Math.min(...xs), max: Math.max(...xs) };
}

function print(runs: RunResult[], model: string) {
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
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)));
  for (const r of rows) console.log(r.map((cell, i) => cell.padEnd(widths[i]!)).join("  "));
  console.log("");
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
const runs: RunResult[] = [];
for (let i = 0; i < Number(values.runs); i++) {
  const cases: CaseResult[] = [];
  for (const id of ids) {
    console.error(`run ${i + 1} ${id}...`);
    cases.push(await runCase(id, values.model));
  }
  runs.push(totals(cases));
}
print(runs, values.model);
if (values.json) {
  const commit = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const summary = { precision: spread(runs.map((r) => r.precision)), recall: spread(runs.map((r) => r.recall)) };
  writeFileSync(values.json, `${JSON.stringify({ model: values.model, commit, date: new Date().toISOString(), lineTolerance: LINE_TOLERANCE, summary, runs }, null, 2)}\n`);
}
