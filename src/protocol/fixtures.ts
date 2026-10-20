// Source of protocol/fixtures and protocol/playback. Edit here, then run `pnpm protocol:emit`.
import { sha256, stableId } from "../domain/types.ts";
import { type Account, type ClientMessage, type DiffFile, type Evidence, type Finding, type FindingDetail, type Pr, type PublicationChange, REVIEW_COMMANDS, type Run, type ServerMessage } from "./v1.ts";

const EPOCH = "epoch-1";
const T0 = Date.parse("2026-10-20T13:00:00Z");
const at = (s: number) => new Date(T0 + s * 1000).toISOString().replace(".000", "");
const sha = (c: string) => c.repeat(40);

const pr = (n: number, extra: Partial<Pr> = {}): Pr => ({
  id: `acme/shop#${n}`,
  repository: "acme/shop",
  number: n,
  title: `PR ${n}`,
  headSha: sha(String(n % 10)),
  baseSha: sha("b"),
  lifecycle: "ACTIVE",
  ...extra,
});

const run = (id: string, p: Pr, extra: Partial<Run> = {}): Run => ({
  id,
  prId: p.id,
  baseSha: p.baseSha,
  headSha: p.headSha,
  phase: "VERIFY",
  status: "RUNNING",
  model: "claude-opus-5-5",
  startedAt: at(0),
  ...extra,
});

const finding = (id: string, r: Run, extra: Partial<Finding> = {}): Finding => ({
  id,
  prId: r.prId,
  runId: r.id,
  fingerprint: `fp-${id}`,
  title: `Finding ${id}`,
  category: "correctness",
  severity: "high",
  status: "DETECTED",
  confidence: 0.82,
  file: "src/booking.ts",
  line: 42,
  evidenceCount: 1,
  stale: false,
  ...extra,
});

type Seq = { epoch?: string; seq: number; prev?: number; t?: number };
type Sequenced = Exclude<Extract<ServerMessage, { type: "event" }>, { eventType: "heartbeat" }>;
const ev = <E extends Sequenced["eventType"]>(
  s: Seq,
  eventType: E,
  data: Extract<Sequenced, { eventType: E }>["data"],
  ids: { prId?: string; runId?: string },
): ServerMessage =>
  ({
    type: "event",
    streamEpoch: s.epoch ?? EPOCH,
    sequence: s.seq,
    prevSequence: s.prev ?? s.seq - 1,
    occurredAt: at(s.t ?? s.seq),
    ...ids,
    eventType,
    data,
  }) as ServerMessage;

const phase = (s: Seq, r: Run, from: Run["phase"], to: Run["phase"], event: string) =>
  ev(s, "review.phase_changed", { from, to, event, detail: null }, { prId: r.prId, runId: r.id });
const activity = (s: Seq, r: Run, tool: string, summary: string) =>
  ev(s, "tool.activity", { tool, summary }, { prId: r.prId, runId: r.id });
const findingUpdated = (s: Seq, f: Finding) => ev(s, "finding.updated", f, { prId: f.prId, runId: f.runId });
const runEvent = (s: Seq, type: "run.started" | "review.completed" | "run.superseded" | "run.failed", r: Run) =>
  ev(s, type, r, { prId: r.prId, runId: r.id });

const welcome = (latestSequence: number, epoch = EPOCH, capabilities: string[] = [], mode?: "engine" | "read-only", readOnlyReason?: string): ServerMessage => ({
  type: "welcome",
  requestId: "req-hello",
  protocolVersion: 1,
  serverVersion: "0.1.0",
  sessionId: "session-1",
  capabilities,
  ...(mode ? { mode } : {}),
  ...(readOnlyReason ? { readOnlyReason } : {}),
  streamEpoch: epoch,
  latestSequence,
});
const publicationChanged = (s: Seq, r: Run, status: PublicationChange["status"], reviewId?: number) =>
  ev(
    s,
    "review.publication_changed",
    { runId: r.id, status, ...(reviewId ? { reviewId, url: `https://github.com/${r.prId.split("#")[0]}/pull/${r.prId.split("#")[1]}#pullrequestreview-${reviewId}` } : {}) },
    { prId: r.prId, runId: r.id },
  );
const snapshot = (sequence: number, prs: Pr[], runs: Run[], findings: Finding[], epoch = EPOCH, accounts?: Account[]): ServerMessage => ({
  type: "snapshot",
  requestId: "req-snapshot",
  streamEpoch: epoch,
  sequence,
  prs,
  runs,
  findings,
  ...(accounts ? { accounts } : {}),
});

// ---------- shared world ----------

const pr1 = pr(184, { title: "Booking API", author: "octocat" });
const pr2 = pr(185, { title: "Fix cache eviction" });
const pr3 = pr(186, { title: "Bump deps", lifecycle: "QUIESCING" });
const run1 = run("rev-184-a", pr1);
const run2 = run("rev-185-a", pr2, { phase: "COMPLETED", status: "COMPLETED", completedAt: at(-60) });
const f1 = finding("f-1", run1);
const f2 = finding("f-2", run2, { severity: "low", category: "performance", line: null, file: "src/cache.ts" });

const account = (host: string, login: string, extra: Partial<Account> = {}): Account => ({
  id: `${host}/${login}`,
  host,
  login,
  state: "ok",
  ghActive: false,
  selected: false,
  scopes: ["gist", "read:org", "repo", "workflow"],
  ...extra,
});
const personal = account("github.com", "diego-arbio-dev", { ghActive: true });
const work = account("github.com", "octo-work", { scopes: ["repo"] });
const expired = account("github.com", "old-bot", { state: "error", scopes: [] });
const accountsUpdated = (s: Seq, accounts: Account[]) => ev(s, "accounts.updated", { accounts }, {});
const PREVIEW_ID = sha256("payload of rev-184-a");
const preview: ServerMessage = {
  type: "publication.preview",
  requestId: "req-pub",
  runId: run1.id,
  previewId: PREVIEW_ID,
  headSha: pr1.headSha,
  body: `## Automated PR review\n\nReviewed commit: \`${pr1.headSha}\`\n\nFound 1 actionable issue.\n\n- High: Finding f-1 (\`src/booking.ts:42\`)\n\nReview performed with isolated read-only tools. Human review is still required.\n<!-- durable-review:${run1.id}:${PREVIEW_ID} -->\n`,
  comments: [{ path: "src/booking.ts", line: 42, side: "RIGHT", body: "**High: Finding f-1**\n\nTwo concurrent bookings can both observe `slot.free` before either sets `held`." }],
  event: "COMMENT",
  status: "awaiting_approval",
};
const publishCommand = {
  type: "command" as const,
  requestId: "req-cmd",
  command: "review.publish" as const,
  params: { runId: run1.id, previewId: PREVIEW_ID, expectedHeadSha: pr1.headSha },
  idempotencyKey: `review.publish:${run1.id}:${PREVIEW_ID}`,
};
const selectCommand = (a: Account) => ({
  type: "command" as const,
  requestId: "req-cmd",
  command: "account.select" as const,
  params: { accountId: a.id },
  idempotencyKey: `account.select:${a.id}`,
});

// ---------- showcase: a believable session for judging TUI designs ----------
// Diffs are written as unified-diff text; findings and evidence look their lines up in them, so a cited line can never
// disagree with the hunk the Diff view shows.

type Hunk = DiffFile["hunks"][number];
type DiffLine = Hunk["lines"][number];

const hunk = (oldStart: number, newStart: number, context: string, body: string): Hunk => {
  let o = oldStart;
  let n = newStart;
  const rows = body.split("\n").slice(1, -1).map((raw): DiffLine => {
    const [mark, text] = [raw[0] ?? " ", raw.slice(1)];
    if (mark === "+") return { kind: "add", new: n++, text };
    if (mark === "-") return { kind: "del", old: o++, text };
    if (mark !== " ") throw new Error(`diff line has no +, - or space marker: ${raw}`);
    return { kind: "context", old: o++, new: n++, text };
  });
  const header = `@@ -${oldStart},${o - oldStart} +${newStart},${n - newStart} @@${context ? ` ${context}` : ""}`;
  return { header, oldStart, newStart, lines: rows };
};

const diffFile = (path: string, hunks: Hunk[], status: DiffFile["status"] = "modified"): DiffFile => {
  const all = hunks.flatMap((h) => h.lines);
  return {
    path,
    status,
    additions: all.filter((l) => l.kind === "add").length,
    deletions: all.filter((l) => l.kind === "del").length,
    hunks,
    truncated: false,
  };
};

/** The head-revision line number of the one line containing `needle`. */
const lineOf = (file: DiffFile, needle: string): number => {
  const hits = file.hunks.flatMap((h) => h.lines).filter((l) => l.kind !== "del" && l.text.includes(needle));
  if (hits.length !== 1) throw new Error(`${file.path}: ${hits.length} head lines contain ${JSON.stringify(needle)}`);
  return hits[0]!.new!;
};

