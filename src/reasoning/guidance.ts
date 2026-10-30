import { matchesGlob } from "node:path";
import { BlobTooLargeError } from "../source/git.ts";

/** A guidance file is read at most this large, and all of them together are cut to this many lines. */
export const MAX_GUIDANCE_BYTES = 64 * 1024;
export const MAX_GUIDANCE_LINES = 500;

export type Guidance = { path: string; text: string; truncated: boolean };

/**
 * The repository's own instructions, read through `read` (the base commit, so a pull request cannot rewrite the rules
 * it is reviewed by), in `files` order. A file that repeats one already read, or a one-line symlink target such as a
 * `CLAUDE.md` pointing at `AGENTS.md`, is left out; the rest share `maxLines`.
 */
export async function loadGuidance(read: (path: string) => Promise<string | undefined>, files: string[], maxLines = MAX_GUIDANCE_LINES): Promise<Guidance[]> {
  const out: Guidance[] = [];
  let room = maxLines;
  for (const path of files) {
    if (room <= 0) break;
    const text = await read(path).catch((error: unknown) => {
      if (error instanceof BlobTooLargeError) return undefined;
      throw error;
    });
    const body = text?.trim();
    if (!body || /^\S+$/.test(body) || out.some((g) => g.text.trim() === body)) continue;
    const lines = body.split("\n");
    out.push({ path, text: lines.slice(0, room).join("\n"), truncated: lines.length > room });
    room -= Math.min(lines.length, room);
  }
  return out;
}

/** A lock file, generated code or vendored code, by `review.generated_paths`: listed to the reviewer, not reviewed. */
export const isGenerated = (path: string, globs: string[]) => globs.some((glob) => matchesGlob(path, glob));
