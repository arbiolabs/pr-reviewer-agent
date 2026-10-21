import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import type { Disposition } from "../src/domain/types.ts";
import { AppStore } from "../src/persistence/store.ts";
import {
  AGENT_REVIEW_MARKER,
  agentFindings,
  crossMatch,
  type DiscardedCandidate,
  discardKind,
  type GradeEntry,
  gradedPrecision,
  type HumanComment,
  humanOverlap,
  type Manifest,
  mean,
  mergeSheet,
  type Results,
  type RunRecord,
  splitComments,
  stability,
  type StoredReview,
  uniqueFindings,
} from "./real-score.ts";
import { LINE_TOLERANCE } from "./score.ts";

const argv = process.argv.slice(2);
const { values } = parseArgs({ args: argv[0] === "--" ? argv.slice(1) : argv, options: { results: { type: "string" }, labels: { type: "string" } } });
if (!values.results) {
  console.error("usage: pnpm eval:report -- --results results.json [--labels labels.json]");
  process.exit(2);
}
const results = JSON.parse(readFileSync(resolve(values.results), "utf8")) as Results;
const dataset = results.dataset;
const manifest = JSON.parse(readFileSync(join(dataset, "manifest.json"), "utf8")) as Manifest;
const read = <T>(pr: number, file: string) => JSON.parse(readFileSync(join(dataset, "prs", String(pr), file), "utf8")) as T;

const pct = (x: number | undefined) => (x === undefined ? "n/a" : `${(x * 100).toFixed(0)}%`);
const fmt = (x: number | undefined, digits = 2) => (x === undefined ? "n/a" : x.toFixed(digits));
const frac = (n: number, of: number) => `${n}/${of}${of > 0 ? ` (${pct(n / of)})` : ""}`;
const median = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length === 0 ? undefined : s.length % 2 ? s[(s.length - 1) / 2]! : (s[s.length / 2 - 1]! + s[s.length / 2]!) / 2;
};
function table(rows: string[][]) {
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)));
  for (const r of rows) console.log(r.map((cell, i) => cell.padEnd(widths[i]!)).join("  ").trimEnd());
  console.log("");
}
const tally = (xs: string[]) => [...xs.reduce((m, x) => m.set(x, (m.get(x) ?? 0) + 1), new Map<string, number>())].sort((a, b) => b[1] - a[1]).map(([k, n]) => `${k} ${n}`).join(", ");

const runCount = Math.max(...results.records.map((r) => r.run));
const prs = manifest.prs.filter((p) => results.records.some((r) => r.pr === p.number));
const records = (pr: number) => results.records.filter((r) => r.pr === pr).sort((a, b) => a.run - b.run);
const completed = results.records.filter((r) => r.state === "COMPLETED");

console.log(`Real-PR eval: ${manifest.repo}, snapshot ${manifest.snapshotAt}, ${results.model}, Jev ${results.jev ? "on" : "off"}, pr-review ${results.commit.slice(0, 12)}`);
console.log(`${prs.length} PRs, ${results.records.length} reviews (${completed.length} completed), up to ${runCount} runs each. Line tolerance ${LINE_TOLERANCE}.\n`);

type PrRow = { pr: number; lines: number; comments: ReturnType<typeof splitComments>; runs: RunRecord[]; overlap: { agree: number; matched: number }[]; jaccard: number | undefined };
const rows: PrRow[] = prs.map((p) => {
  const comments = splitComments(read<HumanComment[]>(p.number, "comments.json"), p.headSha);
  const runs = records(p.number);
  const done = runs.filter((r) => r.state === "COMPLETED");
  return { pr: p.number, lines: p.additions + p.deletions, comments, runs, overlap: runs.map((r) => humanOverlap(r.findings, comments.considered)), jaccard: done.length >= 2 ? stability(done.map((r) => r.findings)) : undefined };
});

const per = (runs: RunRecord[], f: (r: RunRecord) => string) => runs.map(f).join(",");
const header = ["PR", "lines", "files", "state", "mode", "findings", "discarded", "human (used/excl)", "agree", "matched", "jaccard", "sec"];
table([
  header,
  ...rows.map(({ pr, lines, comments, runs, overlap, jaccard: j }) => {
    const excluded = Object.values(comments.excluded).reduce((a, b) => a + b, 0);
    return [
      `#${pr}`,
      String(lines),
      String(manifest.prs.find((p) => p.number === pr)!.changedFiles),
      per(runs, (r) => (r.state === "COMPLETED" ? "ok" : r.state)),
      per(runs, (r) => r.mode ?? "-"),
      per(runs, (r) => String(r.findings.length)),
      per(runs, (r) => String(r.discarded.length)),
      `${comments.considered.length}/${excluded}`,
      overlap.map((o) => String(o.agree)).join(","),
      overlap.map((o) => String(o.matched)).join(","),
      runs.length < 2 ? "-" : fmt(j),
      per(runs, (r) => String(r.seconds)),
    ];
  }),
]);

for (const r of results.records.filter((r) => r.state !== "COMPLETED")) console.log(`ERROR #${r.pr} run ${r.run}: ${r.state} ${r.error ?? ""}`);

