import type { DiffFile } from "../protocol/v1.ts";
import { ControlError } from "./source.ts";

type Hunk = DiffFile["hunks"][number];
type Line = Hunk["lines"][number];

/** Keeps one diff.page well under the protocol's 1 MiB line cap once JSON-encoded. */
export const PAGE_BYTES = 512 * 1024;
export const DEFAULT_FILES_PER_PAGE = 100;

const HUNK_HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** Parse `git diff` output into protocol files. Binary files keep their metadata and get no hunks. */
export function parseDiffFiles(raw: string): DiffFile[] {
  const files: DiffFile[] = [];
  for (const chunk of raw.split(/^(?=diff --git )/m)) {
    if (!chunk.startsWith("diff --git ")) continue;
    const lines = chunk.split("\n");
    const firstHunk = lines.findIndex((l) => l.startsWith("@@"));
    const header = firstHunk === -1 ? lines : lines.slice(0, firstHunk);
    const field = (prefix: string) => header.find((l) => l.startsWith(prefix))?.slice(prefix.length);
    const plus = field("+++ b/");
    const minus = field("--- a/");
    const renamedFrom = field("rename from ");
    // ponytail: paths git quotes (tabs, quotes, newlines) keep their quoting; decode them if such repos show up.
    const fromGitLine = lines[0]!.slice(lines[0]!.lastIndexOf(" b/") + 3);
    const path = plus ?? field("rename to ") ?? minus ?? fromGitLine;
    const binary = header.some((l) => /^Binary files .* differ$/.test(l));
    const status: DiffFile["status"] = binary
      ? "binary"
      : renamedFrom !== undefined
        ? "renamed"
        : field("new file mode") !== undefined
          ? "added"
          : field("deleted file mode") !== undefined
            ? "deleted"
            : "modified";
    const file: DiffFile = { path, status, additions: 0, deletions: 0, hunks: [], truncated: false };
    if (renamedFrom !== undefined) file.oldPath = renamedFrom;
    let hunk: Hunk | undefined;
    let oldLine = 0;
    let newLine = 0;
    for (const line of lines) {
      const h = HUNK_HEADER.exec(line);
      if (h) {
        oldLine = Number(h[1]);
        newLine = Number(h[2]);
        hunk = { header: line, oldStart: oldLine, newStart: newLine, lines: [] };
        file.hunks.push(hunk);
      } else if (hunk && line.startsWith("+")) {
        hunk.lines.push({ kind: "add", new: newLine++, text: line.slice(1) });
        file.additions++;
      } else if (hunk && line.startsWith("-")) {
        hunk.lines.push({ kind: "del", old: oldLine++, text: line.slice(1) });
        file.deletions++;
      } else if (hunk && line.startsWith(" ")) {
        hunk.lines.push({ kind: "context", old: oldLine++, new: newLine++, text: line.slice(1) });
      }
    }
    files.push(file);
  }
  return files;
}

const size = (value: unknown) => Buffer.byteLength(JSON.stringify(value));

export type DiffPage = { files: DiffFile[]; nextCursor?: string; truncated: boolean };

/**
 * One page: at most `limit` files and `maxBytes` of hunks. The cursor is `<file>.<hunk>`, so a file split across
 * pages appears on both with `truncated` set on the first. A single hunk larger than a page keeps the lines that fit
 * and drops the rest.
 */
export function pageDiff(all: DiffFile[], options: { file?: string | undefined; cursor?: string | undefined; limit?: number | undefined }, maxBytes = PAGE_BYTES): DiffPage {
  const files = options.file === undefined ? all : all.filter((f) => f.path === options.file || f.oldPath === options.file);
  const limit = options.limit ?? DEFAULT_FILES_PER_PAGE;
  let [fi, hi] = parseCursor(options.cursor);
  const out: DiffFile[] = [];
  let budget = maxBytes;
  const page = (next?: string): DiffPage => (next === undefined ? { files: out, truncated: false } : { files: out, nextCursor: next, truncated: true });
  for (; fi < files.length; fi++, hi = 0) {
    if (out.length === limit) return page(`${fi}.${hi}`);
    const file = files[fi]!;
    const entry: DiffFile = { ...file, hunks: [], truncated: false };
    out.push(entry);
    for (; hi < file.hunks.length; hi++) {
      const hunk = file.hunks[hi]!;
      const bytes = size(hunk);
      if (bytes <= budget) {
        entry.hunks.push(hunk);
        budget -= bytes;
        continue;
      }
      if (entry.hunks.length > 0 || out.length > 1) {
        if (entry.hunks.length === 0) out.pop();
        else entry.truncated = true;
        return page(`${fi}.${hi}`);
      }
      entry.hunks.push(cutHunk(hunk, budget));
      entry.truncated = true;
      const next = hi + 1 < file.hunks.length ? `${fi}.${hi + 1}` : fi + 1 < files.length ? `${fi + 1}.0` : undefined;
      return page(next);
    }
  }
  return page();
}

function cutHunk(hunk: Hunk, budget: number): Hunk {
  const lines: Line[] = [];
  let left = budget - size({ ...hunk, lines: [] });
  for (const line of hunk.lines) {
    left -= size(line) + 1;
    if (left < 0) break;
    lines.push(line);
  }
  return { ...hunk, lines };
}

function parseCursor(cursor: string | undefined): [number, number] {
  if (cursor === undefined) return [0, 0];
  const m = /^(\d+)\.(\d+)$/.exec(cursor);
  if (!m) throw new ControlError("INVALID_REQUEST", `bad diff cursor ${cursor}`);
  return [Number(m[1]), Number(m[2])];
}
