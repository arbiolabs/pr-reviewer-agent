import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { PullInfo } from "../domain/types.ts";

export type PullRef = { owner: string; repo: string; number: number };

export interface PullSource {
  getPull(ref: PullRef): Promise<PullInfo>;
}

export function parsePullUrl(url: string): PullRef {
  const match = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)\/?$/.exec(url.trim());
  if (!match) throw new Error(`not a GitHub pull request URL: ${url}`);
  return { owner: match[1]!, repo: match[2]!, number: Number(match[3]) };
}

export function parsePullRef(ref: string): PullRef {
  const match = /^([\w.-]+)\/([\w.-]+)#(\d+)$/.exec(ref.trim());
  if (!match) throw new Error(`expected owner/repo#number, got: ${ref}`);
  return { owner: match[1]!, repo: match[2]!, number: Number(match[3]) };
}

/** `GITHUB_TOKEN`, else `gh auth token`, else none (public repositories only, at a low rate limit). */
export async function resolveGitHubToken(): Promise<string | undefined> {
  if (process.env.GITHUB_TOKEN) return process.env.GITHUB_TOKEN;
  try {
    const { stdout } = await promisify(execFile)("gh", ["auth", "token"], { timeout: 5000 });
    return stdout.trim() || undefined;
  } catch {
    return undefined;
  }
}

type GitHubPull = {
  number: number;
  title: string;
  body: string | null;
  state: "open" | "closed";
  merged: boolean;
  head: { sha: string };
  base: { sha: string; repo: { id: number; full_name: string; clone_url: string; private: boolean } };
};

export class GitHubPullSource implements PullSource {
  private readonly token: string | undefined;

  constructor(token: string | undefined) {
    this.token = token;
  }

  async getPull(ref: PullRef): Promise<PullInfo> {
    const response = await fetch(`https://api.github.com/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}`, {
      headers: {
        accept: "application/vnd.github+json",
        "x-github-api-version": "2022-11-28",
        "user-agent": "pr-review-m1",
        ...(this.token ? { authorization: `Bearer ${this.token}` } : {}),
      },
      signal: AbortSignal.timeout(15_000),
    });
    if (response.status === 401 || response.status === 403 || response.status === 404) {
      throw new Error(`cannot read ${ref.owner}/${ref.repo}#${ref.number}: HTTP ${response.status} (${this.token ? "token lacks access" : "no GitHub token; set GITHUB_TOKEN or run gh auth login"})`);
    }
    if (!response.ok) throw new Error(`GitHub API error ${response.status} for ${ref.owner}/${ref.repo}#${ref.number}`);
    const pull = (await response.json()) as GitHubPull;
    return {
      installationId: 0,
      repositoryId: pull.base.repo.id,
      fullName: pull.base.repo.full_name,
      number: pull.number,
      title: pull.title,
      body: pull.body ?? "",
      baseSha: pull.base.sha,
      headSha: pull.head.sha,
      state: pull.state,
      merged: pull.merged,
      private: pull.base.repo.private,
      cloneUrl: pull.base.repo.clone_url,
    };
  }
}
