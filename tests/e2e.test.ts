import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { loadReport, renderMarkdown } from "../src/cli/report.ts";
import { finishReview, startReview } from "../src/runtime/app.ts";
import { createFixture } from "./fixture.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";

it("reviews a fixture PR end to end and names the defect at its file and line", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "e2e-"));
  const app = await open(dataDir, fixture, [readDiff(), submitDefect(fixture)]);
  try {
    const started = await startReview(app, fixture.pull, "1");
    const revision = await finishReview(app, started);
    expect(revision.state).toBe("COMPLETED");
    expect(renderMarkdown(loadReport(app.store, revision.id))).toBe(`## Automated PR review

Reviewed commit: \`${fixture.headSha}\`

Found 1 actionable issue.

- High: Order deletion endpoint skips the admin check (\`src/routes/orders.ts:5\`)

Review performed with isolated read-only tools. Human review is still required.
`);
    expect(app.store.auditEvents(revision.id).map((e) => `${e.from_state} ${e.event} ${e.to_state}`)).toEqual([
      "COLLECT_CONTEXT CONTEXT_PERSISTED TRIAGE",
      "TRIAGE ROUTING_PERSISTED ANALYZE",
      "ANALYZE CANDIDATES_PERSISTED VERIFY",
      "VERIFY CANDIDATES_DISPOSED PREPARE_PUBLICATION",
      "PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH",
      "PUBLISH NO_PUBLICATION_RECORDED COMPLETED",
    ]);
    const again = await startReview(app, fixture.pull, "1");
    expect([again.revision.id, again.taskId]).toEqual([revision.id, undefined]);
  } finally {
    await app.close();
  }
}, 30_000);

it("reports zero findings when the model submits an empty list", async () => {
  const fixture = createFixture();
  const app = await open(mkdtempSync(join(tmpdir(), "e2e-")), fixture, [
    fauxAssistantMessage([fauxToolCall("submit_candidates", { candidates: [] })], { stopReason: "toolUse" }),
  ]);
  try {
    const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
    expect(renderMarkdown(loadReport(app.store, revision.id))).toContain("No actionable issues found in the reviewed scope.\n");
  } finally {
    await app.close();
  }
}, 30_000);

it("supersedes the revision when the head moves before publication", async () => {
  const fixture = createFixture();
  const app = await open(mkdtempSync(join(tmpdir(), "e2e-")), fixture, [
    async () => {
      fixture.pulls.pull = { ...fixture.pull, headSha: "f".repeat(40) };
      return fauxAssistantMessage([fauxToolCall("submit_candidates", { candidates: [] })], { stopReason: "toolUse" });
    },
  ]);
  try {
    const revision = await finishReview(app, await startReview(app, fixture.pull, "1"));
    expect(revision.state).toBe("SUPERSEDED");
    expect(app.store.auditEvents(revision.id).at(-1)).toMatchObject({ event: "NEWER_HEAD", detail: `head moved to ${"f".repeat(40)}` });
  } finally {
    await app.close();
  }
}, 30_000);