/** A code_ref evidence item quoting head lines `from`..`to` (each found by its text) exactly as the diff has them. */
const cite = (file: DiffFile, refSha: string, from: string, to: string): Evidence => {
  const [start, end] = [lineOf(file, from), lineOf(file, to)];
  const text = file.hunks
    .flatMap((h) => h.lines)
    .filter((l) => l.kind !== "del" && l.new! >= start && l.new! <= end)
    .map((l) => l.text);
  if (text.length !== end - start + 1) throw new Error(`${file.path}:${start}-${end} spans a gap between hunks`);
  return { kind: "code_ref", path: file.path, start, end, refSha, text: text.join("\n") };
};

const hexSha = (seed: string) => sha256(seed).slice(0, 40);
const showPr = (repository: string, number: number, title: string, author: string, extra: Partial<Pr> = {}): Pr => ({
  id: stableId("pr", repository, number),
  repository,
  number,
  title,
  author,
  headSha: hexSha(`${repository}#${number} head`),
  baseSha: hexSha(`${repository}#${number} base`),
  lifecycle: "ACTIVE",
  ...extra,
});
const showRun = (p: Pr, startedAt: number, extra: Partial<Run> = {}): Run => ({
  id: stableId("revision", p.id, p.headSha),
  prId: p.id,
  baseSha: p.baseSha,
  headSha: p.headSha,
  phase: "COMPLETED",
  status: "COMPLETED",
  model: "claude-opus-5-5",
  startedAt: at(startedAt),
  ...extra,
});

type Authored = Pick<
  FindingDetail,
  "title" | "category" | "severity" | "confidence" | "file" | "line" | "explanation" | "reasoningSummary" | "suggestedFix" | "evidence"
>;
const showDetail = (r: Run, seenAt: number, a: Authored, extra: Partial<Finding> = {}): FindingDetail => {
  const fingerprint = stableId("fingerprint", r.prId, a.file, a.title);
  return {
    id: stableId("finding", r.id, fingerprint),
    prId: r.prId,
    runId: r.id,
    fingerprint,
    status: "DETECTED",
    evidenceCount: a.evidence.length,
    stale: false,
    ...a,
    ...extra,
    history: [{ runId: r.id, disposition: "DETECTED", at: at(seenAt) }],
  };
};
const findingOf = ({ explanation: _e, reasoningSummary: _r, suggestedFix: _s, evidence: _v, history: _h, ...f }: FindingDetail): Finding => f;

// The session opens at 14:30; T0 is 13:00.
const SHOW_OPEN = 5400;

// 1. acme/shop#1287: COMPLETED, one finding of each severity.
const orders = showPr("acme/shop", 1287, "Orders: self-serve refunds, stock holds at checkout, paged order history", "mkowalski");
const ordersRun = showRun(orders, SHOW_OPEN - 2460, { completedAt: at(SHOW_OPEN - 2190) });

const ordersRoute = diffFile("src/server/routes/orders.ts", [
  hunk(4, 4, "", `
 import { requireAuth } from "../http/auth.ts";
 import { toOrderDto } from "../orders/dto.ts";
+import { RefundBody } from "../schemas/refund.ts";
 import type { Db, Payments } from "../deps.ts";
`),
  hunk(84, 85, "export function ordersRouter(db: Db, payments: Payments) {", `
   router.get("/orders/:id", requireAuth, async (req, res) => {
     const order = await db.order.findUnique({ where: { id: req.params.id } });
     if (!order || order.customerId !== req.user.id) return res.sendStatus(404);
     res.json(toOrderDto(order));
   });

+  router.post("/orders/:id/refund", requireAuth, async (req, res) => {
+    const { amountCents, reason } = RefundBody.parse(req.body);
+    const order = await db.order.findUnique({
+      where: { id: req.params.id },
+      include: { payment: true },
+    });
+    if (!order) return res.sendStatus(404);
+    if (amountCents > order.totalCents - order.refundedCents) {
+      return res.status(422).json({ error: "refund exceeds captured amount" });
+    }
+    const refund = await payments.refund(order.payment.providerId, amountCents, { reason });
+    await db.order.update({
+      where: { id: order.id },
+      data: { refundedCents: { increment: amountCents } },
+    });
+    res.status(201).json({ refundId: refund.id });
+  });
+
   return router;
 }
`),
]);

const reserve = diffFile("src/server/inventory/reserve.ts", [
  hunk(16, 16, "", `
 const HOLD_MINUTES = 15;

 export async function reserveStock(db: Db, sku: string, qty: number, cartId: string) {
-  const [{ ok }] = await db.$queryRaw<{ ok: boolean }[]>\`SELECT reserve_stock(\${sku}, \${qty}, \${cartId}) AS ok\`;
-  if (!ok) throw new OutOfStockError(sku);
+  const item = await db.inventory.findUnique({ where: { sku } });
+  if (!item) throw new UnknownSkuError(sku);
+  const available = item.onHand - item.reserved;
+  if (available < qty) {
+    throw new OutOfStockError(sku, available);
+  }
+  await db.inventory.update({
+    where: { sku },
+    data: { reserved: item.reserved + qty },
+  });
+  await db.reservation.create({
+    data: { sku, qty, cartId, expiresAt: addMinutes(new Date(), HOLD_MINUTES) },
+  });
 }
`),
]);

const history = diffFile(
  "src/server/orders/history.ts",
  [
    hunk(0, 1, "", `
+import type { Db } from "../deps.ts";
+
+export const PAGE_SIZE = 20;
+
+export async function orderHistory(db: Db, customerId: string, page: number) {
+  const total = await db.order.count({ where: { customerId } });
+  const lastPage = Math.max(1, Math.ceil(total / PAGE_SIZE));
+  const orders = await db.order.findMany({
+    where: { customerId },
+    orderBy: { placedAt: "desc" },
+    skip: (page - 1) * PAGE_SIZE,
+    take: PAGE_SIZE,
+  });
+
+  const withItems = [];
+  for (const order of orders) {
+    const items = await db.orderItem.findMany({
+      where: { orderId: order.id },
+      include: { product: true },
+    });
+    withItems.push({ ...order, items });
+  }
+
+  return {
+    orders: withItems,
+    page,
+    lastPage,
+    hasNext: page <= lastPage,
+    hasPrev: page > 1,
+  };
+}
`),
  ],
  "added",
);

