import { execFile } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import type { ChangedFile } from "../domain/types.ts";

const run = promisify(execFile);

// Literal pathspecs: a model-supplied scope like ":(top)" or ":!x" cannot turn on pathspec magic.
const GIT_ENV = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_LITERAL_PATHSPECS: "1" };

export const DEFAULT_OUTPUT_CAP = 64 * 1024;

async function git(dir: string, args: string[], maxBuffer = 64 * 1024 * 1024): Promise<string> {
  const { stdout } = await run("git", ["-C", dir, "-c", "core.quotePath=false", ...args], { env: GIT_ENV, maxBuffer, encoding: "utf8" });
  return stdout;
}

export class UnsafePathError extends Error {}

/** Paths come from the model or from repository content; keep them relative and inside the tree. */
export function assertSafePath(path: string): string {
  if (path === "" || path.includes("\0")) throw new UnsafePathError("path must be non-empty and contain no NUL");
  if (path.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith("\\")) throw new UnsafePathError(`absolute path rejected: ${path}`);
  if (path.split(/[\\/]/).includes("..")) throw new UnsafePathError(`path traversal rejected: ${path}`);
  return path.replace(/^\.\//, "");
}

export function capBytes(text: string, cap = DEFAULT_OUTPUT_CAP): string {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= cap) return text;
  return `${bytes.subarray(0, cap).toString("utf8")}\n[output truncated at ${cap} bytes]`;
}

/**
 * Bare, blob-less clone per repository, refreshed with every branch head and the PR's head ref.
 * ponytail: blob:none fetches blobs lazily one round trip at a time, so search_code over a large tree is slow;
 * prefetch the head tree's blobs if that becomes the bottleneck.
 */
export async function syncRepo(cacheDir: string, fullName: string, cloneUrl: string, prNumber: number): Promise<string> {
  const dir = join(cacheDir, `${fullName}.git`);
  if (!existsSync(join(dir, "HEAD"))) {
    mkdirSync(dirname(dir), { recursive: true });
    await run("git", ["clone", "--bare", "--filter=blob:none", "--quiet", cloneUrl, dir], { env: GIT_ENV });
  }
  await git(dir, ["fetch", "--quiet", "--filter=blob:none", "origin", "+refs/heads/*:refs/heads/*", `+refs/pull/${prNumber}/head:refs/pull/${prNumber}/head`]);
  return dir;
}

/** Read-only access to one repository pinned at a base and head SHA. Nothing here reads a working tree. */
export class PinnedRepo {
  readonly dir: string;
  readonly baseSha: string;
  readonly headSha: string;

  constructor(dir: string, baseSha: string, headSha: string) {
    this.dir = dir;
    this.baseSha = baseSha;
    this.headSha = headSha;
  }

  async assertCommitsPresent(): Promise<void> {
    for (const sha of [this.baseSha, this.headSha]) await git(this.dir, ["cat-file", "-e", `${sha}^{commit}`]);
  }

  diff(path?: string): Promise<string> {
    const args = ["diff", "--no-color", "--no-ext-diff", "-M", `${this.baseSha}...${this.headSha}`];
    return git(this.dir, path === undefined ? args : [...args, "--", assertSafePath(path)]);
  }

  async changedFiles(): Promise<ChangedFile[]> {
    const fields = (await git(this.dir, ["diff", "--name-status", "-z", "-M", `${this.baseSha}...${this.headSha}`])).split("\0");
    const files: ChangedFile[] = [];
    for (let i = 0; i < fields.length - 1; ) {
      const status = fields[i++]!;
      if (status.startsWith("R") || status.startsWith("C")) i++;
      files.push({ path: fields[i++]!, status: status[0]! });
    }
    return files;
  }

  /** `undefined` when the path does not exist at `sha`. */
  async show(sha: string, path: string): Promise<string | undefined> {
    try {
      return await git(this.dir, ["show", `${sha}:${assertSafePath(path)}`]);
    } catch (error) {
      if (error instanceof UnsafePathError) throw error;
      return undefined;
    }
  }

  async lineCount(sha: string, path: string): Promise<number | undefined> {
    const text = await this.show(sha, path);
    if (text === undefined) return undefined;
    return text === "" ? 0 : text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
  }

  async grep(query: string, scope: string | undefined, cap = DEFAULT_OUTPUT_CAP): Promise<string> {
    const args = ["grep", "-n", "-I", "-F", "--max-count=50", "-e", query, this.headSha];
    if (scope !== undefined && scope !== "" && scope !== ".") args.push("--", assertSafePath(scope));
    try {
      const out = await git(this.dir, args);
      return capBytes(out.replaceAll(`${this.headSha}:`, ""), cap);
    } catch (error) {
      if ((error as { code?: number }).code === 1) return "";
      throw error;
    }
  }

  log(path: string, limit: number): Promise<string> {
    return git(this.dir, ["log", `-n${limit}`, "--format=%h %ad %an%n  %s", "--date=short", this.headSha, "--", assertSafePath(path)]);
  }
}
