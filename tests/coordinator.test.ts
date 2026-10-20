import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { Coordinator, routeEvent } from "../src/coordinator/coordinator.ts";
import type { PullInfo } from "../src/domain/types.ts";
import { AppStore } from "../src/persistence/store.ts";

const BOT = "durable-review[bot]";
const base: PullInfo = {
  installationId: 9,
  repositoryId: 5,
  fullName: "acme/shop",
  number: 1,
  title: "t",
  body: "",
  baseSha: "b".repeat(40),
  headSha: "a".repeat(40),
  state: "open",
  merged: false,
  private: false,
  cloneUrl: "file:///x",
};
const head = (c: string) => c.repeat(40);

/** A coordinator over a real store, with GitHub's authoritative PR state and the review starter stubbed. */
function harness() {
  const store = AppStore.open(join(mkdtempSync(join(tmpdir(), "coord-")), "app.sqlite"));
  const live = { pull: { ...base } };
  const started: { head: string; liveHead: string; liveState: string }[] = [];
  const aborted: string[] = [];
  let clock = 1_000_000;
  let delivery = 0;
  const coordinator = new Coordinator({
    store,
    pulls: { getPull: async () => ({ ...live.pull }) },
    botLogin: async () => BOT,
    start: async (pull) => {
      started.push({ head: pull.headSha, liveHead: live.pull.headSha, liveState: live.pull.state });
      const prId = store.upsertPull(pull);
      const revision = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1+publish", reviewerVersion: "m2" });
      store.setDurableTask(revision.id, `task-${pull.headSha.slice(0, 1)}`);
    },
    abort: async (id) => void aborted.push(id),
    debounceMs: 30_000,
    leaseMs: 60_000,
    owner: "worker-1",
    now: () => clock,
    report: (error) => {
      throw error;
    },
  });
  const deliver = (eventType: string, payload: object) =>
    store.recordWebhook({ deliveryId: `d${++delivery}`, eventType, payloadHash: "h", payloadJson: JSON.stringify({ repository: { full_name: "acme/shop" }, installation: { id: 9 }, ...payload }) });
  const pr = (action: string, extra: object = {}) => deliver("pull_request", { action, pull_request: { number: 1, ...extra } });
  return { store, live, started, aborted, coordinator, deliver, pr, advance: (ms: number) => (clock += ms) };
}

describe("routeEvent", () => {
  const repo = { repository: { full_name: "acme/shop" }, installation: { id: 9 } };
  const comment = (body: string, login = "alice", type = "User") => ({ ...repo, action: "created", sender: { type }, issue: { number: 3, pull_request: {} }, comment: { body, user: { login } } });
  it("starts work only for PR lifecycle events and explicit mentions from humans", () => {
    expect(routeEvent("pull_request", { ...repo, action: "synchronize", pull_request: { number: 3 } }, BOT)).toEqual({
      kind: "pr",
      ref: { owner: "acme", repo: "shop", number: 3, installationId: 9 },
      event: "PR_SYNCHRONIZED",
    });
    expect(routeEvent("pull_request", { ...repo, action: "closed", pull_request: { number: 3, merged: true } }, BOT)).toMatchObject({ event: "PR_MERGED" });
    expect(routeEvent("issue_comment", comment("@durable-review review please"), BOT)).toMatchObject({ kind: "pr", event: "COMMENT_CREATED" });
  });
  it("ignores bot senders, the app's own comments and comments without the mention", () => {
    expect(routeEvent("issue_comment", comment("@durable-review review", "dependabot[bot]", "Bot"), BOT)).toEqual({ kind: "ignore", reason: "bot sender" });
    expect(routeEvent("pull_request", { ...repo, action: "opened", sender: { type: "Bot" }, pull_request: { number: 3 } }, BOT)).toEqual({ kind: "ignore", reason: "bot sender" });
    expect(routeEvent("issue_comment", comment("@durable-review review", BOT), BOT)).toEqual({ kind: "ignore", reason: "own comment" });
    expect(routeEvent("issue_comment", comment("please review this"), BOT)).toEqual({ kind: "ignore", reason: "no review mention" });
    expect(routeEvent("issue_comment", comment("@durable-reviewer review"), BOT)).toEqual({ kind: "ignore", reason: "no review mention" });
    expect(routeEvent("push", repo, BOT)).toEqual({ kind: "ignore", reason: "unsupported event push" });
  });
});

it("ignored deliveries start nothing and are marked processed with the reason", async () => {
  const h = harness();
  h.deliver("issue_comment", { action: "created", sender: { type: "Bot" }, issue: { number: 1, pull_request: {} }, comment: { body: "@durable-review review", user: { login: "x[bot]" } } });
  h.deliver("issue_comment", { action: "created", issue: { number: 1, pull_request: {} }, comment: { body: "@durable-review review", user: { login: BOT } } });
  h.deliver("issue_comment", { action: "created", issue: { number: 1, pull_request: {} }, comment: { body: "nice", user: { login: "alice" } } });
  await h.coordinator.tick();
  h.advance(60_000);
  await h.coordinator.tick();
  expect(h.started).toEqual([]);
  expect(h.store.db.prepare("SELECT outcome FROM webhook_events ORDER BY delivery_id").all()).toEqual([
    { outcome: "ignored: bot sender" },
    { outcome: "ignored: own comment" },
    { outcome: "ignored: no review mention" },
  ]);
  h.deliver("issue_comment", { action: "created", issue: { number: 1, pull_request: {} }, comment: { body: "@durable-review review", user: { login: "alice" } } });
  await h.coordinator.tick();
  h.advance(30_000);
  await h.coordinator.tick();
  expect(h.started.map((s) => s.head)).toEqual([head("a")]);
});

