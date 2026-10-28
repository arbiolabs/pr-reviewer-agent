import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { type AutoReview, type Config, DEFAULT_CONFIG } from "../src/config/config.ts";
import { openSource } from "../src/control/daemon.ts";
import { type EngineOptions, EngineSource, MAX_STARTS_PER_POLL } from "../src/control/engine.ts";
import { startControlServer } from "../src/control/server.ts";
import { ControlError } from "../src/control/source.ts";
import type { PullInfo } from "../src/domain/types.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { type Command, idempotencyKey, type ServerMessage } from "../src/protocol/v1.ts";
import { openApp } from "../src/runtime/app.ts";
import { TestClient } from "./control-client.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, type Fixture, pushCommit } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

let fake: FakeGitHub;
let fixture: Fixture;
const cleanup: (() => Promise<void> | void)[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fake.setPull(fixture.pull);
});
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
  await fake.close();
});

const on: AutoReview = { enabled: true, interval_minutes: 5, include_team_requests: false, repos: { include: [], exclude: [] } };

function configWith(auto: Partial<AutoReview>): Config {
  return {
    ...DEFAULT_CONFIG,
    review: { ...DEFAULT_CONFIG.review, publish_mode: "approval", retry_backoff_ms: 10, auto_review: { ...on, ...auto } },
    github: { ...DEFAULT_CONFIG.github, timeout_ms: 300 },
  };
}

/** An engine on the fake GitHub. The poll clock never fires on its own unless `autoReviewMs` says so. */
async function host(options: { config?: Config; dataDir?: string; account?: string | null; autoReviewMs?: EngineOptions["autoReviewMs"]; env?: Record<string, string> } = {}) {
  const config = options.config ?? configWith({});
  const client = new GitHubClient({ auth: { kind: "token", token: "ghp_test" }, baseUrl: fake.url, maxRetries: 1, backoffMs: 1, concurrency: 4, timeoutMs: 5000 });
  const pulls = new GitHubPullSource(() => client);
  const dataDir = options.dataDir ?? mkdtempSync(join(tmpdir(), "auto-"));
  const models = fauxModels([readDiff(), submitDefect(fixture)]);
  const app = await openApp({ dataDir, config, pulls, github: () => client, engine: async () => ({ skipped: "test" }), models, reviewerVersion: "m1+faux/reviewer", lockWaitMs: 0 });
  const source = await EngineSource.host(dataDir, app, {
    config,
    pulls,
    github: () => client,
    env: options.env ?? { PATH: "" },
    ...(options.account === null ? {} : { account: options.account ?? "github.com/octo" }),
    models,
    model: { ref: "faux/reviewer", source: "config" },
    autoReviewMs: options.autoReviewMs ?? { first: 3_600_000 },
  });
  let open = true;
  const stop = async () => {
    if (open) await source.close();
    open = false;
  };
  cleanup.push(stop);
  return { app, source, dataDir, stop };
}

/** A requested PR of the fixture's repository (or another one), stored on the fake. */
function requestPull(number: number, extra: Partial<PullInfo> & { team?: boolean; draft?: boolean } = {}): PullInfo {
  const { team, draft, ...fields } = extra;
  const pull = { ...fixture.pull, number, ...fields };
  fake.setPull(pull);
  fake.requested.push({ pull: `${pull.fullName}#${number}`, ...(team ? { team } : {}), ...(draft ? { draft } : {}) });
  return pull;
}

type Hosted = Awaited<ReturnType<typeof host>>;
const prOf = (h: Hosted, name: string, n: number) => h.app.store.pullByName(name, n);
const heads = (h: Hosted, name: string, n: number) => {
  const pr = prOf(h, name, n);
  return pr ? h.app.store.revisionsForPr(pr.id).map((r) => r.head_sha) : [];
};
const until = async (done: () => boolean, ms = 15_000) => {
  for (const end = Date.now() + ms; !done(); ) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
};

it("tracks and starts a requested PR once across polls and a daemon restart, and says it started automatically", async () => {
  requestPull(1);
  const h = await host();
  expect(await h.source.pollRequests()).toMatchObject({ found: 1, started: 1, skipped: 0 });
  expect(heads(h, "acme/shop", 1)).toEqual([fixture.headSha]);
  expect(await h.source.pollRequests()).toMatchObject({ found: 1, started: 0, skipped: 1 });

  const runId = h.app.store.revisionsForPr(prOf(h, "acme/shop", 1)!.id)[0]!.id;
  const activity = h.source.timeline({ type: "timeline.get", requestId: "t", runId, limit: 500 }).events.filter((e) => e.eventType === "tool.activity");
  expect(activity.map((e) => e.data)).toContainEqual({ tool: "auto-review", summary: "started automatically: review requested" });

  await h.stop();
  const again = await host({ dataDir: h.dataDir });
  expect(await again.source.pollRequests()).toMatchObject({ found: 1, started: 0 });
  expect(heads(again, "acme/shop", 1)).toEqual([fixture.headSha]);
}, 60_000);

