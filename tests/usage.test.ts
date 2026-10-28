import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createModels, type FauxResponseStep, fauxAssistantMessage, fauxProvider } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import { type UsageRow, usageReport, usageSince, usageStart } from "../src/domain/usage.ts";
import { main } from "../src/cli/main.ts";
import { AppStore, usageRows } from "../src/persistence/store.ts";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { finishReview, openApp, startReview } from "../src/runtime/app.ts";
import { createFixture } from "./fixture.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";

const row = (at: string, over: Partial<UsageRow> = {}): UsageRow => ({
  revisionId: "rev-1",
  at,
  model: "anthropic/claude",
  input: 100,
  output: 10,
  cacheRead: 1000,
  cacheWrite: 50,
  cost: 0.25,
  ...over,
});

const NOW = Date.parse("2026-10-28T12:00:00Z");
const starts = (r: ReturnType<typeof usageReport>) => r.buckets.map((b) => b.start);

describe("usageReport", () => {
  it("lists the last 14 days up to today, empty days included, oldest first", () => {
    const r = usageReport([], { by: "day", tz: "UTC", now: NOW });
    expect(r.buckets).toHaveLength(14);
    expect(r.buckets[0]).toMatchObject({ start: "2026-10-15", end: "2026-10-15", responses: 0, cost: 0 });
    expect(r.buckets.at(-1)).toMatchObject({ start: "2026-10-28", end: "2026-10-28" });
    expect(r.totals).toEqual({ reviews: 0, responses: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 });
    expect(r.models).toEqual([]);
    expect(r.timeZone).toBe("UTC");
  });

  it("puts a response on the local date of the daemon's time zone", () => {
    const rows = [row("2026-10-28T03:00:00Z")];
    expect(usageReport(rows, { by: "day", tz: "UTC", now: NOW }).buckets.at(-1)!.responses).toBe(1);
    const ny = usageReport(rows, { by: "day", tz: "America/New_York", now: NOW });
    expect(ny.buckets.find((b) => b.start === "2026-10-27")!.responses).toBe(1);
    expect(ny.buckets.at(-1)!.responses).toBe(0);
  });

  it("sums tokens and cost per bucket, counts distinct reviews, and splits the range by model", () => {
    const rows = [
      row("2026-10-27T09:00:00Z"),
      row("2026-10-27T10:00:00Z", { cost: 0.1 }),
      row("2026-10-27T11:00:00Z", { revisionId: "rev-2", model: "openai/gpt", cost: 0.7, input: 1 }),
      row("2026-10-28T09:00:00Z", { revisionId: "rev-2", model: "openai/gpt", cost: 0.05 }),
    ];
    const r = usageReport(rows, { by: "day", tz: "UTC", now: NOW });
    expect(r.buckets.at(-2)).toEqual({ start: "2026-10-27", end: "2026-10-27", reviews: 2, responses: 3, input: 201, output: 30, cacheRead: 3000, cacheWrite: 150, cost: 1.05 });
    expect(r.totals).toEqual({ reviews: 2, responses: 4, input: 301, output: 40, cacheRead: 4000, cacheWrite: 200, cost: 1.1 });
    expect(r.models.map((m) => [m.model, m.reviews, m.responses, m.cost])).toEqual([
      ["openai/gpt", 1, 2, 0.75],
      ["anthropic/claude", 1, 2, 0.35],
    ]);
  });

  it("ignores rows before the range and after now's bucket", () => {
    const rows = [row("2026-10-14T23:59:59Z"), row("2026-10-15T00:00:00Z"), row("2026-10-29T00:00:00Z")];
    expect(usageReport(rows, { by: "day", tz: "UTC", now: NOW }).totals.responses).toBe(1);
  });

  it("starts weeks on Monday and keeps a week whole across the autumn clock change", () => {
    // Berlin leaves summer time on Sunday 2026-10-25; that week runs Monday 19 to Sunday 25 in local dates.
    const tz = "Europe/Berlin";
    const rows = [
      row("2026-10-18T22:30:00Z"), // Monday 19 00:30 CEST
      row("2026-10-25T22:30:00Z"), // Sunday 25 23:30 CET, the hour the clock gave back
      row("2026-10-25T23:30:00Z"), // Monday 26 00:30 CET
    ];
    const r = usageReport(rows, { by: "week", tz, now: NOW });
    expect(r.buckets).toHaveLength(12);
    expect(r.buckets.at(-1)).toMatchObject({ start: "2026-10-26", end: "2026-11-01", responses: 1 });
    expect(r.buckets.at(-2)).toMatchObject({ start: "2026-10-19", end: "2026-10-25", responses: 2 });
    expect(r.buckets[0]!.start).toBe("2026-08-10");
  });

  it("keeps the spring clock change in its week too", () => {
    const tz = "Europe/Berlin";
    const now = Date.parse("2026-03-31T12:00:00Z");
    const rows = [row("2026-03-28T23:30:00Z"), row("2026-03-29T21:30:00Z"), row("2026-03-29T22:30:00Z")];
    const r = usageReport(rows, { by: "week", tz, now, periods: 2 });
    expect(r.buckets.map((b) => [b.start, b.end, b.responses])).toEqual([
      ["2026-03-23", "2026-03-29", 2],
      ["2026-03-30", "2026-04-05", 1],
    ]);
  });

  it("splits months at local midnight, across year ends and short months", () => {
    const tz = "Europe/Berlin";
    const rows = [row("2026-09-30T21:59:59Z"), row("2026-09-30T22:00:00Z"), row("2026-02-28T23:00:00Z")];
    const r = usageReport(rows, { by: "month", tz, now: NOW });
    expect(starts(r)).toEqual(["2025-11-01", "2025-12-01", "2026-01-01", "2026-02-01", "2026-03-01", "2026-04-01", "2026-05-01", "2026-06-01", "2026-07-01", "2026-08-01", "2026-09-01", "2026-10-01"]);
    expect(r.buckets.map((b) => b.end).slice(2, 4)).toEqual(["2026-01-31", "2026-02-28"]);
    expect(r.buckets.find((b) => b.start === "2026-09-01")!.responses).toBe(1);
    expect(r.buckets.find((b) => b.start === "2026-10-01")!.responses).toBe(1);
    expect(r.buckets.find((b) => b.start === "2026-03-01")!.responses).toBe(1);
  });

  it("takes a custom number of periods and refuses a non-positive one", () => {
    expect(usageReport([], { by: "month", tz: "UTC", now: NOW, periods: 1 }).buckets).toEqual([
      { start: "2026-10-01", end: "2026-10-31", reviews: 0, responses: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 },
    ]);
    expect(() => usageReport([], { by: "day", tz: "UTC", now: NOW, periods: 0 })).toThrow(RangeError);
  });

  describe("breakdowns", () => {
    const shop = { id: "pr-1", repository: "acme/shop", number: 1, title: "Checkout", author: "ana" };
    const rows = [
      row("2026-10-27T09:00:00Z", { pr: shop, cost: 0.4 }),
      row("2026-10-27T10:00:00Z", { revisionId: "rev-2", pr: { ...shop, id: "pr-2", number: 2, title: "Refunds", author: "bo" }, cost: 0.3 }),
      row("2026-10-28T09:00:00Z", { revisionId: "rev-3", pr: { id: "pr-3", repository: "acme/api", number: 9, title: "Auth" }, cost: 0.2 }),
      row("2026-10-28T10:00:00Z", { revisionId: "rev-4", cost: 0.1 }),
      row("2026-09-01T10:00:00Z", { revisionId: "rev-5", pr: { id: "pr-5", repository: "acme/old", number: 3, title: "Old", author: "cy" }, cost: 9 }),
    ];
    const groups = (group: "repo" | "author" | "pr") => usageReport(rows, { by: "day", tz: "UTC", now: NOW, group }).groups!;

    it("splits the range by repository, costliest first, leaving rows outside the range and the totals as they were", () => {
      const r = usageReport(rows, { by: "day", tz: "UTC", now: NOW, group: "repo" });
      expect(r.group).toBe("repo");
      expect(r.totals).toEqual(usageReport(rows, { by: "day", tz: "UTC", now: NOW }).totals);
      expect(r.groups!.map((g) => [g.key, g.label, g.reviews, g.responses, g.cost, g.share])).toEqual([
        ["acme/shop", "acme/shop", 2, 2, 0.7, 0.7],
        ["acme/api", "acme/api", 1, 1, 0.2, 0.2],
        ["(unknown pr)", "Unknown PR", 1, 1, 0.1, 0.1],
      ]);
    });

    it("labels a PR with no known author, and a usage row whose PR is gone", () => {
      expect(groups("author").map((g) => [g.label, g.author])).toEqual([
        ["ana", "ana"],
        ["bo", "bo"],
        ["Unknown author", undefined],
        ["Unknown PR", undefined],
      ]);
    });

    it("carries each PR's repository, number, title, id and author", () => {
      expect(groups("pr")[0]).toEqual({ key: "pr-1", label: "Checkout", repository: "acme/shop", number: 1, title: "Checkout", prId: "pr-1", author: "ana", reviews: 1, responses: 1, input: 100, output: 10, cacheRead: 1000, cacheWrite: 50, cost: 0.4, share: 0.4 });
      expect(groups("pr")[2]).not.toHaveProperty("author");
    });

    it("orders a tie in cost by tokens, then by label", () => {
      const tied = [
        row("2026-10-28T09:00:00Z", { pr: { ...shop, author: "zed" }, cost: 0.1 }),
        row("2026-10-28T09:00:00Z", { pr: { ...shop, id: "pr-2", author: "amy" }, cost: 0.1 }),
        row("2026-10-28T09:00:00Z", { pr: { ...shop, id: "pr-3", author: "max" }, cost: 0.1, output: 11 }),
      ];
      expect(usageReport(tied, { by: "day", tz: "UTC", now: NOW, group: "author" }).groups!.map((g) => g.label)).toEqual(["max", "amy", "zed"]);
    });

    it("shares the range's cost out to 1 within rounding, and to nothing when the range cost nothing", () => {
      const thirds = [0, 1, 2].map((i) => row("2026-10-28T09:00:00Z", { pr: { ...shop, id: `pr-${i}`, author: `a${i}` }, cost: 1 / 3 }));
      const shares = usageReport(thirds, { by: "day", tz: "UTC", now: NOW, group: "author" }).groups!.map((g) => g.share);
      expect(shares.reduce((a, b) => a + b, 0)).toBeCloseTo(1, 5);
      expect(usageReport([row("2026-10-28T09:00:00Z", { cost: 0 })], { by: "day", tz: "UTC", now: NOW, group: "pr" }).groups![0]!.share).toBe(0);
    });

    it("leaves the breakdown out over time", () => {
      expect(usageReport(rows, { by: "day", tz: "UTC", now: NOW })).not.toHaveProperty("groups");
      expect(usageReport(rows, { by: "day", tz: "UTC", now: NOW }).group).toBe("time");
    });
  });

  it("reads rows from an instant no later than the first local midnight in any zone", () => {
    for (const tz of ["Pacific/Kiritimati", "UTC", "Pacific/Pago_Pago"]) {
      const options = { by: "week" as const, tz, now: NOW };
      const midnight = Date.parse(`${usageStart(options)}T00:00:00Z`);
      expect(Date.parse(usageSince(options))).toBeLessThanOrEqual(midnight - 14 * 3_600_000);
    }
  });
});

