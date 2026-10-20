import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { PullInfo } from "../domain/types.ts";
import { type GitHubClient, GitHubError } from "./client.ts";

/** `installationId` picks the GitHub App installation to read through; absent means token auth. */
export type PullRef = { owner: string; repo: string; number: number; installationId?: number };

export interface PullSource {
  getPull(ref: PullRef, options?: { maxSleepMs?: number }): Promise<PullInfo>;
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

/** Reads PRs through the client for the ref's installation (installation 0 is token auth). */
export class GitHubPullSource implements PullSource {
  private readonly clientFor: (installationId: number) => GitHubClient;

  constructor(clientFor: (installationId: number) => GitHubClient) {
    this.clientFor = clientFor;
  }

  async getPull(ref: PullRef, options: { maxSleepMs?: number } = {}): Promise<PullInfo> {
    const installationId = ref.installationId ?? 0;
    const client = this.clientFor(installationId);
    let pull: GitHubPull;
    try {
      pull = (await client.request<GitHubPull>("GET", `/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}`, options)).data;
    } catch (error) {
      // A rate-limited 403 is retryable and passes through; only a definite refusal is reworded, and stays fatal.
      if (error instanceof GitHubError && error.class === "fatal" && (error.status === 401 || error.status === 403 || error.status === 404)) {
        const anonymous = client.auth.kind === "token" && client.auth.token === undefined;
        const why = anonymous ? "no GitHub token; set GITHUB_TOKEN or run gh auth login" : "token lacks access";
        throw new GitHubError("fatal", `cannot read ${ref.owner}/${ref.repo}#${ref.number}: HTTP ${error.status} (${why})`, error.status);
      }
      throw error;
    }
    return {
      installationId,
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
