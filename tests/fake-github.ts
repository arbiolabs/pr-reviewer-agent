import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import type { PullInfo } from "../src/domain/types.ts";

export type StoredReview = { id: number; body: string; commit_id: string; event: string; comments: unknown[]; user: { login: string } };

/** What the server does with one POST of a review: store it and answer, or fail in a way the publisher must survive. */
export type PostBehavior = "ok" | "rate-limited" | "store-then-drop" | "bad-gateway" | "unprocessable" | "slow-insert";

/** An in-process GitHub: pulls, reviews (paginated), the token user, and the App endpoints. It counts review POSTs. */
export class FakeGitHub {
  readonly server: Server;
  pulls = new Map<string, PullInfo>();
  reviews: StoredReview[] = [];
  postAttempts = 0;
  postBehaviors: PostBehavior[] = [];
  listStatus = 200;
  listCalls = 0;
  pageSize = 100;
  viewer = "reviewer-bot";
  /** 403 is what GitHub answers an installation token on `/user`. */
  userStatus = 200;
  /** How long a `slow-insert` POST holds the request before it stores the review. */
  slowInsertMs = 0;
  appSlug = "durable-review";
  tokensMinted = 0;
  requests: { method: string; path: string; authorization: string | undefined }[] = [];
  onGetPull: ((key: string) => void) | undefined;
  /** Answer this many PR reads with a secondary rate limit and `Retry-After: 120`. */
  rateLimitedPullReads = 0;
  onPost: (() => void) | undefined;
  private nextId = 1000;

  constructor() {
    this.server = createServer((req, res) => void this.handle(req, res));
  }

  static async start(): Promise<FakeGitHub> {
    const fake = new FakeGitHub();
    await new Promise<void>((resolve) => fake.server.listen(0, "127.0.0.1", resolve));
    return fake;
  }

  get url(): string {
    return `http://127.0.0.1:${(this.server.address() as AddressInfo).port}`;
  }

  setPull(pull: PullInfo) {
    this.pulls.set(`${pull.fullName}#${pull.number}`, pull);
  }

  /** Reviews another user left, to push ours onto a later page. */
  addForeignReviews(n: number, commitId: string) {
    for (let i = 0; i < n; i++) this.reviews.push({ id: this.nextId++, body: `looks good ${i}`, commit_id: commitId, event: "COMMENT", comments: [], user: { login: "someone" } });
  }

  close(): Promise<void> {
    this.server.closeAllConnections();
    return new Promise((resolve) => this.server.close(() => resolve()));
  }

  private async handle(req: IncomingMessage, res: ServerResponse) {
    let raw = "";
    for await (const chunk of req) raw += chunk;
    const url = new URL(req.url!, this.url);
    this.requests.push({ method: req.method!, path: url.pathname, authorization: req.headers.authorization });
    const json = (status: number, body: unknown, headers: Record<string, string> = {}) => {
      res.writeHead(status, { "content-type": "application/json", ...headers });
      res.end(JSON.stringify(body));
    };
    let m: RegExpExecArray | null;
    if (req.method === "GET" && url.pathname === "/user") return this.userStatus === 200 ? json(200, { login: this.viewer }) : json(this.userStatus, { message: "Resource not accessible by integration" });
    if (req.method === "GET" && url.pathname === "/app") return json(200, { slug: this.appSlug });
    if (req.method === "POST" && /^\/app\/installations\/\d+\/access_tokens$/.test(url.pathname)) {
      this.tokensMinted++;
      return json(201, { token: `ghs_install_${this.tokensMinted}`, expires_at: new Date(Date.now() + 3600_000).toISOString() });
    }
    if ((m = /^\/repos\/([^/]+\/[^/]+)\/pulls\/(\d+)$/.exec(url.pathname)) && req.method === "GET") {
      const key = `${m[1]}#${m[2]}`;
      this.onGetPull?.(key);
      if (this.rateLimitedPullReads > 0 && this.rateLimitedPullReads--) return json(403, { message: "You have exceeded a secondary rate limit" }, { "retry-after": "120" });
      const pull = this.pulls.get(key);
      if (!pull) return json(404, { message: "Not Found" });
      return json(200, {
        number: pull.number,
        title: pull.title,
        body: pull.body,
        state: pull.state,
        merged: pull.merged,
        head: { sha: pull.headSha },
        base: { sha: pull.baseSha, repo: { id: pull.repositoryId, full_name: pull.fullName, clone_url: pull.cloneUrl, private: pull.private } },
      });
    }
    if ((m = /^\/repos\/([^/]+\/[^/]+)\/pulls\/(\d+)\/reviews$/.exec(url.pathname))) {
      if (req.method === "POST") return this.post(raw, req, res, json);
      this.listCalls++;
      if (this.listStatus !== 200) return json(this.listStatus, { message: "boom" });
      const page = Number(url.searchParams.get("page") ?? "1");
      const items = this.reviews.slice((page - 1) * this.pageSize, page * this.pageSize);
      const more = page * this.pageSize < this.reviews.length;
      return json(200, items, more ? { link: `<${this.url}${url.pathname}?per_page=${this.pageSize}&page=${page + 1}>; rel="next"` } : {});
    }
    json(404, { message: "Not Found" });
  }

  private async post(raw: string, req: IncomingMessage, res: ServerResponse, json: (status: number, body: unknown, headers?: Record<string, string>) => void) {
    this.postAttempts++;
    this.onPost?.();
    const behavior = this.postBehaviors.shift() ?? "ok";
    if (behavior === "slow-insert") await new Promise((resolve) => setTimeout(resolve, this.slowInsertMs));
    if (behavior === "rate-limited") return json(429, { message: "You have exceeded a secondary rate limit" }, { "retry-after": "1" });
    if (behavior === "bad-gateway") return json(502, { message: "Bad Gateway" });
    if (behavior === "unprocessable") return json(422, { message: "Line could not be resolved" });
    const body = JSON.parse(raw) as Omit<StoredReview, "id" | "user">;
    const login = req.headers.authorization?.includes("ghs_install") ? `${this.appSlug}[bot]` : this.viewer;
    const review: StoredReview = { id: this.nextId++, body: body.body, commit_id: body.commit_id, event: body.event, comments: body.comments ?? [], user: { login } };
    this.reviews.push(review);
    if (behavior === "store-then-drop") return req.socket.destroy();
    json(200, review);
  }
}