describe("recording", () => {
  it("stores each token count and cost column, attributed to the revision's PR", async () => {
    const fixture = createFixture();
    const app = await open(mkdtempSync(join(tmpdir(), "usage-")), fixture, []);
    try {
      const started = await startReview(app, fixture.pull, "1");
      const cost = { input: 0.003, output: 0.015, cacheRead: 0.0003, cacheWrite: 0.00375, total: 0.02205 };
      app.store.recordUsage(started.revision.id, "anthropic/claude-x", { input: 1000, output: 1000, cacheRead: 1000, cacheWrite: 1000, totalTokens: 4000, cost });
      app.store.recordUsage("no-such-revision", "anthropic/claude-x", { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost });
      expect(app.store.db.prepare("SELECT revision_id, pr_id, model, input, output, cache_read, cache_write, cost_input, cost_output, cost_cache_read, cost_cache_write, cost_total FROM model_usage").all()).toEqual([
        {
          revision_id: started.revision.id,
          pr_id: started.revision.pr_id,
          model: "anthropic/claude-x",
          input: 1000,
          output: 1000,
          cache_read: 1000,
          cache_write: 1000,
          cost_input: 0.003,
          cost_output: 0.015,
          cost_cache_read: 0.0003,
          cost_cache_write: 0.00375,
          cost_total: 0.02205,
        },
      ]);
    } finally {
      await app.close();
    }
  }, 30_000);

  it("reads a usage row whose PR is gone with no PR, and a PR stored before titles with its run's recorded title", () => {
    const store = AppStore.open(join(mkdtempSync(join(tmpdir(), "usage-")), "app.sqlite"));
    try {
      const pull = createFixture().pull;
      const prId = store.upsertPull({ ...pull, author: null });
      store.db.prepare("UPDATE pull_requests SET title = NULL WHERE id = ?").run(prId);
      const rev = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
      store.saveSnapshot({ revisionId: rev.id, pr: { title: "Recorded title", body: "", fullName: "acme/shop", number: 1, cloneUrl: "" }, changedFiles: [], diff: "", limitations: [] });
      const insert = store.db.prepare(
        "INSERT INTO model_usage (revision_id, pr_id, at, model, input, output, cache_read, cache_write, cost_input, cost_output, cost_cache_read, cost_cache_write, cost_total) VALUES (?, ?, '2026-10-28T09:00:00Z', 'm', 1, 1, 0, 0, 0, 0, 0, 0, 0.1)",
      );
      insert.run(rev.id, prId);
      insert.run("rev-gone", "pr-gone");
      expect(usageRows(store.db, "2026-01-01T00:00:00Z").map((r) => r.pr)).toEqual([{ id: prId, repository: "acme/shop", number: 1, title: "Recorded title" }, undefined]);
    } finally {
      store.close();
    }
  });

  const report = (rows: UsageRow[]) => usageReport(rows, { by: "day", tz: "UTC", now: Date.now() });

  it("records one row per model response of a review, priced and attributed, and the report sums them", async () => {
    const fixture = createFixture();
    const app = await open(mkdtempSync(join(tmpdir(), "usage-")), fixture, [readDiff(), submitDefect(fixture)]);
    try {
      const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
      expect(revision.state).toBe("COMPLETED");
      const rows = usageRows(app.store.db, "1970-01-01T00:00:00Z");
      expect(rows).toHaveLength(2);
      for (const r of rows) expect(r).toMatchObject({ revisionId: revision.id, model: "faux/reviewer" });
      expect(rows.every((r) => r.input > 0 && r.output > 0)).toBe(true);
      expect(rows[0]!.pr).toEqual({ id: revision.pr_id, repository: "acme/shop", number: 1, title: "Add order deletion", author: "octocat" });
      expect(report(rows).totals).toMatchObject({
        reviews: 1,
        responses: 2,
        input: rows[0]!.input + rows[1]!.input,
        output: rows[0]!.output + rows[1]!.output,
      });
      expect(app.store.db.prepare("SELECT DISTINCT pr_id FROM model_usage").all()).toEqual([{ pr_id: revision.pr_id }]);
    } finally {
      await app.close();
    }
  }, 30_000);

  it("records every response that stopped with an error, each retry included", async () => {
    const fixture = createFixture();
    const faux = fauxProvider({ provider: "faux", models: [{ id: "reviewer", reasoning: true }] });
    faux.setResponses([fauxAssistantMessage("", { stopReason: "error", errorMessage: "invalid request" })]);
    const models = createModels();
    models.setProvider(faux.provider);
    const app = await openApp({
      dataDir: mkdtempSync(join(tmpdir(), "usage-")),
      config: DEFAULT_CONFIG,
      pulls: fixture.pulls,
      engine: async () => ({ skipped: "Jev disabled by --no-jev" }),
      models,
      reviewerVersion: "m1+faux/reviewer",
    });
    try {
      const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
      expect(revision.state).toBe("FAILED");
      const rows = usageRows(app.store.db, "1970-01-01T00:00:00Z");
      expect(faux.state.callCount).toBeGreaterThan(1);
      expect(rows).toHaveLength(faux.state.callCount);
      expect(rows.every((r) => r.input > 0 && r.output === 0)).toBe(true);
    } finally {
      await app.close();
    }
  }, 30_000);

  it("does not count a response twice when a review killed mid-ANALYZE resumes", async () => {
    const fixture = createFixture();
    const dataDir = mkdtempSync(join(tmpdir(), "usage-resume-"));
    let inFlight!: () => void;
    const reachedAnalyze = new Promise<void>((resolve) => (inFlight = resolve));
    const blocked: FauxResponseStep = (_context, options) =>
      new Promise((resolve) => {
        inFlight();
        options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
      });

    const first = await open(dataDir, fixture, [readDiff(), blocked]);
    const started = await startReview(first, fixture.pull, "1");
    first.harness.resume();
    await reachedAnalyze;
    // The read_diff answer is committed and recorded; the request in flight is cut off before it answers.
    expect(usageRows(first.store.db, "1970-01-01T00:00:00Z")).toHaveLength(1);
    await first.close();

    const second = await open(dataDir, fixture, [submitDefect(fixture)]);
    try {
      second.harness.resume();
      const revision = await finishReview(second, await startReview(second, fixture.pull, "1"));
      expect(revision.state).toBe("COMPLETED");
      const rows = usageRows(second.store.db, "1970-01-01T00:00:00Z");
      expect(rows).toHaveLength(2);
      expect(report(rows).totals).toMatchObject({ reviews: 1, responses: 2 });
    } finally {
      await second.close();
    }
  }, 30_000);
});