it("reviews only the last head of opened + three pushes inside the debounce window", async () => {
  const h = harness();
  h.pr("opened");
  await h.coordinator.tick();
  for (const c of ["b", "c", "d"]) {
    h.advance(10_000);
    h.live.pull = { ...base, headSha: head(c) };
    h.pr("synchronize");
    await h.coordinator.tick();
  }
  expect(h.started).toEqual([]);
  h.advance(29_000);
  await h.coordinator.tick();
  expect(h.started).toEqual([]);
  h.advance(1_000);
  await h.coordinator.tick();
  await h.coordinator.tick();
  expect(h.started).toEqual([{ head: head("d"), liveHead: head("d"), liveState: "open" }]);
});

it("marks an in-flight older revision superseded_requested and aborts its task when a new head arrives", async () => {
  const h = harness();
  h.pr("opened");
  await h.coordinator.tick();
  h.advance(30_000);
  await h.coordinator.tick();
  h.live.pull = { ...base, headSha: head("e") };
  h.pr("synchronize");
  await h.coordinator.tick();
  expect(h.aborted).toEqual(["task-a"]);
  expect(h.store.db.prepare("SELECT head_sha, superseded_requested_at IS NOT NULL AS requested FROM review_revisions").all()).toEqual([{ head_sha: head("a"), requested: 1 }]);
});

it("starts nothing for closed or merged PRs, and resumes on reopen", async () => {
  const h = harness();
  h.pr("opened");
  h.live.pull = { ...base, state: "closed" };
  h.pr("closed");
  await h.coordinator.tick();
  h.advance(60_000);
  h.pr("synchronize");
  await h.coordinator.tick();
  h.advance(60_000);
  await h.coordinator.tick();
  expect([h.started, h.store.pullByName("acme/shop", 1)!.status]).toEqual([[], "CLOSED"]);
  h.live.pull = { ...base };
  h.pr("reopened");
  await h.coordinator.tick();
  h.advance(30_000);
  await h.coordinator.tick();
  expect(h.started.map((s) => s.head)).toEqual([head("a")]);

  const m = harness();
  m.live.pull = { ...base, state: "closed", merged: true };
  m.pr("closed", { merged: true });
  m.pr("reopened");
  m.pr("synchronize");
  await m.coordinator.tick();
  m.advance(60_000);
  await m.coordinator.tick();
  expect([m.started, m.store.pullByName("acme/shop", 1)!.status]).toEqual([[], "MERGED"]);
});

function permutations<T>(items: T[]): T[][] {
  if (items.length <= 1) return [items];
  return items.flatMap((item, i) => permutations([...items.slice(0, i), ...items.slice(i + 1)]).map((rest) => [item, ...rest]));
}

describe("event order never decides which head is reviewed", () => {
  // The true history: opened at A, pushed B, closed, reopened. GitHub's state follows the history; deliveries arrive in any order.
  const history: { action: string; pull: PullInfo }[] = [
    { action: "opened", pull: { ...base } },
    { action: "synchronize", pull: { ...base, headSha: head("b") } },
    { action: "closed", pull: { ...base, headSha: head("b"), state: "closed" } },
    { action: "reopened", pull: { ...base, headSha: head("b") } },
  ];
  for (const order of permutations([0, 1, 2, 3])) {
    it(`deliveries ${order.map((i) => history[i]!.action).join(", ")}`, async () => {
      const h = harness();
      for (const [step, i] of order.entries()) {
        h.live.pull = { ...history[step]!.pull };
        h.pr(history[i]!.action);
        await h.coordinator.tick();
        h.advance(step % 2 === 0 ? 40_000 : 5_000);
        await h.coordinator.tick();
      }
      h.advance(40_000);
      await h.coordinator.tick();
      for (const s of h.started) expect([s.head, s.liveState]).toEqual([s.liveHead, "open"]);
      expect(h.started.at(-1)?.head).toBe(head("b"));
    });
  }
});

it("a crashed owner's lease blocks the PR until it expires, then another worker takes it over", () => {
  const h = harness();
  const prId = h.store.upsertPull(base);
  expect(h.store.acquireLease(prId, "dead-worker", 0, 60_000)).toBe(true);
  expect(h.store.acquireLease(prId, "worker-2", 59_999, 60_000)).toBe(false);
  expect(h.store.acquireLease(prId, "worker-2", 60_000, 60_000)).toBe(true);
  expect(h.store.db.prepare("SELECT owner, expires_at FROM leases").all()).toEqual([{ owner: "worker-2", expires_at: 120_000 }]);
});
