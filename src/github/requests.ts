import type { AutoReview } from "../config/config.ts";
import type { GitHubClient } from "./client.ts";
import type { PullRef } from "./pulls.ts";

type SearchItem = { number: number; draft?: boolean; repository_url: string };

/** One search page. Past it the oldest requests are still read first, and a poll starts only a few anyway. */
export const SEARCH_CAP = 100;

/**
 * Open, non-draft PRs in unarchived repositories that request the token's own review (and its teams' with
 * `include_team_requests`), oldest first, then narrowed to `repos`. Search has no review-request time, so a PR's
 * creation stands in for it. `found` counts what search listed before the repository lists narrowed it.
 */
export async function reviewRequests(client: GitHubClient, a: AutoReview, maxSleepMs: number): Promise<{ found: number; pulls: PullRef[] }> {
  const who = a.include_team_requests ? "review-requested:@me" : "user-review-requested:@me";
  const q = new URLSearchParams({ q: `is:pr is:open draft:false archived:false ${who}`, sort: "created", order: "asc", per_page: String(SEARCH_CAP) });
  const { data } = await client.request<{ items: SearchItem[] }>("GET", `/search/issues?${q}`, { maxSleepMs });
  const listed = data.items.filter((i) => !i.draft).slice(0, SEARCH_CAP);
  const named = (list: string[], name: string) => list.some((r) => r.toLowerCase() === name.toLowerCase());
  const pulls = listed.flatMap((i): PullRef[] => {
    const [owner, repo] = new URL(i.repository_url).pathname.split("/").slice(-2) as [string, string];
    const name = `${owner}/${repo}`;
    const wanted = (a.repos.include.length === 0 || named(a.repos.include, name)) && !named(a.repos.exclude, name);
    return wanted ? [{ owner, repo, number: i.number }] : [];
  });
  return { found: listed.length, pulls };
}
