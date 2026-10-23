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
import { createFixture, DEFECT, type Fixture, pushCommit, ROUTES_HEAD } from "./fixture.ts";
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
  const app = await openApp({ dataDir, config: cfg, pulls, github: () => client, engine: async () => ({ skipped: "test" }), models: fauxModels(responses), reviewerVersion: "m1+faux/reviewer" });
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

const submitResolution = (findingId: string, headSha: string) =>
  fauxAssistantMessage(
    [fauxToolCall("submit_candidates", { candidates: [], resolutions: [{ findingId, codeEvidence: [{ path: DEFECT.file, start: DEFECT.line, end: DEFECT.line, refSha: headSha }], reason: "the route now requires admin", evidenceKind: "confirmed_by_static_trace" }] })],
    { stopReason: "toolUse" },
  );

const findingRows = (app: App) => app.store.db.prepare("SELECT id, status, reopens FROM findings ORDER BY created_at").all() as { id: string; status: string; reopens: string | null }[];

it("resolves a finding on a checked claim, keeps it resolved while the fix holds, and when the defect comes back makes one new finding that the model's re-report joins", async () => {
  const silent: Config = { ...config, review: { ...config.review, publish_mode: "silent_no_findings" } };
  const one = await open([readDiff(), submitDefect(fixture)], silent);
  const first = await review(one.app);
  const [finding] = one.app.store.findingsForRevision(first.id);
  await close(one.app);

  // The fix changes the cited line, and the model claims the finding resolved with that line as evidence.
  const fixed = pushCommit(fixture, { [DEFECT.file]: ROUTES_HEAD.replace('app.delete("/orders/:id", deleteOrder);', 'app.delete("/orders/:id", requireAdmin, deleteOrder);') });
  fake.setPull(fixed);
  const two = await open([readDiff(), submitResolution(finding!.findingId, fixed.headSha), readDiff(), submitNothing()], silent, one.dataDir);
  const second = await review(two.app);
  expect([second.state, fake.reviews.length, inline().length]).toEqual(["COMPLETED", 2, 1]);
  expect(two.app.store.findingsForRevision(second.id).map((f) => [f.findingId, f.status, f.carried, f.anchored])).toEqual([[finding!.findingId, "RESOLVED", true, false]]);
  expect(fake.reviews[1]!.body).toContain("No new actionable issues found in the reviewed scope.");
  expect(fake.reviews[1]!.body).toContain(`Looks resolved since the last review:\n\n- High: Order deletion endpoint skips the admin check (\`src/routes/orders.ts\`, first reported on \`${fixture.headSha.slice(0, 7)}\`)`);
  expect(two.app.store.candidateSet(second.id)!.verdicts).toEqual([{ findingId: finding!.findingId, state: "ACCEPTED", reason: null }]);
  expect(two.app.store.auditEvents(second.id).find((e) => e.event === "CANDIDATES_DISPOSED")!.detail).toBe("0 verified, 0 discarded, 1 carried, 1 resolved, 0 reopened");
  expect(observations(two.app)).toEqual([
    { disposition: "DETECTED", basis: null, path_json: '["DETECTED"]' },
    { disposition: "RESOLVED", basis: "resolution_checked", path_json: '["PUBLISHED","POTENTIALLY_RESOLVED","RESOLVED"]' },
  ]);
  // A true fix: the next push changes something else, the resolved finding is watched, its code is not back, and nothing is posted.
  nextHead();
  const held = await review(two.app);
  expect([held.state, fake.reviews.length, findingRows(two.app)]).toEqual(["COMPLETED", 2, [{ id: finding!.findingId, status: "RESOLVED", reopens: null }]]);
  expect(two.app.store.findingsForRevision(held.id)).toEqual([]);
  await close(two.app);

  // The guard is dropped again: a resolved finding is terminal, so the defect is a new finding with its own comment. The
  // watch and the model both see it, and the model's report joins the reopened finding instead of making a third.
  const back = pushCommit(fixture, { [DEFECT.file]: ROUTES_HEAD });
  fake.setPull(back);
  const three = await open([readDiff(), submitDefect({ ...fixture, headSha: back.headSha }), readDiff(), submitNothing()], silent, one.dataDir);
  const third = await review(three.app);
  const again = three.app.store.findingsForRevision(third.id);
  expect([third.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 3, [{ path: DEFECT.file, line: DEFECT.line }, { path: DEFECT.file, line: DEFECT.line }]]);
  expect(again.map((f) => [f.findingId !== finding!.findingId, f.status, f.carried, f.reopens])).toEqual([[true, "PUBLISHED", false, { findingId: finding!.findingId, resolvedAt: fixed.headSha }]]);
  expect(fake.reviews[2]!.body).toContain(`Found 1 actionable issue.\n\n- High: Order deletion endpoint skips the admin check (\`src/routes/orders.ts:5\`; looked resolved at \`${fixed.headSha.slice(0, 7)}\`, but its code is still here)`);
  expect(fake.reviews[2]!.body).not.toContain("Looks resolved");
  expect(findingRows(three.app)).toEqual([
    { id: finding!.findingId, status: "RESOLVED", reopens: null },
    { id: again[0]!.findingId, status: "PUBLISHED", reopens: finding!.findingId },
  ]);
  expect(observations(three.app).at(-1)).toEqual({ disposition: "DETECTED", basis: "reopened", path_json: '["DETECTED"]' });
  // Once reopened, the resolved finding is watched no more, and the new one is tracked like any other.
  nextHead();
  const fourth = await review(three.app);
  expect([fourth.state, fake.reviews.length, inline().length, findingRows(three.app).length]).toEqual(["COMPLETED", 3, 2, 2]);
  expect(three.app.store.findingsForRevision(fourth.id).map((f) => [f.findingId, f.status, f.carried])).toEqual([[again[0]!.findingId, "STILL_PRESENT", true]]);
}, 90_000);

