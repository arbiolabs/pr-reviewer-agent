import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { defaultSocketPath, startControlServer, type ControlServer, type ServerOptions } from "../src/control/server.ts";
import { FixtureSource } from "../src/control/source.ts";
import { PROTOCOL_DIR } from "../src/protocol/emit.ts";
import type { PlaybackLine } from "../src/protocol/fixtures.ts";
import { MAX_LINE_BYTES, type ServerMessage } from "../src/protocol/v1.ts";
import { TestClient } from "./control-client.ts";

const cleanup: (() => Promise<void> | void)[] = [];
afterEach(async () => {
  for (const fn of cleanup.splice(0).reverse()) await fn();
});

const socketPath = () => join(mkdtempSync(join(tmpdir(), "ctl-")), "run", "reviewer.sock");

async function serve(options: Partial<ServerOptions> & Pick<ServerOptions, "source">): Promise<ControlServer> {
  const server = await startControlServer({ socketPath: socketPath(), ...options });
  cleanup.push(() => server.close());
  return server;
}

async function client(server: ControlServer): Promise<TestClient> {
  const c = await TestClient.connect(server.path);
  cleanup.push(() => c.close());
  return c;
}

const script = (name: string) =>
  readFileSync(join(PROTOCOL_DIR, "playback", `${name}.ndjson`), "utf8")
    .trimEnd()
    .split("\n")
    .map((l) => JSON.parse(l) as PlaybackLine);
const sequenced = (lines: PlaybackLine[]) => lines.map((l) => l.message).filter((m) => m.type === "event" && "sequence" in m);
const withoutRequestId = ({ requestId: _, ...rest }: ServerMessage & { requestId?: string }) => rest;

