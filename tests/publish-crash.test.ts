import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { AppStore } from "../src/persistence/store.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture } from "./fixture.ts";

const SCRIPT = new URL("./fixtures/cli-publish.ts", import.meta.url).pathname;
let fake: FakeGitHub;
beforeEach(async () => (fake = await FakeGitHub.start()));
afterEach(() => fake.close());

function run(env: Record<string, string>): Promise<{ code: number | null; signal: string | null; stderr: string }> {
  const child = spawn(process.execPath, [SCRIPT], { env: { ...process.env, ...env } });
  let stderr = "";
  child.stderr.on("data", (d) => (stderr += d));
  return new Promise((resolve) => child.on("exit", (code, signal) => resolve({ code, signal, stderr })));
}

async function killThenRestart(kill: "before-post" | "after-response") {
  const fixture = createFixture();
  fake.setPull(fixture.pull);
  const dataDir = mkdtempSync(join(tmpdir(), "publish-kill-"));
  // Reconciliation waits a client timeout before each listing; keep it short.
  const configPath = join(dataDir, "config.json");
  writeFileSync(configPath, JSON.stringify({ github: { timeout_ms: 300 } }));
  const env = { FIXTURE_JSON: JSON.stringify(fixture), PR_REVIEW_DATA_DIR: dataDir, FAKE_URL: fake.url, PR_REVIEW_CONFIG: configPath };
  const first = await run({ ...env, KILL: kill });
  expect(first.signal).toBe("SIGKILL");
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  const killed = store.db.prepare("SELECT r.state, o.status, o.attempt_count, o.github_review_id FROM publication_outbox o JOIN review_revisions r ON r.id = o.revision_id").all();
  const reviewsAfterKill = fake.reviews.length;
  store.close();
  // The killed run's data-dir lock would make the restart wait 5 s for it to go stale; cli.test covers that takeover.
  rmSync(join(dataDir, "lock"), { recursive: true });
  const second = await run(env);
  const after = AppStore.open(join(dataDir, "app.sqlite"));
  const outbox = after.db.prepare("SELECT * FROM publication_outbox").get() as { id: string; status: string; github_review_id: number };
  const result = {
    killed,
    reviewsAfterKill,
    exit: second.code,
    revision: after.db.prepare("SELECT state FROM review_revisions").get(),
    outbox: [outbox.status, outbox.github_review_id],
    finding: after.db.prepare("SELECT status, github_review_id FROM findings").get(),
    events: after.outboxEvents(outbox.id).map((e) => `${e.from_status} ${e.event} ${e.to_status}`),
    postAttempts: fake.postAttempts,
    reviews: fake.reviews.map((r) => r.id),
  };
  after.close();
  return result;
}

it("(a) SIGKILL after sending is committed but before the POST: restart reconciles absent, then posts exactly once", async () => {
  expect(await killThenRestart("before-post")).toEqual({
    killed: [{ state: "PUBLISH", status: "sending", attempt_count: 1, github_review_id: null }],
    reviewsAfterKill: 0,
    exit: 0,
    revision: { state: "COMPLETED" },
    outbox: ["published", 1000],
    finding: { status: "PUBLISHED", github_review_id: 1000 },
    events: [
      "pending SEND sending",
      "sending UNCERTAIN needs_reconciliation",
      "needs_reconciliation ABSENT pending",
      "pending SEND sending",
      "sending SENT published",
    ],
    postAttempts: 1,
    reviews: [1000],
  });
}, 60_000);

it("(c) SIGKILL after GitHub answered but before github_review_id is saved: restart finds the review and never re-POSTs", async () => {
  expect(await killThenRestart("after-response")).toEqual({
    killed: [{ state: "PUBLISH", status: "sending", attempt_count: 1, github_review_id: null }],
    reviewsAfterKill: 1,
    exit: 0,
    revision: { state: "COMPLETED" },
    outbox: ["published", 1000],
    finding: { status: "PUBLISHED", github_review_id: 1000 },
    events: ["pending SEND sending", "sending UNCERTAIN needs_reconciliation", "needs_reconciliation FOUND published"],
    postAttempts: 1,
    reviews: [1000],
  });
}, 60_000);