console.log("Human-comment overlap (weak labels)");
const considered = rows.reduce((n, r) => n + r.comments.considered.length, 0);
const excl = (k: keyof PrRow["comments"]["excluded"]) => rows.reduce((n, r) => n + r.comments.excluded[k], 0);
console.log(`  human inline comments used: ${considered}; excluded: ${excl("agent")} by another AI reviewer, ${excl("olderCommit")} on an older commit, ${excl("reply")} replies, ${excl("bot")} by bots, ${excl("noHeadLine")} with no head-side line`);
console.log(`  PRs with at least one usable human comment: ${rows.filter((r) => r.comments.considered.length > 0).length}/${rows.length}`);
for (let run = 1; run <= runCount; run++) {
  const these = rows.flatMap((r) => r.runs.map((x, i) => ({ x, o: r.overlap[i]!, c: r.comments.considered.length })).filter(({ x }) => x.run === run && x.state === "COMPLETED"));
  const findings = these.reduce((n, t) => n + t.x.findings.length, 0);
  console.log(`  run ${run}: findings that agree ${frac(these.reduce((n, t) => n + t.o.agree, 0), findings)}; human comments matched ${frac(these.reduce((n, t) => n + t.o.matched, 0), these.reduce((n, t) => n + t.c, 0))}`);
}
console.log("  Humans also comment on style, naming and questions, and the reviewer reports only defects, so low overlap does not mean low precision.\n");

console.log("Run-to-run stability (Jaccard; same path and category, line within tolerance)");
const multi = rows.filter((r) => r.runs.filter((x) => x.state === "COMPLETED").length >= 2);
const scored = multi.map((r) => r.jaccard).filter((j): j is number => j !== undefined);
console.log(`  PRs with 2+ completed runs: ${multi.length}; all runs empty (no score): ${multi.length - scored.length}; mean Jaccard over the rest: ${fmt(mean(scored))} (min ${fmt(scored.length ? Math.min(...scored) : undefined)}, max ${fmt(scored.length ? Math.max(...scored) : undefined)})\n`);

console.log("Jev routing");
console.log(`  modes: ${tally(completed.map((r) => r.mode ?? "-"))}`);
console.log(`  Jev suggested: ${tally(completed.map((r) => r.routing.jevSuggested ?? "failed or skipped"))}`);
const raised = (r: RunRecord) => r.routing.jevSuggested === "deep" || r.routing.jevSuggested === "critical";
const cross = { jevOnly: 0, pathOnly: 0, both: 0, neither: 0 };
for (const r of completed) cross[raised(r) ? (r.routing.pathRule ? "both" : "jevOnly") : r.routing.pathRule ? "pathOnly" : "neither"]++;
console.log(`  depth raised by Jev only ${cross.jevOnly}, path rule only ${cross.pathOnly}, both ${cross.both}, neither ${cross.neither} (of ${completed.length})`);
console.log(`  Jev categories: ${tally(completed.flatMap((r) => (r.jev ? [r.jev.category] : [])))}`);
const needs = completed.flatMap((r) => (r.jev ? [r.jev.needsDeep] : []));
console.log(`  needs-deep probability: median ${fmt(median(needs))}, min ${fmt(needs.length ? Math.min(...needs) : undefined)}, max ${fmt(needs.length ? Math.max(...needs) : undefined)}; Jev errors ${completed.filter((r) => r.jevError).length}`);
const flips = rows.filter((r) => new Set(r.runs.filter((x) => x.state === "COMPLETED").map((x) => x.mode)).size > 1).map((r) => `#${r.pr}`);
console.log(`  PRs whose mode differed between runs: ${flips.length}${flips.length ? ` (${flips.join(", ")})` : ""}\n`);

console.log("Verifier discards");
const discards = completed.flatMap((r) => r.discarded);
const candidates = discards.length + completed.reduce((n, r) => n + r.findings.length, 0);
console.log(`  ${discards.length} discarded of ${candidates} candidates; reasons: ${tally(discards.map((d) => discardKind(d.reason))) || "none"}\n`);

const agentOf = (n: number) => agentFindings(read<HumanComment[]>(n, "comments.json"), read<StoredReview[]>(n, "reviews.json"), prs.find((p) => p.number === n)!.headSha);
const reviewedByAgent = prs.filter((p) => agentOf(p.number).length > 0 || read<StoredReview[]>(p.number, "reviews.json").some((r) => r.commit_id === p.headSha && r.body?.includes(AGENT_REVIEW_MARKER)));
console.log("Other AI reviewer (vikrrantshah/pr-review-agent, from its reviews on the snapshot head)");
console.log(`  PRs with its review or its comments on the snapshot head: ${reviewedByAgent.length}/${prs.length}`);
if (reviewedByAgent.length > 0) {
  const theirs = reviewedByAgent.map((p) => agentOf(p.number));
  console.log(`  its findings: ${theirs.flat().length} (${tally(theirs.flat().map((f) => f.severity))})`);
  for (let run = 1; run <= runCount; run++) {
    const m = reviewedByAgent.map((p, i) => crossMatch(records(p.number).find((r) => r.run === run && r.state === "COMPLETED")?.findings ?? [], theirs[i]!));
    const sum = (k: "both" | "oursOnly" | "theirsOnly") => m.reduce((n, x) => n + x[k], 0);
    console.log(`  run ${run}, on those PRs: both ${sum("both")}, pr-review only ${sum("oursOnly")}, the other reviewer only ${sum("theirsOnly")}`);
  }
}
console.log("  Its findings are unverified model output, so agreement with it is not correctness; grade both to compare.\n");