describe("handshake", () => {
  it("answers hello with welcome and no write capabilities", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    expect(await c.hello()).toMatchObject({ type: "welcome", requestId: "h", protocolVersion: 1, capabilities: [], streamEpoch: "epoch-1", latestSequence: 10 });
  });

  it("computes the checks for each welcome, and still welcomes when they fail", async () => {
    let calls = 0;
    const checks = async () => {
      calls += 1;
      if (calls === 3) throw new Error("gh hung");
      return [{ id: "model", status: "missing" as const, detail: `no models available (${calls})` }];
    };
    const server = await serve({ source: FixtureSource.load("normal"), checks });
    expect((await (await client(server)).hello()).checks).toEqual([{ id: "model", status: "missing", detail: "no models available (1)" }]);
    expect((await (await client(server)).hello()).checks).toEqual([{ id: "model", status: "missing", detail: "no models available (2)" }]);
    expect(await (await client(server)).hello()).not.toHaveProperty("checks");
  });

  it("answers another protocol version with PROTOCOL_MISMATCH and closes", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    c.send({ type: "hello", requestId: "h", protocolVersion: 2, client: "test", capabilities: [] });
    await c.closed;
    expect(c.messages).toEqual([{ type: "error", requestId: "h", code: "PROTOCOL_MISMATCH", message: "unsupported protocolVersion 2", supported: [1] }]);
  });

  it("closes on a first message that is not hello, and on invalid JSON before hello", async () => {
    const server = await serve({ source: FixtureSource.load("normal") });
    const a = await client(server);
    a.send({ type: "snapshot.get", requestId: "s" });
    await a.closed;
    expect(a.messages).toEqual([{ type: "error", requestId: "s", code: "INVALID_REQUEST", message: "the first message must be hello" }]);
    const b = await client(server);
    b.send("{not json\n");
    await b.closed;
    expect(b.messages).toEqual([{ type: "error", code: "INVALID_REQUEST", message: "line is not valid JSON" }]);
  });

  it("closes on a line over 1 MiB", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    await c.hello();
    c.send(`{"type":"snapshot.get","requestId":"${"x".repeat(MAX_LINE_BYTES)}"}\n`);
    await c.closed;
    expect(c.messages.at(-1)).toEqual({ type: "error", code: "INVALID_REQUEST", message: `line exceeds ${MAX_LINE_BYTES} bytes` });
  });

  it("answers an invalid message with an error that echoes its requestId and stays open", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    await c.hello();
    c.send({ type: "diff.get", requestId: "d1", prId: "p", baseSha: "b", headSha: "h", limit: 0 });
    c.send({ type: "command", requestId: "c1", command: "review.cancel", params: { runId: "r" }, idempotencyKey: "k", actor: "admin" });
    c.send("[1,2]\n");
    const errors = await Promise.all(["d1", "c1"].map((id) => c.waitFor((m) => m.type === "error" && m.requestId === id)));
    expect(errors.map((e) => e.type === "error" && e.code)).toEqual(["INVALID_REQUEST", "INVALID_REQUEST"]);
    expect(await c.waitFor((m) => m.type === "error" && m.requestId === undefined)).toMatchObject({ code: "INVALID_REQUEST", message: expect.stringMatching(/^invalid message: \/ /) });
    c.send({ type: "finding.get", requestId: "f", findingId: "f-2" });
    expect(await c.waitFor((m) => m.type === "finding.detail")).toMatchObject({ requestId: "f", finding: { id: "f-2", severity: "low" } });
    expect(c.socket.destroyed).toBe(false);
  });

  it("rejects every command with CAPABILITY_DENIED", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    await c.hello();
    const names = ["review.add", "review.start", "review.cancel", "review.retry", "review.publish"];
    for (const name of names) c.send(JSON.parse(readFileSync(join(PROTOCOL_DIR, "fixtures", `client.command.${name}.json`), "utf8")));
    await c.waitFor(() => c.messages.filter((m) => m.type === "command.result").length === names.length);
    expect(c.messages.filter((m) => m.type === "command.result")).toEqual(
      names.map((name) => ({ type: "command.result", requestId: "req-cmd", status: "rejected", code: "CAPABILITY_DENIED", message: `${name} is not available from a fixture server` })),
    );
  });

  it("sends unsequenced heartbeats only once the client streams", async () => {
    const c = await client(await serve({ source: FixtureSource.load("normal"), heartbeatMs: 30 }));
    await c.hello();
    await new Promise((r) => setTimeout(r, 120));
    expect(c.messages.map((m) => m.type)).toEqual(["welcome"]);
    c.send({ type: "snapshot.get", requestId: "s" });
    expect(await c.waitFor((m) => m.type === "event" && m.eventType === "heartbeat")).toEqual({
      type: "event",
      eventType: "heartbeat",
      streamEpoch: "epoch-1",
      occurredAt: expect.any(String),
    });
  });
});