it("reviews a tracked PR's new head once, and never an archived PR, however its head moves", async () => {
  requestPull(1);
  requestPull(2);
  const h = await host();
  await h.source.pollRequests();
  await h.source.archivePr(prOf(h, "acme/shop", 2)!.id, true);

  const next = pushCommit(fixture, { "src/later.ts": "export const later = 1;\n" });
  fake.setPull(next);
  fake.setPull({ ...next, number: 2 });
  expect(await h.source.pollRequests()).toMatchObject({ found: 2, started: 1, skipped: 1 });
  expect(await h.source.pollRequests()).toMatchObject({ started: 0 });
  expect(heads(h, "acme/shop", 1)).toEqual([fixture.headSha, next.headSha]);
  expect(heads(h, "acme/shop", 2)).toEqual([fixture.headSha]);
  expect(prOf(h, "acme/shop", 2)!.archived_at).not.toBeNull();
}, 60_000);

it("skips drafts, closed PRs and excluded repositories, and honors the include list", async () => {
  requestPull(1);
  requestPull(2, { draft: true });
  requestPull(3, { state: "closed" });
  requestPull(4, { fullName: "acme/web", repositoryId: 2002 });
  requestPull(5, { fullName: "other/lib", repositoryId: 3003 });
  const h = await host({ config: configWith({ repos: { include: ["acme/shop", "ACME/web"], exclude: ["acme/web"] } }) });
  expect(await h.source.pollRequests()).toMatchObject({ found: 4, started: 1, skipped: 3 });
  expect([1, 2, 3].map((n) => heads(h, "acme/shop", n).length)).toEqual([1, 0, 0]);
  expect([prOf(h, "acme/web", 4), prOf(h, "other/lib", 5)]).toEqual([undefined, undefined]);
}, 60_000);

it("asks for team requests only when they are included", async () => {
  requestPull(1, { team: true });
  const mine = await host();
  expect(await mine.source.pollRequests()).toMatchObject({ found: 0, started: 0 });
  await mine.stop();
  const teams = await host({ config: configWith({ include_team_requests: true }) });
  expect(await teams.source.pollRequests()).toMatchObject({ found: 1, started: 1 });
  expect(fake.searchQueries).toEqual([
    "is:pr is:open draft:false archived:false user-review-requested:@me",
    "is:pr is:open draft:false archived:false review-requested:@me",
  ]);
}, 60_000);

it(`starts at most ${MAX_STARTS_PER_POLL} a poll, oldest request first, and the rest on the next poll`, async () => {
  for (let n = 1; n <= 7; n++) requestPull(n);
  const h = await host();
  expect(await h.source.pollRequests()).toMatchObject({ found: 7, started: 5, skipped: 2 });
  expect([1, 2, 3, 4, 5, 6, 7].map((n) => heads(h, "acme/shop", n).length)).toEqual([1, 1, 1, 1, 1, 0, 0]);
  expect(await h.source.pollRequests()).toMatchObject({ found: 7, started: 2, skipped: 5 });
}, 60_000);

it("runs one poll at a time: an overlapping trigger shares the poll in flight", async () => {
  requestPull(1);
  const h = await host();
  const [a, b] = await Promise.all([h.source.pollRequests(), h.source.pollRequests()]);
  expect(a).toBe(b);
  expect([fake.searchQueries.length, heads(h, "acme/shop", 1).length]).toEqual([1, 1]);
}, 60_000);

it("records a rate-limited poll and retries on the next interval, starting the review with no client command", async () => {
  requestPull(1);
  fake.rateLimitedSearches = 1;
  const h = await host({ autoReviewMs: { first: 0, interval: 100 } });
  await until(() => fake.searchQueries.length >= 1);
  await until(() => h.app.store.revisionsForPr(prOf(h, "acme/shop", 1)?.id ?? "").some((r) => r.state === "AWAITING_APPROVAL"), 30_000);
  expect(fake.searchQueries.length).toBeGreaterThanOrEqual(2);
}, 60_000);

