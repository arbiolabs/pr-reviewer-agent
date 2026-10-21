import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FauxResponseStep, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, it } from "vitest";
import { type Config, DEFAULT_CONFIG } from "../src/config/config.ts";
import { AppDbSource } from "../src/control/appdb.ts";
import { GitHubClient } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { reconcileOutbox } from "../src/publish/publisher.ts";
import { type App, finishReview, openApp, startReview } from "../src/runtime/app.ts";
import { approve } from "../src/runtime/commands.ts";
import { FakeGitHub } from "./fake-github.ts";
import { createFixture, DEFECT, type Fixture, pushCommit } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

let fake: FakeGitHub;
let fixture: Fixture;
const apps: App[] = [];

beforeEach(async () => {
  fake = await FakeGitHub.start();
  fixture = createFixture();
  fake.setPull(fixture.pull);
});
afterEach(async () => {
  for (const app of apps.splice(0)) await app.close();
  await fake.close();
});

const config: Config = { ...DEFAULT_CONFIG, review: { ...DEFAULT_CONFIG.review, retry_backoff_ms: 10 }, github: { ...DEFAULT_CONFIG.github, timeout_ms: 300 } };
const submitNothing = () => fauxAssistantMessage([fauxToolCall("submit_candidates", { candidates: [] })], { stopReason: "toolUse" });

/** One app session on `dataDir`; a second session on the same directory carries the store over, like a later `pr-review review`. */
async function open(responses: FauxResponseStep[], cfg = config, dataDir = mkdtempSync(join(tmpdir(), "tracking-"))) {
  const client = new GitHubClient({ auth: { kind: "token", token: "ghp_test" }, baseUrl: fake.url, maxRetries: 2, backoffMs: 1, concurrency: 2, timeoutMs: 5000, sleep: async () => {} });
  const pulls = new GitHubPullSource(() => client);
  const app = await openApp({ dataDir, config: cfg, pulls, github: () => client, engine: { skipped: "test" }, models: fauxModels(responses), model: { provider: "faux", modelId: "reviewer" }, reviewerVersion: "m1+faux/reviewer" });
  apps.push(app);
  app.harness.resume();
  return { app, pulls, dataDir, client };
}
async function close(app: App) {
  apps.splice(apps.indexOf(app), 1);
  await app.close();
}

/** The next head: an unrelated file changes, so the defect's lines stay where they were. */
let heads = 0;
function nextHead() {
  const pull = pushCommit(fixture, { "src/routes/users.ts": `export const users = [${heads++}];\n` });
  fake.setPull(pull);
  return pull;
}

const review = async (app: App, publish = true) => finishReview(app, await startReview(app, fixture.pulls.pull, "1", { publish }));
const inline = () => fake.reviews.flatMap((r) => (r.comments as { path: string; line: number }[]).map(({ path, line }) => ({ path, line })));
const findingRow = (app: App) => app.store.db.prepare("SELECT status, github_review_id FROM findings").get();
const observations = (app: App) =>
  app.store.db.prepare("SELECT disposition, basis, path_json FROM finding_observations ORDER BY created_at").all() as { disposition: string; basis: string | null; path_json: string }[];

it("posts one inline comment across two revisions and lists the finding as still present on the second", async () => {
  const one = await open([readDiff(), submitDefect(fixture)]);
  const first = await review(one.app);
  expect([first.state, inline().length]).toEqual(["COMPLETED", 1]);
  const finding = one.app.store.findingsForRevision(first.id)[0]!;
  expect(one.app.store.db.prepare("SELECT github_review_id FROM findings").get()).toEqual({ github_review_id: 1000 });
  await close(one.app);

  // The model reports the same defect again on the new head.
  const pull = nextHead();
  const { app, dataDir } = await open([readDiff(), submitDefect({ ...fixture, headSha: pull.headSha })], config, one.dataDir);
  const second = await review(app);
  expect([second.state, fake.reviews.length, inline().length]).toEqual(["COMPLETED", 2, 1]);
  const carried = app.store.findingsForRevision(second.id);
  expect(carried.map((f) => [f.findingId, f.status, f.carried, f.posted, f.anchored, f.candidate.newLine])).toEqual([[finding.findingId, "STILL_PRESENT", true, true, true, DEFECT.line]]);
  expect(fake.reviews[1]!.body).toContain(`Reviewed commit: \`${pull.headSha}\``);
  expect(fake.reviews[1]!.body).toContain("No new actionable issues found in the reviewed scope.");
  expect(fake.reviews[1]!.body).toContain(`Still present from an earlier commit:\n\n- High: Order deletion endpoint skips the admin check (\`src/routes/orders.ts:5\`, first reported on \`${fixture.headSha.slice(0, 7)}\`)`);
  expect(observations(app)).toEqual([
    { disposition: "DETECTED", basis: null, path_json: '["DETECTED"]' },
    { disposition: "STILL_PRESENT", basis: "reverified", path_json: '["PUBLISHED","STILL_PRESENT"]' },
  ]);
  expect(app.store.outboxForRevision(second.id)!.inline_finding_ids_json).toBe("[]");

  // The control stream replays each row with the status it had then, and the history carries the basis.
  const source = AppDbSource.open(dataDir, { PATH: "" });
  try {
    expect(source.eventsAfter(0, 1000).filter((e) => e.eventType === "finding.updated").map((e) => (e.data as { status: string }).status)).toEqual(["DETECTED", "PUBLISHED", "STILL_PRESENT", "STILL_PRESENT"]);
    expect((await source.finding(finding.findingId)).history.map(({ runId, disposition, basis }) => [runId === first.id ? "r1" : "r2", disposition, basis ?? null])).toEqual([
      ["r1", "DETECTED", null],
      ["r1", "PUBLISHED", null],
      ["r2", "STILL_PRESENT", "reverified"],
    ]);
    expect(source.snapshot().findings.map((f) => [f.runId === second.id, f.status, f.stale])).toEqual([[true, "STILL_PRESENT", false]]);
  } finally {
    await source.close();
  }
}, 30_000);

