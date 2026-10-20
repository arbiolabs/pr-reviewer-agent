import { expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { askJev, type JevJudgment, jevSkipReason, MockDecisionEngine, selectReviewMode } from "../src/jev/decision.ts";

const input = { title: "t", body: "", changedFiles: [], diffExcerpt: "" };

const judgment = (overrides: Partial<JevJudgment>): JevJudgment => ({
  primaryCategory: "general",
  categoryProbabilities: { general: 0.9 },
  riskScore: 0.1,
  routingConfidence: 0.8,
  needsDeepReviewProbability: 0.1,
  adapterVersion: "test",
  modelId: "mock",
  ...overrides,
});

it("falls back to standard when Jev throws", async () => {
  const engine = new MockDecisionEngine(() => {
    throw new Error("503 overloaded");
  });
  const { outcome } = await askJev(engine, input, 1000);
  expect(outcome).toEqual({ ok: false, error: "503 overloaded" });
  expect(selectReviewMode(outcome, ["src/util.ts"], DEFAULT_CONFIG)).toEqual({
    mode: "standard",
    reasons: ["Jev failed: 503 overloaded; using standard"],
  });
});

it("falls back to standard when Jev never answers", async () => {
  const engine = { provider: "slow", modelId: "slow", triage: () => new Promise<never>(() => {}) };
  expect((await askJev(engine, input, 20)).outcome).toEqual({ ok: false, error: "timed out after 20 ms" });
});

it("upgrades a low Jev risk to critical on an auth path", () => {
  expect(selectReviewMode({ ok: true, judgment: judgment({}) }, ["src/auth/session.ts", "README.md"], DEFAULT_CONFIG)).toEqual({
    mode: "critical",
    reasons: ["Jev needs-deep-review probability 0.10 suggests standard", "mandatory deep path rule matched: src/auth/session.ts"],
  });
});

it("a zero Jev risk still gets a standard review, never less", () => {
  expect(selectReviewMode({ ok: true, judgment: judgment({ needsDeepReviewProbability: 0, riskScore: 0 }) }, [], DEFAULT_CONFIG)).toEqual({
    mode: "standard",
    reasons: ["Jev needs-deep-review probability 0.00 suggests standard"],
  });
});

it("lets Jev raise the mode, and treats null confidence or out-of-range values as failure", () => {
  expect(selectReviewMode({ ok: true, judgment: judgment({ needsDeepReviewProbability: 0.75 }) }, [], DEFAULT_CONFIG).mode).toBe("deep");
  expect(selectReviewMode({ ok: true, judgment: judgment({ needsDeepReviewProbability: 0.95 }) }, [], DEFAULT_CONFIG).mode).toBe("critical");
  expect(selectReviewMode({ ok: true, judgment: judgment({ needsDeepReviewProbability: 0.95, routingConfidence: null }) }, [], DEFAULT_CONFIG)).toEqual({
    mode: "standard",
    reasons: ["Jev returned no routing confidence; using standard"],
  });
  expect(selectReviewMode({ ok: true, judgment: judgment({ riskScore: 3 }) }, [], DEFAULT_CONFIG).reasons).toEqual([
    "Jev returned invalid riskScore 3; using standard",
  ]);
});

it("skips Jev for a private repository unless it is keyed or the config allows keyless", () => {
  const base = { noJev: false, keyed: false, privateRepo: true, allowKeylessForPrivate: false };
  expect(jevSkipReason(base)).toBe("jev_skipped_private_repo");
  expect(jevSkipReason({ ...base, keyed: true })).toBe(undefined);
  expect(jevSkipReason({ ...base, allowKeylessForPrivate: true })).toBe(undefined);
  expect(jevSkipReason({ ...base, privateRepo: false })).toBe(undefined);
  expect(jevSkipReason({ ...base, keyed: true, noJev: true })).toBe("Jev disabled by --no-jev");
});
