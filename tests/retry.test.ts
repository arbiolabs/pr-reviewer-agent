import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { AppStore } from "../src/persistence/store.ts";
import { finishReview, startReview } from "../src/runtime/app.ts";
import { createFixture } from "./fixture.ts";
import { fauxModels, open, readDiff, submitDefect } from "./helpers.ts";

const URL = "https://github.com/acme/shop/pull/1";

it("keeps a FAILED revision failed until --retry, which is an audited override with a new durable task", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "retry-"));
  const noRetries = { ...DEFAULT_CONFIG, review: { ...DEFAULT_CONFIG.review, max_retries_per_phase: 0 } };
  const first = await open(dataDir, fixture, [fauxAssistantMessage("Looks fine to me.")], noRetries);
  const failed = await finishReview(first, await startReview(first, fixture.pull, "1"));
  await first.close();
  expect(failed.state).toBe("FAILED");

  const errors: string[] = [];
  const deps = { pulls: fixture.pulls, models: fauxModels([readDiff(), submitDefect(fixture)]), isTTY: false, out: () => {}, err: (t: string) => errors.push(t) };
  process.env.PR_REVIEW_DATA_DIR = dataDir;
  try {
    expect(await main(["review", URL, "--no-jev", "--model", "faux/reviewer"], deps)).toBe(1);
    expect(errors.join("")).toContain(
      `revision ${failed.id} FAILED: analyze failed after 1 attempts: the model answered without calling submit_candidates; rerun with --retry to try again`,
    );
    expect(await main(["review", URL, "--no-jev", "--model", "faux/reviewer", "--retry"], deps)).toBe(0);
    errors.length = 0;
    expect(await main(["review", URL, "--no-jev", "--model", "faux/reviewer", "--retry"], deps)).toBe(1);
    expect(errors.at(-1)).toBe("pr-review: MANUAL_RETRY is not legal in COMPLETED\n");
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
  }

  const store = AppStore.open(join(dataDir, "app.sqlite"));
  try {
    const revision = store.revision(failed.id)!;
    expect(revision.state).toBe("COMPLETED");
    expect(revision.durable_task_id).not.toBe(failed.durable_task_id);
    expect(store.auditEvents(failed.id).map((e) => `${e.from_state} ${e.event} ${e.to_state}`)).toEqual([
      "COLLECT_CONTEXT CONTEXT_PERSISTED TRIAGE",
      "TRIAGE ROUTING_PERSISTED ANALYZE",
      "ANALYZE TERMINAL_ERROR FAILED",
      "FAILED MANUAL_RETRY COLLECT_CONTEXT",
      "COLLECT_CONTEXT CONTEXT_PERSISTED TRIAGE",
      "TRIAGE ROUTING_PERSISTED ANALYZE",
      "ANALYZE CANDIDATES_PERSISTED VERIFY",
      "VERIFY CANDIDATES_DISPOSED PREPARE_PUBLICATION",
      "PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH",
      "PUBLISH NO_PUBLICATION_RECORDED COMPLETED",
    ]);
    expect(store.auditEvents(failed.id)[3]!.detail).toBe("manual override: pr-review review --retry (retry 1)");
    expect(store.countFindings(revision.pr_id)).toBe(1);
  } finally {
    store.close();
  }
}, 60_000);
