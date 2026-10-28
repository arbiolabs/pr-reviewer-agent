import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createModels, type FauxResponseStep, fauxAssistantMessage, fauxProvider } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import { type UsageRow, usageReport, usageSince, usageStart } from "../src/domain/usage.ts";
import { usageRows } from "../src/persistence/store.ts";
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
