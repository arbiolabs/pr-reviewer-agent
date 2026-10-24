import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { AppStore } from "../src/persistence/store.ts";
import { createFixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

const URL = "https://github.com/acme/shop/pull/1";

const FULL_RUN = [
  "COLLECT_CONTEXT CONTEXT_PERSISTED TRIAGE",
  "TRIAGE ROUTING_PERSISTED ANALYZE",
  "ANALYZE CANDIDATES_PERSISTED VERIFY",
  "VERIFY CANDIDATES_DISPOSED PREPARE_PUBLICATION",
  "PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH",
  "PUBLISH NO_PUBLICATION_RECORDED COMPLETED",
];

it("reviews a completed head again from scratch as a new revision, setting the old one aside and tracking its finding", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "again-"));
  const errors: string[] = [];
  const deps = { pulls: fixture.pulls, models: fauxModels([readDiff(), submitDefect(fixture), readDiff(), submitDefect(fixture)]), isTTY: false, out: () => {}, err: (t: string) => errors.push(t) };
  const review = (...flags: string[]) => main(["review", URL, "--no-jev", "--model", "faux/reviewer", ...flags], deps);
  process.env.PR_REVIEW_DATA_DIR = dataDir;
  try {
    expect(await review("--retry", "--again")).toBe(2);
    expect(await review()).toBe(0);
    expect(await review("--again")).toBe(0);
    // A plain rerun answers with the head's latest review, not the one it set aside.
    errors.length = 0;
    expect(await review()).toBe(0);
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
  }

  const store = AppStore.open(join(dataDir, "app.sqlite"));
  try {
    const [first, again] = store.revisionsForPr(store.pullByName("acme/shop", 1)!.id);
    expect([first!.state, again!.state, again!.policy_version, again!.head_sha]).toEqual(["SUPERSEDED", "COMPLETED", "1+again1", first!.head_sha]);
    expect(errors.join("")).toContain(`revision ${again!.id} already COMPLETED; showing the stored result\n`);
    const transitions = (id: string) => store.auditEvents(id).map((e) => `${e.from_state} ${e.event} ${e.to_state}`);
    expect(transitions(first!.id)).toEqual([...FULL_RUN, "COMPLETED REVIEW_AGAIN SUPERSEDED"]);
    expect(store.auditEvents(first!.id).at(-1)!.detail).toBe("manual override: pr-review review --again (review again 1)");
    // Every phase ran again: its own context snapshot and durable task, nothing carried over from the first attempt.
    expect(transitions(again!.id)).toEqual(FULL_RUN);
    expect(store.snapshot(again!.id)!.id).not.toBe(store.snapshot(first!.id)!.id);
    expect(again!.durable_task_id).not.toBe(first!.durable_task_id);
    // The same defect found again is the same finding, observed by both revisions, not a second one.
    expect(store.countFindings(again!.pr_id)).toBe(1);
    const observedBy = store.db.prepare("SELECT revision_id FROM finding_observations ORDER BY created_at").all().map((r) => (r as { revision_id: string }).revision_id);
    expect(observedBy).toEqual([first!.id, again!.id]);
  } finally {
    store.close();
  }
}, 60_000);
