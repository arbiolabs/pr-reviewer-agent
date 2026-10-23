import { execFile } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import type { ChangedFile } from "../domain/types.ts";

const run = promisify(execFile);

// Literal pathspecs: a model-supplied scope like ":(top)" or ":!x" cannot turn on pathspec magic.
const GIT_ENV = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_LITERAL_PATHSPECS: "1" };

export const DEFAULT_OUTPUT_CAP = 64 * 1024;

// A blob-less clone fetches a missing blob from origin on demand; readers that must stay off the network turn that off.
const OFFLINE_ENV = { ...GIT_ENV, GIT_NO_LAZY_FETCH: "1" };

/** Git ignores GIT_NO_LAZY_FETCH before 2.45, so older versions would lazy-fetch blobs from origin despite it. */
export const OFFLINE_GIT_MIN = [2, 45] as const;

let offlineCheck: Promise<boolean> | undefined;
/** Whether this machine's git honours GIT_NO_LAZY_FETCH. Asked once per process. */
export function offlineReadsSupported(): Promise<boolean> {
  offlineCheck ??= run("git", ["--version"], { env: GIT_ENV, encoding: "utf8" }).then(
    ({ stdout }) => gitVersionAtLeast(stdout, OFFLINE_GIT_MIN),
    () => false,
  );
  return offlineCheck;
}

export function gitVersionAtLeast(versionOutput: string, [major, minor]: readonly [number, number]): boolean {
  const m = /git version (\d+)\.(\d+)/.exec(versionOutput);
  if (!m) return false;
  const [have, haveMinor] = [Number(m[1]), Number(m[2])];
  return have > major || (have === major && haveMinor >= minor);
}

/** True when git failed only because a blob is not in the local clone. */
export function isMissingObject(error: unknown): boolean {
  return /bad object|unable to read|could not read|missing blob|promisor/i.test(String((error as { stderr?: unknown }).stderr ?? ""));
}

/** The most output one git command may produce; past it the command is killed and `ERR_CHILD_PROCESS_STDIO_MAXBUFFER` thrown. */
const MAX_OUTPUT_BYTES = 64 * 1024 * 1024;

/** `signal` kills git when it aborts: a search the model asked for must not outlive the review that asked. */
async function git(dir: string, args: string[], maxBuffer = MAX_OUTPUT_BYTES, env = GIT_ENV, signal?: AbortSignal): Promise<string> {
  const { stdout } = await run("git", ["-C", dir, "-c", "core.quotePath=false", ...args], { env, maxBuffer, encoding: "utf8", signal });
  return stdout;
}

/** `git` with `input` on its standard input. */
async function gitInput(dir: string, args: string[], input: string, env = GIT_ENV, signal?: AbortSignal): Promise<string> {
  const pending = run("git", ["-C", dir, "-c", "core.quotePath=false", ...args], { env, maxBuffer: MAX_OUTPUT_BYTES, encoding: "utf8", signal });
  pending.child.stdin?.end(input);
  return (await pending).stdout;
}

/** Object ids per `git fetch --stdin`: one round trip each, small enough for any server. */
const PREFETCH_BATCH = 20_000;

export class UnsafePathError extends Error {}

/** A blob larger than the `maxBytes` its reader allowed: git was stopped at that many bytes, so none of it was read. */
export class BlobTooLargeError extends Error {}

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
 * Bare, blob-less clone per repository, refreshed with every branch head and the PR's head ref. A search fetches the
 * blobs it needs first, in one batch (PinnedRepo.prefetch).
 */
