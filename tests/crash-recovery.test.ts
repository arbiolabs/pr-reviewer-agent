import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { type FauxResponseStep, fauxAssistantMessage } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { finishReview, startReview } from "../src/runtime/app.ts";
import { REVIEW_TASK_NAME } from "../src/runtime/task.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";
import { createFixture } from "./fixture.ts";

/** A model response that never arrives: it reports that ANALYZE is mid-inference, then waits to be aborted. */
function blockUntilAborted(onInFlight: () => void): FauxResponseStep {
  return (_context, options) =>
    new Promise((resolve) => {
      onInFlight();
      options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
    });
}

it("recovers a review killed mid-ANALYZE with one candidate set, one conversation and no duplicate findings", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "crash-"));
  let inFlight!: () => void;
  const reachedAnalyze = new Promise<void>((resolve) => (inFlight = resolve));

  const first = await open(dataDir, fixture, [readDiff(), blockUntilAborted(() => inFlight())]);
  const started = await startReview(first, fixture.pull, "1");
  first.harness.resume();
  await reachedAnalyze;
  expect(first.store.revision(started.revision.id)!.state).toBe("ANALYZE");
  await first.close();

  const second = await open(dataDir, fixture, [submitDefect(fixture)]);
  try {
    second.harness.resume();
    const restarted = await startReview(second, fixture.pull, "1");
    expect(restarted.taskId).toBe(started.taskId);
    const revision = await finishReview(second, restarted);
    expect(revision.state).toBe("COMPLETED");
    expect(second.store.countCandidateSets(revision.id)).toBe(1);
    expect(second.store.countFindings(revision.pr_id)).toBe(1);
    expect(second.store.findingsForRevision(revision.id).map((f) => [f.candidate.file, f.candidate.newLine])).toEqual([["src/routes/orders.ts", 5]]);
    const conversations = await second.harness.commit((tx) => tx.scanConversations({ ownerTaskId: started.taskId! }, 10), BACKGROUND_CONTEXT);
    expect(conversations.items.length).toBe(1);
    const tasks = await second.harness.commit((tx) => tx.scanTasks({ kind: REVIEW_TASK_NAME }, 10), BACKGROUND_CONTEXT);
    expect(tasks.items.map((t) => t.state.status)).toEqual(["terminal"]);
    expect(second.store.auditEvents(revision.id).map((e) => e.to_state)).toEqual([
      "TRIAGE",
      "ANALYZE",
      "VERIFY",
      "PREPARE_PUBLICATION",
      "PUBLISH",
      "COMPLETED",
    ]);
  } finally {
    await second.close();
  }
}, 30_000);
