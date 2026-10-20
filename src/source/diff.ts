export type FileDiff = {
  path: string;
  text: string;
  /** Head-side line numbers a review comment can anchor to: added and context lines of a hunk. */
  headLines: Set<number>;
};

/** Split `git diff` output by file, keyed by head-side path. Deleted files have no head side and are left out. */
export function parseDiff(diff: string): Map<string, FileDiff> {
  const files = new Map<string, FileDiff>();
  for (const chunk of diff.split(/^(?=diff --git )/m)) {
    if (!chunk.startsWith("diff --git ")) continue;
    const lines = chunk.split("\n");
    const plus = lines.find((line) => line.startsWith("+++ "));
    const renameTo = lines.find((line) => line.startsWith("rename to "));
    const path = plus?.startsWith("+++ b/") ? plus.slice(6) : renameTo?.slice(10);
    if (path === undefined) continue;
    const headLines = new Set<number>();
    let head = 0;
    let inHunk = false;
    for (const line of lines) {
      const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      if (hunk) {
        head = Number(hunk[1]);
        inHunk = true;
      } else if (inHunk && (line.startsWith("+") || line.startsWith(" "))) {
        headLines.add(head++);
      }
    }
    files.set(path, { path, text: chunk, headLines });
  }
  return files;
}
