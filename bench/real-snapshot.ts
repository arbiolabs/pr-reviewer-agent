import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { type HumanComment, type Manifest, type PullMeta, skipReason } from "./real-score.ts";

/** Every GitHub call the snapshot makes goes through here, and here is always a GET. */
function ghGet<T>(path: string, paginate = false): T {
  const args = ["api", "-X", "GET", path, ...(paginate ? ["--paginate", "--jq", ".[]"] : [])];
  const out = execFileSync("gh", args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  return (paginate ? out.split("\n").filter((l) => l !== "").map((l) => JSON.parse(l)) : JSON.parse(out)) as T;
}

type GhPull = {
  number: number;
  draft: boolean;
  title: string;
  body: string | null;
  state: "open" | "closed";
  merged: boolean;
  user: { login: string; type: string };
  head: { sha: string; ref: string };
  base: { sha: string; ref: string; repo: { id: number; full_name: string; private: boolean } };
  changed_files: number;
  additions: number;
  deletions: number;
};

type GhUser = { login: string; type: string } | null;
type GhComment = { id: number; path: string; line?: number | null; side?: string | null; original_line?: number | null; commit_id: string; original_commit_id: string; in_reply_to_id?: number; user: GhUser; body: string };
type GhReview = { id: number; user: GhUser; state: string; commit_id: string; submitted_at: string; body: string };

const writeJson = (path: string, value: unknown) => writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);

const argv = process.argv.slice(2);
const { values } = parseArgs({
  args: argv[0] === "--" ? argv.slice(1) : argv,
  options: { repo: { type: "string" }, pr: { type: "string", multiple: true }, limit: { type: "string" }, dataset: { type: "string" } },
});
if (!values.repo || !/^[\w.-]+\/[\w.-]+$/.test(values.repo)) {
  console.error("usage: pnpm eval:snapshot -- --repo owner/name [--pr N]... [--limit N] [--dataset DIR]");
  process.exit(2);
}
const repo = values.repo;
const dataset = resolve(values.dataset ?? join(".data/evals", repo.split("/")[1]!));
mkdirSync(join(dataset, "prs"), { recursive: true });
try {
  execFileSync("git", ["-C", dataset, "check-ignore", "-q", dataset], { stdio: "ignore" });
} catch {
  throw new Error(`${dataset} is not gitignored by the checkout that holds it; refusing to write private data there`);
}

const numbers = values.pr?.map(Number) ?? ghGet<{ number: number }[]>(`/repos/${repo}/pulls?state=open&per_page=100`, true).map((p) => p.number);
const manifestPath = join(dataset, "manifest.json");
const previous = existsSync(manifestPath) ? (JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest) : undefined;
const kept = new Map((previous?.prs ?? []).map((p) => [p.number, p]));
const skipped: { number: number; reason: string }[] = [];
const limit = values.limit ? Number(values.limit) : Infinity;

for (const number of numbers.sort((a, b) => b - a)) {
  const dir = join(dataset, "prs", String(number));
  if (kept.has(number) && existsSync(join(dir, "pull.json"))) continue;
  if (kept.size >= limit) break;
  const p = ghGet<GhPull>(`/repos/${repo}/pulls/${number}`);
  const meta: PullMeta = { number, draft: p.draft, title: p.title, author: p.user.login, authorType: p.user.type, changedFiles: p.changed_files, additions: p.additions, deletions: p.deletions };
  const reason = p.state === "open" ? skipReason(meta) : "not open";
  if (reason) {
    skipped.push({ number, reason });
    continue;
  }
  mkdirSync(dir, { recursive: true });
  const comments = ghGet<GhComment[]>(`/repos/${repo}/pulls/${number}/comments?per_page=100`, true).map((c): HumanComment => ({
    id: c.id,
    path: c.path,
    line: c.line ?? null,
    side: c.side ?? null,
    original_line: c.original_line ?? null,
    commit_id: c.commit_id,
    original_commit_id: c.original_commit_id,
    in_reply_to_id: c.in_reply_to_id ?? null,
    user: { login: c.user?.login ?? "", type: c.user?.type ?? "" },
    body: c.body,
  }));
  const reviews = ghGet<GhReview[]>(`/repos/${repo}/pulls/${number}/reviews?per_page=100`, true).map((r) => ({
    id: r.id,
    user: { login: r.user?.login ?? "", type: r.user?.type ?? "" },
    state: r.state,
    commit_id: r.commit_id,
    submitted_at: r.submitted_at,
    body: r.body,
  }));
  writeJson(join(dir, "comments.json"), comments);
  writeJson(join(dir, "reviews.json"), reviews);
  // pull.json goes last: its presence marks the PR as fully snapshotted.
  writeJson(join(dir, "pull.json"), {
    ...meta,
    fullName: p.base.repo.full_name,
    repositoryId: p.base.repo.id,
    private: p.base.repo.private,
    state: p.state,
    merged: p.merged,
    body: p.body ?? "",
    baseSha: p.base.sha,
    headSha: p.head.sha,
    baseRef: p.base.ref,
    headRef: p.head.ref,
  });
  kept.set(number, { number, baseSha: p.base.sha, headSha: p.head.sha, changedFiles: p.changed_files, additions: p.additions, deletions: p.deletions });
  console.error(`kept #${number} (${p.changed_files} files, +${p.additions}/-${p.deletions})`);
}

// A full bare clone, unlike the reviewer's blob-less one, so a review reads every blob without the network.
const gitDir = join(dataset, "repo.git");
const token = execFileSync("gh", ["auth", "token"], { encoding: "utf8" }).trim();
const auth = ["-c", `http.extraHeader=Authorization: Basic ${Buffer.from(`x-access-token:${token}`).toString("base64")}`];
const git = (...args: string[]) => execFileSync("git", [...auth, "-C", gitDir, ...args], { encoding: "utf8", env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } }).trim();
if (!existsSync(join(gitDir, "HEAD"))) execFileSync("git", [...auth, "clone", "--bare", "--quiet", `https://github.com/${repo}.git`, gitDir], { stdio: "inherit", env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } });
const has = (sha: string) => {
  try {
    git("cat-file", "-e", `${sha}^{commit}`);
    return true;
  } catch {
    return false;
  }
};
const prs = [...kept.values()].sort((a, b) => a.number - b.number);
git("fetch", "--quiet", "origin", "+refs/heads/*:refs/heads/*", ...prs.map((p) => `+refs/pull/${p.number}/head:refs/pull/${p.number}/head`));
for (const p of prs) {
  for (const sha of [p.baseSha, p.headSha]) if (!has(sha)) git("fetch", "--quiet", "origin", sha);
  for (const sha of [p.baseSha, p.headSha]) if (!has(sha)) throw new Error(`#${p.number}: commit ${sha} is missing from ${gitDir}`);
  // The reviewer fetches refs/pull/N/head; pin it to the snapshot head even if the PR moved since.
  git("update-ref", `refs/pull/${p.number}/head`, p.headSha);
}

const allSkipped = [...skipped, ...(previous?.skipped ?? []).filter((s) => !skipped.some((t) => t.number === s.number) && !kept.has(s.number))].sort((a, b) => a.number - b.number);
writeJson(manifestPath, { repo, snapshotAt: previous?.snapshotAt ?? new Date().toISOString(), updatedAt: new Date().toISOString(), prs, skipped: allSkipped });
console.error(`${prs.length} PRs kept, ${allSkipped.length} skipped; manifest at ${manifestPath}`);
