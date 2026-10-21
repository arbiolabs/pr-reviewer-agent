// Finding identity across heads (docs/design/finding-tracking.md). A finding is the code it cites, not a line number:
// each observation stores the normalized text of its head-side evidence, and a later head is searched for that text.
import { transition } from "../domain/machine.ts";
import { FINDING_MACHINE, type FindingEvent, type FindingState } from "../domain/machines.ts";
import type { CandidateFinding, ChangedFile } from "../domain/types.ts";
import type { PinnedRepo } from "../source/git.ts";

/** The text a finding points at, enough to find it again without the old commit. */
export type EvidenceText = { path: string; start: number; end: number; text: string };

/** `location_json` of an observation: where the finding was last seen. `newLine` and `evidence` are valid at `headSha`. */
export type ObservedLocation = { file: string; newLine: number | null; headSha: string; evidence: EvidenceText[] };

/** What locating one open finding at a new head found. */
export type Located =
  | { kind: "same"; location: ObservedLocation }
  | { kind: "changed"; file: string }
  | { kind: "file_gone" };

/** Why an observation ended where it did. A first observation has none. */
export type Basis = "reverified" | "located" | "changed" | "file_gone";

export type PriorFinding = {
  findingId: string;
  fingerprint: string;
  status: FindingState;
  candidate: CandidateFinding;
  location: ObservedLocation;
  located: Located;
};

/** One observation per finding per revision: the states it walked and the candidate and location it ended with. */
export type Observation = { findingId: string; path: FindingState[]; basis: Basis | null; candidate: CandidateFinding; location: ObservedLocation };

/** Reads one head: file contents, the head path of a rename, and the paths whose text has a literal line. */
export type HeadReader = {
  read(path: string): Promise<string | undefined>;
  renamedTo(path: string): string | undefined;
  grep(line: string): Promise<string[]>;
};

/** The repository's head, with the renames the revision's diff reports (base to head, so a rename of a file the PR added is found by the tree-wide search instead). */
export function headReader(repo: PinnedRepo, changedFiles: ChangedFile[]): HeadReader {
  const renames = new Map(changedFiles.flatMap((f) => (f.from === undefined ? [] : [[f.from, f.path] as const])));
  return {
    read: (path) => repo.show(repo.headSha, path),
    renamedTo: (path) => renames.get(path),
    grep: async (line) => [...new Set((await repo.grep(line, undefined)).split("\n").filter((l) => l.includes(":")).map((l) => l.slice(0, l.indexOf(":"))))],
  };
}

/** A candidate within this many lines of a located anchor is the same defect when its category agrees. */
export const LINK_DISTANCE = 3;
/** A tree-wide search or a text match needs this much text, so `}` alone never relocates a finding. */
const MIN_LINES = 2;
const MIN_CHARS = 40;
/** A finding whose evidence cites only the base SHA is located by this many lines on each side of its anchor. */
const ANCHOR_CONTEXT = 2;

