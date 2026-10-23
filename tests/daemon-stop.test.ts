import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FauxResponseStep, fauxAssistantMessage } from "@earendil-works/pi-ai";
import { afterEach, expect, it } from "vitest";
import { type Config, DEFAULT_CONFIG } from "../src/config/config.ts";
import { AppDbSource } from "../src/control/appdb.ts";
import { COMMANDS_OFF, runDaemon } from "../src/control/daemon.ts";
import { EngineSource } from "../src/control/engine.ts";
import { startControlServer } from "../src/control/server.ts";
import { AppStore } from "../src/persistence/store.ts";
import type { ServerMessage } from "../src/protocol/v1.ts";
import { TestClient } from "./control-client.ts";
import { createFixture, type Fixture } from "./fixture.ts";
import { open, readDiff, submitDefect } from "./helpers.ts";

const dryRun: Config = { ...DEFAULT_CONFIG, review: { ...DEFAULT_CONFIG.review, publish_mode: "dry_run" } };
const cleanup: (() => Promise<unknown> | void)[] = [];
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
});

/** A model response that never arrives: it reports that ANALYZE is mid-inference, then waits to be aborted. */
function blockUntilAborted(onInFlight: () => void): FauxResponseStep {
  return (_context, options) =>
    new Promise((resolve) => {
      onInFlight();
      options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })), { once: true });
    });
}

const engine = (fixture: Fixture, responses: FauxResponseStep[]) => async (dataDir: string) =>
  EngineSource.host(dataDir, await open(dataDir, fixture, responses, dryRun, 0), {
    config: dryRun,
    pulls: fixture.pulls,
    github: () => {
      throw new Error("no GitHub in this test");
    },
    env: { PATH: "" },
  });

async function streaming(socket: string) {
  const c = await TestClient.connect(socket);
  cleanup.push(() => c.close());
  const welcome = await c.hello();
  c.send({ type: "snapshot.get", requestId: "s" });
  await c.waitFor((m) => m.type === "snapshot");
  return { c, welcome };
}

/** `pr-review daemon --allow-commands` in process, on its own socket and data dir, with a client streaming from it. */
async function daemon(fixture: Fixture, responses: FauxResponseStep[], dataDir = mkdtempSync(join(tmpdir(), "stop-"))) {
  const socket = join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock");
  const notes: string[] = [];
  const exit = runDaemon(["--socket", socket, "--allow-commands"], dataDir, (t) => notes.push(t), { openEngine: engine(fixture, responses) });
  while (!notes.some((n) => n.includes("listening on"))) await new Promise((r) => setTimeout(r, 10));
  return { ...(await streaming(socket)), exit, socket, dataDir };
}

type Result = Extract<ServerMessage, { type: "command.result" }>;
let requests = 0;
function stop(c: TestClient, pauseReviews: boolean): Promise<Result> {
  const requestId = `stop-${++requests}`;
  c.send({ type: "command", requestId, command: "daemon.stop", params: { pauseReviews }, idempotencyKey: "daemon.stop" });
  return c.waitFor<Result>((m) => m.type === "command.result" && m.requestId === requestId);
}

it("stops on daemon.stop: replies first, then closes, removes the socket, frees the data dir and exits 0", async () => {
  const fixture = createFixture();
  const d = await daemon(fixture, []);
  expect(d.welcome.capabilities).toEqual(["account.select", "review.add", "review.start", "review.retry", "review.cancel", "daemon.stop"]);
  expect(await stop(d.c, false)).toEqual({ type: "command.result", requestId: expect.any(String), status: "accepted", operationId: "daemon.stop" });
  await d.c.closed;
  expect(await d.exit).toBe(0);
  expect(existsSync(d.socket)).toBe(false);
  // Nothing holds the lock: another pr-review opens the data dir at once.
  await (await open(d.dataDir, fixture, [], dryRun, 0)).close();
}, 30_000);

it("is not granted by a read-only daemon, which may belong to another process, and keeps serving", async () => {
  let stops = 0;
  const source = AppDbSource.open(mkdtempSync(join(tmpdir(), "stop-")), { PATH: "" }, COMMANDS_OFF);
  cleanup.push(() => source.close());
  const server = await startControlServer({ socketPath: join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "s.sock"), source, stop: () => stops++ });
  cleanup.push(() => server.close());
  const { c, welcome } = await streaming(server.path);
  expect(welcome).toMatchObject({ mode: "read-only", capabilities: ["account.select"] });
  expect(await stop(c, true)).toMatchObject({ status: "rejected", code: "CAPABILITY_DENIED", message: `daemon.stop is not available: ${COMMANDS_OFF}` });
  expect((await streaming(server.path)).welcome).toMatchObject({ type: "welcome" });
  expect(stops).toBe(0);
}, 30_000);

it("asks before pausing a running review, then stops as SIGTERM does, and the review resumes on the next start", async () => {
  const fixture = createFixture();
  let inFlight!: () => void;
  const reachedAnalyze = new Promise<void>((resolve) => (inFlight = resolve));
  const d = await daemon(fixture, [readDiff(), blockUntilAborted(() => inFlight())]);
  const store = AppStore.open(join(d.dataDir, "app.sqlite"));
  const prId = store.upsertPull(fixture.pull);
  store.close();
  const key = `review.start:${prId}:${fixture.headSha}`;
  d.c.send({ type: "command", requestId: "start", command: "review.start", params: { prId, expectedHeadSha: fixture.headSha }, idempotencyKey: key });
  const started = await d.c.waitFor<Result>((m) => m.type === "command.result" && m.requestId === "start");
  const runId = (started as { operationId: string }).operationId.slice("review.start:".length);
  await reachedAnalyze;

  expect(await stop(d.c, false)).toMatchObject({ status: "rejected", code: "CONFLICT", message: "1 review is running; stopping pauses it until the next start (send pauseReviews: true)" });
  expect(await stop(d.c, true)).toMatchObject({ status: "accepted", operationId: "daemon.stop" });
  expect(await d.exit).toBe(0);
  const paused = AppStore.open(join(d.dataDir, "app.sqlite"));
  expect(paused.revision(runId)!.state).toBe("ANALYZE");
  paused.close();

  const again = await daemon(fixture, [submitDefect(fixture)], d.dataDir);
  expect(await again.c.waitFor((m) => m.type === "event" && m.eventType === "review.completed", 0, 20_000)).toMatchObject({ runId, data: { status: "COMPLETED" } });
  expect(await stop(again.c, false)).toMatchObject({ status: "accepted" });
  expect(await again.exit).toBe(0);
}, 60_000);
