import { connect, type Socket } from "node:net";
import { Compile } from "typebox/compile";
import { ServerMessage } from "../src/protocol/v1.ts";

const valid = Compile(ServerMessage);

/** Every `type` / `eventType` any test client received, for the contract coverage check. */
export const produced = new Set<string>();

/** A raw NDJSON client. Every line it receives must pass the v1 server schema. */
export class TestClient {
  readonly socket: Socket;
  readonly messages: ServerMessage[] = [];
  readonly closed: Promise<void>;
  private buffer = "";
  private waiters: (() => void)[] = [];

  private constructor(socket: Socket) {
    this.socket = socket;
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => {
      this.buffer += chunk;
      for (let nl = this.buffer.indexOf("\n"); nl !== -1; nl = this.buffer.indexOf("\n")) {
        const message = JSON.parse(this.buffer.slice(0, nl));
        this.buffer = this.buffer.slice(nl + 1);
        if (!valid.Check(message)) throw new Error(`invalid server message ${JSON.stringify([...valid.Errors(message)].slice(0, 2))}: ${JSON.stringify(message).slice(0, 300)}`);
        produced.add(message.type === "event" ? `event:${message.eventType}` : message.type);
        this.messages.push(message);
      }
      for (const w of this.waiters.splice(0)) w();
    });
    socket.on("error", () => {});
    this.closed = new Promise((resolve) => socket.on("close", () => resolve()));
    this.closed.then(() => this.waiters.splice(0).forEach((w) => w()));
  }

  static connect(path: string): Promise<TestClient> {
    return new Promise((resolve, reject) => {
      const socket = connect(path);
      socket.once("connect", () => resolve(new TestClient(socket)));
      socket.once("error", reject);
    });
  }

  send(message: object | string): void {
    this.socket.write(typeof message === "string" ? message : `${JSON.stringify(message)}\n`);
  }

  /** Resolve with the first message at or after index `from` that matches. */
  async waitFor<T extends ServerMessage = ServerMessage>(match: (m: ServerMessage) => boolean, from = 0, timeoutMs = 10_000): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const found = this.messages.slice(from).find(match);
      if (found) return found as T;
      if (this.socket.destroyed && this.socket.readableEnded) throw new Error(`closed; got ${JSON.stringify(this.messages).slice(0, 500)}`);
      if (Date.now() > deadline) throw new Error(`timed out; got ${JSON.stringify(this.messages.map((m) => m.type)).slice(0, 500)}`);
      await new Promise<void>((resolve) => {
        const t = setTimeout(resolve, 50);
        this.waiters.push(() => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  }

  async hello(): Promise<Extract<ServerMessage, { type: "welcome" }>> {
    this.send({ type: "hello", requestId: "h", protocolVersion: 1, client: "test", capabilities: [] });
    return this.waitFor((m) => m.type === "welcome");
  }

  /** Sequenced events received so far, in order. */
  events() {
    return this.messages.filter((m): m is Extract<ServerMessage, { type: "event"; sequence: number }> => m.type === "event" && "sequence" in m);
  }

  close(): Promise<void> {
    this.socket.destroy();
    return this.closed;
  }
}