it("corrects a false resolution on the next push: the finding comes back as a new finding with one inline comment, once", async () => {
  const one = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing(), readDiff(), submitNothing(), readDiff(), submitNothing()]);
  const first = await review(one.app);
  const [finding] = one.app.store.findingsForRevision(first.id);
  // Nothing at the defect changed, yet the finding is marked resolved: the shape a resolution check that was fooled leaves behind.
  const falsely = nextHead();
  const second = await review(one.app);
  expect(one.app.store.findingsForRevision(second.id)[0]!.status).toBe("STILL_PRESENT");
  one.app.store.db.prepare("UPDATE findings SET status = 'RESOLVED' WHERE id = ?").run(finding!.findingId);
  one.app.store.db.prepare("UPDATE finding_observations SET disposition = 'RESOLVED', basis = 'resolution_checked' WHERE revision_id = ?").run(second.id);

  // The next push finds the finding's code where it always was: a new finding, commented on once, linked to the resolved one.
  nextHead();
  const third = await review(one.app);
  const [reopened] = one.app.store.findingsForRevision(third.id);
  expect([third.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 3, [{ path: DEFECT.file, line: DEFECT.line }, { path: DEFECT.file, line: DEFECT.line }]]);
  expect([reopened!.findingId !== finding!.findingId, reopened!.status, reopened!.carried, reopened!.posted, reopened!.candidate.newLine, reopened!.reopens]).toEqual([true, "PUBLISHED", false, true, DEFECT.line, { findingId: finding!.findingId, resolvedAt: falsely.headSha }]);
  expect(fake.reviews[2]!.body).toContain(`looked resolved at \`${falsely.headSha.slice(0, 7)}\`, but its code is still here`);
  expect(one.app.store.auditEvents(third.id).find((e) => e.event === "CANDIDATES_DISPOSED")!.detail).toBe("0 verified, 0 discarded, 0 carried, 0 resolved, 1 reopened");
  expect(findingRows(one.app)).toEqual([
    { id: finding!.findingId, status: "RESOLVED", reopens: null },
    { id: reopened!.findingId, status: "PUBLISHED", reopens: finding!.findingId },
  ]);

  // The push after that reopens nothing more: the resolved finding has its successor, which is now tracked as still present.
  nextHead();
  const fourth = await review(one.app);
  expect([fourth.state, fake.reviews.length, inline().length, findingRows(one.app).length]).toEqual(["COMPLETED", 4, 2, 2]);
  expect(one.app.store.findingsForRevision(fourth.id).map((f) => [f.findingId, f.status, f.carried, f.reopens])).toEqual([[reopened!.findingId, "STILL_PRESENT", true, { findingId: finding!.findingId, resolvedAt: falsely.headSha }]]);
  expect(fake.reviews[3]!.body).toContain("Still present from an earlier commit:");
}, 90_000);

