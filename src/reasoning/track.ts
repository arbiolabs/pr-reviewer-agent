// Finding identity across heads (docs/design/finding-tracking.md). A finding is the code it cites, not a line number:
// each observation stores the normalized text of its head-side evidence, and a later head is searched for that text.
import { transition } from "../domain/machine.ts";
import { FINDING_MACHINE, type FindingEvent, type FindingState } from "../domain/machines.ts";
import type { CandidateFinding, ChangedFile, ResolutionVerdict } from "../domain/types.ts";
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

/**
 * Why an observation ended where it did. A first observation has none. `resolution_checked` is a resolution claim
 * code accepted, `resolution_refused` one it refused, and `reported_unverified` a re-report of the finding that
 * failed VERIFY, which leaves a doubted finding UNCERTAIN.
 */
export type Basis = "reverified" | "located" | "changed" | "file_gone" | "resolution_checked" | "resolution_refused" | "reported_unverified";

export type PriorFinding = {
  findingId: string;
  status: FindingState;
  candidate: CandidateFinding;
  location: ObservedLocation;
  located: Located;
};

/** One observation per finding per revision: the states it walked and the candidate and location it ended with. */
export type Observation = { findingId: string; path: FindingState[]; basis: Basis | null; candidate: CandidateFinding; location: ObservedLocation };

/** Reads one head: file contents, the head path of a rename, and the paths whose text has a literal line (undefined when too many to list). */
export type HeadReader = {
  read(path: string): Promise<string | undefined>;
  renamedTo(path: string): string | undefined;
  grep(line: string): Promise<string[] | undefined>;
};