export async function syncRepo(cacheDir: string, fullName: string, cloneUrl: string, prNumber: number, token?: string): Promise<string> {
  const dir = join(cacheDir, `${fullName}.git`);
  // GitHub's git endpoint rejects `Bearer` for user tokens and takes Basic x-access-token. `-c` before the subcommand
  // lives only for this command: not in the environment, not in the bare repo's config.
  const basic = token === undefined ? undefined : Buffer.from(`x-access-token:${token}`).toString("base64");
  const auth = basic === undefined ? [] : ["-c", `http.extraHeader=Authorization: Basic ${basic}`];
  try {
    if (!existsSync(join(dir, "HEAD"))) {
      mkdirSync(dirname(dir), { recursive: true });
      await run("git", [...auth, "clone", "--bare", "--filter=blob:none", "--quiet", cloneUrl, dir], { env: GIT_ENV });
    }
    await git(dir, [...auth, "fetch", "--quiet", "--filter=blob:none", "origin", "+refs/heads/*:refs/heads/*", `+refs/pull/${prNumber}/head:refs/pull/${prNumber}/head`]);
  } catch (error) {
    // execFile's message echoes the command line; rethrow a plain Error so no field still carries the header.
    let message = error instanceof Error ? error.message : String(error);
    for (const secret of [token, basic]) if (secret) message = message.replaceAll(secret, "***");
    throw new Error(message);
  }
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

  /** Head-tree scopes whose blobs are local; "" is the whole tree. */
  private readonly prefetched = new Set<string>();

  /**
   * Fetches the head tree's missing blobs under `scope` in one batch. Without it, git grep on the blob-less clone
   * fetches each missing blob in its own round trip, so a search over a large directory took half an hour. Best
   * effort: a failure leaves grep to fetch lazily, as before. A git that would ignore GIT_NO_LAZY_FETCH skips it, since
   * listing what is missing would then fetch it blob by blob.
   */
  async prefetch(scope: string | undefined, signal?: AbortSignal): Promise<void> {
    const key = scope === undefined || scope === "." ? "" : assertSafePath(scope);
    if (this.prefetched.has("") || this.prefetched.has(key) || !(await offlineReadsSupported())) return;
    try {
      const listed = await git(this.dir, ["ls-tree", "-r", "-z", "--format=%(objecttype) %(objectname)", this.headSha, ...(key === "" ? [] : ["--", key])], undefined, OFFLINE_ENV, signal);
      const blobs = listed.split("\0").flatMap((entry) => (entry.startsWith("blob ") ? [entry.slice(5)] : []));
      const missing =
        blobs.length === 0
          ? []
          : (await gitInput(this.dir, ["cat-file", "--batch-check=%(objectname)"], `${blobs.join("\n")}\n`, OFFLINE_ENV, signal))
              .split("\n")
              .flatMap((line) => (line.endsWith(" missing") ? [line.slice(0, -" missing".length)] : []));
      for (let i = 0; i < missing.length; i += PREFETCH_BATCH) {
        const batch = missing.slice(i, i + PREFETCH_BATCH);
        // The fetch git itself runs for one missing blob, given every id at once.
        const args = ["-c", "fetch.negotiationAlgorithm=noop", "fetch", "--quiet", "origin", "--no-tags", "--no-write-fetch-head", "--recurse-submodules=no", "--filter=blob:none", "--stdin"];
        await gitInput(this.dir, args, `${batch.join("\n")}\n`, GIT_ENV, signal);
      }
      this.prefetched.add(key);
    } catch (error) {
      if ((error as { name?: string }).name === "AbortError") throw error;
    }
  }

  async assertCommitsPresent(): Promise<void> {
    for (const sha of [this.baseSha, this.headSha]) await git(this.dir, ["cat-file", "-e", `${sha}^{commit}`]);
  }

  /** `offline` never fetches a blob the clone lacks: git fails instead. */
  diff(path?: string, options: { offline?: boolean } = {}): Promise<string> {
    const args = ["diff", "--no-color", "--no-ext-diff", "-M", `${this.baseSha}...${this.headSha}`];
    return git(this.dir, path === undefined ? args : [...args, "--", assertSafePath(path)], undefined, options.offline ? OFFLINE_ENV : GIT_ENV);
  }

  async changedFiles(): Promise<ChangedFile[]> {
    const fields = (await git(this.dir, ["diff", "--name-status", "-z", "-M", `${this.baseSha}...${this.headSha}`])).split("\0");
    const files: ChangedFile[] = [];
    for (let i = 0; i < fields.length - 1; ) {
      const status = fields[i++]!;
      const from = status.startsWith("R") || status.startsWith("C") ? fields[i++] : undefined;
      files.push({ path: fields[i++]!, status: status[0]!, ...(from === undefined ? {} : { from }) });
    }
    return files;
  }

  /**
   * `undefined` when the path or object does not exist at `sha`; any other git failure throws so the caller retries.
   * `offline` never fetches a blob the clone lacks: git fails with "bad object" instead. A blob over `maxBytes`
   * (MAX_OUTPUT_BYTES by default) throws BlobTooLargeError once that many bytes have arrived, so a huge file costs at most the cap.
   */
  async show(sha: string, path: string, options: { offline?: boolean; maxBytes?: number; signal?: AbortSignal } = {}): Promise<string | undefined> {
    const safe = assertSafePath(path);
    try {
      return await git(this.dir, ["show", `${sha}:${safe}`], options.maxBytes, options.offline ? OFFLINE_ENV : GIT_ENV, options.signal);
    } catch (error) {
      const { code, stderr } = error as { code?: unknown; stderr?: unknown };
      if (code === 128 && /does not exist|exists on disk, but not in|invalid object name/.test(String(stderr))) return undefined;
      if (code === "ERR_CHILD_PROCESS_STDIO_MAXBUFFER") throw new BlobTooLargeError(`${sha}:${safe} is over ${options.maxBytes ?? MAX_OUTPUT_BYTES} bytes`);
      throw error;
    }
  }

  async grep(query: string, scope: string | undefined, cap = DEFAULT_OUTPUT_CAP, signal?: AbortSignal): Promise<string> {
    const args = ["grep", "-n", "-I", "-F", "--max-count=50", "-e", query, this.headSha];
    if (scope !== undefined && scope !== "" && scope !== ".") args.push("--", assertSafePath(scope));
    await this.prefetch(scope === "" ? undefined : scope, signal);
    try {
      const out = await git(this.dir, args, undefined, undefined, signal);
      return capBytes(out.replaceAll(`${this.headSha}:`, ""), cap);
    } catch (error) {
      if ((error as { code?: number }).code === 1) return "";
      throw error;
    }
  }

  /**
   * The head paths with a line that contains `query`, NUL-delimited by git so a colon in a path survives, or undefined
   * when the list runs past `cap` bytes: that many files hold the line, so no caller can call one of them unique.
   * Files git calls binary, such as one holding a NUL byte, are searched too (`-a`): code moved into one must still be
   * found, or a resolution claim on it would pass.
   */
  async grepPaths(query: string, cap = DEFAULT_OUTPUT_CAP): Promise<string[] | undefined> {
    await this.prefetch(undefined);
    try {
      const out = await git(this.dir, ["grep", "-l", "-z", "-a", "-F", "-e", query, this.headSha]);
      if (Buffer.byteLength(out) > cap) return undefined;
      return out.split("\0").filter((p) => p !== "").map((p) => p.slice(this.headSha.length + 1));
    } catch (error) {
      if ((error as { code?: number }).code === 1) return [];
      throw error;
    }
  }

  log(path: string, limit: number, signal?: AbortSignal): Promise<string> {
    const args = ["log", `-n${limit}`, "--format=%h %ad %an%n  %s", "--date=short", this.headSha, "--", assertSafePath(path)];
    return git(this.dir, args, undefined, undefined, signal);
  }
}
