import { sign } from "node:crypto";

export type GitHubAuth =
  /** A PAT or `gh auth token`; `undefined` reads public repositories anonymously. */
  | { kind: "token"; token: string | undefined }
  | { kind: "app"; appId: string; privateKeyPem: string; installationId: number };

export type ResponseClass = "ok" | "retryable" | "ambiguous" | "fatal";

/** What one HTTP attempt produced. `sent: false` only when the connection never opened, so the server saw nothing. */
export type Attempt = { kind: "response"; status: number; headers: Headers; body: string } | { kind: "network"; sent: boolean };

const status = (a: Attempt, test: (s: number) => boolean) => a.kind === "response" && test(a.status);
const rateLimited = (a: Attempt) =>
  a.kind === "response" && (a.headers.get("x-ratelimit-remaining") === "0" || a.headers.has("retry-after") || /secondary rate limit/i.test(a.body));

// First match wins. A rate-limit answer means GitHub refused the request unprocessed, so even a write may repeat it.
// A write that may have reached GitHub (5xx, a dropped or timed-out connection) is ambiguous: only reconciliation decides.
const RULES: { when: (a: Attempt) => boolean; read: ResponseClass; write: ResponseClass }[] = [
  { when: (a) => status(a, (s) => s >= 200 && s < 300), read: "ok", write: "ok" },
  { when: (a) => status(a, (s) => s === 429), read: "retryable", write: "retryable" },
  { when: (a) => status(a, (s) => s === 403) && rateLimited(a), read: "retryable", write: "retryable" },
  { when: (a) => status(a, (s) => s >= 500), read: "retryable", write: "ambiguous" },
  { when: (a) => a.kind === "network" && !a.sent, read: "retryable", write: "retryable" },
  { when: (a) => a.kind === "network" && a.sent, read: "retryable", write: "ambiguous" },
];

export function classify(attempt: Attempt, write: boolean): ResponseClass {
  const rule = RULES.find((r) => r.when(attempt));
  return rule ? rule[write ? "write" : "read"] : "fatal";
}

/** `Retry-After`, else the primary-limit reset, else exponential backoff with jitter. */
export function retryDelayMs(attempt: Attempt, retry: number, nowMs: number, backoffMs: number, random = Math.random): number {
  if (attempt.kind === "response") {
    const after = attempt.headers.get("retry-after");
    if (after !== null && /^\d+$/.test(after)) return Number(after) * 1000;
    const reset = attempt.headers.get("x-ratelimit-reset");
    if (attempt.headers.get("x-ratelimit-remaining") === "0" && reset !== null) return Math.max(0, Number(reset) * 1000 - nowMs);
  }
  return Math.round(backoffMs * 2 ** retry * (0.5 + random() / 2));
}

export class GitHubError extends Error {
  readonly class: Exclude<ResponseClass, "ok">;
  readonly status: number | undefined;

  constructor(cls: Exclude<ResponseClass, "ok">, message: string, status?: number) {
    super(message);
    this.class = cls;
    this.status = status;
  }
}

/** RS256 GitHub App JWT; `iat` is backdated 60 s for clock drift and `exp` stays under GitHub's 10 minute cap. */
export function appJwt(appId: string, privateKeyPem: string, nowSec = Math.floor(Date.now() / 1000)): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const unsigned = `${part({ alg: "RS256", typ: "JWT" })}.${part({ iat: nowSec - 60, exp: nowSec + 540, iss: appId })}`;
  return `${unsigned}.${sign("sha256", Buffer.from(unsigned), privateKeyPem).toString("base64url")}`;
}

// Errors where the TCP connection never opened, so no byte of the request reached GitHub.
const NOT_SENT = new Set(["ECONNREFUSED", "ENOTFOUND", "EAI_AGAIN", "ENETUNREACH", "EHOSTUNREACH"]);

export type ClientOptions = {
  auth: GitHubAuth;
  baseUrl?: string;
  maxRetries: number;
  backoffMs: number;
  /** Requests in flight at once for this client, which is per installation. */
  concurrency: number;
  timeoutMs: number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
};

/** `maxSleepMs` caps one retry wait: a longer rate-limit wait throws a retryable error instead of blocking the caller. */
export type RequestOptions = { body?: unknown; signal?: AbortSignal; jwt?: boolean; idempotent?: boolean; maxSleepMs?: number };

export class GitHubClient {
  readonly baseUrl: string;
  readonly auth: GitHubAuth;
  private readonly options: ClientOptions;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly now: () => number;
  private installationToken: { token: string; expiresAt: number } | undefined;
  private login: string | undefined;
  private active = 0;
  private readonly queue: (() => void)[] = [];

  constructor(options: ClientOptions) {
    this.options = options;
    this.auth = options.auth;
    this.baseUrl = (options.baseUrl ?? "https://api.github.com").replace(/\/$/, "");
    this.sleep = options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.now = options.now ?? Date.now;
  }

