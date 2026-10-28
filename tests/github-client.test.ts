import { generateKeyPairSync, verify } from "node:crypto";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type Attempt, appJwt, classify, GitHubClient, GitHubError, retryDelayMs } from "../src/github/client.ts";
import { GitHubPullSource } from "../src/github/pulls.ts";
import { FakeGitHub } from "./fake-github.ts";

const res = (status: number, headers: Record<string, string> = {}, body = ""): Attempt => ({ kind: "response", status, headers: new Headers(headers), body });

describe("classify", () => {
  const table: [string, Attempt, string, string][] = [
    ["200", res(200), "ok", "ok"],
    ["201", res(201), "ok", "ok"],
    ["429", res(429, { "retry-after": "3" }), "retryable", "retryable"],
    ["403 primary limit", res(403, { "x-ratelimit-remaining": "0", "x-ratelimit-reset": "1700000000" }), "retryable", "retryable"],
    ["403 with retry-after", res(403, { "retry-after": "60" }), "retryable", "retryable"],
    ["403 secondary limit body", res(403, {}, '{"message":"You have exceeded a secondary rate limit"}'), "retryable", "retryable"],
    ["403 forbidden", res(403, { "x-ratelimit-remaining": "4999" }, '{"message":"Resource not accessible by integration"}'), "fatal", "fatal"],
    ["401", res(401), "fatal", "fatal"],
    ["404", res(404), "fatal", "fatal"],
    ["422", res(422), "fatal", "fatal"],
    ["500", res(500), "retryable", "ambiguous"],
    ["502", res(502), "retryable", "ambiguous"],
    ["connection refused", { kind: "network", sent: false }, "retryable", "retryable"],
    ["dropped after send", { kind: "network", sent: true }, "retryable", "ambiguous"],
  ];
  for (const [name, attempt, read, write] of table) {
    it(`${name}: read ${read}, write ${write}`, () => {
      expect([classify(attempt, false), classify(attempt, true)]).toEqual([read, write]);
    });
  }
});

it("waits Retry-After, then the primary reset, then backoff with jitter", () => {
  expect(retryDelayMs(res(429, { "retry-after": "7" }), 0, 0, 500)).toBe(7000);
  expect(retryDelayMs(res(403, { "x-ratelimit-remaining": "0", "x-ratelimit-reset": "100" }), 0, 90_000, 500)).toBe(10_000);
  expect(retryDelayMs(res(502), 2, 0, 500, () => 0)).toBe(1000);
  expect(retryDelayMs(res(502), 2, 0, 500, () => 1)).toBe(2000);
});

it("builds an RS256 App JWT that verifies with the public key and carries iat, exp and iss", () => {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwt = appJwt("12345", privateKey.export({ type: "pkcs8", format: "pem" }).toString(), 1_700_000_000);
  const [header, payload, signature] = jwt.split(".") as [string, string, string];
  expect(verify("sha256", Buffer.from(`${header}.${payload}`), publicKey, Buffer.from(signature, "base64url"))).toBe(true);
  expect(JSON.parse(Buffer.from(header, "base64url").toString())).toEqual({ alg: "RS256", typ: "JWT" });
  expect(JSON.parse(Buffer.from(payload, "base64url").toString())).toEqual({ iat: 1_699_999_940, exp: 1_700_000_540, iss: "12345" });
});