const ordersSeen = SHOW_OPEN - 2230;
const refundAuthz = showDetail(ordersRun, ordersSeen, {
  title: "Any signed-in customer can refund any order",
  category: "security",
  severity: "critical",
  confidence: 0.94,
  file: ordersRoute.path,
  line: lineOf(ordersRoute, "if (!order) return res.sendStatus(404);"),
  explanation:
    "POST /orders/:id/refund loads the order by id and refunds it without checking that the caller owns it. `requireAuth` only proves the caller is signed in. Order ids appear in receipt emails and support tickets, so anyone who learns one can refund another customer's order up to the captured amount while it still ships.",
  reasoningSummary:
    "Compared the new handler with GET /orders/:id in the same router, which returns 404 when order.customerId is not req.user.id. requireAuth (src/server/http/auth.ts) sets req.user and checks nothing else. No router-level middleware checks ownership or the support role.",
  suggestedFix:
    'Apply the same ownership check as the GET handler before refunding, and let support staff through explicitly:\n\nif (!order || (order.customerId !== req.user.id && !req.user.roles.includes("support"))) {\n  return res.sendStatus(404);\n}',
  evidence: [
    cite(ordersRoute, orders.headSha, 'router.post("/orders/:id/refund"', "const refund = await payments.refund("),
    cite(ordersRoute, orders.headSha, 'router.get("/orders/:id"', "res.json(toOrderDto(order));"),
    { kind: "tool_summary", text: 'search_code "customerId !== req.user.id": 4 matches; every other order route checks ownership' },
  ],
});
const oversell = showDetail(ordersRun, ordersSeen + 6, {
  title: "Two checkouts can both reserve the last unit",
  category: "concurrency",
  severity: "high",
  confidence: 0.88,
  file: reserve.path,
  line: lineOf(reserve, "data: { reserved: item.reserved + qty },"),
  explanation:
    "reserveStock now reads the row, checks availability in application code, and writes back an absolute `reserved` value. Two checkouts for the last unit can read the same row, both pass the `available < qty` check, and both write `item.reserved + qty`. The second write overwrites the first, so two reservations hold one unit and `reserved` undercounts. The removed reserve_stock() SQL function did the check and the increment under a row lock.",
  reasoningSummary:
    "The diff deletes the only call to reserve_stock(). git_history shows it was added in #977 to fix overselling on limited drops. The new code runs outside db.$transaction, and an update with a computed literal is not a compare-and-set.",
  suggestedFix:
    "Make the check and the increment one conditional statement, and create the reservation in the same transaction:\n\nconst held = await tx.$executeRaw`UPDATE inventory SET reserved = reserved + ${qty} WHERE sku = ${sku} AND on_hand - reserved >= ${qty}`;\nif (held === 0) throw new OutOfStockError(sku);\n\nOr restore the reserve_stock() call.",
  evidence: [
    cite(reserve, orders.headSha, "const item = await db.inventory.findUnique(", "data: { reserved: item.reserved + qty },"),
    {
      kind: "scenario",
      text: "Carts A and B check out TEE-BLK-M (onHand 10, reserved 9) 4 ms apart. Both read reserved = 9 and see 1 available. Both write reserved = 10. Two reservations now exist for one unit.",
    },
    { kind: "tool_summary", text: 'git log -S reserve_stock: added in #977 "Fix oversell on limited drops" (row lock inside the SQL function)' },
  ],
});
const historyN1 = showDetail(ordersRun, ordersSeen + 11, {
  title: "Order history runs one item query per order",
  category: "performance",
  severity: "medium",
  confidence: 0.9,
  file: history.path,
  line: lineOf(history, "const items = await db.orderItem.findMany({"),
  explanation:
    "orderHistory fetches a page of orders, then awaits a separate orderItem.findMany for each one, in sequence. A full page costs 22 round trips (count, page, 20 item lookups) and none of them overlap, so latency grows with the page size. Repeat customers hit a full page on every visit.",
  reasoningSummary:
    "The loop awaits inside for...of, so the queries are sequential. The Order model already has an `items` relation (prisma/schema.prisma), so the items can come back with the orders in one query.",
  suggestedFix:
    'Load the items with the orders and drop the loop:\n\nconst orders = await db.order.findMany({\n  where: { customerId },\n  orderBy: { placedAt: "desc" },\n  skip: (page - 1) * PAGE_SIZE,\n  take: PAGE_SIZE,\n  include: { items: { include: { product: true } } },\n});',
  evidence: [
    cite(history, orders.headSha, "const withItems = [];", "withItems.push({ ...order, items });"),
    { kind: "tool_summary", text: 'search_code "orderHistory(": 1 caller, GET /account/orders (src/server/routes/account.ts:61), PAGE_SIZE 20' },
  ],
});
const pagerOffByOne = showDetail(ordersRun, ordersSeen + 14, {
  title: "Last page of order history still reports a next page",
  category: "correctness",
  severity: "low",
  confidence: 0.83,
  file: history.path,
  line: lineOf(history, "hasNext: page <= lastPage,"),
  explanation:
    "`lastPage` is 1-based and inclusive, so `page <= lastPage` is still true on the last page. The account page renders a Next link there. Following it skips past every order and shows an empty list labelled \"Page 4 of 3\".",
  reasoningSummary: "page is 1-based (skip uses page - 1), and lastPage is ceil(total / PAGE_SIZE), the number of the last page. The comparison should be strict.",
  suggestedFix: "Use a strict comparison: hasNext: page < lastPage,",
  evidence: [
    cite(history, orders.headSha, "return {", "hasPrev: page > 1,"),
    cite(history, orders.headSha, "const total = await db.order.count(", "take: PAGE_SIZE,"),
    { kind: "scenario", text: "47 orders, PAGE_SIZE 20: lastPage = 3. On page 3, hasNext = (3 <= 3) = true. Page 4 skips 60 rows and returns none." },
  ],
});

// 2. acme/api#341: COMPLETED, nothing actionable.
const requestIds = showPr("acme/api", 341, "Propagate X-Request-ID into structured logs", "tnakamura", { lifecycle: "MERGED" });
const requestIdsRun = showRun(requestIds, SHOW_OPEN - 5320, { completedAt: at(SHOW_OPEN - 5130) });
const requestIdGo = diffFile(
  "internal/httpx/requestid.go",
  [
    hunk(0, 1, "", `
+package httpx
+
+import (
+\t"context"
+\t"log/slog"
+\t"net/http"
+
+\t"github.com/google/uuid"
+)
+
+type ctxKey struct{}
+
+// RequestID reuses an inbound X-Request-ID or mints one, and attaches it to the request's logger.
+func RequestID(next http.Handler) http.Handler {
+\treturn http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
+\t\tid := r.Header.Get("X-Request-ID")
+\t\tif id == "" || len(id) > 64 {
+\t\t\tid = uuid.NewString()
+\t\t}
+\t\tw.Header().Set("X-Request-ID", id)
+\t\tlogger := slog.Default().With("request_id", id)
+\t\tnext.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), ctxKey{}, logger)))
+\t})
+}
+
+// Logger returns the request-scoped logger, or the default one outside a request.
+func Logger(ctx context.Context) *slog.Logger {
+\tif l, ok := ctx.Value(ctxKey{}).(*slog.Logger); ok {
+\t\treturn l
+\t}
+\treturn slog.Default()
+}
`),
  ],
  "added",
);
const apiMain = diffFile("cmd/api/main.go", [
  hunk(47, 47, "func main() {", `
 \tdeps := mustDeps(cfg)
-\thandler := routes(deps)
+\thandler := httpx.RequestID(routes(deps))
 \tsrv := &http.Server{Addr: cfg.Addr, Handler: handler, ReadHeaderTimeout: 5 * time.Second}
`),
]);

// 3. acme/shop#1291: RUNNING when the script starts; it advances live and ends COMPLETED with two findings.
const signIn = showPr("acme/shop", 1291, "Sign-in: send users back to the page they started from", "priya-r");
const signInRun = showRun(signIn, SHOW_OPEN + 0.6, { phase: "COLLECT_CONTEXT", status: "RUNNING" });
const loginTs = diffFile("src/server/auth/login.ts", [
  hunk(6, 6, "export function loginRouter(oauth: OAuthClient) {", `
   router.get("/login", (req, res) => {
+    const returnTo = typeof req.query.returnTo === "string" ? req.query.returnTo : "/";
+    res.cookie("returnTo", returnTo, { httpOnly: true, sameSite: "lax", maxAge: 10 * 60 * 1000 });
     res.redirect(oauth.authorizeUrl());
   });
`),
]);
const callbackTs = diffFile("src/server/auth/callback.ts", [
  hunk(10, 10, "export function callbackRouter(oauth: OAuthClient) {", `
   const router = Router();

   router.get("/auth/callback", async (req, res) => {
     const { code, state } = CallbackQuery.parse(req.query);
     const session = await oauth.exchange(code, state);
-    await regenerateSession(req);
     req.session.userId = session.userId;
-    res.redirect("/");
+    const returnTo = req.cookies.returnTo ?? "/";
+    res.clearCookie("returnTo");
+    res.redirect(returnTo);
   });

   return router;
`),
]);
const signInTsx = diffFile("src/client/pages/SignIn.tsx", [
  hunk(31, 31, "export function SignInButton() {", `
 export function SignInButton() {
-  return <a href="/login">Sign in</a>;
+  const returnTo = encodeURIComponent(location.pathname + location.search);
+  return <a href={\`/login?returnTo=\${returnTo}\`}>Sign in</a>;
 }
`),
]);

// The script's clock: seconds after T0 at which the stream reaches each finding.
const SIGNIN_REDIRECT_AT = SHOW_OPEN + 14.24;
const SIGNIN_SESSION_AT = SHOW_OPEN + 15.44;
const openRedirect = showDetail(signInRun, SIGNIN_REDIRECT_AT, {
  title: "Sign-in redirects to any URL passed in returnTo",
  category: "security",
  severity: "high",
  confidence: 0.92,
  file: callbackTs.path,
  line: lineOf(callbackTs, "res.redirect(returnTo);"),
  explanation:
    "/login stores `req.query.returnTo` in a cookie without checking it, and /auth/callback redirects to that cookie after a real sign-in. A link to https://shop.acme.com/login?returnTo=https://acme-shop.help/verify takes the customer through the genuine sign-in page and then to the attacker's site, which can ask them to re-enter their password or card. Browsers also treat //evil.example and /\\evil.example as absolute.",
  reasoningSummary:
    "Followed returnTo from SignIn.tsx through the login cookie to res.redirect in the callback. Nothing on that path validates it. The repo already has safeRedirect() in src/server/http/safeRedirect.ts, used by logout and account settings, but not here.",
  suggestedFix:
    'Redirect through the existing helper, which accepts only same-origin paths that start with a single "/":\n\nimport { safeRedirect } from "../http/safeRedirect.ts";\n\nsafeRedirect(res, req.cookies.returnTo, "/");',
  evidence: [
    cite(callbackTs, signIn.headSha, 'router.get("/auth/callback"', "res.redirect(returnTo);"),
    cite(loginTs, signIn.headSha, 'router.get("/login"', "res.redirect(oauth.authorizeUrl());"),
    {
      kind: "scenario",
      text: "GET /login?returnTo=https%3A%2F%2Facme-shop.help%2Fverify, then sign in with valid credentials: 302 Location: https://acme-shop.help/verify",
    },
  ],
});
const sessionFixation = showDetail(signInRun, SIGNIN_SESSION_AT, {
  title: "Session id is no longer rotated at sign-in",
  category: "security",
  severity: "medium",
  confidence: 0.74,
  file: callbackTs.path,
  line: lineOf(callbackTs, "req.session.userId = session.userId;"),
  explanation:
    "The PR deletes the `regenerateSession(req)` call that ran before the user id was written. Without it, the session id a visitor had before signing in stays valid afterwards. Anyone who planted that id, on a shared computer or through a cookie set on a sibling subdomain, is now signed in as the victim.",
  reasoningSummary:
    "The deleted line was the only call to regenerateSession. express-session keeps the existing id unless regenerate() is called. The PR description does not mention removing it, so it looks accidental.",
  suggestedFix: "Restore the call before writing the user id:\n\nawait regenerateSession(req);\nreq.session.userId = session.userId;",
  evidence: [
    cite(callbackTs, signIn.headSha, "const { code, state } = CallbackQuery.parse(req.query);", "res.redirect(returnTo);"),
    { kind: "tool_summary", text: 'search_code "regenerateSession(": 1 definition (src/server/session.ts:52), 0 callers after this PR' },
  ],
});

