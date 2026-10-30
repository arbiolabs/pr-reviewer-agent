import type { PullInfo } from "../domain/types.ts";
import { type GitHubClient, GitHubError } from "./client.ts";

/** `installationId` picks the GitHub App installation to read through; absent means token auth. */
export type PullRef = { owner: string; repo: string; number: number; installationId?: number };

/** An issue the pull request says it closes, as GitHub has it. */
export type LinkedIssue = { number: number; title: string; body: string };

export interface PullSource {
  getPull(ref: PullRef, options?: { maxSleepMs?: number }): Promise<PullInfo>;
  /** The issue `number` in the ref's repository; `undefined` when it is missing, unreadable, or a pull request. */
  getIssue?(ref: PullRef, number: number): Promise<LinkedIssue | undefined>;
}

/** The most issues a review reads, and how much of each. */
export const MAX_LINKED_ISSUES = 3;
export const MAX_ISSUE_BODY_CHARS = 4000;

/**
 * The issues `body` says the pull request closes, with GitHub's closing keywords (`Fixes #12`, `closes owner/repo#12`,
 * `resolves https://github.com/owner/repo/issues/12`), in order and each once. Only issues in `fullName` count: one in
 * another repository could carry that repository's private text into this review.
 */
export function linkedIssueNumbers(body: string, fullName: string, self: number): number[] {
  const keyword = String.raw`\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?\s+`;
  const target = String.raw`(?:https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/issues\/|([\w.-]+\/[\w.-]+)?#)(\d+)\b`;
  const numbers: number[] = [];
  for (const m of body.matchAll(new RegExp(keyword + target, "gi"))) {
    const repo = m[1] ?? m[2];
    const n = Number(m[3]);
    if (repo !== undefined && repo.toLowerCase() !== fullName.toLowerCase()) continue;
    if (n !== self && !numbers.includes(n)) numbers.push(n);
  }
  return numbers.slice(0, MAX_LINKED_ISSUES);
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

type GitHubPull = {
  number: number;
  title: string;
  user: { login: string } | null;
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
        const why = anonymous ? "no GitHub token; set GITHUB_TOKEN, run gh auth login, or pick one with pr-review accounts use" : "token lacks access";
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
      author: pull.user?.login ?? null,
      body: pull.body ?? "",
      baseSha: pull.base.sha,
      headSha: pull.head.sha,
      state: pull.state,
      merged: pull.merged,
      private: pull.base.repo.private,
      cloneUrl: pull.base.repo.clone_url,
    };
  }

  async getIssue(ref: PullRef, number: number): Promise<LinkedIssue | undefined> {
    type GitHubIssue = { number: number; title: string; body: string | null; pull_request?: unknown };
    try {
      const { data } = await this.clientFor(ref.installationId ?? 0).request<GitHubIssue>("GET", `/repos/${ref.owner}/${ref.repo}/issues/${number}`);
      if (data.pull_request !== undefined) return undefined;
      return { number: data.number, title: data.title.slice(0, 200), body: (data.body ?? "").slice(0, MAX_ISSUE_BODY_CHARS) };
    } catch (error) {
      // A missing, deleted or unreadable issue only leaves the review without it; a retryable failure retries the phase.
      if (error instanceof GitHubError && error.class === "fatal") return undefined;
      throw error;
    }
  }
}