describe("socket safety", () => {
  it("defaults to the runtime dir, then the per-platform state dir", () => {
    expect(defaultSocketPath({ XDG_RUNTIME_DIR: "/run/user/501" }, "linux", "/home/d")).toBe("/run/user/501/pr-review/reviewer.sock");
    expect(defaultSocketPath({}, "darwin", "/Users/d")).toBe("/Users/d/Library/Application Support/pr-review/reviewer.sock");
    expect(defaultSocketPath({}, "linux", "/home/d")).toBe("/home/d/.local/state/pr-review/reviewer.sock");
  });

  it("creates the directory 0700 and the socket 0600", async () => {
    const server = await serve({ source: FixtureSource.load("normal") });
    expect(statSync(join(server.path, "..")).mode & 0o777).toBe(0o700);
    expect(statSync(server.path).mode & 0o777).toBe(0o600);
  });

  it("refuses a socket path the kernel would truncate, before creating anything, and serves one at the limit", async () => {
    const limit = process.platform === "darwin" ? 103 : 107;
    const root = mkdtempSync(join(tmpdir(), "ctl-"));
    const at = (bytes: number) => `${root}/d/${"s".repeat(bytes - `${root}/d/`.length)}`;
    expect(Buffer.byteLength(at(limit + 1))).toBe(limit + 1);
    await expect(startControlServer({ socketPath: at(limit + 1), source: FixtureSource.load("normal") })).rejects.toThrow(
      `socket path is ${limit + 1} bytes, over the ${limit}-byte limit for a Unix socket on ${process.platform}; pass a shorter --socket`,
    );
    expect(existsSync(join(root, "d"))).toBe(false);
    const server = await serve({ source: FixtureSource.load("normal"), socketPath: at(limit) });
    const c = await TestClient.connect(server.path);
    expect(await c.hello()).toMatchObject({ type: "welcome" });
    await c.close();
  });

  it("refuses a symlinked directory and a loose-mode directory", async () => {
    const root = mkdtempSync(join(tmpdir(), "ctl-"));
    const real = join(root, "real");
    mkdirSync(real, { mode: 0o700 });
    symlinkSync(real, join(root, "link"));
    await expect(startControlServer({ socketPath: join(root, "link", "s.sock"), source: FixtureSource.load("normal") })).rejects.toThrow("is a symlink");
    const loose = join(root, "loose");
    mkdirSync(loose);
    chmodSync(loose, 0o755);
    await expect(startControlServer({ socketPath: join(loose, "s.sock"), source: FixtureSource.load("normal") })).rejects.toThrow("mode 755, expected 700");
    const file = join(real, "file.sock");
    writeFileSync(file, "");
    await expect(startControlServer({ socketPath: file, source: FixtureSource.load("normal") })).rejects.toThrow("not a socket");
  });

  it("replaces a stale socket but never steals a live one", async () => {
    const path = socketPath();
    const first = await startControlServer({ socketPath: path, source: FixtureSource.load("normal") });
    await expect(startControlServer({ socketPath: path, source: FixtureSource.load("normal") })).rejects.toThrow("another daemon is listening");
    await first.close();

    // What a kill -9 leaves behind: a socket file nobody listens on.
    const holder = spawn(process.execPath, ["-e", `require("net").createServer().listen(${JSON.stringify(path)}, () => console.log("up"))`]);
    await new Promise((resolve) => holder.stdout.once("data", resolve));
    holder.kill("SIGKILL");
    await new Promise((resolve) => holder.once("exit", resolve));
    expect(statSync(path).isSocket()).toBe(true);
    const second = await startControlServer({ socketPath: path, source: FixtureSource.load("normal") });
    cleanup.push(() => second.close());
    expect((await (await client(second)).hello()).type).toBe("welcome");
  });
});