// 4. acme/api#338: the first run SUPERSEDED with a now-stale finding; the author pushed a fix and a new run is under way.
const drainOld = showPr("acme/api", 338, "Drain webhook workers on SIGTERM", "jdoyle");
const drain = { ...drainOld, headSha: hexSha("acme/api#338 head 2") };
const drainOldRun = showRun(drainOld, SHOW_OPEN - 1520, { phase: "SUPERSEDED", status: "SUPERSEDED", completedAt: at(SHOW_OPEN - 140) });
const drainRun = showRun(drain, SHOW_OPEN - 95, { phase: "TRIAGE", status: "RUNNING" });
const dispatcherOld = diffFile("internal/webhooks/dispatcher.go", [
  hunk(58, 58, "func (d *Dispatcher) Run(ctx context.Context) error {", `
 // Run starts the delivery workers and blocks until ctx is cancelled.
 func (d *Dispatcher) Run(ctx context.Context) error {
-\tgo d.loop(ctx)
-\treturn nil
+\tfor i := 0; i < d.workers; i++ {
+\t\tgo func() {
+\t\t\tfor job := range d.jobs {
+\t\t\t\td.deliver(ctx, job)
+\t\t\t}
+\t\t}()
+\t}
+\t<-ctx.Done()
+\treturn ctx.Err()
 }
`),
]);
const dispatcherNew = diffFile("internal/webhooks/dispatcher.go", [
  hunk(58, 58, "func (d *Dispatcher) Run(ctx context.Context) error {", `
 // Run starts the delivery workers and blocks until ctx is cancelled.
 func (d *Dispatcher) Run(ctx context.Context) error {
-\tgo d.loop(ctx)
-\treturn nil
+\tvar wg sync.WaitGroup
+\tfor i := 0; i < d.workers; i++ {
+\t\twg.Add(1)
+\t\tgo func() {
+\t\t\tdefer wg.Done()
+\t\t\tfor job := range d.jobs {
+\t\t\t\td.deliver(context.WithoutCancel(ctx), job)
+\t\t\t}
+\t\t}()
+\t}
+\t<-ctx.Done()
+\tclose(d.jobs)
+\twg.Wait()
+\treturn ctx.Err()
 }
`),
]);
const unDrained = showDetail(
  drainOldRun,
  SHOW_OPEN - 1290,
  {
    title: "Run returns before in-flight deliveries finish",
    category: "concurrency",
    severity: "high",
    confidence: 0.86,
    file: dispatcherOld.path,
    line: lineOf(dispatcherOld, "return ctx.Err()"),
    explanation:
      "Run returns as soon as ctx is cancelled, without closing d.jobs or waiting for the workers. cmd/worker exits when Run returns, so a delivery in flight at SIGTERM is cut off mid-request. It is never marked delivered, so the receiver gets it again after the restart. The workers also pass the cancelled ctx to deliver, which aborts the HTTP call immediately.",
    reasoningSummary:
      "Nothing in Run joins the goroutines it starts. cmd/worker/main.go calls os.Exit(0) right after dispatcher.Run returns. deliver() marks a job done only after the receiver answers 2xx.",
    suggestedFix:
      "Track the workers with a sync.WaitGroup, close d.jobs once ctx is done, and wait before returning. Give in-flight deliveries a context that survives the cancellation (context.WithoutCancel plus a timeout) so they can finish.",
    evidence: [
      cite(dispatcherOld, drainOld.headSha, "func (d *Dispatcher) Run(ctx context.Context) error {", "return ctx.Err()"),
      { kind: "tool_summary", text: "read_file cmd/worker/main.go:40-58: os.Exit(0) runs immediately after dispatcher.Run returns" },
    ],
  },
  { stale: true },
);

// 5. acme/api#345: FAILED before analysis.
const ledger = showPr("acme/api", 345, "Partition ledger_entries by month", "abhatia");
const ledgerRun = showRun(ledger, SHOW_OPEN - 610, {
  phase: "FAILED",
  status: "FAILED",
  completedAt: at(SHOW_OPEN - 588),
  failureReason:
    "Diff too large to review: 212 files and 18,406 changed lines exceed the 8,000-line budget. Most of it is migrations/0042_partition_ledger.sql. Exclude generated files or split the PR.",
});

const showAccounts = [
  account("github.com", "dperalta", { ghActive: true }),
  account("github.com", "acme-review-bot", { selected: true, scopes: ["read:org", "repo"] }),
];

