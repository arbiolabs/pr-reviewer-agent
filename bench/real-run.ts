import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { main } from "../src/cli/main.ts";
import { loadConfig } from "../src/config/config.ts";
import type { Disposition, PullInfo } from "../src/domain/types.ts";
import { GitHubClient } from "../src/github/client.ts";
import type { JevJudgment } from "../src/jev/decision.ts";
import { AppStore } from "../src/persistence/store.ts";
import { FakePullSource } from "../tests/fixture.ts";
import { assertDryRun, type EvalFinding, type Manifest, type Results, type RunRecord, routing, type StoredPull } from "./real-score.ts";

const argv = process.argv.slice(2);
const args = argv[0] === "--" ? argv.slice(1) : argv;
const { values } = parseArgs({
  args,
  options: {
    dataset: { type: "string" },
    model: { type: "string" },
    runs: { type: "string", default: "1" },
    pr: { type: "string", multiple: true },
    jev: { type: "boolean", default: true },
    "no-jev": { type: "boolean" },
    out: { type: "string" },
    concurrency: { type: "string", default: "1" },
  },
});
if (!values.dataset || !values.model) {
  console.error("usage: pnpm eval:real -- --dataset DIR --model provider/id [--runs N] [--pr N]... [--jev | --no-jev] [--out results.json] [--concurrency N]");
  process.exit(2);
}
const dataset = resolve(values.dataset);
const jev = values.jev && !values["no-jev"];
// Every review reads this config, and the guard refuses to start unless it can only dry-run.
const configFile = join(dataset, "pr-review.config.json");
writeFileSync(configFile, `${JSON.stringify({ review: { publish_mode: "dry_run" } }, null, 2)}\n`);
process.env.PR_REVIEW_CONFIG = configFile;
assertDryRun(args, loadConfig());
if (jev && !process.env.TYPESAFE_API_KEY) throw new Error("--jev needs TYPESAFE_API_KEY: a private repository's diff never goes to the keyless tier. Pass --no-jev to route by path rules only.");

const manifest = JSON.parse(readFileSync(join(dataset, "manifest.json"), "utf8")) as Manifest;
const prs = values.pr ? manifest.prs.filter((p) => values.pr!.includes(String(p.number))) : manifest.prs;
const outPath = resolve(dataset, values.out ?? "results.json");
const results: Results = existsSync(outPath)
  ? (JSON.parse(readFileSync(outPath, "utf8")) as Results)
  : { dataset, model: values.model, commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(), jev, date: new Date().toISOString(), records: [] };
if (results.model !== values.model || results.jev !== jev) throw new Error(`${outPath} holds a ${results.model} run with jev ${results.jev}; pick another --out`);
const save = () => writeFileSync(outPath, `${JSON.stringify(results, null, 2)}\n`);

// No GitHub call can leave the process: reads come from the snapshot, and this client points at a closed local port.
const github = new GitHubClient({ auth: { kind: "token", token: undefined }, baseUrl: "http://127.0.0.1:9", maxRetries: 0, backoffMs: 1, concurrency: 1, timeoutMs: 1000 });

async function reviewOnce(n: number, run: number): Promise<RunRecord> {
  const stored = JSON.parse(readFileSync(join(dataset, "prs", String(n), "pull.json"), "utf8")) as StoredPull;
  const pull: PullInfo = { installationId: 0, repositoryId: stored.repositoryId, fullName: stored.fullName, number: n, title: stored.title, body: stored.body, baseSha: stored.baseSha, headSha: stored.headSha, state: "open", merged: false, private: stored.private, cloneUrl: join(dataset, "repo.git") };
  const dataDir = join(dataset, "work", `pr-${n}-run-${run}`);
  rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(dataDir, { recursive: true });
  // main reads PR_REVIEW_DATA_DIR before its first await, so reviews running side by side each keep their own.
  process.env.PR_REVIEW_DATA_DIR = dataDir;
  let stdout = "";
  let stderr = "";
  const started = Date.now();
  const reviewArgs = ["review", `https://github.com/${stored.fullName}/pull/${n}`, "--dry-run", "--json", ...(jev ? [] : ["--no-jev"]), "--model", values.model!];
  assertDryRun(reviewArgs, loadConfig());
  const code = await main(reviewArgs, {
    pulls: new FakePullSource(pull),
    github,
    isTTY: false,
    out: (t) => (stdout += t),
    err: (t) => (stderr += t),
  });
  const seconds = Math.round((Date.now() - started) / 1000);
  const report = stdout ? (JSON.parse(stdout) as { revisionId: string; state: string; reviewMode: string | null; reviewModeReasons: string[]; findings: (EvalFinding & { newLine: number | null })[]; discarded: Disposition[] }) : undefined;
  const state = report?.state ?? "ERROR";
  let decision: { normalized: unknown; error: string | null } | undefined;
  if (report) {
    const store = AppStore.open(join(dataDir, "app.sqlite"));
    try {
      decision = store.decision(report.revisionId);
    } finally {
      store.close();
    }
  }
  const j = decision?.normalized as JevJudgment | undefined;
  return {
    pr: n,
    run,
    state,
    error: state === "COMPLETED" ? null : (stderr.trim().split("\n").at(-1) ?? `exit ${code}`),
    seconds,
    mode: report?.reviewMode ?? null,
    modeReasons: report?.reviewModeReasons ?? [],
    routing: routing(report?.reviewModeReasons ?? []),
    jev: j ? { category: j.primaryCategory, risk: j.riskScore, needsDeep: j.needsDeepReviewProbability, confidence: j.routingConfidence } : null,
    jevError: decision?.error ?? null,
    findings: (report?.findings ?? []).map((f) => ({ file: f.file, line: f.newLine, severity: f.severity, category: f.category, title: f.title, explanation: f.explanation })),
    discarded: (report?.discarded ?? []).map(({ candidateId, reason }) => ({ candidateId, reason })),
  };
}

const done = new Set(results.records.filter((r) => r.state === "COMPLETED").map((r) => `${r.pr}/${r.run}`));
const queue = Array.from({ length: Number(values.runs) }, (_, i) => prs.map((p) => ({ pr: p.number, run: i + 1 }))).flat().filter((j) => !done.has(`${j.pr}/${j.run}`));
console.error(`${queue.length} reviews to run, ${done.size} already in ${outPath}`);
async function worker() {
  for (let job = queue.shift(); job; job = queue.shift()) {
    console.error(`#${job.pr} run ${job.run}...`);
    const record = await reviewOnce(job.pr, job.run);
    results.records = [...results.records.filter((r) => !(r.pr === record.pr && r.run === record.run)), record];
    save();
    console.error(`#${job.pr} run ${job.run}: ${record.state} ${record.mode ?? "-"} ${record.findings.length} findings ${record.seconds}s${record.error ? ` (${record.error})` : ""}`);
  }
}
await Promise.all(Array.from({ length: Number(values.concurrency) }, worker));
save();
console.error(`results in ${outPath}`);
