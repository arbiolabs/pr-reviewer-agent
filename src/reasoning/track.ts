// Finding identity across heads (docs/design/finding-tracking.md). A finding is the code it cites, not a line number:
// each observation stores the normalized text of its head-side evidence, and a later head is searched for that text,
// compared as code tokens so that comments, blank lines, spacing and separators never count as a change.
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

/** Reads one head: file contents, the head path of a rename, and the paths whose text contains a literal word (undefined when too many to list). */
export type HeadReader = {
  read(path: string): Promise<string | undefined>;
  renamedTo(path: string): string | undefined;
  grep(word: string): Promise<string[] | undefined>;
};

/** The repository's head, with the renames the revision's diff reports (base to head, so a rename of a file the PR added is found by the tree-wide search instead). */
export function headReader(repo: PinnedRepo, changedFiles: ChangedFile[]): HeadReader {
  const renames = new Map(changedFiles.flatMap((f) => (f.from === undefined ? [] : [[f.from, f.path] as const])));
  const texts = new Map<string, Promise<string | undefined>>();
  return {
    read: (path) => texts.get(path) ?? texts.set(path, repo.show(repo.headSha, path)).get(path)!,
    renamedTo: (path) => renames.get(path),
    grep: (word) => repo.grepPaths(word),
  };
}

/** A candidate within this many lines of a located anchor is the same defect when its category agrees. */
export const LINK_DISTANCE = 3;
/**
 * A tree-wide search, a text match or a shared run needs this many code tokens, so `}` or `return x;` never relocates
 * a finding or links two reports, while one distinctive line such as `db.orders.delete(id);` does.
 */
export const MIN_TOKENS = 6;
/** A finding whose evidence cites only the base SHA is located by this many lines on each side of its anchor. */
const ANCHOR_CONTEXT = 2;