describe("fixture source", () => {
  it("replays normal.ndjson: the client receives the script's snapshot and sequenced events verbatim", async () => {
    const lines = script("normal");
    const c = await client(await serve({ source: FixtureSource.load("normal") }));
    await c.hello();
    c.send({ type: "snapshot.get", requestId: "s" });
    await c.waitFor((m) => m.type === "event" && m.eventType === "review.completed");
    const snapshot = c.messages.find((m) => m.type === "snapshot")!;
    expect(withoutRequestId(snapshot)).toEqual(withoutRequestId(lines.find((l) => l.message.type === "snapshot")!.message));
    expect(c.events()).toEqual(sequenced(lines));
  });

  it("resumes from a sequence it still has, and demands a resync across gap.ndjson's hole", async () => {
    const server = await serve({ source: FixtureSource.load("gap") });
    const a = await client(server);
    await a.hello();
    a.send({ type: "snapshot.get", requestId: "s" });
    await a.waitFor((m) => m.type === "event" && "sequence" in m && m.sequence === 14);
    expect(a.events().map((e) => [e.sequence, e.prevSequence])).toEqual([[11, 10], [13, 12], [14, 13]]);

    const b = await client(server);
    await b.hello();
    b.send({ type: "resume", requestId: "r1", streamEpoch: "epoch-1", afterSequence: 10 });
    await b.waitFor((m) => m.type === "event" && "sequence" in m && m.sequence === 11);
    b.send({ type: "resume", requestId: "r2", streamEpoch: "epoch-1", afterSequence: 11 });
    expect(await b.waitFor((m) => m.type === "resync.required")).toEqual({ type: "resync.required", requestId: "r2", reason: "sequence 11 cannot be resumed from" });
    b.send({ type: "resume", requestId: "r3", streamEpoch: "epoch-0", afterSequence: 10 });
    expect(await b.waitFor((m) => m.type === "resync.required" && m.requestId === "r3")).toMatchObject({ reason: "stream epoch epoch-0 is gone; the current one is epoch-1" });
  });

  it("tells a streaming client to resync when epoch-change.ndjson moves to a new epoch", async () => {
    const c = await client(await serve({ source: FixtureSource.load("epoch-change"), pollMs: 20 }));
    await c.hello();
    c.send({ type: "snapshot.get", requestId: "s" });
    await c.waitFor((m) => m.type === "resync.required");
    c.send({ type: "snapshot.get", requestId: "s2" });
    const snapshot = await c.waitFor((m) => m.type === "snapshot" && m.requestId === "s2");
    expect(snapshot).toMatchObject({ streamEpoch: "epoch-2", sequence: 3 });
    const event = await c.waitFor((m) => m.type === "event" && m.streamEpoch === "epoch-2");
    expect(event).toMatchObject({ sequence: 4, prevSequence: 3, eventType: "review.phase_changed" });
  });
  it("answers finding.get with the showcase's recorded explanation and evidence", async () => {
    const c = await client(await serve({ source: FixtureSource.load("showcase") }));
    await c.hello();
    c.send({ type: "snapshot.get", requestId: "s" });
    await c.waitFor((m) => m.type === "snapshot");
    c.send({ type: "finding.get", requestId: "f", findingId: "ffadc4e5592b38cfb55eaa5e99afddd8" });
    const detail = await c.waitFor((m) => m.type === "finding.detail");
    expect(detail.type === "finding.detail" && detail.finding).toMatchObject({
      title: "Order history runs one item query per order",
      explanation: expect.stringMatching(/^orderHistory fetches a page of orders/),
      evidence: [{ kind: "code_ref", path: "src/server/orders/history.ts", start: 15, end: 21 }, { kind: "tool_summary" }],
      history: [{ disposition: "DETECTED" }],
    });
  });

  it("answers usage.get with the recorded report for that grouping", async () => {
    const c = await client(await serve({ source: FixtureSource.load("showcase") }));
    await c.hello();
    c.send({ type: "usage.get", requestId: "u", by: "month" });
    const report = await c.waitFor((m) => m.type === "usage.report");
    expect(report).toMatchObject({ requestId: "u", by: "month", timeZone: "Europe/Berlin", currency: "USD" });
    expect(report.type === "usage.report" && report.buckets).toHaveLength(12);
  });
});

describe("slow client", () => {
  it("cuts a client that never reads over to resync.required with a bounded queue, and the others are unaffected", async () => {
    const server = await serve({ source: FixtureSource.load("burst"), maxQueue: 200, pollMs: 10 });
    const slow = await client(server);
    await slow.hello();
    slow.socket.pause();
    slow.send({ type: "snapshot.get", requestId: "s" });
    const fast = await client(server);
    await fast.hello();
    fast.send({ type: "snapshot.get", requestId: "s" });
    await fast.waitFor((m) => m.type === "event" && "sequence" in m && m.sequence === 5010, 0, 20_000);
    const seqs = fast.events().map((e) => e.sequence);
    expect(seqs).toEqual(Array.from({ length: 5000 }, (_, i) => 11 + i));
    expect(fast.messages.some((m) => m.type === "resync.required")).toBe(false);
    await new Promise((r) => setTimeout(r, 100));
    expect(Math.max(...server.queueDepths())).toBeLessThanOrEqual(200);
    slow.socket.resume();
    await slow.waitFor((m) => m.type === "resync.required");
    expect(slow.messages.find((m) => m.type === "resync.required")).toEqual({ type: "resync.required", reason: "client fell behind the event stream" });
    expect(slow.events().length).toBeLessThan(5000);
  }, 30_000);
});