  async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<{ data: T; headers: Headers }> {
    const url = path.startsWith("http") ? path : `${this.baseUrl}${path}`;
    // A pagination link must not carry our credentials to another host.
    if (!url.startsWith(`${this.baseUrl}/`)) throw new GitHubError("fatal", `refusing to call ${url} outside ${this.baseUrl}`);
    const write = method !== "GET" && !options.idempotent;
    for (let retry = 0; ; retry++) {
      // Resolved outside the slot: minting an installation token is itself a request and needs a slot.
      const credential = options.jwt && this.auth.kind === "app" ? appJwt(this.auth.appId, this.auth.privateKeyPem, Math.floor(this.now() / 1000)) : await this.token();
      const attempt = await this.limited(() => this.attempt(method, url, credential, options));
      const cls = classify(attempt, write);
      if (cls === "ok" && attempt.kind === "response") {
        return { data: (attempt.body ? JSON.parse(attempt.body) : undefined) as T, headers: attempt.headers };
      }
      const what = attempt.kind === "response" ? `HTTP ${attempt.status}` : attempt.sent ? "connection lost after sending" : "connection failed";
      const error = new GitHubError(cls === "ok" ? "fatal" : cls, `${method} ${url.slice(this.baseUrl.length)}: ${what}`, attempt.kind === "response" ? attempt.status : undefined);
      if (cls !== "retryable" || retry >= this.options.maxRetries || options.signal?.aborted) throw error;
      const delay = retryDelayMs(attempt, retry, this.now(), this.options.backoffMs);
      if (options.maxSleepMs !== undefined && delay > options.maxSleepMs) {
        throw new GitHubError("retryable", `${error.message}; rate limited for ${delay} ms, over the ${options.maxSleepMs} ms cap`, error.status);
      }
      await this.sleep(delay);
    }
  }

  /** Every item of a list endpoint, following `Link: rel="next"` to the last page. */
  async paginate<T>(path: string): Promise<T[]> {
    const items: T[] = [];
    for (let next: string | undefined = `${path}${path.includes("?") ? "&" : "?"}per_page=100`; next; ) {
      const { data, headers }: { data: T[]; headers: Headers } = await this.request<T[]>("GET", next);
      items.push(...data);
      next = /<([^>]+)>;\s*rel="next"/.exec(headers.get("link") ?? "")?.[1];
    }
    return items;
  }

  /** The login a review we post is authored by: the app's bot user, or the token's user. */
  async viewerLogin(options: { maxSleepMs?: number } = {}): Promise<string> {
    if (this.login === undefined) {
      this.login =
        this.auth.kind === "app"
          ? `${(await this.request<{ slug: string }>("GET", "/app", { jwt: true, ...options })).data.slug}[bot]`
          : (await this.request<{ login: string }>("GET", "/user", options)).data.login;
    }
    return this.login;
  }

  /** The installation token, minted on first use and again within 60 s of expiry. */
  async token(): Promise<string | undefined> {
    if (this.auth.kind === "token") return this.auth.token;
    if (!this.installationToken || this.installationToken.expiresAt - 60_000 <= this.now()) {
      const { data } = await this.request<{ token: string; expires_at: string }>("POST", `/app/installations/${this.auth.installationId}/access_tokens`, {
        jwt: true,
        idempotent: true,
      });
      this.installationToken = { token: data.token, expiresAt: Date.parse(data.expires_at) };
    }
    return this.installationToken.token;
  }

  private async attempt(method: string, url: string, credential: string | undefined, options: RequestOptions): Promise<Attempt> {
    // Aborted before fetch was called: no byte left this process.
    if (options.signal?.aborted) return { kind: "network", sent: false };
    const signal = AbortSignal.any([AbortSignal.timeout(this.options.timeoutMs), ...(options.signal ? [options.signal] : [])]);
    try {
      const response = await fetch(url, {
        method,
        headers: {
          accept: "application/vnd.github+json",
          "x-github-api-version": "2022-11-28",
          "user-agent": "pr-review",
          ...(credential ? { authorization: `Bearer ${credential}` } : {}),
          ...(options.body === undefined ? {} : { "content-type": "application/json" }),
        },
        ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
        signal,
      });
      return { kind: "response", status: response.status, headers: response.headers, body: await response.text() };
    } catch (error) {
      const code = (error as { cause?: { code?: string } }).cause?.code;
      return { kind: "network", sent: !(code && NOT_SENT.has(code)) };
    }
  }

  private async limited<T>(fn: () => Promise<T>): Promise<T> {
    // A finishing request hands its slot straight to the next waiter, so a newcomer cannot slip past the cap.
    if (this.active < this.options.concurrency) this.active++;
    else await new Promise<void>((resolve) => this.queue.push(resolve));
    try {
      return await fn();
    } finally {
      const next = this.queue.shift();
      if (next) next();
      else this.active--;
    }
  }
}

/** A client with the configured retry budget, against `GITHUB_API_URL` when set (tests point it at a fake). */
export function clientFor(config: { max_retries: number; backoff_ms: number; concurrency_per_installation: number; timeout_ms: number }, auth: GitHubAuth): GitHubClient {
  return new GitHubClient({
    auth,
    ...(process.env.GITHUB_API_URL ? { baseUrl: process.env.GITHUB_API_URL } : {}),
    maxRetries: config.max_retries,
    backoffMs: config.backoff_ms,
    concurrency: config.concurrency_per_installation,
    timeoutMs: config.timeout_ms,
  });
}