describe("pr-review usage", () => {
  const run = async (args: string[], dataDir: string) => {
    let out = "";
    let err = "";
    process.env.PR_REVIEW_DATA_DIR = dataDir;
    try {
      const code = await main(["usage", ...args], { out: (t) => (out += t), err: (t) => (err += t) });
      return { code, out, err };
    } finally {
      delete process.env.PR_REVIEW_DATA_DIR;
    }
  };

  it("prints a table with totals and the model split, or JSON, from the store", async () => {
    const dataDir = mkdtempSync(join(tmpdir(), "usage-cli-"));
    const store = AppStore.open(join(dataDir, "app.sqlite"));
    const prId = store.upsertPull(createFixture().pull);
    const revision = store.ensureRevision({ prId, baseSha: "b".repeat(40), headSha: "h".repeat(40), policyVersion: "1", reviewerVersion: "m1" });
    const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 1.234 };
    store.recordUsage(revision.id, "anthropic/claude-x", { input: 1500, output: 2_500_000, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost });
    store.close();

    const text = await run(["--by", "week"], dataDir);
    expect(text.code).toBe(0);
    expect(text.out).toContain("Estimated at the model's list price in USD");
    const lines = text.out.split("\n");
    expect(lines[1]).toMatch(/^Week of\s+Reviews\s+Responses\s+Input\s+Output\s+Cache read\s+Cache write\s+Cost$/);
    expect(lines.filter((l) => /^\d{4}-\d{2}-\d{2} /.test(l))).toHaveLength(12);
    expect(lines.find((l) => l.startsWith("Total"))).toMatch(/^Total\s+1\s+1\s+1\.5K\s+2\.5M\s+0\s+0\s+\$1\.23$/);
    expect(text.out).toContain("anthropic/claude-x  1 review(s)  1 response(s)  $1.23");

    const json = await run(["--by", "month", "--json"], dataDir);
    const parsed = JSON.parse(json.out);
    expect(parsed).toMatchObject({ by: "month", currency: "USD", priceSource: "catalog", totals: { responses: 1, cost: 1.234 } });
    expect(parsed.buckets).toHaveLength(12);
  });

  it("splits the range by repository, author or PR, with each row's share and the same totals", async () => {
    const dataDir = mkdtempSync(join(tmpdir(), "usage-cli-"));
    const store = AppStore.open(join(dataDir, "app.sqlite"));
    const pull = createFixture().pull;
    const cost = (total: number) => ({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total });
    for (const [n, author, total] of [[1, "octocat", 0.75], [2, null, 0.25]] as const) {
      const prId = store.upsertPull({ ...pull, number: n, title: `PR ${n}`, author });
      const revision = store.ensureRevision({ prId, baseSha: "b".repeat(40), headSha: "h".repeat(40), policyVersion: "1", reviewerVersion: "m1" });
      store.recordUsage(revision.id, "anthropic/claude-x", { input: 1000, output: 100, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: cost(total) });
    }
    store.close();

    const byAuthor = (await run(["--by", "week", "--group", "author"], dataDir)).out.split("\n");
    expect(byAuthor[0]).toMatch(/^Usage by week from \d{4}-\d{2}-\d{2} to \d{4}-\d{2}-\d{2}, split by author, /);
    expect(byAuthor[1]).toMatch(/^Author\s+Reviews\s+Responses\s+Input\s+Output\s+Cache read\s+Cache write\s+Cost\s+Share$/);
    expect(byAuthor[2]).toMatch(/^octocat\s+1\s+1\s+1K\s+100\s+0\s+0\s+\$0\.75\s+75\.0%$/);
    expect(byAuthor[3]).toMatch(/^Unknown author\s+1\s+1\s+1K\s+100\s+0\s+0\s+\$0\.25\s+25\.0%$/);
    expect(byAuthor[4]).toMatch(/^Total\s+2\s+2\s+2K\s+200\s+0\s+0\s+\$1\.00\s+100%$/);
    const byPr = (await run(["--group", "pr"], dataDir)).out;
    expect(byPr).toContain(", split by PR, ");
    expect(byPr).toContain("acme/shop#1 PR 1 ");
    const json = JSON.parse((await run(["--group", "repo", "--json"], dataDir)).out);
    expect(json).toMatchObject({ group: "repo", totals: { cost: 1 }, groups: [{ key: "acme/shop", label: "acme/shop", cost: 1, share: 1 }] });
    expect(await run(["--group", "model"], dataDir)).toMatchObject({ code: 1, err: "pr-review: --group must be one of time, repo, author, pr\n" });
  });

  it("says when nothing is recorded, and refuses an unknown grouping", async () => {
    const dataDir = mkdtempSync(join(tmpdir(), "usage-cli-"));
    expect((await run([], dataDir)).out).toContain("No usage recorded in this range. Usage is recorded from this version on.");
    expect(await run(["--by", "year"], dataDir)).toMatchObject({ code: 1, err: "pr-review: --by must be one of day, week, month\n" });
  });
});