it("carries a reopening whose own review never reached GitHub into the next review as news, with its marker, once", async () => {
  const silent: Config = { ...config, review: { ...config.review, publish_mode: "silent_no_findings" } };
  const { app } = await open([readDiff(), submitDefect(fixture), readDiff(), submitNothing(), readDiff(), submitNothing(), readDiff(), submitNothing(), readDiff(), submitNothing()], silent);
  const first = await review(app);
  const [finding] = app.store.findingsForRevision(first.id);
  const falsely = nextHead();
  const second = await review(app);
  app.store.db.prepare("UPDATE findings SET status = 'RESOLVED' WHERE id = ?").run(finding!.findingId);
  app.store.db.prepare("UPDATE finding_observations SET disposition = 'RESOLVED', basis = 'resolution_checked' WHERE revision_id = ?").run(second.id);
  // The push that reopens it is reviewed in dry run, the shape a superseded revision leaves: the new finding is in the store, and no review carries it.
  nextHead();
  const dry = await review(app, false);
  const [reopened] = app.store.findingsForRevision(dry.id);
  expect([dry.state, fake.reviews.length, reopened?.carried, reopened?.listed, reopened?.reopens]).toEqual(["COMPLETED", 1, false, false, { findingId: finding!.findingId, resolvedAt: falsely.headSha }]);
  // The next push carries it, yet nobody has seen it: it is news, listed as new with its marker, and commented on once.
  nextHead();
  const next = await review(app);
  expect([next.state, fake.reviews.length, inline()]).toEqual(["COMPLETED", 2, [{ path: DEFECT.file, line: DEFECT.line }, { path: DEFECT.file, line: DEFECT.line }]]);
  expect(app.store.findingsForRevision(next.id).map((f) => [f.findingId, f.status, f.carried, f.listed, f.posted])).toEqual([[reopened!.findingId, "STILL_PRESENT", true, false, true]]);
  expect(fake.reviews[1]!.body).toContain(`Found 1 actionable issue.\n\n- High: Order deletion endpoint skips the admin check (\`src/routes/orders.ts:5\`; looked resolved at \`${falsely.headSha.slice(0, 7)}\`, but its code is still here)`);
  // Once that review is on GitHub the finding is carried like any other, and a quiet push posts nothing.
  nextHead();
  const after = await review(app);
  expect([after.state, fake.reviews.length, app.store.findingsForRevision(after.id).map((f) => [f.carried, f.listed])]).toEqual(["COMPLETED", 2, [[true, true]]]);
}, 90_000);

it("keeps a finding open on a refused claim, with the refusal recorded, and posts nothing for it under silent_no_findings", async () => {
  const silent: Config = { ...config, review: { ...config.review, publish_mode: "silent_no_findings" } };
  const one = await open([readDiff(), submitDefect(fixture)], silent);
  const first = await review(one.app);
  const [finding] = one.app.store.findingsForRevision(first.id);
  await close(one.app);
  // Nothing at the finding changed, so a claim on it cites code found in place.
  const pull = nextHead();
  const { app } = await open([readDiff(), submitResolution(finding!.findingId, pull.headSha)], silent, one.dataDir);
  const second = await review(app);
  expect([second.state, fake.reviews.length, app.store.findingsForRevision(second.id)[0]!.status]).toEqual(["COMPLETED", 1, "STILL_PRESENT"]);
  expect(app.store.candidateSet(second.id)!.verdicts).toEqual([{ findingId: finding!.findingId, state: "REFUSED", reason: `its cited code is unchanged at ${DEFECT.file}:${DEFECT.line}` }]);
  expect(observations(app).at(-1)).toEqual({ disposition: "STILL_PRESENT", basis: "resolution_refused", path_json: '["PUBLISHED","STILL_PRESENT"]' });
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
