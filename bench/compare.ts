import { readFileSync } from "node:fs";

type Spread = { mean: number; min: number; max: number } | undefined;
type Results = {
  model: string;
  commit: string;
  date: string;
  summary: { precision: Spread; recall: Spread };
  runs: { cases: { id: string; matched: unknown[]; missed: unknown[]; extra: unknown[] }[] }[];
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