/** The repository's head, with the renames the revision's diff reports (base to head, so a rename of a file the PR added is found by the tree-wide search instead). */
export function headReader(repo: PinnedRepo, changedFiles: ChangedFile[]): HeadReader {
  const renames = new Map(changedFiles.flatMap((f) => (f.from === undefined ? [] : [[f.from, f.path] as const])));
  const texts = new Map<string, Promise<string | undefined>>();
  return {
    read: (path) => texts.get(path) ?? texts.set(path, repo.show(repo.headSha, path)).get(path)!,
    renamedTo: (path) => renames.get(path),
    grep: (line) => repo.grepPaths(line),
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

/** A snippet below the floor (`}`, an empty line, one short statement) is no evidence of identity, however equal. */
const substantial = (text: string) => text.split("\n").length >= MIN_LINES && text.length >= MIN_CHARS;

/** True when the snippet's normalized lines sit at `start` in `lines`. */
const foundAt = (snippet: string, lines: string[], start: number) => snippet.split("\n").every((line, k) => lines[start - 1 + k] !== undefined && normalizeLine(lines[start - 1 + k]!) === line);

/** True when both texts are substantial and equal. A broad range that merely contains the other is not the same text. */
export function sameText(a: string, b: string): boolean {
  return substantial(a) && a === b;
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

type LinesOf = (path: string) => Promise<string[] | undefined>;
/** The lines of each file at head, read once. */
const lineCache = (head: HeadReader): LinesOf => {
  const files = new Map<string, Promise<string[] | undefined>>();
  return (path) => files.get(path) ?? files.set(path, head.read(path).then(splitLines)).get(path)!;
};

/**
 * Every place at head where a substantial `item`'s text sits, by one tree-wide search for its longest line, or undefined
 * when the path list was cut at its cap and so names no place.
 */
async function treeHits(item: EvidenceText, head: HeadReader, linesOf: LinesOf): Promise<{ path: string; start: number }[] | undefined> {
  const query = item.text.split("\n").reduce((a, b) => (b.length > a.length ? b : a));
  const paths = await head.grep(query);
  if (paths === undefined) return undefined;
  const hits: { path: string; start: number }[] = [];
  for (const path of paths) {
    const start = findSnippet(item.text, (await linesOf(path)) ?? [], item.start);
    if (start !== undefined) hits.push({ path, start });
  }
  return hits;
}

/**
 * Search a new head for a finding's evidence text: the same path, then the head path of its rename, then once across
 * the tree when the snippet is substantial and exactly one file holds it. Every item found is `same`, with the anchor
 * shifted by the same offset as the evidence that contains it. Otherwise the finding is `changed` while its file (or
 * rename) still exists, and `file_gone` when neither does. `treeWide: false` skips the tree-wide search, for a finding
 * whose text an earlier head already searched for and missed.
 */
export async function locate(location: ObservedLocation, headSha: string, head: HeadReader, options: { treeWide: boolean } = { treeWide: true }): Promise<Located> {
  const linesOf = lineCache(head);
  const present = async (path: string) => {
    if (await linesOf(path)) return path;
    const renamed = head.renamedTo(path);
    return renamed !== undefined && (await linesOf(renamed)) ? renamed : undefined;
  };
  const findItem = async (item: EvidenceText): Promise<{ path: string; start: number } | undefined> => {
    const own = await present(item.path);
    if (own !== undefined) {
      const lines = (await linesOf(own))!;
      // A snippet below the floor is found only where it was: moving it would pin the finding to any `}` in the file.
      const start = substantial(item.text) ? findSnippet(item.text, lines, item.start) : foundAt(item.text, lines, item.start) ? item.start : undefined;
      if (start !== undefined) return { path: own, start };
    }
    if (!substantial(item.text) || !options.treeWide) return undefined;
    const hits = (await treeHits(item, head, linesOf))?.filter((hit) => hit.path !== own);
    return hits?.length === 1 ? hits[0] : undefined;
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

/**
 * Where the finding's own-file code still sits at this head, as "at path:line", or undefined when none of it does.
 * `located` says only that some item went missing, which a changed helper in another file, a comment inserted in the
 * cited block, or a one-line shift of a short item also does, so a resolution check asks this instead: each own-file
 * item is looked for anywhere in the finding's current file, and a substantial one anywhere in the tree. A tree-wide
 * search cut at its cap counts as found, since an open finding beats a false resolution.
 */
export async function survivingCode(f: PriorFinding, head: HeadReader): Promise<string | undefined> {
  const linesOf = lineCache(head);
  const file = currentFile(f);
  const lines = file === undefined ? undefined : await linesOf(file);
  for (const item of f.location.evidence) {
    if (item.path !== f.location.file || item.text === "") continue;
    const start = lines && findSnippet(item.text, lines, item.start);
    if (start !== undefined) return `at ${file}:${start}`;
    if (!substantial(item.text)) continue;
    const hits = await treeHits(item, head, linesOf);
    if (hits === undefined) return "in more files than the search lists";
    const hit = hits.find((h) => h.path !== file);
    if (hit) return `at ${hit.path}:${hit.start}`;
  }
  return undefined;
}

const overlaps = (a: EvidenceText[], b: EvidenceText[]) => a.some((x) => b.some((y) => x.path === y.path && x.start <= y.end && y.start <= x.end));

type Fingerprint = (c: Pick<CandidateFinding, "category" | "file" | "title">) => string;
/** A verified candidate with its evidence text, plus, when it names a prior finding, that finding's own code found in the candidate's file (`findOwnEvidence`). */
export type Report = { candidate: CandidateFinding; location: ObservedLocation; priorHits?: EvidenceText[] };

/**
 * The longest substantial run of consecutive lines of `text` that occurs exactly once in `lines`, as a 1-based range
 * with its text. A cited block often spans the code that moved and the code that changed next to it; the part that
 * survived is still the finding's code. The floor keeps `}` or one short statement from counting, and a run that
 * occurs twice names no place.
 */
export function findLongestRun(text: string, lines: string[]): Omit<EvidenceText, "path"> | undefined {
  const wanted = text.split("\n");
  const normalized = lines.map(normalizeLine);
  const positions = new Map<string, number[]>();
  normalized.forEach((line, i) => positions.set(line, [...(positions.get(line) ?? []), i]));
  // Maximal runs only: a start that continues the previous line's run was found from there.
  const runs: { o: number; p: number; k: number }[] = [];
  for (let o = 0; o < wanted.length; o++) {
    for (const p of positions.get(wanted[o]!) ?? []) {
      if (o > 0 && p > 0 && normalized[p - 1] === wanted[o - 1]) continue;
      let k = 1;
      while (o + k < wanted.length && normalized[p + k] === wanted[o + k]) k++;
      runs.push({ o, p, k });
    }
  }
  for (const run of runs.sort((a, b) => b.k - a.k)) {
    const sub = wanted.slice(run.o, run.o + run.k).join("\n");
    if (!substantial(sub)) break;
    if (runs.filter((r) => r.o <= run.o && r.o + r.k >= run.o + run.k).length === 1) return { start: run.p + 1, end: run.p + run.k, text: sub };
  }
  return undefined;
}

/**
 * The finding's own-file evidence found in `lines`, the file a candidate that names the finding reports, at the
 * position found: for each item, its longest substantial run that occurs once there. This is how a named link checks
 * a finding whose code was not located: the model says which file, and code finds the finding's text there before
 * the candidate's citation is compared with it.
 */
export function findOwnEvidence(f: PriorFinding, path: string, lines: string[]): EvidenceText[] {
  return f.location.evidence.flatMap((e) => {
    const run = e.path === f.location.file ? findLongestRun(e.text, lines) : undefined;
    return run === undefined ? [] : [{ ...run, path }];
  });
}

/**
 * The finding's evidence in its own file, at the path that file has at this head: the located ranges when its code was
 * found, else the ranges last seen, under the rename the diff reports. Evidence in another file, such as a helper that
 * two defects both call, says nothing about which defect a report is, so it never takes part in a link.
 */
function ownEvidence(f: PriorFinding): EvidenceText[] {
  if (f.located.kind === "same") {
    const at = f.located.location;
    return at.evidence.filter((e) => e.path === at.file);
  }
  const file = f.located.kind === "changed" ? f.located.file : f.location.file;
  return f.location.evidence.filter((e) => e.path === f.location.file).map((e) => ({ ...e, path: file }));
}

/**
 * Whether a candidate that names the finding (`priorFindingId`) reports it where code can see it: the anchor is
 * within LINK_DISTANCE of the finding's anchor at this head, or a cited range overlaps the finding's evidence at this
 * head. That evidence is the located ranges when the code was found, else the finding's own text found in the
 * candidate's file (`priorHits`), plus the ranges last seen when the candidate reports the finding's file. The
 * finding's file, when it still exists, must be the candidate's: a report elsewhere is a new finding.
 */
function namedLink(candidate: CandidateFinding, cited: EvidenceText[], f: PriorFinding, priorHits: EvidenceText[]): boolean {
  const file = currentFile(f);
  if (file !== undefined && file !== candidate.file) return false;
  const evidence = file === undefined ? priorHits : [...ownEvidence(f), ...priorHits];
  const lastLine = f.located.kind === "same" ? f.located.location.newLine : file === undefined ? null : f.location.newLine;
  const nearAnchor = candidate.newLine !== null && lastLine !== null && Math.abs(candidate.newLine - lastLine) <= LINK_DISTANCE;
  return nearAnchor || overlaps(cited, evidence);
}

/**
 * How strongly a verified candidate reads as another report of an open finding, or undefined when it does not. Every
 * link needs the candidate in the finding's category and in its file at this head, after the rename the diff reports,
 * and reads only what each cites in its own file. 4: the candidate names the finding and `namedLink` confirms where
 * it reports it. 3: the fingerprint agrees, so the title names the same defect, and the report is where the finding
 * was: its code is found in place, or the anchor is within LINK_DISTANCE of the finding's last anchor, or the cited
 * ranges overlap its last evidence. A fixed defect and a second instance of it with the same title far down the file
 * are two findings. 2: a cited text agrees (substantial and equal). 1: the anchor is within LINK_DISTANCE of the
 * located anchor and the cited ranges overlap the located evidence.
 * When the finding's file is gone and the diff names no rename (a file the PR added shows as added, not renamed),
 * a substantial equal cited text, or a named link whose file holds the finding's text, is what says where the code
 * went, so those links alone may cross files.
 * Two defects on neighbouring lines in one category are told apart by what they cite: a report that cites none of
 * the finding's code is a new finding. That can duplicate a comment; it never hides a defect.
 */
function linkScore({ candidate, location, priorHits = [] }: Report, fingerprintOf: Fingerprint, f: PriorFinding): number | undefined {
  if (candidate.category !== f.candidate.category) return undefined;
  const own = ownEvidence(f);
  const cited = location.evidence.filter((e) => e.path === candidate.file);
  if (candidate.priorFindingId === f.findingId && namedLink(candidate, cited, f, priorHits)) return 4;
  const sameCitedText = cited.some((e) => own.some((p) => sameText(e.text, p.text)));
  const file = currentFile(f);
  if (file === undefined) return sameCitedText ? 2 : undefined;
  if (file !== candidate.file) return undefined;
  const lastLine = f.located.kind === "same" ? f.located.location.newLine : f.location.newLine;
  const nearAnchor = candidate.newLine !== null && lastLine !== null && Math.abs(candidate.newLine - lastLine) <= LINK_DISTANCE;
  const sameLines = overlaps(cited, own);
  // The file is settled above, so the fingerprint is compared without it: the finding's own was built from its first path.
  if ((f.located.kind === "same" || nearAnchor || sameLines) && fingerprintOf({ ...candidate, file: "" }) === fingerprintOf({ ...f.candidate, file: "" })) return 3;
  if (sameCitedText) return 2;
  return f.located.kind === "same" && nearAnchor && sameLines ? 1 : undefined;
}

const anchorDistance = (candidate: CandidateFinding, f: PriorFinding) =>
  f.located.kind === "same" && candidate.newLine !== null && f.located.location.newLine !== null ? Math.abs(candidate.newLine - f.located.location.newLine) : Number.POSITIVE_INFINITY;

/**
 * The open finding each verified candidate is another report of, if any, as a one-to-one assignment: the best-scoring
 * pair links first, then the next pair whose candidate and finding are both unclaimed, and so on. A candidate left
 * over is a new finding, never a duplicate of the one that claimed its prior.
 */
export function linkPriors(reports: Report[], fingerprintOf: Fingerprint, prior: PriorFinding[]): (PriorFinding | undefined)[] {
  const pairs = reports.flatMap((r, i) =>
    prior.flatMap((f) => {
      const score = linkScore(r, fingerprintOf, f);
      return score === undefined ? [] : [{ i, f, score, distance: anchorDistance(r.candidate, f) }];
    }),
  );
  pairs.sort((a, b) => b.score - a.score || a.distance - b.distance || a.i - b.i);
  const linked: (PriorFinding | undefined)[] = reports.map(() => undefined);
  const claimed = new Set<string>();
  for (const { i, f } of pairs) {
    if (linked[i] !== undefined || claimed.has(f.findingId)) continue;
    linked[i] = f;
    claimed.add(f.findingId);
  }
  return linked;
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

/** `path` extended by one more step on `event` when the machine allows it from where the path ends. */
const extend = (path: FindingState[], event: FindingEvent): FindingState[] => [...path, ...walk(path.at(-1)!, event).slice(1)];

/**
 * What else this revision said about open findings: the resolution claims code accepted or refused (`ResolutionVerdict`),
 * and the findings a candidate named as still present without a verified link (`unverified`).
 */
export type Claims = { verdicts: ResolutionVerdict[]; unverified: Iterable<string> };

/**
 * One observation per finding this revision touched: a verified candidate that links to an open finding re-observes
 * it as present, every other open finding is observed as located, and a candidate linked to nothing is a new finding.
 * A located finding then takes what the model claimed about it, as far as code confirmed it: an accepted resolution
 * claim resolves a doubted finding, and an unverified re-report leaves a doubted finding UNCERTAIN.
 */
export function observe(prior: PriorFinding[], verified: { findingId: string; candidate: CandidateFinding; location: ObservedLocation }[], claims: Claims = { verdicts: [], unverified: [] }): Observation[] {
  const linked = new Map(verified.map((v) => [v.findingId, v]));
  const verdicts = new Map(claims.verdicts.map((v) => [v.findingId, v.state]));
  const unverified = new Set(claims.unverified);
  const carried = prior.map((f): Observation => {
    const link = linked.get(f.findingId);
    if (link) return { findingId: f.findingId, path: walk(f.status, "OBSERVED_PRESENT"), basis: "reverified", candidate: link.candidate, location: link.location };
    const [event, located] = CARRY[f.located.kind];
    let path = walk(f.status, event);
    let basis: Basis = located;
    // Each claim moves the finding only where the machine allows it: a finding found in place stays STILL_PRESENT.
    const resolved = verdicts.get(f.findingId) === "ACCEPTED" ? extend(path, "CONFIRMED_RESOLVED") : path;
    const uncertain = unverified.has(f.findingId) ? extend(path, "OBSERVED_UNCERTAIN") : path;
    if (resolved.length > path.length) [path, basis] = [resolved, "resolution_checked"];
    else if (uncertain.length > path.length) [path, basis] = [uncertain, "reported_unverified"];
    else if (verdicts.get(f.findingId) === "REFUSED") basis = "resolution_refused";
    if (f.located.kind !== "same") return { findingId: f.findingId, path, basis, candidate: f.candidate, location: f.location };
    const at = f.located.location;
    const candidate = { ...f.candidate, file: at.file, newLine: at.newLine, codeEvidence: at.evidence.map(({ path, start, end }) => ({ path, start, end, refSha: at.headSha })) };
    return { findingId: f.findingId, path, basis, candidate, location: at };
  });
  const known = new Set(prior.map((f) => f.findingId));
  const fresh = verified.filter((v) => !known.has(v.findingId)).map((v): Observation => ({ findingId: v.findingId, path: ["DETECTED"], basis: null, candidate: v.candidate, location: v.location }));
  return [...fresh, ...carried];
}