describe("against a fake GitHub", () => {
  let fake: FakeGitHub;
  beforeEach(async () => (fake = await FakeGitHub.start()));
  afterEach(() => fake.close());

  const client = (extra: Partial<ConstructorParameters<typeof GitHubClient>[0]> = {}) => {
    const sleeps: number[] = [];
    const c = new GitHubClient({
      auth: { kind: "token", token: "ghp_test" },
      baseUrl: fake.url,
      maxRetries: 2,
      backoffMs: 10,
      concurrency: 2,
      timeoutMs: 5000,
      sleep: async (ms) => void sleeps.push(ms),
      ...extra,
    });
    return { c, sleeps };
  };

  it("retries a rate-limited POST after Retry-After and posts once", async () => {
    fake.postBehaviors = ["rate-limited"];
    const { c, sleeps } = client();
    const { data } = await c.request<{ id: number }>("POST", "/repos/a/b/pulls/1/reviews", { body: { body: "x", commit_id: "c", event: "COMMENT" } });
    expect([fake.postAttempts, fake.reviews.length, sleeps, data.id]).toEqual([2, 1, [1000], 1000]);
  });

  it("never retries an ambiguous write", async () => {
    fake.postBehaviors = ["bad-gateway", "ok"];
    const { c } = client();
    const error = await c.request("POST", "/repos/a/b/pulls/1/reviews", { body: {} }).catch((e) => e);
    expect([error instanceof GitHubError && error.class, fake.postAttempts]).toEqual(["ambiguous", 1]);
  });

  it("gives up a read after max_retries", async () => {
    fake.listStatus = 500;
    const { c, sleeps } = client();
    const error = await c.paginate("/repos/a/b/pulls/1/reviews").catch((e) => e);
    expect([error.class, fake.listCalls, sleeps.length]).toEqual(["retryable", 3, 2]);
  });

  it("follows every page", async () => {
    fake.pageSize = 2;
    fake.addForeignReviews(5, "c");
    const { c } = client();
    expect((await c.paginate<{ id: number }>("/repos/a/b/pulls/1/reviews")).map((r) => r.id)).toEqual([1000, 1001, 1002, 1003, 1004]);
    expect(fake.listCalls).toBe(3);
  });

  it("caps requests in flight per installation", async () => {
    let inFlight = 0;
    let peak = 0;
    const original = fake.server.listeners("request")[0] as (...a: unknown[]) => void;
    fake.server.removeAllListeners("request");
    fake.server.on("request", (req, res) => {
      peak = Math.max(peak, ++inFlight);
      res.on("finish", () => inFlight--);
      setTimeout(() => original(req, res), 20);
    });
    const { c } = client();
    await Promise.all(Array.from({ length: 6 }, () => c.request("GET", "/repos/a/b/pulls/1").catch(() => undefined)));
    expect(peak).toBe(2);
  });

  it("mints an installation token with the App JWT once and reuses it", async () => {
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    const { c } = client({ auth: { kind: "app", appId: "1", privateKeyPem: privateKey.export({ type: "pkcs8", format: "pem" }).toString(), installationId: 77 } });
    await c.request("GET", "/repos/a/b/pulls/1").catch(() => undefined);
    await c.request("GET", "/repos/a/b/pulls/1").catch(() => undefined);
    expect(await c.viewerLogin()).toBe("durable-review[bot]");
    expect(fake.tokensMinted).toBe(1);
    expect(fake.requests.map((r) => `${r.method} ${r.path} ${r.authorization?.startsWith("Bearer ey") ? "jwt" : r.authorization}`)).toEqual([
      "POST /app/installations/77/access_tokens jwt",
      "GET /repos/a/b/pulls/1 Bearer ghs_install_1",
      "GET /repos/a/b/pulls/1 Bearer ghs_install_1",
      "GET /app jwt",
    ]);
  });

  it("refuses to send credentials to a pagination link on another host", async () => {
    const { c } = client();
    await expect(c.request("GET", "https://evil.example/x")).rejects.toThrow(`refusing to call https://evil.example/x outside ${fake.url}`);
  });
});

it("gives up on a rate-limit wait longer than the caller's cap with a retryable error, without sleeping", async () => {
  const fake = await FakeGitHub.start();
  try {
    const sleeps: number[] = [];
    const client = new GitHubClient({ auth: { kind: "token", token: "t" }, baseUrl: fake.url, maxRetries: 3, backoffMs: 1, concurrency: 1, timeoutMs: 5000, sleep: async (ms) => void sleeps.push(ms) });
    fake.setPull({ installationId: 0, repositoryId: 1, fullName: "acme/shop", number: 1, title: "t", author: "octocat", body: "", baseSha: "b", headSha: "h", state: "open", merged: false, private: false, cloneUrl: "x" });
    fake.rateLimitedPullReads = 1;
    const error = await new GitHubPullSource(() => client).getPull({ owner: "acme", repo: "shop", number: 1 }, { maxSleepMs: 30_000 }).catch((e: unknown) => e);
    expect([error instanceof GitHubError && error.class, (error as Error).message, sleeps]).toEqual([
      "retryable",
      "GET /repos/acme/shop/pulls/1: HTTP 403; rate limited for 120000 ms, over the 30000 ms cap",
      [],
    ]);
    fake.rateLimitedPullReads = 1;
    expect((await new GitHubPullSource(() => client).getPull({ owner: "acme", repo: "shop", number: 1 })).headSha).toBe("h");
    expect(sleeps).toEqual([120_000]);
  } finally {
    await fake.close();
  }
});

it("reads the PR author's login, and null for a PR GitHub reports no user on", async () => {
  const fake = await FakeGitHub.start();
  try {
    const client = new GitHubClient({ auth: { kind: "token", token: "t" }, baseUrl: fake.url, maxRetries: 0, backoffMs: 1, concurrency: 1, timeoutMs: 5000 });
    const pulls = new GitHubPullSource(() => client);
    const pull = { installationId: 0, repositoryId: 1, fullName: "acme/shop", number: 1, title: "t", author: "octocat", body: "", baseSha: "b", headSha: "h", state: "open" as const, merged: false, private: false, cloneUrl: "x" };
    fake.setPull(pull);
    expect((await pulls.getPull({ owner: "acme", repo: "shop", number: 1 })).author).toBe("octocat");
    fake.setPull({ ...pull, author: null });
    expect((await pulls.getPull({ owner: "acme", repo: "shop", number: 1 })).author).toBeNull();
  } finally {
    await fake.close();
  }
});
