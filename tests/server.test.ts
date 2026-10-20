import { createHmac } from "node:crypto";
import { mkdtempSync } from "node:fs";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { AppStore } from "../src/persistence/store.ts";
import { createHttpServer } from "../src/server/http.ts";

const SECRET = "webhook-secret";
const ADMIN = "admin-token";
let store: AppStore;
let server: Server;
let url: string;
let wakes: number;
let retried: string[];

beforeEach(async () => {
  store = AppStore.open(join(mkdtempSync(join(tmpdir(), "server-")), "app.sqlite"));
  wakes = 0;
  retried = [];
  server = createHttpServer({
    store,
    webhookSecret: SECRET,
    adminToken: ADMIN,
    wake: () => void wakes++,
    ready: () => ({ ok: true, checks: { db: true } }),
    inspect: (id) => (id === "a".repeat(32) ? { revision: id } : undefined),
    retry: async (id) => {
      retried.push(id);
      return { status: 202, body: { retried: id } };
    },
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterEach(async () => {
  await new Promise((resolve) => server.close(resolve));
  store.close();
});

const sign = (body: string, secret = SECRET) => `sha256=${createHmac("sha256", secret).update(body).digest("hex")}`;

const deliver = (body: string, headers: Record<string, string>) =>
  fetch(`${url}/webhooks/github`, { method: "POST", body, headers: { "x-github-delivery": "d-1", "x-github-event": "pull_request", ...headers } });

const inbox = () => store.db.prepare("SELECT delivery_id, event_type, payload_json, processed_at FROM webhook_events").all();

it("accepts a validly signed delivery, persists it with its payload, and treats a redelivery as a no-op", async () => {
  const body = JSON.stringify({ action: "opened" });
  const first = await deliver(body, { "x-hub-signature-256": sign(body) });
  expect([first.status, await first.json()]).toEqual([202, { accepted: "d-1" }]);
  const again = await deliver(body, { "x-hub-signature-256": sign(body) });
  expect([again.status, await again.json()]).toEqual([200, { duplicate: true }]);
  expect(inbox()).toEqual([{ delivery_id: "d-1", event_type: "pull_request", payload_json: body, processed_at: null }]);
  expect(wakes).toBe(1);
});

it("rejects forged, missing and wrong-length signatures before parsing the body", async () => {
  // Not JSON: a 400 here would mean the body was parsed before the signature check.
  const body = "{not json";
  const forged = await deliver(body, { "x-hub-signature-256": sign(body, "wrong-secret") });
  const missing = await deliver(body, {});
  const short = await deliver(body, { "x-hub-signature-256": sign(body).slice(0, 40) });
  const notHex = await deliver(body, { "x-hub-signature-256": "sha256=zz" });
  expect([forged.status, missing.status, short.status, notHex.status]).toEqual([401, 401, 401, 401]);
  expect(await forged.json()).toEqual({ error: "bad signature" });
  expect(inbox()).toEqual([]);
  const signed = await deliver(body, { "x-hub-signature-256": sign(body) });
  expect(signed.status).toBe(400);
});

it("needs the admin token for readiness, inspection and retry, but not for healthz", async () => {
  const id = "a".repeat(32);
  const anonymous = await Promise.all([
    fetch(`${url}/readyz`),
    fetch(`${url}/reviews/${id}`),
    fetch(`${url}/reviews/${id}/retry`, { method: "POST" }),
    fetch(`${url}/reviews/${id}`, { headers: { authorization: "Bearer wrong" } }),
  ]);
  expect(anonymous.map((r) => r.status)).toEqual([401, 401, 401, 401]);
  expect(retried).toEqual([]);
  expect((await fetch(`${url}/healthz`)).status).toBe(200);
  const auth = { authorization: `Bearer ${ADMIN}` };
  const inspect = await fetch(`${url}/reviews/${id}`, { headers: auth });
  const retry = await fetch(`${url}/reviews/${id}/retry`, { method: "POST", headers: auth });
  const ready = await fetch(`${url}/readyz`, { headers: auth });
  expect([inspect.status, await inspect.json(), retry.status, ready.status]).toEqual([200, { revision: id }, 202, 200]);
  expect(retried).toEqual([id]);
});

it("lists dead deliveries with their reason to the admin only", async () => {
  store.recordWebhook({ deliveryId: "d-9", eventType: "pull_request", payloadHash: "h", payloadJson: "{}" });
  store.failWebhook("d-9", "cannot read acme/shop#9: HTTP 404 (token lacks access)", null);
  expect((await fetch(`${url}/webhooks/dead`)).status).toBe(401);
  const dead = await (await fetch(`${url}/webhooks/dead`, { headers: { authorization: `Bearer ${ADMIN}` } })).json();
  expect(dead.map(({ processed_at, ...rest }: { processed_at: string }) => rest)).toEqual([
    { delivery_id: "d-9", event_type: "pull_request", attempts: 1, last_error: "cannot read acme/shop#9: HTTP 404 (token lacks access)" },
  ]);
});