it("reports a rate-limited poll as its error", async () => {
  requestPull(1);
  fake.rateLimitedSearches = 1;
  const h = await host();
  expect(await h.source.pollRequests()).toMatchObject({ found: 0, started: 0, error: expect.stringMatching(/rate limited/) });
  expect(await h.source.pollRequests()).toMatchObject({ found: 1, started: 1 });
  expect((await h.source.pollRequests()).error).toBeUndefined();
}, 60_000);

it("never searches while off, without a GitHub account, or from a read-only daemon", async () => {
  requestPull(1);
  const off = await host({ config: configWith({ enabled: false }), autoReviewMs: { first: 0, interval: 20 } });
  const anonymous = await host({ dataDir: mkdtempSync(join(tmpdir(), "auto-")), account: null });
  expect(await anonymous.source.pollRequests()).toMatchObject({ started: 0, error: expect.stringMatching(/no GitHub account/) });
  const readOnly = await openSource(mkdtempSync(join(tmpdir(), "auto-")), false, {}, () => {});
  cleanup.push(() => readOnly.close());
  expect("pollRequests" in readOnly).toBe(false);
  await new Promise((r) => setTimeout(r, 200));
  expect(fake.searchQueries).toEqual([]);
  expect(prOf(off, "acme/shop", 1)).toBeUndefined();
}, 60_000);

it("settings.set saves auto-review, polls at once when it turns on, and refuses a bad value", async () => {
  requestPull(1);
  const dir = mkdtempSync(join(tmpdir(), "settings-"));
  const configPath = join(dir, "pr-review.config.json");
  writeFileSync(configPath, JSON.stringify({ review: { publish_mode: "approval" } }));
  const h = await host({ config: configWith({ enabled: false }), env: { PATH: "", PR_REVIEW_CONFIG: configPath }, autoReviewMs: { first: 0, interval: 3_600_000 } });
  const server = await startControlServer({ socketPath: join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock"), source: h.source, pollMs: 20 });
  cleanup.push(() => server.close());
  const c = await TestClient.connect(server.path);
  cleanup.push(() => c.close());
  await c.hello();
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  let n = 0;
  const send = (params: Record<string, unknown>) => {
    const message = { type: "command", requestId: `cmd-${++n}`, command: "settings.set", params, idempotencyKey: "" } as Command;
    message.idempotencyKey = idempotencyKey(message);
    c.send(message);
    return c.waitFor<ServerMessage>((m) => "requestId" in m && m.requestId === message.requestId);
  };
  const settings = async () => {
    c.send({ type: "settings.get", requestId: `get-${++n}` });
    return c.waitFor<Extract<ServerMessage, { type: "settings" }>>((m) => m.type === "settings" && m.requestId === `get-${n}`);
  };
  expect((await settings()).autoReview).toEqual({ enabled: false, intervalMinutes: 5, includeTeamRequests: false, repos: { include: [], exclude: [] } });
  expect((await settings()).publishMode).toBe("approval");

  expect(await send({ autoReview: { enabled: true, intervalMinutes: 15, repos: { include: ["acme/shop"] } } })).toMatchObject({ type: "command.result", status: "accepted" });
  expect(JSON.parse(readFileSync(configPath, "utf8")).review).toEqual({
    publish_mode: "approval",
    auto_review: { enabled: true, interval_minutes: 15, include_team_requests: false, repos: { include: ["acme/shop"], exclude: [] } },
  });
  await until(() => heads(h, "acme/shop", 1).length === 1);
  for (let i = 0; i < 100 && !(await settings()).autoReview?.lastPoll; i++) await new Promise((r) => setTimeout(r, 20));
  expect((await settings()).autoReview).toMatchObject({ enabled: true, intervalMinutes: 15, repos: { include: ["acme/shop"], exclude: [] }, lastPoll: { found: 1, started: 1, skipped: 0 } });

  expect(await send({ autoReview: { intervalMinutes: 7 } })).toMatchObject({ type: "error", code: "INVALID_REQUEST" });
  expect(await send({ autoReview: { repos: { exclude: ["not a repo"] } } })).toMatchObject({ type: "error", code: "INVALID_REQUEST" });
  await expect(h.source.setSettings({ autoReview: { repos: { include: ["bad name"] } } })).rejects.toEqual(new ControlError("INVALID_REQUEST", 'autoReview: repos must list owner/name, not "bad name"'));
  expect(JSON.parse(readFileSync(configPath, "utf8")).review.auto_review.interval_minutes).toBe(15);
}, 60_000);
