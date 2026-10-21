import { randomUUID } from "node:crypto";
import { chmodSync, lstatSync, mkdirSync, readFileSync, unlinkSync } from "node:fs";
import { connect, createServer, type Socket } from "node:net";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { Compile } from "typebox/compile";
import { ClientMessage, type ErrorCode, idempotencyKey, MAX_LINE_BYTES, PROTOCOL_VERSION, type ServerMessage, type SetupCheck } from "../protocol/v1.ts";
import { ControlError, type ControlSource } from "./source.ts";

const SERVER_VERSION = (JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")) as { version: string }).version;
const validClient = Compile(ClientMessage);

export function defaultSocketPath(env = process.env, platform = process.platform, home = homedir()): string {
  if (env.XDG_RUNTIME_DIR) return join(env.XDG_RUNTIME_DIR, "pr-review", "reviewer.sock");
  if (platform === "darwin") return join(home, "Library", "Application Support", "pr-review", "reviewer.sock");
  return join(home, ".local", "state", "pr-review", "reviewer.sock");
}

/** The socket's directory is the access control: owned by us, mode 0700, and not a symlink someone else could aim. */
export function ensurePrivateDir(dir: string): void {
  let stat = lstatSync(dir, { throwIfNoEntry: false });
  if (stat === undefined) {
    mkdirSync(dirname(dir), { recursive: true, mode: 0o700 });
    mkdirSync(dir, { mode: 0o700 });
    chmodSync(dir, 0o700);
    stat = lstatSync(dir);
  }
  if (stat.isSymbolicLink()) throw new Error(`refusing socket directory ${dir}: it is a symlink`);
  if (!stat.isDirectory()) throw new Error(`refusing socket directory ${dir}: not a directory`);
  if (stat.uid !== process.getuid!()) throw new Error(`refusing socket directory ${dir}: owned by uid ${stat.uid}, not ${process.getuid!()}`);
  if ((stat.mode & 0o077) !== 0) throw new Error(`refusing socket directory ${dir}: mode ${(stat.mode & 0o777).toString(8)}, expected 700 (chmod 700 it)`);
}

/** Remove a socket file left by a dead daemon; never one a live daemon is listening on, and never a non-socket. */
async function clearStaleSocket(path: string): Promise<void> {
  const stat = lstatSync(path, { throwIfNoEntry: false });
  if (stat === undefined) return;
  if (!stat.isSocket()) throw new Error(`refusing to replace ${path}: not a socket`);
  const live = await new Promise<boolean>((resolve) => {
    const probe = connect(path);
    probe.once("connect", () => {
      probe.destroy();
      resolve(true);
    });
    probe.once("error", () => resolve(false));
  });
  if (live) throw new Error(`another daemon is listening on ${path}`);
  unlinkSync(path);
}

export type ServerOptions = {
  socketPath: string;
  source: ControlSource;
  pollMs?: number;
  heartbeatMs?: number;
  /** Messages waiting for a slow client before it is cut over to resync.required. */
  maxQueue?: number;
  /** The readiness checks each `welcome` carries, recomputed per hello. */
  checks?: () => Promise<SetupCheck[]>;
};

export type ControlServer = { path: string; close(): Promise<void>; queueDepths(): number[] };

type Conn = {
  socket: Socket;
  // hello: nothing but hello accepted. idle: no events until snapshot.get or resume. streaming: events after `cursor`.
  phase: "hello" | "idle" | "streaming";
  cursor: number;
  epoch: string;
  queue: string[];
  blocked: boolean;
  closed: boolean;
};

const EVENT_BATCH = 500;
const ACCOUNTS_MAX_AGE_MS = 30_000;

const report = (error: unknown) => process.stderr.write(`pr-review daemon: ${error instanceof Error ? error.message : String(error)}\n`);

export async function startControlServer(options: ServerOptions): Promise<ControlServer> {
  const { socketPath, source } = options;
  const maxQueue = options.maxQueue ?? 2000;
  const conns = new Set<Conn>();
  // listen() succeeds on a longer path, but the kernel truncates sun_path, so no client could ever connect.
  const limit = process.platform === "darwin" ? 103 : 107;
  const length = Buffer.byteLength(socketPath);
  if (length > limit) throw new Error(`socket path is ${length} bytes, over the ${limit}-byte limit for a Unix socket on ${process.platform}; pass a shorter --socket`);
  ensurePrivateDir(dirname(socketPath));
  await clearStaleSocket(socketPath);

  const flush = (c: Conn) => {
    while (!c.blocked && c.queue.length > 0) if (!c.socket.write(c.queue.shift()!)) c.blocked = true;
  };

  /** The client fell behind: drop everything it has not read and make it resync. */
  const overflow = (c: Conn) => {
    c.queue = [`${JSON.stringify({ type: "resync.required", reason: "client fell behind the event stream" } satisfies ServerMessage)}\n`];
    c.phase = "idle";
    flush(c);
  };

  const send = (c: Conn, message: ServerMessage) => {
    if (c.closed) return;
    if (c.queue.length >= maxQueue) return overflow(c);
    c.queue.push(`${JSON.stringify(message)}\n`);
    flush(c);
  };

  const sendAndClose = (c: Conn, message: ServerMessage) => {
    send(c, message);
    c.closed = true;
    c.socket.end();
  };

  const resync = (c: Conn, reason: string, requestId?: string) => {
    c.phase = "idle";
    send(c, { type: "resync.required", ...(requestId ? { requestId } : {}), reason });
  };

  const pump = (c: Conn) => {
    if (c.phase !== "streaming") return;
    if (source.epoch() !== c.epoch) return resync(c, `stream epoch changed from ${c.epoch} to ${source.epoch()}`);
    // At least one event even when the queue is full, so a client that stopped reading overflows into a resync.
    const room = Math.max(1, Math.min(EVENT_BATCH, maxQueue - c.queue.length));
    for (const event of source.eventsAfter(c.cursor, room)) {
      if (c.phase !== "streaming") return;
      send(c, event);
      c.cursor = event.sequence;
    }
  };

  const reply = async (c: Conn, message: ClientMessage): Promise<void> => {
    const requestId = message.requestId;
    switch (message.type) {
      case "hello":
        throw new ControlError("INVALID_REQUEST", "hello was already received");
      case "snapshot.get": {
        await source.refreshAccounts?.(ACCOUNTS_MAX_AGE_MS);
        const snapshot = source.snapshot();
        send(c, { type: "snapshot", requestId, ...snapshot });
        Object.assign(c, { phase: "streaming", cursor: snapshot.sequence, epoch: snapshot.streamEpoch });
        return;
      }
      case "resume": {
        const epoch = source.epoch();
        if (message.streamEpoch !== epoch) return resync(c, `stream epoch ${message.streamEpoch} is gone; the current one is ${epoch}`, requestId);
        const next = source.eventsAfter(message.afterSequence, 1)[0];
        const known = next === undefined ? message.afterSequence === source.latestSequence() : next.prevSequence === message.afterSequence;
        if (!known) return resync(c, `sequence ${message.afterSequence} cannot be resumed from`, requestId);
        Object.assign(c, { phase: "streaming", cursor: message.afterSequence, epoch });
        return pump(c);
      }
      case "diff.get": {
        const page = await source.diff(message);
        send(c, { type: "diff.page", requestId, prId: message.prId, baseSha: message.baseSha, headSha: message.headSha, ...page });
        return;
      }
      case "finding.get":
        send(c, { type: "finding.detail", requestId, finding: await source.finding(message.findingId) });
        return;
      case "publication.get": {
        if (!source.publication) throw new ControlError("NOT_FOUND", `no stored publication for run ${message.runId}`);
        send(c, { type: "publication.preview", requestId, ...(await source.publication(message.runId)) });
        return;
      }
      case "timeline.get":
        send(c, { type: "timeline.page", requestId, ...source.timeline(message) });
        return;
      case "command": {
        const reject = (code: ErrorCode, text: string) => send(c, { type: "command.result", requestId, status: "rejected", code, message: text });
        // The server is the gate, whatever the UI shows: only a capability the welcome granted runs, and only while streaming.
        const granted = message.command === "account.select" ? Boolean(source.selectAccount) : Boolean(source.command) && (source.capabilities?.() ?? []).includes(message.command);
        if (!granted) return reject("CAPABILITY_DENIED", source.unavailable?.(message.command) ?? `${message.command} is not available in this version`);
        if (c.phase !== "streaming") return reject("CONFLICT", `send snapshot.get or resume before ${message.command}`);
        const expectedKey = idempotencyKey(message);
        if (message.idempotencyKey !== expectedKey) return reject("INVALID_REQUEST", `idempotencyKey must be ${expectedKey}`);
        try {
          const operationId = message.command === "account.select" ? await source.selectAccount!(message.params.accountId) : await source.command!(message);
          send(c, { type: "command.result", requestId, status: "accepted", operationId });
        } catch (error) {
          if (error instanceof ControlError) return reject(error.code, error.message);
          // A command's failure is always its own result, so the client never waits on a bare error for a requestId.
          report(error);
          return reject("INTERNAL_ERROR", error instanceof Error ? error.message : String(error));
        }
        return pump(c);
      }
    }
  };

  const handleLine = async (c: Conn, line: string): Promise<void> => {
    if (line.trim() === "") return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      const error: ServerMessage = { type: "error", code: "INVALID_REQUEST", message: "line is not valid JSON" };
      return c.phase === "hello" ? sendAndClose(c, error) : void send(c, error);
    }
    const raw = parsed as { type?: unknown; requestId?: unknown; protocolVersion?: unknown } | null;
    const requestId = typeof raw?.requestId === "string" && raw.requestId !== "" ? raw.requestId : undefined;
    const fail = (code: ErrorCode, message: string, extra: { supported?: number[] } = {}): ServerMessage => ({
      type: "error",
      ...(requestId ? { requestId } : {}),
      code,
      message,
      ...extra,
    });
    const valid = validClient.Check(parsed);
    if (c.phase === "hello") {
      if (raw?.type !== "hello") return sendAndClose(c, fail("INVALID_REQUEST", "the first message must be hello"));
      if (!valid) return sendAndClose(c, fail("INVALID_REQUEST", "malformed hello"));
      const hello = parsed as Extract<ClientMessage, { type: "hello" }>;
      if (hello.protocolVersion !== PROTOCOL_VERSION) {
        return sendAndClose(c, fail("PROTOCOL_MISMATCH", `unsupported protocolVersion ${hello.protocolVersion}`, { supported: [PROTOCOL_VERSION] }));
      }
      // A failed check never holds up the welcome: the client then shows none.
      const checks = await options.checks?.().catch((error: unknown) => void report(error));
      c.phase = "idle";
      send(c, {
        type: "welcome",
        requestId: hello.requestId,
        protocolVersion: PROTOCOL_VERSION,
        serverVersion: SERVER_VERSION,
        sessionId: randomUUID(),
        capabilities: [...(source.selectAccount ? ["account.select"] : []), ...(source.capabilities?.() ?? [])],
        ...(source.mode ? { mode: source.mode } : {}),
        ...(source.readOnlyReason ? { readOnlyReason: source.readOnlyReason } : {}),
        ...(checks ? { checks } : {}),
        streamEpoch: source.epoch(),
        latestSequence: source.latestSequence(),
      });
      return;
    }
    if (!valid) {
      const first = [...validClient.Errors(parsed)][0];
      return void send(c, fail("INVALID_REQUEST", `invalid ${String(raw?.type ?? "message")}${first ? `: ${first.instancePath || "/"} ${first.message}` : ""}`));
    }
    try {
      await reply(c, parsed as ClientMessage);
    } catch (error) {
      if (error instanceof ControlError) send(c, fail(error.code, error.message));
      else send(c, fail("INTERNAL_ERROR", error instanceof Error ? error.message : String(error)));
    }
  };

  const server = createServer((socket) => {
    const c: Conn = { socket, phase: "hello", cursor: 0, epoch: "", queue: [], blocked: false, closed: false };
    conns.add(c);
    let pending = Buffer.alloc(0);
    // Lines are handled one at a time, in order, even when a reply awaits git.
    let chain: Promise<unknown> = Promise.resolve();
    let oversized = false;
    socket.on("drain", () => {
      c.blocked = false;
      flush(c);
    });
    socket.on("data", (chunk: Buffer) => {
      if (c.closed || oversized) return;
      pending = Buffer.concat([pending, chunk]);
      for (let nl = pending.indexOf(10); nl !== -1 && nl <= MAX_LINE_BYTES; nl = pending.indexOf(10)) {
        const line = pending.subarray(0, nl).toString("utf8");
        pending = pending.subarray(nl + 1);
        chain = chain.then(() => (c.closed ? undefined : handleLine(c, line))).catch(report);
      }
      if (pending.length > MAX_LINE_BYTES) {
        oversized = true;
        pending = Buffer.alloc(0);
        chain = chain.then(() => sendAndClose(c, { type: "error", code: "INVALID_REQUEST", message: `line exceeds ${MAX_LINE_BYTES} bytes` }));
      }
    });
    socket.on("error", () => {});
    socket.on("close", () => {
      c.closed = true;
      conns.delete(c);
    });
  });

  const tick = (every: number, fn: (c: Conn) => void) =>
    setInterval(() => {
      for (const c of conns) {
        try {
          fn(c);
        } catch (error) {
          report(error);
        }
      }
    }, every);
  await source.refreshAccounts?.(0);
  const timers = [
    setInterval(() => source.refreshAccounts?.(0).catch(report), 60_000),
    // ponytail: one journal query per streaming client per tick; share one read per cursor if many clients attach.
    tick(options.pollMs ?? 200, pump),
    tick(options.heartbeatMs ?? 15_000, (c) => {
      if (c.phase === "streaming") send(c, { type: "event", eventType: "heartbeat", streamEpoch: c.epoch, occurredAt: new Date().toISOString() });
    }),
  ];

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => resolve());
  });
  chmodSync(socketPath, 0o600);

  return {
    path: socketPath,
    queueDepths: () => [...conns].map((c) => c.queue.length),
    async close() {
      for (const t of timers) clearInterval(t);
      for (const c of conns) c.socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
