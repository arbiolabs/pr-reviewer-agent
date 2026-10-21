import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeAll, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import type { CandidateFinding } from "../src/domain/types.ts";
import { fingerprint, type VerifyContext, verifyCandidates } from "../src/reasoning/verify.ts";
import { parseDiff } from "../src/source/diff.ts";
import { PinnedRepo, syncRepo } from "../src/source/git.ts";
import { createFixture, DEFECT, type Fixture } from "./fixture.ts";

let fixture: Fixture;
let ctx: VerifyContext;

beforeAll(async () => {
  fixture = createFixture();
  const repo = new PinnedRepo(await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/shop", fixture.repoDir, 1), fixture.baseSha, fixture.headSha);
  ctx = {
    prId: "pr-1",
    revisionId: "rev-1",
    baseSha: fixture.baseSha,
    headSha: fixture.headSha,
    files: parseDiff(await repo.diff()),
    read: (sha, path) => repo.show(sha, path),
    prior: [],
    thresholds: DEFAULT_CONFIG.verification,
  };
});

const candidate = (overrides: Partial<CandidateFinding>): CandidateFinding => ({
  id: "c1",
  category: "security",
  severity: "high",
  title: "Order deletion endpoint skips the admin check",
  explanation: "Every other order route is guarded by requireAdmin; DELETE /orders/:id is not.",
  file: DEFECT.file,
  newLine: DEFECT.line,
  codeEvidence: [{ path: DEFECT.file, start: 4, end: 5, refSha: fixture.headSha }],
  reasoningSummary: "Compared the new route with its sibling.",
  confidence: 0.9,
  evidenceKind: "confirmed_by_static_trace",
  ...overrides,
});

async function disposition(c: CandidateFinding) {
  const { dispositions } = await verifyCandidates([c], ctx);
  return dispositions[0]!;
}

it("verifies a candidate anchored on an added line with in-range evidence", async () => {
  const c = candidate({});
  const result = await verifyCandidates([c], ctx);
  expect(result.dispositions).toEqual([
    {
      candidateId: "c1",
      state: "VERIFIED",
      path: ["DISCOVERED", "INVESTIGATING", "EVIDENCE_READY", "VERIFIED"],
      reason: null,
      findingId: result.verified[0]!.findingId,
    },
  ]);
  expect(result.verified.map((v) => v.fingerprint)).toEqual([fingerprint(c)]);
});

it("discards an anchor outside every head-side hunk", async () => {
  expect(await disposition(candidate({ newLine: 1 }))).toMatchObject({
    state: "DISCARDED",
    path: ["DISCOVERED", "INVESTIGATING", "DISCARDED"],
    reason: "src/routes/orders.ts:1 is outside every hunk of the head-side diff",
  });
});

it("discards evidence past the end of the cited file", async () => {
  const d = await disposition(candidate({ codeEvidence: [{ path: DEFECT.file, start: 4, end: 40, refSha: fixture.headSha }] }));
  expect(d.reason).toBe(`evidence src/routes/orders.ts:4-40@${fixture.headSha.slice(0, 12)} is outside 1-6`);
});

it("discards a file outside the changed set", async () => {
  expect((await disposition(candidate({ file: "src/auth/require-admin.ts", newLine: 2 }))).reason).toBe(
    "src/auth/require-admin.ts is not in the changed set",
  );
});

it("rejects traversal in evidence paths", async () => {
  const d = await disposition(candidate({ codeEvidence: [{ path: "../../etc/passwd", start: 1, end: 1, refSha: fixture.headSha }] }));
  expect(d.reason).toBe("path traversal rejected: ../../etc/passwd");
});

it("sends suspected evidence through INCONCLUSIVE to DISCARDED", async () => {
  expect(await disposition(candidate({ evidenceKind: "suspected" }))).toMatchObject({
    state: "DISCARDED",
    path: ["DISCOVERED", "INVESTIGATING", "INCONCLUSIVE", "DISCARDED"],
    reason: "evidence is suspected",
  });
});

it("applies severity and confidence thresholds, and dedupes by fingerprint", async () => {
  expect((await disposition(candidate({ severity: "low" }))).reason).toBe("severity low is below medium");
  expect((await disposition(candidate({ confidence: 0.2 }))).reason).toBe("confidence 0.2 is below 0.5");
  const twice = await verifyCandidates([candidate({}), candidate({ id: "c2", title: "Admin check skips order deletion endpoint" })], ctx);
  expect(twice.dispositions.map((d) => [d.candidateId, d.state, d.reason])).toEqual([
    ["c1", "VERIFIED", null],
    ["c2", "DISCARDED", "duplicate of candidate c1"],
  ]);
});