const normalizeLine = (line: string) => line.trim().replace(/\s+/g, " ");
export const normalize = (lines: string[]) => lines.map(normalizeLine).join("\n");
export const splitLines = (text: string | undefined) => (text === undefined ? undefined : (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n"));

/** One code token: its text, and the 1-based line it starts on in the text it was read from. */
export type Token = { text: string; line: number };

/** One way of reading the two markers no lexer can settle without knowing the language; see `VARIANTS`. */
type Variant = { lineLocal: boolean; dashComment: boolean };
/**
 * A backtick or `/*` may open a string or a comment that runs on for lines, or sit in a regex or a shell glob and open
 * nothing; `--` may start a comment or decrement. Code is compared under every reading, both sides read the same way,
 * and any match counts. A reading can only find more code, never less, so each one keeps more findings open and lets
 * no claim pass that failed without it. `lineLocal` closes a backtick and a `/*` at the line end; `dashComment` reads
 * `--` after a space as a comment.
 */
const VARIANTS: Variant[] = [false, true].flatMap((lineLocal) => [true, false].map((dashComment) => ({ lineLocal, dashComment })));
const DEFAULT_VARIANT = VARIANTS[0]!;

const WORD = /[\p{L}\p{N}_$]/u;
const isSpace = (code: number) => code === 32 || (code >= 9 && code <= 13) || (code > 127 && /\s/u.test(String.fromCharCode(code)));
const isWord = (code: number) => (code >= 97 && code <= 122) || (code >= 65 && code <= 90) || (code >= 48 && code <= 57) || code === 95 || code === 36 || (code > 127 && WORD.test(String.fromCharCode(code)));
const isDigit = (code: number) => code >= 48 && code <= 57;
/** Brackets and separators say nothing about which code this is: without them `f(a);` and `f(a)` read the same. */
const SEPARATORS = new Set([";", ",", "(", ")", "{", "}", "[", "]"].map((c) => c.charCodeAt(0)));
const [NEWLINE, HASH, DASH, SLASH, STAR, LESS, DQUOTE, SQUOTE, BACKTICK, BACKSLASH] = [10, 35, 45, 47, 42, 60, 34, 39, 96, 92];

/**
 * The code in `text` as tokens, read the same way for every language that uses these markers: words (letters, digits,
 * `_`, `$`; a word that starts with a digit is lowercased, so `0XFF` is `0xff`), string literals as one token each
 * with `'` folded to `"` and `\'` and `\"` to the bare quote (Prettier's rewrites), and every other character on its
 * own. Whitespace, `//` comments, `#` and `--` comments at a line start or after a space (so `i--` and `this.#x` stay
 * code), `/* *\/` and `<!-- -->` comments, triple-quoted strings and separators drop out. A `"` or `'` string ends at
 * its line end when unterminated; a backtick string spans lines unless `variant.lineLocal`.
 * Limits, each the same on both sides of a comparison: `#include`, a CSS `#id` and a `#private` field at a line start
 * lose the rest of their line, as does Python's `//` floor division; an apostrophe outside a string eats to the line
 * end; a change inside a docstring is invisible. A line that reads as a comment is still found as written (`linesAt`).
 */
export function tokenize(text: string, variant: Variant = DEFAULT_VARIANT): Token[] {
  const out: Token[] = [];
  const n = text.length;
  let line = 1;
  let i = 0;
  const at = (k: number) => text.charCodeAt(k);
  const toLineEnd = () => {
    const eol = text.indexOf("\n", i);
    i = eol === -1 ? n : eol;
  };
  const skipTo = (close: string, from: number) => {
    const found = text.indexOf(close, from);
    const stop = found === -1 ? n : found + close.length;
    for (let k = i; k < stop; k++) if (at(k) === NEWLINE) line++;
    i = stop;
  };
  while (i < n) {
    const c = at(i);
    const next = at(i + 1);
    if (c === NEWLINE) {
      line++;
      i++;
    } else if (isSpace(c)) {
      i++;
    } else if ((c === SLASH && next === SLASH) || ((c === HASH || (c === DASH && next === DASH && variant.dashComment)) && (i === 0 || isSpace(at(i - 1))))) {
      toLineEnd();
    } else if (c === SLASH && next === STAR) {
      if (variant.lineLocal) toLineEnd();
      else skipTo("*/", i + 2);
    } else if (c === LESS && text.startsWith("<!--", i)) {
      skipTo("-->", i + 4);
    } else if (c === DQUOTE || c === SQUOTE || c === BACKTICK) {
      const quote = text[i]!;
      if (next === c && at(i + 2) === c) {
        skipTo(quote.repeat(3), i + 3);
        continue;
      }
      const start = line;
      const spans = c === BACKTICK && !variant.lineLocal;
      let j = i + 1;
      while (j < n && at(j) !== c && (at(j) !== NEWLINE || spans)) {
        if (at(j) === BACKSLASH) j++;
        if (at(j) === NEWLINE) line++;
        j++;
      }
      const closed = j < n && at(j) === c;
      const shown = c === BACKTICK ? "`" : '"';
      out.push({ text: `${shown}${text.slice(i + 1, j).replace(/\\(["'])/g, "$1")}${closed ? shown : ""}`, line: start });
      i = closed ? j + 1 : j;
    } else if (isWord(c)) {
      let j = i + 1;
      while (j < n && isWord(at(j))) j++;
      const word = text.slice(i, j);
      out.push({ text: isDigit(c) ? word.toLowerCase() : word, line });
      i = j;
    } else {
      if (!SEPARATORS.has(c)) out.push({ text: text[i]!, line });
      i++;
    }
  }
  return out;
}

/**
 * The token streams a stored text may stand for under one reading. A text cut from a file can begin inside a block
 * comment, an HTML comment, a docstring or a backtick string, whose tail would then read as code; when its closer
 * comes before any opener, the text is also read as that tail, so the code after it is still found.
 */
function parses(text: string, variant: Variant): Token[][] {
  const out = [tokenize(text, variant)];
  const tailOf = (open: string, close: string) => {
    const at = text.indexOf(close);
    return at !== -1 && (text.indexOf(open) === -1 || at < text.indexOf(open));
  };
  if (tailOf("/*", "*/")) out.push(tokenize(`/*${text}`, variant));
  if (tailOf("<!--", "-->")) out.push(tokenize(`<!--${text}`, variant));
  for (const q of ['"""', "'''", "`"]) if (text.split(q).length % 2 === 0) out.push(tokenize(`${q}${text}`, variant).filter((t, k) => k > 0 || !t.text.startsWith(q)));
  return out.filter((p) => p.length > 0);
}

const texts = (tokens: Token[]) => tokens.map((t) => t.text);

/** The index in `hay` at or after `from` where `needle` sits whole, or -1. */
function indexOfRun(needle: string[], hay: string[], from = 0): number {
  search: for (let i = from; i + needle.length <= hay.length; i++) {
    for (let k = 0; k < needle.length; k++) if (hay[i + k] !== needle[k]) continue search;
    return i;
  }
  return -1;
}

export const hasCode = (text: string) => tokenize(text).length > 0;

/** True when, under some reading, some parse of `a` sits whole in some parse of `b`: `a` is a piece of `b`'s code, unchanged. */
export const within = (a: string, b: string) => VARIANTS.some((v) => parses(a, v).some((pa) => parses(b, v).some((pb) => indexOfRun(texts(pa), texts(pb)) !== -1)));

/** True when, under some reading, some parse of `a` and the tokens of `b` share a run of MIN_TOKENS or more: `b` still holds a substantial piece of `a`'s code. */
export const sharesCode = (a: string, b: string) =>
  VARIANTS.some((v) => {
    const hay = texts(tokenize(b, v));
    return parses(a, v).some((pa) => commonRun(texts(pa), hay) >= MIN_TOKENS);
  });

/** A 1-based line range. */
type Range = { start: number; end: number };

/** A text's tokens under one reading, with their texts ready for comparison. */
type Tokenized = { tokens: Token[]; texts: string[] };
/** A text under every reading, in VARIANTS order. */
const readings = (text: string): Tokenized[] =>
  VARIANTS.map((v) => {
    const tokens = tokenize(text, v);
    return { tokens, texts: texts(tokens) };
  });

/**
 * Every line range of `file` where a parse of `text` sits whole under the same reading, in file order. The range
 * starts where the text's first line would and ends where its last line would, so a comment inserted inside widens it
 * by a line.
 */
function occurrences(text: string, file: Tokenized[]): Range[] {
  const lines = text.split("\n").length;
  const found: Range[] = [];
  for (const [k, variant] of VARIANTS.entries()) {
    const { tokens, texts: hay } = file[k]!;
    for (const parse of parses(text, variant)) {
      const needle = texts(parse);
      for (let i = indexOfRun(needle, hay); i !== -1; i = indexOfRun(needle, hay, i + 1)) {
        found.push({ start: tokens[i]!.line - parse[0]!.line + 1, end: tokens[i + needle.length - 1]!.line + lines - parse.at(-1)!.line });
      }
    }
  }
  return found.sort((a, b) => a.start - b.start);
}

const nearest = (ranges: Range[], near: number) => ranges.reduce<Range | undefined>((best, r) => (best === undefined || Math.abs(r.start - near) < Math.abs(best.start - near) ? r : best), undefined);

/** Text with enough tokens to be looked for across the tree, or to say that two texts are the same code. */
const substantial = (text: string) => tokenize(text).length >= MIN_TOKENS;

/**
 * Every 1-based line of `lines`, already normalized, where the text's lines sit as written: how a text with no code
 * tokens, such as `}` or a `#field` line, or a file too large to tokenize, is found. A text's tokens come from its
 * normalized lines, so a line found this way is the same code under every reading.
 */
function linesAt(text: string, lines: string[]): number[] {
  const want = text.split("\n");
  const out: number[] = [];
  for (let start = 0; start + want.length <= lines.length; start++) if (want.every((line, k) => lines[start + k] === line)) out.push(start + 1);
  return out;
}

/** True when both texts are substantial and, under some reading, the same code: equal token by token, whatever their comments, spacing and separators. */
export function sameText(a: string, b: string): boolean {
  return substantial(a) && VARIANTS.some((v) => texts(tokenize(a, v)).join("\0") === texts(tokenize(b, v)).join("\0"));
}

/**
 * The normalized text of a candidate's evidence at `headSha`: its ranges that cite the head, plus the lines around its
 * anchor when none of them is in the anchor's file. `read` returns a file at a SHA, or undefined when it does not exist there.
 */
export async function evidenceTexts(candidate: CandidateFinding, headSha: string, read: (sha: string, path: string) => Promise<string | undefined>): Promise<EvidenceText[]> {
  const out: EvidenceText[] = [];
  for (const e of candidate.codeEvidence) {
    if (e.refSha !== headSha || e.start < 1 || e.end < e.start) continue;
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
 * A file longer than this is compared by its lines as written only, never by tokens: a finding in it is found in place
 * or at the one place its lines sit, it survives when its lines are found, and a claim on it is otherwise refused as
 * code that may survive. Tokenizing such a file under every reading and searching each would cost seconds and hundreds
 * of megabytes per finding, so the bound fails toward an open finding, never toward a resolution.
 */
export const MAX_FILE_CHARS = 4_000_000;

/** A file at head: its normalized lines, and its tokens under every reading, or none past MAX_FILE_CHARS. */
type FileAt = { lines: string[]; readings: Tokenized[] | undefined };
type FilesOf = (path: string) => Promise<FileAt | undefined>;
const FILES = new WeakMap<HeadReader, Map<string, Promise<FileAt | undefined>>>();
/** Each file at head, read and tokenized once per head, however many findings are checked against it. */
const fileCache = (head: HeadReader): FilesOf => {
  const files = FILES.get(head) ?? new Map<string, Promise<FileAt | undefined>>();
  FILES.set(head, files);
  const load = async (path: string) => {
    const raw = splitLines(await head.read(path));
    if (!raw) return undefined;
    const lines = raw.map(normalizeLine);
    const text = lines.join("\n");
    return { lines, readings: text.length > MAX_FILE_CHARS ? undefined : readings(text) };
  };
  return (path) => files.get(path) ?? files.set(path, load(path)).get(path)!;
};

/** Every range of `file` that holds `item`, one per start line: its tokens under any reading, or its lines as written. */
function places(item: EvidenceText, file: FileAt): Range[] {
  const byStart = new Map<number, Range>();
  for (const r of file.readings ? occurrences(item.text, file.readings) : []) byStart.set(r.start, r);
  for (const start of linesAt(item.text, file.lines)) if (!byStart.has(start)) byStart.set(start, { start, end: start + item.end - item.start });
  return [...byStart.values()].sort((a, b) => a.start - b.start);
}

/** The longest word of a parse, the token a tree-wide search looks for: a word is written the same in every file that holds it, where a stored line is not once its spacing or separators changed. */
const searchWord = (parse: Token[]) => texts(parse).filter((t) => WORD.test(t[0]!)).reduce((a, b) => (b.length > a.length ? b : a), "");

/**
 * Every file at head where a substantial `item` sits, with the place nearest its old line, by one tree-wide search per
 * distinct longest word of its parses and a token comparison of each file found, or undefined when a path list was cut
 * at its cap and so names no place.
 */
async function treeHits(item: EvidenceText, head: HeadReader, filesOf: FilesOf): Promise<(Range & { path: string })[] | undefined> {
  const paths = new Set<string>();
  for (const word of new Set(VARIANTS.flatMap((v) => parses(item.text, v).map(searchWord)))) {
    const found = word === "" ? [] : await head.grep(word);
    if (found === undefined) return undefined;
    for (const path of found) paths.add(path);
  }
  const hits: (Range & { path: string })[] = [];
  for (const path of paths) {
    const file = await filesOf(path);
    const hit = file && nearest(places(item, file), item.start);
    if (hit) hits.push({ path, ...hit });
  }
  return hits;
}

/**
 * Search a new head for a finding's evidence text: the same path, then the head path of its rename, then once across
 * the tree when the text is substantial and exactly one file holds it. Every item found is `same`, with the anchor
 * shifted by the same offset as the evidence that contains it. Otherwise the finding is `changed` while its file (or
 * rename) still exists, and `file_gone` when neither does. `treeWide: false` skips the tree-wide search, for a finding
 * whose text an earlier head already searched for and missed.
 */
export async function locate(location: ObservedLocation, headSha: string, head: HeadReader, options: { treeWide: boolean } = { treeWide: true }): Promise<Located> {
  const filesOf = fileCache(head);
  const present = async (path: string) => {
    if (await filesOf(path)) return path;
    const renamed = head.renamedTo(path);
    return renamed !== undefined && (await filesOf(renamed)) ? renamed : undefined;
  };
  const findItem = async (item: EvidenceText): Promise<(Range & { path: string }) | undefined> => {
    const own = await present(item.path);
    const code = tokenize(item.text).length;
    if (own !== undefined) {
      const file = (await filesOf(own))!;
      const found = places(item, file);
      // Below the floor: found in place, since moving it would pin the finding to any `return x;` in the file. With no code
      // tokens at all, as in `}` or a `#field` line, also at the one place its lines sit, when there is exactly one.
      const inPlace = found.find((r) => r.start === item.start) ?? (code === 0 && found.length === 1 ? found[0] : undefined);
      const hit = code >= MIN_TOKENS ? nearest(found, item.start) : inPlace;
      if (hit) return { path: own, ...hit };
    }
    if (code < MIN_TOKENS || !options.treeWide) return undefined;
    const hits = (await treeHits(item, head, filesOf))?.filter((hit) => hit.path !== own);
    return hits?.length === 1 ? hits[0] : undefined;
  };

  const file = await present(location.file);
  const miss = (): Located => (file === undefined ? { kind: "file_gone" } : { kind: "changed", file });
  const found: EvidenceText[] = [];
  for (const item of location.evidence) {
    const hit = await findItem(item);
    if (!hit) return miss();
    found.push({ ...item, path: hit.path, start: hit.start, end: hit.end });
  }
  if (found.length === 0) return miss();
  const inFile = location.evidence.map((e, i) => i).filter((i) => location.evidence[i]!.path === location.file);
  const anchor = inFile.find((i) => location.newLine !== null && location.evidence[i]!.start <= location.newLine && location.newLine <= location.evidence[i]!.end) ?? inFile[0];
  if (anchor === undefined) return file === undefined ? miss() : { kind: "same", location: { ...location, file, headSha, evidence: found } };
  const newLine = location.newLine === null ? null : Math.max(1, location.newLine + found[anchor]!.start - location.evidence[anchor]!.start);
  return { kind: "same", location: { file: found[anchor]!.path, newLine, headSha, evidence: found } };
}

/** The file an open finding sits in at the new head, when it still has one. */
export const currentFile = (f: Pick<PriorFinding, "located">) => (f.located.kind === "same" ? f.located.location.file : f.located.kind === "changed" ? f.located.file : undefined);

/** Where a finding's own-file code still sits at this head (`found`, with the item and the range that holds it), or why code cannot say it is gone. */
export type Survival = { kind: "found"; item: EvidenceText; path: string; start: number; end: number } | { kind: "unknown"; reason: string };

/**
 * Where the finding's own-file code still sits at this head, or undefined when none of it does. `located` says only
 * that some item went missing, which a changed helper in another file or a short item shifted a line also does, so a
 * resolution check asks this instead: each own-file item is looked for anywhere in the finding's current file, by its
 * tokens under any reading or by its lines as written, and a substantial item anywhere in the tree unless
 * `treeWide: false`. A tree-wide search cut at its cap counts as found, since an open finding beats a false resolution.
 */
export async function survivingCode(f: Pick<PriorFinding, "location" | "located">, head: HeadReader, options: { treeWide: boolean } = { treeWide: true }): Promise<Survival | undefined> {
  const filesOf = fileCache(head);
  const file = currentFile(f);
  const at = file === undefined ? undefined : await filesOf(file);
  for (const item of f.location.evidence) {
    if (item.path !== f.location.file || item.text === "") continue;
    const hit = at && nearest(places(item, at), item.start);
    if (hit) return { kind: "found", item, path: file!, ...hit };
    if (tokenize(item.text).length < MIN_TOKENS || !options.treeWide) continue;
    const hits = await treeHits(item, head, filesOf);
    if (hits === undefined) return { kind: "unknown", reason: "in more files than the search lists" };
    const elsewhere = hits.find((h) => h.path !== file);
    if (elsewhere) return { kind: "found", item, ...elsewhere };
  }
  return undefined;
}

const overlaps = (a: EvidenceText[], b: EvidenceText[]) => a.some((x) => b.some((y) => x.path === y.path && x.start <= y.end && y.start <= x.end));

type Fingerprint = (c: Pick<CandidateFinding, "category" | "file" | "title">) => string;
/** A verified candidate with its evidence text, plus, when it names a prior finding, that finding's own code found in the candidate's file (`findOwnEvidence`). */
export type Report = { candidate: CandidateFinding; location: ObservedLocation; priorHits?: EvidenceText[] };

const HASH_MOD = 2147483647;
const HASH_BASE = 1 << 20;

/** The polynomial hash of every window of `length` over `ids`, by window start; products stay under 2^53. */
function windowHashes(ids: number[], length: number): number[] {
  const out: number[] = [];
  if (length === 0 || length > ids.length) return out;
  let top = 1;
  for (let k = 1; k < length; k++) top = (top * HASH_BASE) % HASH_MOD;
  let h = 0;
  for (let k = 0; k < length; k++) h = (h * HASH_BASE + ids[k]!) % HASH_MOD;
  out.push(h);
  for (let k = length; k < ids.length; k++) {
    h = ((h - ((ids[k - length]! * top) % HASH_MOD) + HASH_MOD) * HASH_BASE + ids[k]!) % HASH_MOD;
    out.push(h);
  }
  return out;
}

/** `a` and `b` as small integers under one numbering, so a window hash is cheap and a hit is confirmed by `equalAt`. */
function numbered(a: string[], b: string[]): [number[], number[]] {
  const ids = new Map<string, number>();
  const id = (t: string) => ids.get(t) ?? (ids.set(t, ids.size), ids.size - 1);
  return [a.map(id), b.map(id)];
}

const equalAt = (x: number[], o: number[], i: number, j: number, length: number) => {
  for (let k = 0; k < length; k++) if (x[i + k] !== o[j + k]) return false;
  return true;
};

/** The length of the longest run of tokens `a` and `b` share. */
const commonRun = (a: string[], b: string[]) => longestShared(...numbered(a, b));

/** The length of the longest run `x` and `y` share: halving on the length, since a shorter run exists whenever a longer one does. */
function longestShared(x: number[], y: number[]): number {
  const shares = (length: number) => {
    const at = new Map<number, number[]>();
    windowHashes(y, length).forEach((h, p) => {
      const positions = at.get(h);
      if (positions) positions.push(p);
      else at.set(h, [p]);
    });
    return windowHashes(x, length).some((h, o) => at.get(h)?.some((p) => equalAt(x, y, o, p, length)) ?? false);
  };
  let lo = 0;
  let hi = Math.min(x.length, y.length);
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (shares(mid)) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** A run of `length` tokens of `x` whose content occurs exactly once in `y`, as `[offset in x, position in y]`, or undefined. */
function uniqueRun(x: number[], y: number[], length: number): [number, number] | undefined {
  const count = new Map<number, number>();
  const first = new Map<number, number>();
  windowHashes(y, length).forEach((h, p) => {
    count.set(h, (count.get(h) ?? 0) + 1);
    if (!first.has(h)) first.set(h, p);
  });
  for (const [o, h] of windowHashes(x, length).entries()) {
    const p = first.get(h);
    if (p !== undefined && count.get(h) === 1 && equalAt(x, y, o, p, length)) return [o, p];
  }
  return undefined;
}

/** How many shorter lengths `findLongestRun` tries once the longest shared run occurs more than once, before it gives up. */
const SHORTER_RUNS = 64;

/**
 * The longest run of code tokens that `text` and `lines` share, as the 1-based line range of `lines` that holds it
 * and that range's text, when the run has MIN_TOKENS or more and occurs exactly once there, else the next shorter
 * such run. A cited block often spans the code that moved and the code that changed next to it; the part that
 * survived is still the finding's code. A run that occurs twice names no place. A run at either end of the text
 * takes the text's leading or trailing comment and blank lines with it. Each shorter length costs one pass over both
 * texts, so after SHORTER_RUNS of them the search gives up and names no place, which leaves the finding unlinked, as
 * does a file past MAX_FILE_CHARS.
 */
export function findLongestRun(text: string, lines: string[]): Omit<EvidenceText, "path"> | undefined {
  if (!substantial(text)) return undefined;
  const normalized = normalize(lines);
  if (normalized.length > MAX_FILE_CHARS) return undefined;
  const total = text.split("\n").length;
  for (const variant of VARIANTS) {
    const file = tokenize(normalized, variant);
    for (const parse of parses(text, variant)) {
      const [x, y] = numbered(texts(parse), texts(file));
      const longest = longestShared(x, y);
      for (let length = longest; length >= MIN_TOKENS && length > longest - SHORTER_RUNS; length--) {
        const once = uniqueRun(x, y, length);
        if (!once) continue;
        const [o, p] = once;
        const start = file[p]!.line - (o === 0 ? parse[0]!.line - 1 : 0);
        const end = file[p + length - 1]!.line + (o + length === parse.length ? total - parse.at(-1)!.line : 0);
        return { start, end, text: normalize(lines.slice(start - 1, end)) };
      }
    }
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
 * within LINK_DISTANCE of the finding's anchor at this head, or of its own text found in the candidate's file
 * (`priorHits`), and a cited range overlaps the finding's evidence at this head. That evidence is the located ranges
 * when the code was found, else `priorHits` plus the ranges last seen when the candidate reports the finding's file.
 * The name alone is never trusted: a nearby report that cites none of the finding's lines is a new defect, and so is
 * one on its lines from far away. The finding's file, when it still exists, must be the candidate's.
 */
function namedLink(candidate: CandidateFinding, cited: EvidenceText[], f: PriorFinding, priorHits: EvidenceText[]): boolean {
  const file = currentFile(f);
  if (file !== undefined && file !== candidate.file) return false;
  if (candidate.newLine === null) return false;
  const evidence = file === undefined ? priorHits : [...ownEvidence(f), ...priorHits];
  const lastLine = f.located.kind === "same" ? f.located.location.newLine : file === undefined ? null : f.location.newLine;
  const line = candidate.newLine;
  const nearAnchor = (lastLine !== null && Math.abs(line - lastLine) <= LINK_DISTANCE) || priorHits.some((h) => h.start - LINK_DISTANCE <= line && line <= h.end + LINK_DISTANCE);
  return nearAnchor && overlaps(cited, evidence);
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
  // The first claim on a finding was judged on its merits; a later one is refused as its duplicate and must not outvote it.
  const verdicts = new Map<string, ResolutionVerdict["state"]>();
  for (const v of claims.verdicts) if (!verdicts.has(v.findingId)) verdicts.set(v.findingId, v.state);
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