const normalizeLine = (line: string) => line.trim().replace(/\s+/g, " ");
export const normalize = (lines: string[]) => lines.map(normalizeLine).join("\n");
export const splitLines = (text: string | undefined) => (text === undefined ? undefined : (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n"));

/** The 1-based start of the occurrence of `snippet` in `lines` nearest to `near`, or undefined. */
export function findSnippet(snippet: string, lines: string[], near: number): number | undefined {
  const wanted = snippet.split("\n");
  const normalized = lines.map(normalizeLine);
  let best: number | undefined;
  for (let i = 0; i + wanted.length <= normalized.length; i++) {
    if (wanted.every((line, k) => normalized[i + k] === line) && (best === undefined || Math.abs(i + 1 - near) < Math.abs(best - near))) best = i + 1;
  }
  return best;
}

/** True when one text is the other, or contains it on line boundaries and the shorter one is substantial. */
export function sameText(a: string, b: string): boolean {
  if (a === b) return true;
  const [short, long] = a.length <= b.length ? [a, b] : [b, a];
  if (short.split("\n").length < MIN_LINES || short.length < MIN_CHARS) return false;
  return `\n${long}\n`.includes(`\n${short}\n`);
}

/**
 * The normalized text of a candidate's evidence at `headSha`: its ranges that cite the head, plus the lines around its
 * anchor when none of them is in the anchor's file. `read` returns a file at a SHA, or undefined when it does not exist there.
 */
export async function evidenceTexts(candidate: CandidateFinding, headSha: string, read: (sha: string, path: string) => Promise<string | undefined>): Promise<EvidenceText[]> {
  const out: EvidenceText[] = [];
  for (const e of candidate.codeEvidence) {
    if (e.refSha !== headSha) continue;
    const lines = splitLines(await read(headSha, e.path));
    if (lines) out.push({ path: e.path, start: e.start, end: e.end, text: normalize(lines.slice(e.start - 1, e.end)) });
  }
  if (candidate.newLine !== null && !out.some((e) => e.path === candidate.file)) {
    const lines = splitLines(await read(headSha, candidate.file));
    if (lines) {
      const [start, end] = [Math.max(1, candidate.newLine - ANCHOR_CONTEXT), Math.min(lines.length, candidate.newLine + ANCHOR_CONTEXT)];
      out.push({ path: candidate.file, start, end, text: normalize(lines.slice(start - 1, end)) });
    }
  }
  return out;
}

/**
 * Search a new head for a finding's evidence text: the same path, then the head path of its rename, then once across
 * the tree when the snippet is substantial and exactly one file holds it. Every item found is `same`, with the anchor
 * shifted by the same offset as the evidence that contains it. Otherwise the finding is `changed` while its file (or
 * rename) still exists, and `file_gone` when neither does.
 */
export async function locate(location: ObservedLocation, headSha: string, head: HeadReader): Promise<Located> {
  const files = new Map<string, string[] | undefined>();
  const linesOf = async (path: string) => {
    if (!files.has(path)) files.set(path, splitLines(await head.read(path)));
    return files.get(path);
  };
  const present = async (path: string) => {
    if (await linesOf(path)) return path;
    const renamed = head.renamedTo(path);
    return renamed !== undefined && (await linesOf(renamed)) ? renamed : undefined;
  };
  const findItem = async (item: EvidenceText): Promise<{ path: string; start: number } | undefined> => {
    const own = await present(item.path);
    const inOwn = own === undefined ? undefined : findSnippet(item.text, (await linesOf(own))!, item.start);
    if (own !== undefined && inOwn !== undefined) return { path: own, start: inOwn };
    const snippetLines = item.text.split("\n");
    if (snippetLines.length < MIN_LINES || item.text.length < MIN_CHARS) return undefined;
    const query = snippetLines.reduce((a, b) => (b.length > a.length ? b : a));
    const hits: { path: string; start: number }[] = [];
    for (const path of await head.grep(query)) {
      if (path === own) continue;
      const start = findSnippet(item.text, (await linesOf(path)) ?? [], item.start);
      if (start !== undefined) hits.push({ path, start });
    }
    return hits.length === 1 ? hits[0] : undefined;
  };

  const file = await present(location.file);
  const miss = (): Located => (file === undefined ? { kind: "file_gone" } : { kind: "changed", file });
  const found: EvidenceText[] = [];
  for (const item of location.evidence) {
    const hit = await findItem(item);
    if (!hit) return miss();
    found.push({ ...item, path: hit.path, start: hit.start, end: hit.start + item.end - item.start });
  }
  if (found.length === 0) return miss();
  const inFile = location.evidence.map((e, i) => i).filter((i) => location.evidence[i]!.path === location.file);
  const anchor = inFile.find((i) => location.newLine !== null && location.evidence[i]!.start <= location.newLine && location.newLine <= location.evidence[i]!.end) ?? inFile[0];
  if (anchor === undefined) return file === undefined ? miss() : { kind: "same", location: { ...location, file, headSha, evidence: found } };
  const newLine = location.newLine === null ? null : Math.max(1, location.newLine + found[anchor]!.start - location.evidence[anchor]!.start);
  return { kind: "same", location: { file: found[anchor]!.path, newLine, headSha, evidence: found } };
}

/** The file an open finding sits in at the new head, when it still has one. */
export const currentFile = (f: PriorFinding) => (f.located.kind === "same" ? f.located.location.file : f.located.kind === "changed" ? f.located.file : undefined);

const overlaps = (a: EvidenceText[], b: EvidenceText[]) => a.some((x) => b.some((y) => x.path === y.path && x.start <= y.end && y.start <= x.end));

/**
 * The open finding a verified candidate is another report of, if any: its fingerprint after rename mapping, its cited
 * text, or its location (same file and category, anchor within LINK_DISTANCE or overlapping evidence ranges).
 */
export function priorFor(candidate: CandidateFinding, location: ObservedLocation, fingerprintOf: (c: Pick<CandidateFinding, "category" | "file" | "title">) => string, prior: PriorFinding[]): PriorFinding | undefined {
  return prior.find((f) => {
    const file = currentFile(f);
    if (fingerprintOf(candidate) === f.fingerprint) return true;
    if (file === candidate.file && fingerprintOf({ ...candidate, file: f.location.file }) === f.fingerprint) return true;
    if (location.evidence.some((e) => f.location.evidence.some((p) => sameText(e.text, p.text)))) return true;
    if (f.located.kind !== "same" || file !== candidate.file || candidate.category !== f.candidate.category) return false;
    const at = f.located.location;
    const nearAnchor = candidate.newLine !== null && at.newLine !== null && Math.abs(candidate.newLine - at.newLine) <= LINK_DISTANCE;
    return nearAnchor || overlaps(location.evidence, at.evidence);
  });
}

const CARRY: Record<Located["kind"], [FindingEvent, Basis]> = {
  same: ["OBSERVED_PRESENT", "located"],
  changed: ["OBSERVED_POSSIBLY_FIXED", "changed"],
  file_gone: ["OBSERVED_OUTDATED", "file_gone"],
};

/** The states a finding walks on `event`: one step when the machine allows it, else it stays where it is. */
export function walk(status: FindingState, event: FindingEvent): FindingState[] {
  const result = transition(FINDING_MACHINE, status, event);
  return result.ok ? [status, result.state] : [status];
}

/**
 * One observation per finding this revision touched: a verified candidate that links to an open finding re-observes
 * it as present, every other open finding is observed as located, and a candidate linked to nothing is a new finding.
 */
export function observe(prior: PriorFinding[], verified: { findingId: string; candidate: CandidateFinding; location: ObservedLocation }[]): Observation[] {
  const linked = new Map(verified.map((v) => [v.findingId, v]));
  const carried = prior.map((f): Observation => {
    const link = linked.get(f.findingId);
    if (link) return { findingId: f.findingId, path: walk(f.status, "OBSERVED_PRESENT"), basis: "reverified", candidate: link.candidate, location: link.location };
    const [event, basis] = CARRY[f.located.kind];
    if (f.located.kind !== "same") return { findingId: f.findingId, path: walk(f.status, event), basis, candidate: f.candidate, location: f.location };
    const at = f.located.location;
    const candidate = { ...f.candidate, file: at.file, newLine: at.newLine, codeEvidence: at.evidence.map(({ path, start, end }) => ({ path, start, end, refSha: at.headSha })) };
    return { findingId: f.findingId, path: walk(f.status, event), basis, candidate, location: at };
  });
  const known = new Set(prior.map((f) => f.findingId));
  const fresh = verified.filter((v) => !known.has(v.findingId)).map((v): Observation => ({ findingId: v.findingId, path: ["DETECTED"], basis: null, candidate: v.candidate, location: v.location }));
  return [...fresh, ...carried];
}