const showDetails = [refundAuthz, oversell, historyN1, pagerOffByOne, openRedirect, sessionFixation, unDrained];
const showDiffs: [string, Pr, Run, DiffFile[]][] = [
  ["shop-1287", orders, ordersRun, [ordersRoute, reserve, history]],
  ["api-341", requestIds, requestIdsRun, [apiMain, requestIdGo]],
  ["shop-1291", signIn, signInRun, [signInTsx, callbackTs, loginTs]],
  ["api-338-old", drainOld, drainOldRun, [dispatcherOld]],
  ["api-338", drain, drainRun, [dispatcherNew]],
];
// What the journal recorded before the session opens, one timeline page per snapshot PR. Sequences run up to the
// snapshot's 40, in time order across PRs, so Timeline has a story for every PR before the script plays anything.
type Recorded = [t: number, make: (s: Seq) => ServerMessage];
const started = (r: Run): Run => ({ ...r, phase: "COLLECT_CONTEXT", status: "RUNNING", completedAt: undefined });
const fullRun = (r: Run, t: number, tools: [number, string, string][], findings: [number, FindingDetail][]): Recorded[] => {
  const end = Date.parse(r.completedAt!) / 1000 - T0 / 1000;
  const steps: [Run["phase"], Run["phase"], string, number][] = [
    ["COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED", t + 20],
    ["TRIAGE", "ANALYZE", "ROUTING_PERSISTED", t + 45],
    ["ANALYZE", "VERIFY", "CANDIDATES_PERSISTED", (findings[0]?.[0] ?? end - 40) - 10],
    ["VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED", end - 10],
    ["PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED", end - 6],
    ["PUBLISH", "COMPLETED", "NO_PUBLICATION_RECORDED", end - 2],
  ];
  return [
    [t, (s) => runEvent(s, "run.started", started(r))],
    ...steps.map(([from, to, event, at]): Recorded => [at, (s) => phase(s, r, from, to, event)]),
    ...tools.map(([at, tool, summary]): Recorded => [at, (s) => activity(s, r, tool, summary)]),
    ...findings.map(([at, d]): Recorded => [at, (s) => findingUpdated(s, { ...findingOf(d), stale: false })]),
    [end, (s) => runEvent(s, "review.completed", r)],
  ];
};
const prOpened = (p: Pr, t: number): Recorded => [t, (s) => ev(s, "pr.updated", p, { prId: p.id })];
const showHistory: [string, Recorded[]][] = [
  [
    "shop-1287",
    [
      prOpened(orders, SHOW_OPEN - 2490),
      ...fullRun(
        ordersRun,
        SHOW_OPEN - 2460,
        [[SHOW_OPEN - 2450, "read_diff", "3 files changed (+41 -6): src/server/routes/orders.ts, src/server/orders/reserve.ts, src/server/orders/history.ts"]],
        [[ordersSeen, refundAuthz], [ordersSeen + 6, oversell], [ordersSeen + 11, historyN1], [ordersSeen + 14, pagerOffByOne]],
      ),
    ],
  ],
  ["api-341", [prOpened(requestIds, SHOW_OPEN - 5340), ...fullRun(requestIdsRun, SHOW_OPEN - 5320, [], [])]],
  ["shop-1291", [prOpened(signIn, SHOW_OPEN - 30)]],
  [
    "api-338",
    [
      prOpened(drainOld, SHOW_OPEN - 1540),
      [SHOW_OPEN - 1520, (s) => runEvent(s, "run.started", started(drainOldRun))],
      [SHOW_OPEN - 1500, (s) => phase(s, drainOldRun, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
      [SHOW_OPEN - 1450, (s) => phase(s, drainOldRun, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
      [SHOW_OPEN - 1300, (s) => phase(s, drainOldRun, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
      [SHOW_OPEN - 1290, (s) => findingUpdated(s, { ...findingOf(unDrained), stale: false })],
      [SHOW_OPEN - 142, (s) => ev(s, "pr.updated", drain, { prId: drain.id })],
      [SHOW_OPEN - 140, (s) => phase(s, drainOldRun, "VERIFY", "SUPERSEDED", "NEWER_HEAD")],
      [SHOW_OPEN - 140, (s) => runEvent(s, "run.superseded", drainOldRun)],
      [SHOW_OPEN - 95, (s) => runEvent(s, "run.started", started(drainRun))],
      [SHOW_OPEN - 80, (s) => phase(s, drainRun, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
    ],
  ],
  [
    "api-345",
    [
      prOpened(ledger, SHOW_OPEN - 620),
      [SHOW_OPEN - 610, (s) => runEvent(s, "run.started", started(ledgerRun))],
      [SHOW_OPEN - 588, (s) => phase(s, ledgerRun, "COLLECT_CONTEXT", "FAILED", "TERMINAL_ERROR")],
      [SHOW_OPEN - 588, (s) => runEvent(s, "run.failed", ledgerRun)],
    ],
  ],
];
const showTimelines = (): [string, ServerMessage][] => {
  const all = showHistory.flatMap(([name, rows]) => rows.map(([t, make], k) => ({ name, t, k, make })));
  all.sort((a, b) => a.t - b.t || a.k - b.k);
  if (all.length > 40) throw new Error(`showcase history has ${all.length} events; the snapshot is at sequence 40`);
  const events = all.map((e, i) => ({ name: e.name, message: e.make({ seq: 40 - all.length + i + 1, t: e.t }) }));
  return showHistory.map(([name]): [string, ServerMessage] => [
    `server.timeline.page.showcase-${name}`,
    { type: "timeline.page", requestId: "req-tl", events: events.filter((e) => e.name === name).map((e) => e.message).reverse() as Sequenced[] },
  ]);
};

// What approving #1291 posts: the publisher's summary (src/cli/report.ts renderSummary) and one comment per finding.
const signInPreviewId = sha256(`payload of ${signInRun.id}`);
const signInPreview: ServerMessage = {
  type: "publication.preview",
  requestId: "req-pub",
  runId: signInRun.id,
  previewId: signInPreviewId,
  headSha: signIn.headSha,
  body: [
    "## Automated PR review",
    "",
    `Reviewed commit: \`${signIn.headSha}\``,
    "",
    "Found 2 actionable issues.",
    "",
    ...[openRedirect, sessionFixation].map((f) => `- ${f.severity[0].toUpperCase()}${f.severity.slice(1)}: ${f.title} (\`${f.file}:${f.line}\`)`),
    "",
    "Review performed with isolated read-only tools. Human review is still required.",
    `<!-- durable-review:${signInRun.id}:${signInPreviewId} -->`,
    "",
  ].join("\n"),
  comments: [openRedirect, sessionFixation].map((f) => ({
    path: f.file,
    line: f.line!,
    side: "RIGHT" as const,
    body: `**${f.severity[0].toUpperCase()}${f.severity.slice(1)}: ${f.title}**\n\n${f.explanation}`,
  })),
  event: "COMMENT",
  status: "awaiting_approval",
};

const showcaseFixtures: Record<string, ServerMessage> = Object.fromEntries([
  ...showTimelines(),
  ...showDiffs.map(([name, p, r, files]): [string, ServerMessage] => [
    `server.diff.page.showcase-${name}`,
    { type: "diff.page", requestId: "req-diff", prId: p.id, baseSha: r.baseSha, headSha: r.headSha, files, truncated: false },
  ]),
  ...showDetails.map((d, i): [string, ServerMessage] => [
    `server.finding.detail.showcase-${i + 1}`,
    { type: "finding.detail", requestId: "req-finding", finding: d },
  ]),
  ["server.publication.preview.showcase-shop-1291", signInPreview],
]);

const signInActivity = (tool: string, summary: string) => (s: Seq) => activity(s, signInRun, tool, summary);
const drainActivity = (tool: string, summary: string) => (s: Seq) => activity(s, drainRun, tool, summary);
// The step after `after` waits until the client sends that command; playback accepts it (protocol/README.md).
type ShowStep = [delayMs: number, make: ((s: Seq) => ServerMessage) | "heartbeat", after?: "review.publish"];
const signInPublication = (status: PublicationChange["status"], reviewId?: number) => (s: Seq) =>
  ev(
    s,
    "review.publication_changed",
    { runId: signInRun.id, status, ...(reviewId ? { reviewId, url: `https://github.com/${signIn.repository}/pull/${signIn.number}#pullrequestreview-${reviewId}` } : {}) },
    { prId: signIn.id, runId: signInRun.id },
  );
const SIGNIN_REVIEW_ID = 2209184;
const showSteps: ShowStep[] = [
  [600, (s) => runEvent(s, "run.started", signInRun)],
  [420, signInActivity("read_diff", "3 files changed (+6 -3): src/server/auth/callback.ts, src/server/auth/login.ts, src/client/pages/SignIn.tsx")],
  [510, signInActivity("git_history", "src/server/auth/: 14 commits in 90 days; last change #1240 \"Move sessions to Redis\" (mkowalski)")],
  [900, (s) => phase(s, signInRun, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
  [340, signInActivity("read_file", "src/server/auth/callback.ts:1-38")],
  [290, signInActivity("search_code", "\"returnTo\": 6 matches in 3 files")],
  [450, (s) => phase(s, drainRun, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
  [800, (s) => phase(s, signInRun, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
  [310, signInActivity("read_file", "src/server/auth/login.ts:1-24")],
  [180, drainActivity("read_diff", "1 file changed (+15 -2): internal/webhooks/dispatcher.go")],
  [260, signInActivity("read_file", "src/client/pages/SignIn.tsx:1-58")],
  [420, signInActivity("search_code", "\"res.redirect(\": 11 matches in 7 files")],
  [380, signInActivity("read_file", "src/server/http/safeRedirect.ts:1-24")],
  [230, drainActivity("read_file", "internal/webhooks/dispatcher.go:1-112")],
  [550, signInActivity("git_history", "src/server/http/safeRedirect.ts: added in #1102 \"Block open redirects on /logout\" (tnakamura)")],
  [300, signInActivity("search_code", "\"safeRedirect(\": 2 matches, src/server/routes/logout.ts:14, src/server/routes/account.ts:52")],
  [470, "heartbeat"],
  [160, signInActivity("read_file", "src/server/routes/logout.ts:1-30")],
  [480, signInActivity("search_code", "\"regenerateSession(\": 1 definition (src/server/session.ts:52), 0 callers")],
  [350, signInActivity("git_history", "src/server/auth/callback.ts: regenerateSession(req) added in #1077 \"Rotate session id on sign-in\"")],
  [210, drainActivity("search_code", "\"dispatcher.Run(\": 1 match, cmd/worker/main.go:44")],
  [390, signInActivity("read_file", "src/server/session.ts:40-64")],
  [270, signInActivity("search_code", "\"req.session.userId =\": 3 matches in auth/callback.ts, auth/dev.ts, tests/helpers/login.ts")],
  [600, signInActivity("read_file", "tests/auth/callback.test.ts:1-80")],
  [330, signInActivity("search_code", "\"returnTo\" in tests/: 1 match, callback.test.ts:44 (relative path only)")],
  [240, drainActivity("read_file", "cmd/worker/main.go:30-60")],
  [410, signInActivity("read_file", "src/server/app.ts:1-40")],
  [450, signInActivity("read_file", "src/server/http/auth.ts:1-46")],
  [380, signInActivity("search_code", "\"returnTo\" in src/client/: 2 matches, pages/SignIn.tsx:32, components/Header.tsx:18")],
  [190, signInActivity("search_code", "\"cookieParser(\": src/server/app.ts:22, no signing secret")],
  [900, (s) => phase(s, signInRun, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
  [360, signInActivity("read_file", "src/server/auth/callback.ts:12-22")],
  [290, signInActivity("read_file", "src/server/auth/login.ts:6-11")],
  [440, signInActivity("search_code", "\"isSafeReturnPath|safeRedirect\" on the sign-in path: 0 matches")],
  [380, (s) => findingUpdated(s, findingOf(openRedirect))],
  [580, signInActivity("git_history", "#1077 review thread: rotation required by the 2025 pen test (finding AC-14)")],
  [620, (s) => findingUpdated(s, findingOf(sessionFixation))],
  [1100, (s) => phase(s, signInRun, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED")],
  // Approval mode: the review is stored and parks until the reader approves the preview.
  [800, (s) => phase(s, signInRun, "PREPARE_PUBLICATION", "AWAITING_APPROVAL", "APPROVAL_REQUIRED")],
  [40, signInPublication("awaiting_approval")],
  [4000, "heartbeat"],
  [600, (s) => phase(s, signInRun, "AWAITING_APPROVAL", "PUBLISH", "APPROVED"), "review.publish"],
  [300, signInPublication("publishing")],
  [1200, signInPublication("published", SIGNIN_REVIEW_ID)],
  [80, (s) => findingUpdated(s, { ...findingOf(openRedirect), status: "PUBLISHED" })],
  [60, (s) => findingUpdated(s, { ...findingOf(sessionFixation), status: "PUBLISHED" })],
  [400, (s) => phase(s, signInRun, "PUBLISH", "COMPLETED", "REVIEW_RECONCILED")],
  [40, (s) => runEvent(s, "review.completed", { ...signInRun, phase: "COMPLETED", status: "COMPLETED", completedAt: at(s.t!) })],
  [4000, "heartbeat"],
  [4000, "heartbeat"],
];


// ---------- message fixtures ----------

const diffFiles: DiffFile[] = [
  {
    path: "src/booking.ts",
    status: "modified",
    additions: 2,
    deletions: 1,
    hunks: [
      {
        header: "@@ -40,3 +40,4 @@ export function book()",
        oldStart: 40,
        newStart: 40,
        lines: [
          { kind: "context", old: 40, new: 40, text: "  const slot = await lock(id);" },
          { kind: "del", old: 41, text: "  if (slot.free) {" },
          { kind: "add", new: 41, text: "  if (slot.free && !slot.held) {" },
          { kind: "add", new: 42, text: "    slot.held = true;" },
          { kind: "context", old: 42, new: 43, text: "  }" },
        ],
      },
    ],
    truncated: false,
  },
  { path: "assets/logo.png", status: "binary", additions: 0, deletions: 0, hunks: [], truncated: false },
  { path: "src/slots.ts", oldPath: "src/slot.ts", status: "renamed", additions: 0, deletions: 0, hunks: [], truncated: false },
  {
    path: "src/generated/schema.ts",
    status: "added",
    additions: 4000,
    deletions: 0,
    hunks: [{ header: "@@ -0,0 +1,4000 @@", oldStart: 0, newStart: 1, lines: [{ kind: "add", new: 1, text: "// generated" }] }],
    truncated: true,
  },
];

const detail: FindingDetail = {
  ...f1,
  explanation: "Two concurrent bookings can both observe `slot.free` before either sets `held`.",
  reasoningSummary: "Traced book() -> lock(); lock() is advisory and not awaited by the second caller.",
  suggestedFix: "Take the row lock with SELECT ... FOR UPDATE before checking `free`.",
  evidence: [
    { kind: "code_ref", path: "src/booking.ts", start: 40, end: 43, refSha: pr1.headSha, text: "if (slot.free && !slot.held) {" },
    { kind: "tool_summary", text: "grep lock( src/ -> 3 matches" },
    { kind: "test_result", text: "booking.race.test.ts: 1 failed" },
    { kind: "scenario", text: "Two clients POST /book for slot 9 within 5 ms." },
    { kind: "link", text: "https://example.com/docs/locking" },
  ],
  history: [{ runId: run1.id, disposition: "DETECTED", at: at(30) }],
};

const hostileTitle = "\u001b[31mred\u001b[0m \u001b]8;;https://evil.example\u0007click\u001b]8;;\u0007 \u009b2J\u0085done";

export const fixtures: Record<string, ClientMessage | ServerMessage | Record<string, unknown>> = {
  "client.hello": { type: "hello", requestId: "req-hello", protocolVersion: 1, client: "ratatui", capabilities: ["snapshots", "events"] },
  "client.hello.future-version": { type: "hello", requestId: "req-hello", protocolVersion: 2, client: "ratatui", capabilities: [] },
  "client.snapshot.get": { type: "snapshot.get", requestId: "req-snapshot" },
  "client.resume": { type: "resume", requestId: "req-resume", streamEpoch: EPOCH, afterSequence: 108 },
  "client.diff.get": {
    type: "diff.get",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    file: "src/booking.ts",
    cursor: "c-2",
    limit: 100,
  },
  "client.diff.get.minimal": { type: "diff.get", requestId: "req-diff", prId: pr1.id, baseSha: pr1.baseSha, headSha: pr1.headSha },
  "client.finding.get": { type: "finding.get", requestId: "req-finding", findingId: "f-1" },
  "client.timeline.get": { type: "timeline.get", requestId: "req-tl", prId: pr1.id, runId: run1.id, findingId: "f-1", beforeSequence: 100, limit: 50 },
  "client.timeline.get.minimal": { type: "timeline.get", requestId: "req-tl", limit: 50 },
  "client.command.review.start": {
    type: "command",
    requestId: "req-cmd",
    command: "review.start",
    params: { prId: pr1.id, expectedHeadSha: pr1.headSha },
    idempotencyKey: `review.start:${pr1.id}:${pr1.headSha}`,
  },
  "client.command.review.cancel": { type: "command", requestId: "req-cmd", command: "review.cancel", params: { runId: run1.id }, idempotencyKey: `review.cancel:${run1.id}` },
  "client.command.review.retry": { type: "command", requestId: "req-cmd", command: "review.retry", params: { runId: run1.id }, idempotencyKey: `review.retry:${run1.id}` },
  "client.command.review.publish": publishCommand,
  "client.command.account-select": selectCommand(work),
  "client.publication.get": { type: "publication.get", requestId: "req-pub", runId: run1.id },

  "server.welcome": welcome(108),
  "server.welcome.engine": welcome(108, EPOCH, ["account.select", ...REVIEW_COMMANDS], "engine"),
  "server.welcome.read-only": welcome(108, EPOCH, ["account.select"], "read-only", "another pr-review is running on /srv/pr-review/.data"),
  "server.publication.preview": preview,
  "server.snapshot": snapshot(108, [pr1, pr2, pr3], [run1, run2], [f1, f2]),
  "server.snapshot.accounts": snapshot(108, [pr1], [run1], [f1], EPOCH, [
    account("env", "GITHUB_TOKEN", { scopes: [] }),
    personal,
    { ...work, selected: true },
    expired,
  ]),
  "server.snapshot.empty": { type: "snapshot", streamEpoch: EPOCH, sequence: 0, prs: [], runs: [], findings: [] },
  "server.snapshot.hostile": snapshot(5, [pr(1, { title: hostileTitle })], [], [finding("f-h", run1, { title: "safe\rEVIL\u001bc" })]),
  "server.event.pr.updated": ev({ seq: 109 }, "pr.updated", pr1, { prId: pr1.id }),
  "server.event.run.started": runEvent({ seq: 109 }, "run.started", run(run1.id, pr1, { phase: "COLLECT_CONTEXT" })),
  "server.event.review.phase_changed": phase({ seq: 109 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  "server.event.finding.updated": findingUpdated({ seq: 109 }, f1),
  "server.event.tool.activity": activity({ seq: 109 }, run1, "read_file", "src/booking.ts:1-120"),
  "server.event.review.completed": runEvent({ seq: 109 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(109) }),
  "server.event.run.superseded": runEvent({ seq: 109 }, "run.superseded", { ...run1, phase: "SUPERSEDED", status: "SUPERSEDED", completedAt: at(109) }),
  "server.event.run.failed": runEvent({ seq: 109 }, "run.failed", {
    ...run1,
    phase: "FAILED",
    status: "FAILED",
    completedAt: at(109),
    failureReason: "aborted by operator",
  }),
  "server.event.accounts-updated": accountsUpdated({ seq: 109 }, [personal, { ...work, selected: true }]),
  "server.event.review.publication_changed": publicationChanged({ seq: 109 }, run1, "awaiting_approval"),
  "server.event.review.publication_changed.published": publicationChanged({ seq: 109 }, run1, "published", 3141),
  "server.event.heartbeat": { type: "event", eventType: "heartbeat", streamEpoch: EPOCH, occurredAt: at(110) },
  // A reserved name (TUI_SPEC §3B's reinvestigation) stands in for whatever a newer server may emit.
  "server.event.unknown-type": {
    type: "event",
    streamEpoch: EPOCH,
    sequence: 109,
    prevSequence: 108,
    occurredAt: at(109),
    prId: pr1.id,
    eventType: "review.reinvestigation_requested",
    data: { state: "PENDING", whatever: [1, 2] },
  },
  "server.event.extra-fields": {
    ...(findingUpdated({ seq: 109 }, { ...f1, futureField: { nested: true } } as Finding) as object),
    traceId: "abc",
  },
  "server.resync.required": { type: "resync.required", requestId: "req-resume", reason: "epoch epoch-0 is no longer available" },
  "server.resync.required.unsolicited": { type: "resync.required", reason: "client queue overflow" },
  "server.diff.page": {
    type: "diff.page",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    files: diffFiles,
    nextCursor: "c-2",
    truncated: true,
  },
  "server.diff.page.last": {
    type: "diff.page",
    requestId: "req-diff",
    prId: pr1.id,
    baseSha: pr1.baseSha,
    headSha: pr1.headSha,
    files: [],
    truncated: false,
  },
  "server.finding.detail": { type: "finding.detail", requestId: "req-finding", finding: detail },
  "server.finding.detail.minimal": {
    type: "finding.detail",
    requestId: "req-finding",
    finding: { ...f2, explanation: "e", reasoningSummary: "r", evidence: [], history: [] },
  },
  "server.timeline.page": {
    type: "timeline.page",
    requestId: "req-tl",
    events: [
      phase({ seq: 100 }, run1, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED"),
      activity({ seq: 99 }, run1, "search", "lock("),
    ] as Sequenced[],
    nextBeforeSequence: 99,
  },
  "server.timeline.page.end": { type: "timeline.page", requestId: "req-tl", events: [] },
  "server.command.result.accepted": { type: "command.result", requestId: "req-cmd", status: "accepted", operationId: "op-1" },
  "server.command.result.rejected": {
    type: "command.result",
    requestId: "req-cmd",
    status: "rejected",
    code: "CAPABILITY_DENIED",
    message: "review.start is not available in this version",
  },
  "server.error": { type: "error", requestId: "req-finding", code: "NOT_FOUND", message: "no finding f-9" },
  "server.error.protocol-mismatch": { type: "error", requestId: "req-hello", code: "PROTOCOL_MISMATCH", message: "unsupported protocolVersion 2", supported: [1] },
  "server.error.no-request-id": { type: "error", code: "INVALID_REQUEST", message: "line is not valid JSON" },
  ...showcaseFixtures,
};

// ---------- negative fixtures: name -> [message, why] ----------

export const invalidFixtures: Record<string, [unknown, string]> = {
  "client.command.unknown-param": [
    { ...(fixtures["client.command.review.start"] as object), params: { prId: pr1.id, expectedHeadSha: pr1.headSha, force: true } },
    "Command params are closed: an unknown param is rejected rather than ignored.",
  ],
  "client.command.extra-field": [
    { ...(fixtures["client.command.review.cancel"] as object), actor: "admin" },
    "Command messages are closed: identity comes from the connection, never from a caller-chosen field.",
  ],
  "client.command.unknown-command": [
    { type: "command", requestId: "r", command: "review.delete", params: {}, idempotencyKey: "k" },
    "Only the four reserved command names exist.",
  ],
  "client.hello.no-request-id": [{ type: "hello", protocolVersion: 1, client: "ratatui", capabilities: [] }, "Every request carries a requestId."],
  "client.diff.get.limit-zero": [{ ...(fixtures["client.diff.get.minimal"] as object), limit: 0 }, "limit is 1..500."],
  "client.unknown-type": [{ type: "subscribe", requestId: "r" }, "Unknown client message types are invalid requests."],
  "client.command.account-select.no-account-id": [
    { ...selectCommand(work), params: {} },
    "account.select names the account to use.",
  ],
  "server.snapshot.account-bad-shape": [
    { ...(fixtures["server.snapshot.accounts"] as object), accounts: [{ ...personal, state: "success", scopes: "gist, repo" }] },
    "Account state is ok or error and scopes is a list; gh's raw words and comma-joined scope string are mapped by the server.",
  ],
  "server.welcome.version-2": [{ ...(fixtures["server.welcome"] as object), protocolVersion: 2 }, "A v1 welcome always says protocolVersion 1."],
  "server.event.missing-prev-sequence": [
    (({ prevSequence: _, ...rest }) => rest)(fixtures["server.event.tool.activity"] as Record<string, unknown>),
    "Sequenced events carry prevSequence; the client detects gaps with it.",
  ],
  "server.event.sequence-zero": [{ ...(fixtures["server.event.tool.activity"] as object), sequence: 0 }, "Journal sequences start at 1."],
  "server.event.finding.bad-severity": [
    { ...(fixtures["server.event.finding.updated"] as object), data: { ...f1, severity: "blocker" } },
    "A known eventType with malformed data does not fall through to the unknown-event shape.",
  ],
  "server.event.phase.cancelled": [
    { ...(fixtures["server.event.review.phase_changed"] as object), data: { from: "VERIFY", to: "CANCELLED", event: "X", detail: null } },
    "Phases are the engine's states; CANCELLED does not exist (an abort is FAILED with a reason).",
  ],
  "server.event.tool.activity.long-summary": [
    { ...(fixtures["server.event.tool.activity"] as object), data: { tool: "read_file", summary: "x".repeat(201) } },
    "tool.activity summaries are capped at 200 chars.",
  ],
  "server.event.heartbeat.no-epoch": [{ type: "event", eventType: "heartbeat", occurredAt: at(1) }, "Heartbeats still name the stream epoch."],
  "server.command.result.accepted-no-operation": [
    { type: "command.result", requestId: "r", status: "accepted" },
    "An accepted command names the operation that later events confirm.",
  ],
  "server.event.publication.bad-status": [
    { ...(fixtures["server.event.review.publication_changed"] as object), data: { runId: run1.id, status: "posted" } },
    "Publication statuses are a closed list: awaiting_approval, publishing, published, uncertain, failed.",
  ],
  "server.error.unknown-code": [{ type: "error", code: "TEAPOT", message: "m" }, "Error codes are a closed list."],
  "server.finding.line-zero": [
    { ...(fixtures["server.finding.detail"] as Record<string, unknown>), finding: { ...detail, line: 0 } },
    "Lines are 1-based; a file-level finding uses null.",
  ],
};

// ---------- playback scripts ----------

/** `after`: the line waits until the client sends that command, then its delay runs (protocol/README.md). */
export type PlaybackLine = { delayMs: number; after?: string; message: ServerMessage };
export type Expect = {
  streamEpoch: string;
  lastSequence: number;
  resyncRequested: boolean;
  toolActivityCount: number;
  prs: { id: string; headSha: string; title: string }[];
  runs: { id: string; phase: string; status: string }[];
  findings: { id: string; title: string; status: string; stale: boolean }[];
  /** Present only when the script's snapshot carries accounts. */
  accounts?: { id: string; state: string; ghActive: boolean; selected: boolean }[];
  /** The latest `review.publication_changed` per run, sorted by `runId`; present only when the script carries one. */
  publications?: { runId: string; status: string }[];
};

const lines = (delayMs: number, messages: ServerMessage[]): PlaybackLine[] => messages.map((message) => ({ delayMs, message }));
const prRows = (...ps: Pr[]) => ps.map((p) => ({ id: p.id, headSha: p.headSha, title: p.title }));
const baseSnapshot = snapshot(10, [pr1, pr2, pr3], [run1, run2], [f2]);

const normal = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "src/booking.ts:1-120"),
  { type: "event", eventType: "heartbeat", streamEpoch: EPOCH, occurredAt: at(11) },
  findingUpdated({ seq: 12 }, f1),
  phase({ seq: 13 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  findingUpdated({ seq: 14 }, { ...f1, confidence: 0.9, evidenceCount: 2 }),
  {
    type: "event",
    streamEpoch: EPOCH,
    sequence: 15,
    prevSequence: 14,
    occurredAt: at(15),
    eventType: "review.reinvestigation_requested",
    data: { reserved: true },
  } as ServerMessage,
  phase({ seq: 16 }, run1, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED"),
  phase({ seq: 17 }, run1, "PUBLISH", "COMPLETED", "NO_PUBLICATION_RECORDED"),
  runEvent({ seq: 18 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(18) }),
]);

const gap = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  phase({ seq: 13, prev: 12 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  activity({ seq: 14 }, run1, "read_file", "after the gap; must not apply"),
]);

const duplicate = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  findingUpdated({ seq: 12 }, f1),
  findingUpdated({ seq: 12 }, f1),
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  phase({ seq: 13 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
]);

const epochChange = lines(40, [
  welcome(10),
  baseSnapshot,
  activity({ seq: 11 }, run1, "read_file", "a.ts"),
  activity({ epoch: "epoch-2", seq: 3 }, run1, "read_file", "new epoch; must not apply"),
  snapshot(3, [pr1, pr2], [run(run1.id, pr1, { phase: "ANALYZE" })], [], "epoch-2"),
  phase({ epoch: "epoch-2", seq: 4 }, run1, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED"),
]);

const pr1New = { ...pr1, headSha: sha("c") };
const run1New = run("rev-184-b", pr1New, { phase: "COLLECT_CONTEXT", startedAt: at(13) });
const superseded = lines(40, [
  welcome(10),
  snapshot(10, [pr1, pr2, pr3], [run1, run2], [f1, f2]),
  ev({ seq: 11 }, "pr.updated", pr1New, { prId: pr1.id }),
  runEvent({ seq: 12 }, "run.superseded", { ...run1, phase: "SUPERSEDED", status: "SUPERSEDED", completedAt: at(12) }),
  findingUpdated({ seq: 13 }, { ...f1, stale: true }),
  runEvent({ seq: 14 }, "run.started", run1New),
]);

const longTitle = "Refactor booking ".repeat(118).slice(0, 2000);
const cjk = "修复竞态条件 🔥 👩‍💻 ｗｉｄｅ";
const hostilePr1 = pr(1, { title: hostileTitle });
const hostilePr2 = pr(2, { title: longTitle });
const hostilePr3 = pr(3, { title: cjk });
const hostileRun = run("rev-1", hostilePr1);
const hostile = lines(40, [
  welcome(1),
  snapshot(1, [hostilePr1, hostilePr2, hostilePr3], [hostileRun], []),
  activity({ seq: 2 }, hostileRun, "read_file", "\u001b]52;c;ZXZpbA==\u0007src/a.ts \u001b[2K\u001b[1A"),
  findingUpdated({ seq: 3 }, finding("f-h", hostileRun, { title: "safe\rEVIL\u001bc \u0090q\u009c tail" })),
  ev({ seq: 4 }, "pr.updated", { ...hostilePr3, title: `${cjk}\u001b[5m!` }, { prId: hostilePr3.id }),
]);

const burst: PlaybackLine[] = [
  ...lines(0, [welcome(10), baseSnapshot]),
  ...Array.from({ length: 5000 }, (_, i) => ({
    delayMs: 0,
    message: activity({ seq: 11 + i, t: 11 }, run1, "read_file", `src/file-${i}.ts`),
  })),
];

// The client shows a selection only once accounts.updated confirms it, never on command.result accepted.
const accounts = lines(40, [
  welcome(10, EPOCH, ["account.select"]),
  snapshot(10, [pr1, pr2, pr3], [run1, run2], [f2], EPOCH, [personal, work]),
  { type: "command.result", requestId: "req-cmd", status: "accepted", operationId: "account.select:11" },
  accountsUpdated({ seq: 11 }, [personal, { ...work, selected: true }]),
]);


// Approval mode end to end: the run parks, the preview is shown, the approval is accepted, then the publisher's
// own statuses arrive. The client never shows a publication as done on `accepted`, only on `published`.
const approval = lines(40, [
  welcome(10, EPOCH, ["account.select", ...REVIEW_COMMANDS], "engine"),
  snapshot(10, [pr1, pr2, pr3], [run1, run2], [f1, f2], EPOCH, [personal, work]),
  phase({ seq: 11 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  phase({ seq: 12 }, run1, "PREPARE_PUBLICATION", "AWAITING_APPROVAL", "APPROVAL_REQUIRED"),
  publicationChanged({ seq: 13 }, run1, "awaiting_approval"),
  preview,
  { type: "command.result", requestId: "req-cmd", status: "accepted", operationId: `review.publish:${run1.id}` },
  phase({ seq: 14 }, run1, "AWAITING_APPROVAL", "PUBLISH", "APPROVED"),
  publicationChanged({ seq: 15 }, run1, "publishing"),
  publicationChanged({ seq: 16 }, run1, "published", 3141),
  phase({ seq: 17 }, run1, "PUBLISH", "COMPLETED", "REVIEW_RECONCILED"),
  runEvent({ seq: 18 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(18) }),
]);

// Each step's occurredAt is the script clock, so the timestamps match the cadence the TUI plays at.
const showcaseScript = (): PlaybackLine[] => {
  let [seq, clock] = [40, SHOW_OPEN];
  const steps = showSteps.map(([delayMs, make, after]): PlaybackLine => {
    clock += delayMs / 1000;
    if (make === "heartbeat") return { delayMs, message: { type: "event", eventType: "heartbeat", streamEpoch: EPOCH, occurredAt: at(clock) } };
    seq += 1;
    return { delayMs, ...(after ? { after } : {}), message: make({ seq, t: clock }) };
  });
  return [
    { delayMs: 0, message: welcome(40, EPOCH, ["account.select", ...REVIEW_COMMANDS], "engine") },
    {
      delayMs: 0,
      message: snapshot(
        40,
        [orders, requestIds, signIn, drain, ledger],
        [ordersRun, requestIdsRun, drainOldRun, drainRun, ledgerRun],
        [refundAuthz, oversell, historyN1, pagerOffByOne, unDrained].map(findingOf),
        EPOCH,
        showAccounts,
      ),
    },
    ...steps,
  ];
};

const runRow = (r: Run, phase = r.phase, status = r.status) => ({ id: r.id, phase, status });
const baseExpect = (over: Partial<Expect>): Expect => ({
  streamEpoch: EPOCH,
  lastSequence: 10,
  resyncRequested: false,
  toolActivityCount: 0,
  prs: prRows(pr1, pr2, pr3),
  runs: [runRow(run1), runRow(run2)],
  findings: [{ id: f2.id, title: f2.title, status: f2.status, stale: false }],
  ...over,
});

export const playback: Record<string, { script: PlaybackLine[]; expect: Expect }> = {
  normal: {
    script: normal,
    expect: baseExpect({
      lastSequence: 18,
      toolActivityCount: 1,
      runs: [runRow(run1, "COMPLETED", "COMPLETED"), runRow(run2)],
      findings: [
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
        { id: f1.id, title: f1.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  gap: { script: gap, expect: baseExpect({ lastSequence: 11, resyncRequested: true, toolActivityCount: 1 }) },
  duplicate: {
    script: duplicate,
    expect: baseExpect({
      lastSequence: 13,
      toolActivityCount: 1,
      runs: [runRow(run1, "PREPARE_PUBLICATION"), runRow(run2)],
      findings: [
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
        { id: f1.id, title: f1.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  "epoch-change": {
    script: epochChange,
    expect: {
      streamEpoch: "epoch-2",
      lastSequence: 4,
      resyncRequested: true,
      toolActivityCount: 1,
      prs: prRows(pr1, pr2),
      runs: [runRow(run1, "VERIFY", "RUNNING")],
      findings: [],
    },
  },
  superseded: {
    script: superseded,
    expect: baseExpect({
      lastSequence: 14,
      prs: prRows(pr1New, pr2, pr3),
      runs: [runRow(run1, "SUPERSEDED", "SUPERSEDED"), runRow(run2), runRow(run1New)],
      findings: [
        { id: f1.id, title: f1.title, status: "DETECTED", stale: true },
        { id: f2.id, title: f2.title, status: "DETECTED", stale: false },
      ],
    }),
  },
  hostile: {
    script: hostile,
    expect: {
      streamEpoch: EPOCH,
      lastSequence: 4,
      resyncRequested: false,
      toolActivityCount: 1,
      prs: [
        { id: hostilePr1.id, headSha: hostilePr1.headSha, title: "red click done" },
        { id: hostilePr2.id, headSha: hostilePr2.headSha, title: longTitle },
        { id: hostilePr3.id, headSha: hostilePr3.headSha, title: `${cjk}!` },
      ],
      runs: [runRow(hostileRun)],
      findings: [{ id: "f-h", title: "safeEVIL  tail", status: "DETECTED", stale: false }],
    },
  },
  accounts: {
    script: accounts,
    expect: baseExpect({
      lastSequence: 11,
      accounts: [
        { id: personal.id, state: "ok", ghActive: true, selected: false },
        { id: work.id, state: "ok", ghActive: false, selected: true },
      ],
    }),
  },
  showcase: {
    script: showcaseScript(),
    expect: {
      streamEpoch: EPOCH,
      lastSequence: 86,
      resyncRequested: false,
      toolActivityCount: 29,
      prs: prRows(orders, requestIds, signIn, drain, ledger),
      runs: [
        runRow(ordersRun),
        runRow(requestIdsRun),
        runRow(signInRun, "COMPLETED", "COMPLETED"),
        runRow(drainOldRun),
        runRow(drainRun, "ANALYZE", "RUNNING"),
        runRow(ledgerRun),
      ],
      findings: showDetails.map((f) => ({ id: f.id, title: f.title, status: f.runId === signInRun.id ? "PUBLISHED" : "DETECTED", stale: f.stale })),
      accounts: [
        { id: "github.com/acme-review-bot", state: "ok", ghActive: false, selected: true },
        { id: "github.com/dperalta", state: "ok", ghActive: true, selected: false },
      ],
      publications: [{ runId: signInRun.id, status: "published" }],
    },
  },
  approval: {
    script: approval,
    expect: baseExpect({
      lastSequence: 18,
      runs: [runRow(run1, "COMPLETED", "COMPLETED"), runRow(run2)],
      findings: [
        { id: f1.id, title: f1.title, status: f1.status, stale: false },
        { id: f2.id, title: f2.title, status: f2.status, stale: false },
      ],
      accounts: [
        { id: personal.id, state: "ok", ghActive: true, selected: false },
        { id: work.id, state: "ok", ghActive: false, selected: false },
      ],
      publications: [{ runId: run1.id, status: "published" }],
    }),
  },
  burst: { script: burst, expect: baseExpect({ lastSequence: 5010, toolActivityCount: 5000 }) },
};