console.log("Cost proxies");
const secs = completed.map((r) => r.seconds);
console.log(`  seconds per review: median ${fmt(median(secs), 0)}, max ${fmt(secs.length ? Math.max(...secs) : undefined, 0)}, total ${secs.reduce((a, b) => a + b, 0)}`);
for (let run = 1; run <= runCount; run++) {
  const these = completed.filter((r) => r.run === run);
  const lines = these.reduce((n, r) => n + rows.find((x) => x.pr === r.pr)!.lines, 0);
  const findings = these.reduce((n, r) => n + r.findings.length, 0);
  console.log(`  run ${run}: ${findings} findings over ${lines} changed lines = ${fmt(lines ? (findings * 1000) / lines : undefined)} per 1k lines`);
}

/** The candidates the verifier dropped in one review, read from that review's store: results.json keeps only the reasons. */
function discardedOf(r: RunRecord): DiscardedCandidate[] {
  const db = join(dataset, "work", `pr-${r.pr}-run-${r.run}`, "app.sqlite");
  if (!existsSync(db)) return [];
  const store = AppStore.open(db);
  try {
    const row = store.db.prepare("SELECT candidates_json, dispositions_json FROM candidate_sets ORDER BY created_at DESC LIMIT 1").get() as { candidates_json: string; dispositions_json: string } | undefined;
    if (!row) return [];
    const candidates = JSON.parse(row.candidates_json) as { id: string; file: string; newLine: number | null; severity: string; category: string; title: string; explanation: string }[];
    return (JSON.parse(row.dispositions_json) as Disposition[])
      .filter((d) => d.state === "DISCARDED")
      .flatMap((d) => {
        const c = candidates.find((x) => x.id === d.candidateId);
        return c ? [{ file: c.file, line: c.newLine, severity: c.severity, category: c.category, title: c.title, explanation: c.explanation, discardReason: d.reason }] : [];
      });
  } finally {
    store.close();
  }
}

if (values.labels) {
  const path = resolve(values.labels);
  const completedOf = (n: number) => records(n).filter((r) => r.state === "COMPLETED");
  const fresh = prs.flatMap((p) => [
    ...uniqueFindings(p.number, p.headSha, completedOf(p.number).map((r) => r.findings)),
    ...uniqueFindings(p.number, p.headSha, completedOf(p.number).map(discardedOf), LINE_TOLERANCE, "discarded"),
    ...uniqueFindings(p.number, p.headSha, [agentOf(p.number)], LINE_TOLERANCE, "agent"),
  ]);
  const all = mergeSheet(fresh, existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as GradeEntry[]) : []);
  writeFileSync(path, `${JSON.stringify(all, null, 2)}\n`);
  const sheet = all.filter((e) => e.kind === "reported");
  const dropped = all.filter((e) => e.kind === "discarded");
  const g = gradedPrecision(sheet);
  const d = gradedPrecision(dropped);
  const theirs = all.filter((e) => e.kind === "agent");
  const a = gradedPrecision(theirs);
  console.log(`\nGraded precision (${path})`);
  console.log(`  ${sheet.length} unique findings, ${g.graded} graded: ${g.real} real, ${g.notReal} not real, ${g.unsure} unsure; precision over real and not real ${pct(g.precision)}`);
  console.log(`  ${dropped.length} unique discarded candidates, ${d.graded} graded: ${d.real} real, ${d.notReal} not real, ${d.unsure} unsure; real among graded discards ${pct(d.precision)} (what the verifier threw away)`);
  console.log(`  ${theirs.length} unique findings by the other AI reviewer, ${a.graded} graded: ${a.real} real, ${a.notReal} not real, ${a.unsure} unsure; its precision over real and not real ${pct(a.precision)}`);
  if (g.graded > 0) {
    const real = sheet.filter((e) => e.verdict === "real");
    const commentsOf = (pr: number) => rows.find((r) => r.pr === pr)!.comments.considered;
    const agree = real.filter((e) => humanOverlap([e], commentsOf(e.pr)).agree > 0).length;
    const matchedAny = rows.reduce((n, r) => n + humanOverlap(sheet.filter((e) => e.pr === r.pr), r.comments.considered).matched, 0);
    const matchedReal = rows.reduce((n, r) => n + humanOverlap(real.filter((e) => e.pr === r.pr), r.comments.considered).matched, 0);
    console.log(`  real findings that agree with a human comment: ${frac(agree, real.length)}; human comments matched by a real finding: ${frac(matchedReal, matchedAny)} of those matched by any finding`);
  }
}
