import { readFileSync } from "node:fs";

type Spread = { mean: number; min: number; max: number } | undefined;
type Rate = { n: number; of: number };
type Tracking = Record<"falseResolution" | "identityKept" | "duplicateComments" | "resolutionRecall" | "newFound", Rate>;
type Results = {
  model: string;
  commit: string;
  date: string;
  summary: { precision: Spread; recall: Spread; tracking?: Tracking };
  runs: { cases: { id: string; matched: unknown[]; missed: unknown[]; extra: unknown[] }[]; tracked?: { id: string; steps: { comments: unknown[] }[]; duplicateComments: Rate; identityKept: Rate; falseResolution: Rate }[] }[];
};

const args = process.argv.slice(2).filter((a) => a !== "--");
if (args.length !== 2) {
  console.error("usage: pnpm bench:compare a.json b.json");
  process.exit(2);
}
const [a, b] = args.map((path) => JSON.parse(readFileSync(path, "utf8")) as Results) as [Results, Results];
const pct = (s: Spread) => (s ? `${(s.mean * 100).toFixed(0)}% (${(s.min * 100).toFixed(0)}-${(s.max * 100).toFixed(0)})` : "n/a");
const counts = (r: Results, id: string) =>
  r.runs.map((run) => run.cases.find((c) => c.id === id)).map((c) => (c ? `${c.matched.length}/${c.missed.length}/${c.extra.length}` : "-")).join(" ");

for (const [name, r] of [["a", a], ["b", b]] as const) console.log(`${name}: ${r.model} @ ${r.commit.slice(0, 12)} ${r.date}  precision ${pct(r.summary.precision)}  recall ${pct(r.summary.recall)}`);
console.log("\ncase  TP/FN/FP per run: a | b");
const ids = [...new Set([a, b].flatMap((r) => r.runs[0]?.cases.map((c) => c.id) ?? []))].sort();
for (const id of ids) console.log(`${id}  ${counts(a, id)} | ${counts(b, id)}`);

// Multi-commit cases, summed over runs: the rates in priority order, then per case the inline comments per step.
const frac = (r: Rate | undefined) => (r ? `${r.n}/${r.of}` : "n/a");
if (a.summary.tracking || b.summary.tracking) {
  console.log("\ntracking (n/of): a | b");
  for (const k of ["falseResolution", "identityKept", "duplicateComments", "resolutionRecall", "newFound"] as const) console.log(`${k}  ${frac(a.summary.tracking?.[k])} | ${frac(b.summary.tracking?.[k])}`);
  const tracked = (r: Results, id: string) =>
    r.runs.map((run) => run.tracked?.find((t) => t.id === id)).map((t) => (t ? `${t.steps.map((s) => s.comments.length).join(",")} dup ${t.duplicateComments.n}` : "-")).join(" ");
  console.log("\ncase  inline comments per step, duplicates: a | b");
  for (const id of [...new Set([a, b].flatMap((r) => r.runs[0]?.tracked?.map((t) => t.id) ?? []))].sort()) console.log(`${id}  ${tracked(a, id)} | ${tracked(b, id)}`);
}
