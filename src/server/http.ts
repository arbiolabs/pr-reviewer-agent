import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { sha256 } from "../domain/types.ts";
import type { AppStore } from "../persistence/store.ts";

// GitHub caps webhook payloads at 25 MB.
const MAX_BODY_BYTES = 25 * 1024 * 1024;

/** §10: `X-Hub-Signature-256` is `sha256=` + hex HMAC of the raw body; compared in constant time at equal length. */
export function verifySignature(secret: string, body: Buffer, header: string | undefined): boolean {
  if (!header?.startsWith("sha256=")) return false;
  const given = Buffer.from(header.slice(7), "hex");
  const expected = createHmac("sha256", secret).update(body).digest();
  return given.length === expected.length && timingSafeEqual(given, expected);
}

/** Bearer token check that leaks neither the token nor its length: both sides are hashed to 32 bytes first. */
export function isAdmin(expected: string | undefined, header: string | undefined): boolean {
  if (!expected || !header?.startsWith("Bearer ")) return false;
  const digest = (s: string) => createHash("sha256").update(s).digest();
  return timingSafeEqual(digest(header.slice(7)), digest(expected));
}

export type ServerDeps = {
  store: AppStore;
  webhookSecret: string;
  adminToken: string | undefined;
  /** Called after a new delivery is persisted, so the worker drains it without waiting for its next tick. */
  wake(): void;
  ready(): { ok: boolean; checks: Record<string, boolean> };
  inspect(revisionId: string): unknown;
  retry(revisionId: string): Promise<{ status: number; body: unknown }>;
  report?(error: unknown): void;
};

class HttpError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function readBody(req: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req as AsyncIterable<Buffer>) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) throw new HttpError(413, "payload too large");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

function send(res: ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

export function createHttpServer(deps: ServerDeps): Server {
  async function webhook(req: IncomingMessage, res: ServerResponse) {
    const raw = await readBody(req);
    const signature = req.headers["x-hub-signature-256"];
    // Rejected before parsing: an unsigned body is never interpreted.
    if (!verifySignature(deps.webhookSecret, raw, typeof signature === "string" ? signature : undefined)) throw new HttpError(401, "bad signature");
    const deliveryId = req.headers["x-github-delivery"];
    const eventType = req.headers["x-github-event"];
    if (typeof deliveryId !== "string" || typeof eventType !== "string") throw new HttpError(400, "missing delivery or event header");
    try {
      JSON.parse(raw.toString("utf8"));
    } catch {
      throw new HttpError(400, "body is not JSON");
    }
    const fresh = deps.store.recordWebhook({ deliveryId, eventType, payloadHash: sha256(raw.toString("utf8")), payloadJson: raw.toString("utf8") });
    if (!fresh) return send(res, 200, { duplicate: true });
    send(res, 202, { accepted: deliveryId });
    deps.wake();
  }

  async function route(req: IncomingMessage, res: ServerResponse) {
    const { pathname } = new URL(req.url ?? "/", "http://localhost");
    if (req.method === "POST" && pathname === "/webhooks/github") return webhook(req, res);
    if (req.method === "GET" && pathname === "/healthz") return send(res, 200, { ok: true });
    if (!isAdmin(deps.adminToken, req.headers.authorization)) throw new HttpError(401, "admin token required");
    if (req.method === "GET" && pathname === "/readyz") {
      const ready = deps.ready();
      return send(res, ready.ok ? 200 : 503, ready);
    }
    if (req.method === "GET" && pathname === "/webhooks/dead") return send(res, 200, deps.store.deadWebhooks());
    const review = /^\/reviews\/([0-9a-f]{32})(\/retry)?$/.exec(pathname);
    if (review && req.method === "GET" && !review[2]) {
      const found = deps.inspect(review[1]!);
      return found === undefined ? send(res, 404, { error: "no such revision" }) : send(res, 200, found);
    }
    if (review && req.method === "POST" && review[2]) {
      const { status, body } = await deps.retry(review[1]!);
      return send(res, status, body);
    }
    throw new HttpError(404, "not found");
  }

  return createServer((req, res) => {
    route(req, res).catch((error: unknown) => {
      if (res.headersSent) return;
      if (error instanceof HttpError) return send(res, error.status, { error: error.message });
      deps.report?.(error);
      send(res, 500, { error: "internal error" });
    });
  });
}