it("locates a finding the model did not report again, and posts its first inline comment when a dry run detected it", async () => {
  const { app } = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing()]);
  const first = await review(app, false);
  expect([first.state, fake.reviews.length, app.store.findingsForRevision(first.id)[0]!.status]).toEqual(["COMPLETED", 0, "DETECTED"]);
  nextHead();
  const second = await review(app);
  const [finding] = app.store.findingsForRevision(second.id);
  expect([second.state, finding!.status, finding!.carried, finding!.posted, fake.reviews.length]).toEqual(["COMPLETED", "STILL_PRESENT", true, true, 1]);
  expect(inline()).toEqual([{ path: DEFECT.file, line: DEFECT.line }]);
  expect(app.store.outboxForRevision(second.id)!.inline_finding_ids_json).toBe(JSON.stringify([finding!.findingId]));
  expect(observations(app).map((o) => [o.disposition, o.basis])).toEqual([
    ["DETECTED", null],
    ["STILL_PRESENT", "located"],
  ]);
  expect(app.store.db.prepare("SELECT status, github_review_id FROM findings").get()).toEqual({ status: "STILL_PRESENT", github_review_id: 1000 });
}, 30_000);

it("under silent_no_findings, a revision with nothing new posts nothing", async () => {
  const { app } = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing()], { ...config, review: { ...config.review, publish_mode: "silent_no_findings" } });
  await review(app);
  nextHead();
  const second = await review(app);
  expect([second.state, fake.reviews.length, app.store.findingsForRevision(second.id)[0]!.status]).toEqual(["COMPLETED", 1, "STILL_PRESENT"]);
  expect(app.store.auditEvents(second.id).at(-1)!.detail).toBe("nothing new to post: silent_no_findings");
}, 30_000);

it("completes an in-flight p1 outbox: its payload posts as stored and its findings are recorded as posted", async () => {
  const { app, pulls } = await open([readDiff(), submitDefect(fixture)], { ...config, review: { ...config.review, publish_mode: "approval" } });
  const parked = await review(app);
  const outbox = app.store.outboxForRevision(parked.id)!;
  expect(parked.state).toBe("AWAITING_APPROVAL");
  // A row the previous publisher stored before the upgrade: keyed p1, with no record of which findings it comments on.
  app.store.db.prepare("UPDATE publication_outbox SET idempotency_key = ?, inline_finding_ids_json = NULL WHERE id = ?").run(`${parked.id}:p1`, outbox.id);
  const done = await finishReview(app, await approve(app, pulls, { revisionId: parked.id, previewId: outbox.payload_hash }, "test"));
  expect([done.state, app.store.outbox(outbox.id)!.status, fake.postAttempts, inline().length]).toEqual(["COMPLETED", "published", 1, 1]);
  expect(findingRow(app)).toEqual({ status: "PUBLISHED", github_review_id: 1000 });
  expect(app.store.db.prepare("SELECT count(*) AS n FROM publication_outbox").get()).toEqual({ n: 1 });
}, 30_000);

it("comments once on a finding whose revision failed with its outbox pending: nothing of it ever reached GitHub", async () => {
  fake.postBehaviors = Array(20).fill("rate-limited");
  const { app } = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing()]);
  const first = await review(app);
  expect([first.state, app.store.outboxForRevision(first.id)!.status, fake.reviews.length]).toEqual(["FAILED", "pending", 0]);
  fake.postBehaviors = [];
  nextHead();
  const second = await review(app);
  expect([second.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 1, [{ path: DEFECT.file, line: DEFECT.line }]]);
  expect(findingRow(app)).toEqual({ status: "STILL_PRESENT", github_review_id: 1000 });
}, 30_000);

it.each([
  [
    "the listing before the first POST kept failing",
    () => {
      fake.listStatus = 500;
    },
  ],
  [
    "the POST was ambiguous and reconciliation could not list",
    () => {
      fake.postBehaviors = ["bad-gateway"];
      fake.onPost = () => {
        fake.listStatus = 500;
        fake.onPost = undefined;
      };
    },
  ],
])("keeps a finding whose outbox needs reconciliation out of later comments until reconcile proves it absent, then comments once (%s)", async (_case, setup) => {
  setup();
  const { app, client } = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing(), readDiff(), submitNothing()]);
  const first = await review(app);
  const outbox = app.store.outboxForRevision(first.id)!;
  expect([first.state, outbox.status, fake.reviews.length]).toEqual(["FAILED", "needs_reconciliation", 0]);
  fake.listStatus = 200;
  // The write may have landed: the next head lists the finding as still present and does not comment on it.
  nextHead();
  const second = await review(app);
  expect([second.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 1, []]);
  expect(app.store.findingsForRevision(second.id).map((f) => [f.status, f.posted])).toEqual([["STILL_PRESENT", true]]);
  // The operator reconciles: absent, so the outbox is pending on its failed revision and the finding is not posted.
  expect(await reconcileOutbox(app.store, client, { fullName: fixture.pull.fullName, number: fixture.pull.number }, outbox)).toEqual({ kind: "absent" });
  expect(app.store.outbox(outbox.id)!.status).toBe("pending");
  nextHead();
  const third = await review(app);
  expect([third.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 2, [{ path: DEFECT.file, line: DEFECT.line }]]);
  expect(findingRow(app)).toEqual({ status: "STILL_PRESENT", github_review_id: 1001 });
}, 60_000);
