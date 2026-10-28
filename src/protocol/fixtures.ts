// Source of protocol/fixtures and protocol/playback. Edit here, then run `pnpm protocol:emit`.
import { sha256, stableId } from "../domain/types.ts";
import { type Account, type ActivitySpan, type ClientMessage, type DiffFile, type Evidence, type Finding, type FindingDetail, type Pr, type PublicationChange, REVIEW_COMMANDS, type Run, type ServerMessage, type SetupCheck } from "./v1.ts";

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
const activity = (s: Seq, r: Run, tool: string, summary: string, span?: ActivitySpan) =>
  ev(s, "tool.activity", span ? { tool, summary, span } : { tool, summary }, { prId: r.prId, runId: r.id });
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
// Later heads of pr1: one run each.
const pr1New = { ...pr1, headSha: sha("c") };
const run1New = run("rev-184-b", pr1New, { phase: "COLLECT_CONTEXT", startedAt: at(13) });
const pr1Third = { ...pr1, headSha: sha("d") };
const run1Third = run("rev-184-c", pr1Third, { phase: "COLLECT_CONTEXT", startedAt: at(130) });

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
}, { unconfirmed: true });
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

// 6. acme/shop#1296: tracked, never reviewed. The showcase's s starts its first review, which runs to COMPLETED.
const giftCards = showPr("acme/shop", 1296, "Gift cards: redeem a balance at checkout", "rsingh");
const giftCardsRun = showRun(giftCards, SHOW_OPEN, { phase: "COLLECT_CONTEXT", status: "RUNNING" });
const redeemTs = diffFile("src/server/checkout/giftCards.ts", [
  hunk(12, 12, "export async function applyGiftCard(db: Db, cartId: string, code: string) {", `
 export async function applyGiftCard(db: Db, cartId: string, code: string) {
   const cart = await db.cart.findUniqueOrThrow({ where: { id: cartId } });
-  throw new NotSupportedError("gift cards");
+  const card = await db.giftCard.findUnique({ where: { code } });
+  if (!card || card.balance <= 0) throw new InvalidGiftCardError(code);
+  const amount = Math.min(card.balance, cart.total);
+  await db.giftCard.update({ where: { code }, data: { balance: card.balance - amount } });
+  return db.cart.update({ where: { id: cartId }, data: { giftCardAmount: amount } });
 }
`),
]);
const doubleSpend = showDetail(giftCardsRun, SHOW_OPEN, {
  title: "A gift card redeemed in two carts at once is spent twice",
  category: "data_integrity",
  severity: "high",
  confidence: 0.83,
  file: redeemTs.path,
  line: lineOf(redeemTs, "data: { balance: card.balance - amount }"),
  explanation:
    "applyGiftCard reads the card's balance, then writes back `card.balance - amount` as an absolute value. Two checkouts using the same code can both read a balance of 50, both apply 50 to their carts, and both write 0. The card pays 100 out of a 50 balance.",
  reasoningSummary:
    "The read and the write are separate statements outside a transaction, and the write is not conditional on the balance it read. Gift card codes are shared by design (gifts), so concurrent use is expected, not an edge case.",
  suggestedFix:
    "Debit with a conditional decrement and check that it applied:\n\nconst debited = await db.giftCard.updateMany({ where: { code, balance: { gte: amount } }, data: { balance: { decrement: amount } } });\nif (debited.count === 0) throw new InvalidGiftCardError(code);",
  evidence: [
    cite(redeemTs, giftCards.headSha, "const card = await db.giftCard.findUnique({ where: { code } });", "await db.giftCard.update({ where: { code }, data: { balance: card.balance - amount } });"),
    { kind: "scenario", text: "Two carts apply code GIFT-7Q2M (balance 50) 3 ms apart. Both read 50, both take 50 off their totals, both write balance 0." },
  ],
});

const showPersonal = account("github.com", "dperalta", { ghActive: true });
const showAccounts = [
  showPersonal,
  account("github.com", "acme-review-bot", { selected: true, scopes: ["read:org", "repo"] }),
];

const showDetails = [refundAuthz, oversell, historyN1, pagerOffByOne, openRedirect, sessionFixation, unDrained, doubleSpend];
const showDiffs: [string, Pr, Run, DiffFile[]][] = [
  ["shop-1287", orders, ordersRun, [ordersRoute, reserve, history]],
  ["api-341", requestIds, requestIdsRun, [apiMain, requestIdGo]],
  ["shop-1291", signIn, signInRun, [signInTsx, callbackTs, loginTs]],
  ["api-338-old", drainOld, drainOldRun, [dispatcherOld]],
  ["api-338", drain, drainRun, [dispatcherNew]],
  ["shop-1296", giftCards, giftCardsRun, [redeemTs]],
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
  ["shop-1296", [prOpened(giftCards, SHOW_OPEN - 260)]],
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
    ...[openRedirect, sessionFixation].map((f) => `- ${f.severity.charAt(0).toUpperCase()}${f.severity.slice(1)}: ${f.title} (\`${f.file}:${f.line}\`)`),
    "",
    "Review performed with isolated read-only tools. Human review is still required.",
    `<!-- durable-review:${signInRun.id}:${signInPreviewId} -->`,
    "",
  ].join("\n"),
  comments: [openRedirect, sessionFixation].map((f) => ({
    path: f.file,
    line: f.line!,
    side: "RIGHT" as const,
    body: `**${f.severity.charAt(0).toUpperCase()}${f.severity.slice(1)}: ${f.title}**\n\n${f.explanation}`,
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
// A step with `after` waits until the client sends a command that gate names; playback accepts it (protocol/README.md).
type ShowStep = [delayMs: number, make: ((s: Seq) => ServerMessage) | "heartbeat", after?: string];

// The waterfall's spans, as the engine journals them: a line opens a span, later lines of the same id update it, and
// the one with `endedAt` closes it. A span starts at the clock of the line that opened it.
type SpanKind = ActivitySpan["kind"];
const spanStarts = new Map<string, number>();
const spanLine =
  (r: Run, tool: string, summary: string, id: string, kind: SpanKind, parent: string | undefined, end: boolean, outputTokens?: number) =>
  (s: Seq): ServerMessage => {
    const key = `${r.id}/${id}`;
    if (!spanStarts.has(key)) spanStarts.set(key, s.t!);
    const startedAt = at(spanStarts.get(key)!);
    if (end) spanStarts.delete(key);
    return activity(s, r, tool, summary, {
      id,
      ...(parent ? { parent } : {}),
      kind,
      startedAt,
      ...(end ? { endedAt: at(s.t!) } : {}),
      ...(outputTokens === undefined ? {} : { outputTokens }),
    });
  };
/** A phase span's two lines: the open as the phase starts, the close once its transition is journaled. */
const phaseOpen = (r: Run, name: string, attempt = "") => spanLine(r, "phase", `${name} started${attempt}`, `phase:${name}`, "phase", undefined, false);
const phaseClose = (r: Run, name: string, took: string) => spanLine(r, "phase", `${name} done in ${took}`, `phase:${name}`, "phase", undefined, true);
/** A tool call that ran in `turn` and took `ms`, closed as it is journaled. */
const toolCall = (r: Run, turn: string, tool: string, summary: string, ms = 120) => (s: Seq) =>
  activity(s, r, tool, summary, { id: `tool:${s.seq}`, parent: turn, kind: "tool", startedAt: at(s.t! - ms / 1000), endedAt: at(s.t!) });
const turnLine = (r: Run, n: number, summary: string, end: boolean, outputTokens?: number) =>
  spanLine(r, "model", summary, `turn:${n}`, "model", "phase:analyze", end, outputTokens);
const signInTool = (n: number, tool: string, summary: string) => toolCall(signInRun, `turn:${n}`, tool, summary);
const signInStep = (tool: string, summary: string, id: string, phase: string, end: boolean) => spanLine(signInRun, tool, summary, `step:${id}`, "step", `phase:${phase}`, end);
const signInPublication = (status: PublicationChange["status"], reviewId?: number) => (s: Seq) =>
  ev(
    s,
    "review.publication_changed",
    { runId: signInRun.id, status, ...(reviewId ? { reviewId, url: `https://github.com/${signIn.repository}/pull/${signIn.number}#pullrequestreview-${reviewId}` } : {}) },
    { prId: signIn.id, runId: signInRun.id },
  );
const SIGNIN_REVIEW_ID = 2209184;
const ledgerRetry: Run = { ...started(ledgerRun), failureReason: undefined, attempt: 2 };
/** A run the reader started or retried, from its first tool call to COMPLETED in about 14 s, all behind `gate`. */
const toTheEnd = (r: Run, gate: string, tool: [string, string], findings: FindingDetail[]): ShowStep[] => {
  const verifyMs = 400 * findings.length + 2400;
  const steps: ShowStep[] = [
    [0, phaseOpen(r, "collect")],
    [900, (s) => activity(s, r, ...tool)],
    [1600, (s) => phase(s, r, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
    [0, phaseClose(r, "collect", "2.5s")],
    [0, phaseOpen(r, "triage")],
    [2200, (s) => phase(s, r, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
    [0, phaseClose(r, "triage", "2.2s")],
    [0, phaseOpen(r, "analyze")],
    [0, turnLine(r, 1, "turn 1 started", false)],
    [3400, turnLine(r, 1, "turn 1: 3.4s, 1.6k output tokens, called submit_candidates", false, 1600)],
    [100, turnLine(r, 1, "turn 1: 1 tool result(s) in 0.1s", true, 1600)],
    [0, (s) => phase(s, r, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
    [0, phaseClose(r, "analyze", "3.5s")],
    [0, phaseOpen(r, "verify")],
    ...findings.map((d): ShowStep => [400, (s) => findingUpdated(s, { ...findingOf(d), runId: r.id, stale: false })]),
    [2400, (s) => phase(s, r, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED")],
    [0, phaseClose(r, "verify", `${(verifyMs / 1000).toFixed(1)}s`)],
    [0, phaseOpen(r, "prepare")],
    [1200, (s) => phase(s, r, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED")],
    [900, (s) => phase(s, r, "PUBLISH", "COMPLETED", "NO_PUBLICATION_RECORDED")],
    [0, phaseClose(r, "prepare", "2.1s")],
  ];
  // The run started 500 ms before these lines, when the reader's command played its run.started.
  const took = (500 + steps.reduce((sum, [delay]) => sum + delay, 0)) / 1000;
  const completed = { ...r, phase: "COMPLETED" as const, status: "COMPLETED" as const };
  steps.push([40, (s) => runEvent(s, "review.completed", { ...completed, startedAt: at(s.t! - took), completedAt: at(s.t!) })]);
  return steps.map(([delay, make]) => [delay, make, gate]);
};
const showSteps: ShowStep[] = [
  [600, (s) => runEvent(s, "run.started", signInRun)],
  [0, phaseOpen(signInRun, "collect")],
  [0, signInStep("github", "fetching PR #1291", "pr", "collect", false)],
  [180, signInStep("github", `PR #1291: head ${signIn.headSha.slice(0, 7)} (0.2s)`, "pr", "collect", true)],
  [0, signInStep("git", "fetching origin for acme/shop", "sync", "collect", false)],
  [240, signInStep("git", "fetched origin in 0.2s", "sync", "collect", true)],
  [0, signInStep("git", `diffing ${signInRun.baseSha.slice(0, 7)}..${signIn.headSha.slice(0, 7)}`, "diff", "collect", false)],
  [510, signInStep("git", "diff: 3 files, +6 -3", "diff", "collect", true)],
  [900, (s) => phase(s, signInRun, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
  [0, phaseClose(signInRun, "collect", "1.8s")],
  [0, phaseOpen(signInRun, "triage")],
  [0, signInStep("jev", "rating the risk with jev-systemone", "jev", "triage", false)],
  [340, signInStep("jev", "risk 0.78, security, deep review 84% (0.3s)", "jev", "triage", true)],
  [290, (s) => activity(s, signInRun, "route", "deep: sensitive paths src/server/auth/**; risk 0.78 over 0.6")],
  [450, (s) => phase(s, drainRun, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
  [800, (s) => phase(s, signInRun, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
  [0, phaseClose(signInRun, "triage", "1.9s")],
  [0, phaseOpen(signInRun, "analyze")],
  [0, turnLine(signInRun, 1, "turn 1 started", false)],
  [190, turnLine(signInRun, 1, "turn 1: 0.2s, 388 output tokens, called read_file x2, search_code", false, 388)],
  [120, signInTool(1, "read_file", "src/server/auth/login.ts:1-24")],
  [180, drainActivity("read_diff", "1 file changed (+15 -2): internal/webhooks/dispatcher.go")],
  [260, signInTool(1, "read_file", "src/client/pages/SignIn.tsx:1-58")],
  [420, signInTool(1, "search_code", "\"res.redirect(\": 11 matches in 7 files")],
  [0, turnLine(signInRun, 1, "turn 1: 3 tool result(s) in 1.0s", true, 388)],
  [0, turnLine(signInRun, 2, "turn 2 started", false)],
  [220, turnLine(signInRun, 2, "turn 2: 0.2s, 512 output tokens, called read_file, git_history, search_code", false, 512)],
  [160, signInTool(2, "read_file", "src/server/http/safeRedirect.ts:1-24")],
  [230, drainActivity("read_file", "internal/webhooks/dispatcher.go:1-112")],
  [550, signInTool(2, "git_history", "src/server/http/safeRedirect.ts: added in #1102 \"Block open redirects on /logout\" (tnakamura)")],
  [300, signInTool(2, "search_code", "\"safeRedirect(\": 2 matches, src/server/routes/logout.ts:14, src/server/routes/account.ts:52")],
  [0, turnLine(signInRun, 2, "turn 2: 3 tool result(s) in 1.2s", true, 512)],
  [0, turnLine(signInRun, 3, "turn 3 started", false)],
  [360, turnLine(signInRun, 3, "turn 3: 0.4s, 641 output tokens, called read_file, search_code, git_history", false, 641)],
  [110, "heartbeat"],
  [160, signInTool(3, "read_file", "src/server/routes/logout.ts:1-30")],
  [480, signInTool(3, "search_code", "\"regenerateSession(\": 1 definition (src/server/session.ts:52), 0 callers")],
  [350, signInTool(3, "git_history", "src/server/auth/callback.ts: regenerateSession(req) added in #1077 \"Rotate session id on sign-in\"")],
  [0, turnLine(signInRun, 3, "turn 3: 3 tool result(s) in 1.1s", true, 641)],
  [0, turnLine(signInRun, 4, "turn 4 started", false)],
  [210, drainActivity("search_code", "\"dispatcher.Run(\": 1 match, cmd/worker/main.go:44")],
  [240, turnLine(signInRun, 4, "turn 4: 0.5s, 1.2k output tokens, called read_file x2, search_code x2", false, 1204)],
  [150, signInTool(4, "read_file", "src/server/session.ts:40-64")],
  [270, signInTool(4, "search_code", "\"req.session.userId =\": 3 matches in auth/callback.ts, auth/dev.ts, tests/helpers/login.ts")],
  [600, signInTool(4, "read_file", "tests/auth/callback.test.ts:1-80")],
  [330, signInTool(4, "search_code", "\"returnTo\" in tests/: 1 match, callback.test.ts:44 (relative path only)")],
  [0, turnLine(signInRun, 4, "turn 4: 4 tool result(s) in 1.4s", true, 1204)],
  [0, turnLine(signInRun, 5, "turn 5 started", false)],
  [240, drainActivity("read_file", "cmd/worker/main.go:30-60")],
  [160, turnLine(signInRun, 5, "turn 5: 0.4s, 880 output tokens, called read_file x2, search_code x2", false, 880)],
  [250, signInTool(5, "read_file", "src/server/app.ts:1-40")],
  [450, signInTool(5, "read_file", "src/server/http/auth.ts:1-46")],
  [380, signInTool(5, "search_code", "\"returnTo\" in src/client/: 2 matches, pages/SignIn.tsx:32, components/Header.tsx:18")],
  [190, signInTool(5, "search_code", "\"cookieParser(\": src/server/app.ts:22, no signing secret")],
  [0, turnLine(signInRun, 5, "turn 5: 4 tool result(s) in 1.3s", true, 880)],
  [0, turnLine(signInRun, 6, "turn 6 started", false)],
  [730, turnLine(signInRun, 6, "turn 6: 0.7s, 2.1k output tokens, called submit_candidates", false, 2114)],
  [170, signInTool(6, "submit_candidates", "3 candidate(s), 0 resolution claim(s)")],
  [0, turnLine(signInRun, 6, "turn 6: 1 tool result(s) in 0.2s", true, 2114)],
  [0, (s) => phase(s, signInRun, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
  [0, phaseClose(signInRun, "analyze", "8.5s")],
  [0, phaseOpen(signInRun, "verify")],
  [0, signInStep("verify", "locating earlier findings at this head", "locate", "verify", false)],
  [360, signInStep("verify", "0 earlier finding(s) located (0.4s)", "locate", "verify", true)],
  [0, signInStep("verify", "checking 3 candidate(s)", "check", "verify", false)],
  [290, signInActivity("read_file", "src/server/auth/login.ts:6-11")],
  [440, signInActivity("search_code", "\"isSafeReturnPath|safeRedirect\" on the sign-in path: 0 matches")],
  [380, (s) => findingUpdated(s, findingOf(openRedirect))],
  [580, signInActivity("git_history", "#1077 review thread: rotation required by the 2025 pen test (finding AC-14)")],
  [620, (s) => findingUpdated(s, findingOf(sessionFixation))],
  [0, signInStep("verify", "2 kept, 1 discarded (2.3s)", "check", "verify", true)],
  [0, signInActivity("verify", `kept: ${openRedirect.title}`)],
  [0, signInActivity("verify", `kept: ${sessionFixation.title}`)],
  [0, signInActivity("verify", "discarded: Session cookie is not marked Secure (cited line sets secure: true)")],
  [300, (s) => phase(s, drainRun, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
  [800, (s) => phase(s, signInRun, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED")],
  [0, phaseClose(signInRun, "verify", "3.8s")],
  [0, phaseOpen(signInRun, "prepare")],
  [0, signInActivity("prepare", "payload: 2 finding(s), 2 inline comment(s), 612-char summary, parks for approval")],
  [0, signInStep("github", "checking the PR head", "head", "prepare", false)],
  [300, signInStep("github", "head unchanged (0.3s)", "head", "prepare", true)],
  // Approval mode: the review is stored and parks until the reader approves the preview.
  [500, (s) => phase(s, signInRun, "PREPARE_PUBLICATION", "AWAITING_APPROVAL", "APPROVAL_REQUIRED")],
  [0, phaseClose(signInRun, "prepare", "0.8s")],
  [40, signInPublication("awaiting_approval")],
  // #338 ends on its own about 80 s in, unless the reader cancels it first (the GIF does, after the start on #1296);
  // playback drops a run's lines once it has ended.
  [5000, drainActivity("read_file", "internal/webhooks/dispatcher_test.go:1-90")],
  [30000, drainActivity("search_code", "\"WithoutCancel(\": 1 match, internal/webhooks/dispatcher.go:68")],
  [25000, drainActivity("read_file", "internal/webhooks/deliver.go:1-74")],
  [3000, (s) => phase(s, drainRun, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED")],
  [1500, (s) => phase(s, drainRun, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED")],
  [800, (s) => phase(s, drainRun, "PUBLISH", "COMPLETED", "NO_PUBLICATION_RECORDED")],
  [40, (s) => runEvent(s, "review.completed", { ...drainRun, phase: "COMPLETED", status: "COMPLETED", completedAt: at(s.t!) })],
  [600, (s) => phase(s, signInRun, "AWAITING_APPROVAL", "PUBLISH", "APPROVED"), "review.publish"],
  [300, signInPublication("publishing"), "review.publish"],
  [1200, signInPublication("published", SIGNIN_REVIEW_ID), "review.publish"],
  [80, (s) => findingUpdated(s, { ...findingOf(openRedirect), status: "PUBLISHED" }), "review.publish"],
  [60, (s) => findingUpdated(s, { ...findingOf(sessionFixation), status: "PUBLISHED" }), "review.publish"],
  [400, (s) => phase(s, signInRun, "PUBLISH", "COMPLETED", "REVIEW_RECONCILED"), "review.publish"],
  [40, (s) => runEvent(s, "review.completed", { ...signInRun, phase: "COMPLETED", status: "COMPLETED", completedAt: at(s.t!) }), "review.publish"],
  [4000, "heartbeat"],
  [4000, "heartbeat"],
  // What s, c and R do in the showcase, each on the one target it scripts: they play whenever the reader sends them.
  [500, (s) => runEvent(s, "run.started", { ...giftCardsRun, startedAt: at(s.t!) }), `review.start:${giftCards.id}:${giftCards.headSha}`],
  ...toTheEnd(giftCardsRun, `review.start:${giftCards.id}:${giftCards.headSha}`, ["read_diff", "1 file changed (+5 -1): src/server/checkout/giftCards.ts"], [doubleSpend]),
  [500, (s) => phase(s, drainRun, "ANALYZE", "FAILED", "CANCELLED"), `review.cancel:${drainRun.id}`],
  [40, (s) => runEvent(s, "run.failed", { ...drainRun, phase: "FAILED", status: "FAILED", completedAt: at(s.t!), failureReason: "cancelled from the TUI" }), `review.cancel:${drainRun.id}`],
  [500, (s) => runEvent(s, "run.started", { ...ledgerRetry, startedAt: at(s.t!) }), `review.retry:${ledgerRun.id}`],
  ...toTheEnd(ledgerRetry, `review.retry:${ledgerRun.id}`, ["read_diff", "4 files changed (+96 -12) once migrations/ is excluded as generated"], []),
  [300, (s) => accountsUpdated(s, showAccounts.map((a) => ({ ...a, selected: a.id === showPersonal.id }))), `account.select:${showPersonal.id}`],
];


// A fresh machine: no model credentials, no gh login, and a read-only daemon with commands off.
const setupChecks: SetupCheck[] = [
  { id: "git", status: "ok", detail: "/usr/bin/git" },
  { id: "config", status: "ok", detail: "defaults, no pr-review.config.json" },
  { id: "model", status: "missing", detail: "no models available", fix: "run `pr-review login anthropic`, or set ANTHROPIC_API_KEY" },
  { id: "github", status: "warn", detail: "no token, public repositories only", fix: "run `gh auth login`, or set GITHUB_TOKEN" },
  {
    id: "jev",
    status: "warn",
    detail: "OpenCode Zen free tier for public repositories; private repositories skip Jev",
    fix: "set TYPESAFE_API_KEY to use TypeSafe; without it, serve and daemon --allow-commands skip Jev for every repository",
  },
];
const welcomeChecks = {
  ...welcome(0, EPOCH, ["account.select"], "read-only", "review commands are off; start the daemon with --allow-commands or set control.allow_commands"),
  checks: setupChecks,
} as ServerMessage;

// ---------- tracking: one PR reviewed on three heads ----------
// acme/shop#1302's first review posted three findings. The second saw two of them again, verified the third fixed and
// found a fourth. The third saw one unchanged, one whose cited lines changed, one it could not settle, and the fixed
// one's code back again, so that finding reopens. Every run's CANDIDATES_DISPOSED says how many candidates the
// verifier discarded.

const holdsBase = showPr("acme/shop", 1302, "Bookings: hold a slot while the guest pays", "lmoreau");
const holdsAt = (n: number): Pr => ({ ...holdsBase, headSha: hexSha(`acme/shop#1302 head ${n + 1}`) });
const HOLDS_T = [60, 900, 1740];
const holdsRuns = HOLDS_T.map((t, n) => showRun(holdsAt(n), t, { completedAt: at(t + 240) }));
const [holdsFirst, holdsSecond, holdsThird] = holdsRuns as [Run, Run, Run];
const holdsHead = holdsAt(2);

const holdsTs = diffFile(
  "src/server/bookings/holds.ts",
  [
    hunk(0, 1, "", `
+import type { Clock, Db, Payments } from "../deps.ts";
+
+export const HOLD_MINUTES = 10;
+
+export async function placeHold(db: Db, clock: Clock, slotId: string, guestId: string) {
+  const hold = await db.hold.create({
+    data: { slotId, guestId, expiresAt: addMinutes(clock.now(), HOLD_MINUTES) },
+  });
+  setTimeout(() => releaseHold(db, hold.id), HOLD_MINUTES * 60_000);
+  return hold;
+}
+
+export async function releaseHold(db: Db, holdId: string) {
+  await db.hold.update({ where: { id: holdId }, data: { releasedAt: new Date() } });
+}
+
+export async function cancelHold(db: Db, payments: Payments, holdId: string) {
+  const hold = await db.hold.findUniqueOrThrow({ where: { id: holdId }, include: { deposit: true } });
+  await releaseHold(db, holdId);
+  if (hold.deposit?.capturedAt) await payments.refund(hold.deposit.providerId, hold.deposit.amountCents);
+}
`),
  ],
  "added",
);
const holdsWebhook = diffFile(
  "src/server/bookings/webhook.ts",
  [
    hunk(0, 1, "", `
+import type { Db, PaymentEvent } from "../deps.ts";
+
+export async function onPaymentSucceeded(db: Db, event: PaymentEvent) {
+  const hold = await db.hold.findUnique({ where: { paymentIntentId: event.intentId } });
+  if (!hold) return;
+  await db.booking.create({ data: { slotId: hold.slotId, guestId: hold.guestId } });
+  await db.hold.delete({ where: { id: hold.id } });
+}
`),
  ],
  "added",
);
const holdsSlots = diffFile("src/server/bookings/slots.ts", [
  hunk(8, 8, "export function slotsFor(venue: Venue, day: string) {", `
 export function slotsFor(venue: Venue, day: string) {
-  const start = startOfDay(parseISO(day));
+  const start = startOfDay(parseISO(\`\${day}T00:00:00\`), { in: tz(venue.timeZone) });
   return venue.openings.map((o) => addMinutes(start, o.minuteOfDay));
 }
`),
]);

type Seen = { runId: string; disposition: FindingDetail["status"]; at: string; basis?: string };
const seen = (r: Run, disposition: FindingDetail["status"], basis?: string): Seen => ({
  runId: r.id,
  disposition,
  at: at(Date.parse(r.completedAt!) / 1000 - T0 / 1000 - 30),
  ...(basis ? { basis } : {}),
});
const tracked = (r: Run, a: Authored, status: FindingDetail["status"], history: Seen[], extra: Partial<Finding> = {}): FindingDetail => ({
  ...showDetail(r, 0, a, { status, ...extra }),
  history,
});

const expiry: Authored = {
  title: "Hold expiry lives in a timer in the web process",
  category: "correctness",
  severity: "high",
  confidence: 0.91,
  file: holdsTs.path,
  line: lineOf(holdsTs, "setTimeout(() => releaseHold(db, hold.id), HOLD_MINUTES * 60_000);"),
  explanation:
    "placeHold releases the hold from a `setTimeout` in the process that served the request. A deploy, a crash or a scale-down drops the timer, and the slot stays held with nobody paying for it. The hold row has `expiresAt`, but nothing reads it.",
  reasoningSummary:
    "No job, cron or query reads hold.expiresAt (search_code). The web processes restart on every deploy (deploy/web.yaml rolls them), several times a day.",
  suggestedFix: "Release expired holds from the worker: a scheduled job that releases every hold whose expiresAt has passed, and drop the timer.",
  evidence: [
    cite(holdsTs, holdsHead.headSha, "export async function placeHold(", "return hold;"),
    { kind: "tool_summary", text: 'search_code "expiresAt": 2 matches, both writes; no reader in src/ or jobs/' },
  ],
};
const zone: Authored = {
  title: "Slot times ignore the venue's time zone",
  category: "correctness",
  severity: "medium",
  confidence: 0.86,
  file: holdsSlots.path,
  line: lineOf(holdsSlots, "const start = startOfDay("),
  explanation:
    "slotsFor computed the day's start in the server's zone, so a venue east of UTC listed its slots shifted by its offset. On this head the line builds the start in `venue.timeZone`, which may fix it.",
  reasoningSummary: "The cited line changed on the newest head. The reviewer did not verify the new line, so it reads as looking resolved, not resolved.",
  suggestedFix: "Build the start of the day in the venue's zone, as this head now does, and add a test for a venue east of UTC.",
  evidence: [cite(holdsSlots, holdsHead.headSha, "const start = startOfDay(", "return venue.openings.map(")],
};
const released: Authored = {
  title: "The payment webhook can confirm a released hold",
  category: "data_integrity",
  severity: "high",
  confidence: 0.89,
  file: holdsWebhook.path,
  line: lineOf(holdsWebhook, "if (!hold) return;"),
  explanation:
    "onPaymentSucceeded books the slot for any hold it finds, released or not. A guest who pays after the hold expired gets a booking for a slot that may already be someone else's.",
  reasoningSummary:
    "The second head guarded it with `if (!hold || hold.releasedAt) return;`, and the reviewer verified that fixed. This head is back to `if (!hold) return;`, so the code that was fixed has returned.",
  suggestedFix: "Skip a released hold, and refund its payment instead of booking:\n\nif (!hold || hold.releasedAt) return refundLatePayment(event);",
  evidence: [cite(holdsWebhook, holdsHead.headSha, "const hold = await db.hold.findUnique(", "await db.booking.create(")],
};
const deposit: Authored = {
  title: "Cancelling a hold may leave an authorized deposit uncaptured and unreleased",
  category: "correctness",
  severity: "low",
  confidence: 0.58,
  file: holdsTs.path,
  line: lineOf(holdsTs, "if (hold.deposit?.capturedAt)"),
  explanation:
    "cancelHold refunds a captured deposit, but an authorized one that was never captured is neither voided nor released, so the guest's card may show a pending charge for days.",
  reasoningSummary:
    "Whether the payment provider voids an uncaptured authorization on its own depends on the account's settings, which the reviewer can't read. It reported the finding again without verifying it.",
  evidence: [cite(holdsTs, holdsHead.headSha, "export async function cancelHold(", "if (hold.deposit?.capturedAt)")],
};

const holdsExpiry = tracked(holdsThird, expiry, "STILL_PRESENT", [
  seen(holdsFirst, "DETECTED"),
  seen(holdsFirst, "PUBLISHED"),
  seen(holdsSecond, "STILL_PRESENT", "located"),
  seen(holdsThird, "STILL_PRESENT", "located"),
]);
const holdsZone = tracked(holdsThird, zone, "POTENTIALLY_RESOLVED", [
  seen(holdsFirst, "DETECTED"),
  seen(holdsFirst, "PUBLISHED"),
  seen(holdsSecond, "STILL_PRESENT", "located"),
  seen(holdsThird, "POTENTIALLY_RESOLVED", "changed"),
]);
// The fixed one keeps its row, resolved on the second head; the third head's reopening is a new finding seeded from it,
// with the same fingerprint.
const holdsFixed = tracked(
  holdsSecond,
  { ...released, line: 5, evidence: [{ kind: "code_ref", path: holdsWebhook.path, start: 4, end: 6, refSha: holdsAt(1).headSha, text: "  const hold = await db.hold.findUnique({ where: { paymentIntentId: event.intentId } });\n  if (!hold || hold.releasedAt) return;\n  await db.booking.create({ data: { slotId: hold.slotId, guestId: hold.guestId } });" }] },
  "RESOLVED",
  [seen(holdsFirst, "DETECTED"), seen(holdsFirst, "PUBLISHED"), seen(holdsSecond, "RESOLVED", "resolution_checked")],
  { stale: true },
);
const holdsReopened = tracked(holdsThird, released, "DETECTED", [seen(holdsThird, "DETECTED", "reopened")]);
const holdsDeposit = tracked(holdsThird, deposit, "UNCERTAIN", [seen(holdsSecond, "DETECTED"), seen(holdsThird, "UNCERTAIN", "reported_unverified")]);
const holdsDetails = [holdsExpiry, holdsZone, holdsReopened, holdsDeposit, holdsFixed];

const holdsDisposed = (s: Seq, r: Run, detail: string) =>
  ev(s, "review.phase_changed", { from: "VERIFY", to: "PREPARE_PUBLICATION", event: "CANDIDATES_DISPOSED", detail }, { prId: r.prId, runId: r.id });
/** One run on its head: the phases, its tool calls, then what the verifier kept and what it discarded. */
const holdsRun = (r: Run, t: number, tools: [string, string][], disposed: string, findings: Finding[], end: "NO_PUBLICATION_RECORDED" | "REVIEW_RECONCILED"): Recorded[] => [
  [t, (s) => runEvent(s, "run.started", started(r))],
  [t + 20, (s) => phase(s, r, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED")],
  [t + 40, (s) => phase(s, r, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED")],
  ...tools.map(([tool, summary], i): Recorded => [t + 50 + i * 10, (s) => activity(s, r, tool, summary)]),
  [t + 150, (s) => phase(s, r, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED")],
  ...findings.map((f, i): Recorded => [t + 170 + i * 4, (s) => findingUpdated(s, f)]),
  [t + 220, (s) => holdsDisposed(s, r, disposed)],
  [t + 226, (s) => phase(s, r, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED")],
  [t + 236, (s) => phase(s, r, "PUBLISH", "COMPLETED", end)],
  [t + 240, (s) => runEvent(s, "review.completed", r)],
];
const asOf = (d: FindingDetail, r: Run, status: Finding["status"], stale = false): Finding => ({ ...findingOf(d), runId: r.id, status, stale });
const holdsRecorded: Recorded[] = [
  ...holdsRun(
    holdsFirst,
    HOLDS_T[0]!,
    [["read_diff", "3 files changed (+34 -1): src/server/bookings/holds.ts, webhook.ts, slots.ts"], ["search_code", '"expiresAt": 2 matches, both writes']],
    "3 verified, 3 discarded, 0 carried, 0 resolved, 0 reopened",
    [asOf(holdsExpiry, holdsFirst, "DETECTED"), asOf(holdsZone, holdsFirst, "DETECTED"), asOf(holdsFixed, holdsFirst, "DETECTED")],
    "REVIEW_RECONCILED",
  ),
  [
    HOLDS_T[0]! + 232,
    (s) =>
      ev(
        s,
        "review.publication_changed",
        { runId: holdsFirst.id, status: "published", reviewId: 2207711, url: `https://github.com/${holdsBase.repository}/pull/${holdsBase.number}#pullrequestreview-2207711` },
        { prId: holdsBase.id, runId: holdsFirst.id },
      ),
  ],
  [HOLDS_T[0]! + 234, (s) => findingUpdated(s, asOf(holdsExpiry, holdsFirst, "PUBLISHED"))],
  [HOLDS_T[0]! + 234, (s) => findingUpdated(s, asOf(holdsZone, holdsFirst, "PUBLISHED"))],
  [HOLDS_T[0]! + 234, (s) => findingUpdated(s, asOf(holdsFixed, holdsFirst, "PUBLISHED"))],
  [HOLDS_T[1]! - 20, (s) => ev(s, "pr.updated", holdsAt(1), { prId: holdsBase.id })],
  ...holdsRun(
    holdsSecond,
    HOLDS_T[1]!,
    [["read_diff", "2 files changed (+3 -2): src/server/bookings/webhook.ts, holds.ts"], ["read_file", "src/server/bookings/webhook.ts:1-9"]],
    "1 verified, 1 discarded, 2 carried, 1 resolved, 0 reopened",
    [
      asOf(holdsExpiry, holdsSecond, "STILL_PRESENT"),
      asOf(holdsZone, holdsSecond, "STILL_PRESENT"),
      asOf(holdsFixed, holdsSecond, "RESOLVED"),
      asOf(holdsDeposit, holdsSecond, "DETECTED"),
    ],
    "NO_PUBLICATION_RECORDED",
  ),
  [HOLDS_T[2]! - 20, (s) => ev(s, "pr.updated", holdsHead, { prId: holdsBase.id })],
  [HOLDS_T[2]! - 19, (s) => findingUpdated(s, asOf(holdsFixed, holdsSecond, "RESOLVED", true))],
  ...holdsRun(
    holdsThird,
    HOLDS_T[2]!,
    [["read_diff", "2 files changed (+2 -2): src/server/bookings/webhook.ts, slots.ts"], ["read_file", "src/server/bookings/slots.ts:1-20"], ["git_history", "webhook.ts: the releasedAt guard was reverted in 41e0c2a \"Simplify webhook\""]],
    "0 verified, 2 discarded, 3 carried, 0 resolved, 1 reopened",
    [
      asOf(holdsExpiry, holdsThird, "STILL_PRESENT"),
      asOf(holdsZone, holdsThird, "POTENTIALLY_RESOLVED"),
      asOf(holdsDeposit, holdsThird, "UNCERTAIN"),
      asOf(holdsReopened, holdsThird, "DETECTED"),
    ],
    "NO_PUBLICATION_RECORDED",
  ),
];
const holdsDiff: ServerMessage = {
  type: "diff.page",
  requestId: "req-diff",
  prId: holdsHead.id,
  baseSha: holdsThird.baseSha,
  headSha: holdsThird.headSha,
  files: [holdsTs, holdsWebhook, holdsSlots],
  truncated: false,
};

// ---------- a huge PR the reviewer could still read: about sixty files, a third of them generated ----------

const tokensPr = showPr("acme/web", 1310, "Design system: move buttons, inputs and forms onto tokens", "jhalvorsen");
const tokensRun = showRun(tokensPr, 2400, { completedAt: at(2700) });
const COMPONENTS = ["Button", "Input", "Select", "Checkbox", "Radio", "Form", "Field", "Label", "Toggle", "TextArea"];
const component = (name: string): DiffFile =>
  diffFile(`src/components/${name}/${name}.tsx`, [
    hunk(1, 1, "", `
-import styles from "./${name}.module.css";
+import { tokens } from "../../tokens";

 export function ${name}(props: ${name}Props) {
-  return <${name.toLowerCase()}Base className={styles.root} {...props} />;
+  return <${name.toLowerCase()}Base style={tokens.${name.toLowerCase()}} {...props} />;
 }
`),
  ]);
const buttonTsx = diffFile("src/components/Button/Button.tsx", [
  hunk(1, 1, "", `
-import styles from "./Button.module.css";
+import { tokens } from "../../tokens";

 export function Button({ disabled, onClick, ...props }: ButtonProps) {
-  return <button className={styles.root} disabled={disabled} onClick={onClick} {...props} />;
+  return (
+    <button
+      style={tokens.button}
+      aria-disabled={disabled}
+      onClick={disabled ? undefined : onClick}
+      {...props}
+    />
+  );
 }
`),
]);
const inputTsx = diffFile("src/components/Input/Input.tsx", [
  hunk(1, 1, "", `
-import styles from "./Input.module.css";
+import { tokens } from "../../tokens";

 export function Input({ invalid, ...props }: InputProps) {
-  return <input className={invalid ? styles.invalid : styles.root} {...props} />;
+  const color = invalid ? tokens.color.danger : tokens.color.text;
+  return <input style={{ ...tokens.input, color, borderColor: color }} {...props} />;
 }
`),
]);
const fieldTsx = diffFile("src/components/Field/Field.tsx", [
  hunk(1, 1, "", `
 export function Field({ label, children }: FieldProps) {
+  const id = useId();
   return (
     <div style={tokens.field}>
-      <label>{label}</label>
+      <label htmlFor={id}>{label}</label>
       {children}
     </div>
   );
 }
`),
]);
const colorsTs = diffFile(
  "src/tokens/colors.ts",
  [
    hunk(0, 1, "", `
+export const color = {
+  text: "#1d1d1f",
+  muted: "#6e6e73",
+  danger: "#ff6b6b",
+  accent: "#4247c8",
+};
`),
  ],
  "added",
);
const small = (path: string, line: string, status: DiffFile["status"] = "modified"): DiffFile => {
  const body = { added: hunk(0, 1, "", `\n+${line}\n`), deleted: hunk(1, 0, "", `\n-${line}\n`) }[status as "added" | "deleted"];
  return diffFile(path, [body ?? hunk(1, 1, "", `\n-// ${path}\n+${line}\n`)], status);
};
const generated = (path: string, additions: number, line: string): DiffFile => ({ ...small(path, line, "added"), additions, truncated: true });
const tokensFiles: DiffFile[] = [
  colorsTs,
  small("src/tokens/spacing.ts", "export const space = [0, 4, 8, 12, 16, 24, 32];", "added"),
  small("src/tokens/typography.ts", 'export const type = { body: 13, small: 12, title: 18, family: "system-ui" };', "added"),
  small("src/tokens/index.ts", 'export * as tokens from "./all";', "added"),
  ...COMPONENTS.flatMap((name) => [
    name === "Button" ? buttonTsx : name === "Input" ? inputTsx : name === "Field" ? fieldTsx : component(name),
    small(`src/components/${name}/${name}.test.tsx`, `it("renders ${name} from tokens", () => expect(render(<${name} />)).toMatchSnapshot());`),
    small(`src/components/${name}/${name}.stories.tsx`, `export const Default = { args: { tokens: "default" } };`),
    small(`src/components/${name}/${name}.module.css`, ".root { color: #1d1d1f; }", "deleted"),
  ]),
  small("src/styles/global.css", ":root { color-scheme: light dark; }"),
  small("src/styles/theme.css", '@import "../tokens/generated/tokens.css";'),
  small(".storybook/preview.ts", 'import "../src/tokens/generated/tokens.css";'),
  small("docs/design-system.md", "Every component reads its colors, spacing and type from src/tokens."),
  small("package.json", '    "tokens": "style-dictionary build",'),
  small("eslint.config.js", '  "no-restricted-imports": ["error", { patterns: ["*.module.css"] }],'),
  generated("src/tokens/generated/tokens.css", 412, ":root { --color-text: #1d1d1f; }"),
  generated("src/tokens/generated/tokens.json", 388, '{ "color": { "text": { "value": "#1d1d1f" } } }'),
  generated("src/tokens/generated/tokens.d.ts", 96, "export declare const tokens: Tokens;"),
  ...COMPONENTS.slice(0, 6).map((name) => generated(`src/components/${name}/__snapshots__/${name}.test.tsx.snap`, 40 + name.length * 3, `exports[\`renders ${name} from tokens 1\`] = \`<div />\`;`)),
  generated("pnpm-lock.yaml", 1204, "  style-dictionary@4.3.0:"),
];
const tokensDetails = [
  showDetail(tokensRun, 2650, {
    title: "A disabled button still submits its form",
    category: "correctness",
    severity: "high",
    confidence: 0.9,
    file: buttonTsx.path,
    line: lineOf(buttonTsx, "aria-disabled={disabled}"),
    explanation:
      "The button now sets `aria-disabled` instead of `disabled`, and drops only its own onClick. A button inside a form still submits it, on a click and on Enter, because the browser only skips submission for a truly disabled button.",
    reasoningSummary: "Form.tsx renders children inside a <form>, and Button's default type is submit. aria-disabled has no effect on form submission.",
    suggestedFix: "Keep the native attribute alongside the ARIA one: disabled={disabled} aria-disabled={disabled}.",
    evidence: [cite(buttonTsx, tokensPr.headSha, "<button", "{...props}")],
  }),
  showDetail(tokensRun, 2655, {
    title: "The invalid input's color fails contrast on white",
    category: "general",
    severity: "medium",
    confidence: 0.87,
    file: inputTsx.path,
    line: lineOf(inputTsx, "const color = invalid ? tokens.color.danger : tokens.color.text;"),
    explanation: "An invalid input sets its text to `color.danger`, #ff6b6b, which is 2.8:1 on white. The error state becomes the hardest one to read.",
    reasoningSummary: "Computed the WCAG ratio of #ff6b6b on #ffffff: 2.78:1, under the 4.5:1 body-text minimum.",
    suggestedFix: "Use a darker danger token for text, for example #c62828 (5.6:1), and keep #ff6b6b for borders and icons.",
    evidence: [cite(inputTsx, tokensPr.headSha, "const color = invalid", "return <input"), cite(colorsTs, tokensPr.headSha, "export const color = {", "};")],
  }),
  showDetail(tokensRun, 2660, {
    title: "Field's label points at an id its input never gets",
    category: "correctness",
    severity: "low",
    confidence: 0.8,
    file: fieldTsx.path,
    line: lineOf(fieldTsx, "<label htmlFor={id}>{label}</label>"),
    explanation: "Field creates an id for its label's htmlFor, but never passes it to the child input, so the label is not associated and clicking it does nothing.",
    reasoningSummary: "children is rendered as given; nothing clones it with the id.",
    suggestedFix: "Pass the id down: render children as a function of id, or clone the single child with id={id}.",
    evidence: [cite(fieldTsx, tokensPr.headSha, "const id = useId();", "{children}")],
  }),
];
const tokensDiff: ServerMessage = {
  type: "diff.page",
  requestId: "req-diff",
  prId: tokensPr.id,
  baseSha: tokensRun.baseSha,
  headSha: tokensRun.headSha,
  files: tokensFiles,
  truncated: false,
};
const tokensRecorded: Recorded[] = [
  prOpened(tokensPr, 2390),
  ...fullRun(
    tokensRun,
    2400,
    [[2410, "read_diff", `${tokensFiles.length} files changed; 10 generated files read as summaries only`]],
    tokensDetails.map((d, i): [number, FindingDetail] => [2650 + i * 5, d]),
  ),
];

/** What the playbacks grant: the review commands they script an outcome for, which leaves out review.again. */
const PLAYED_COMMANDS = REVIEW_COMMANDS.filter((c) => c !== "review.again");

/** A script that plays `recorded` in time order from an empty snapshot of `prs`, so every event is in the stream. */
const replay =(prs: Pr[], recorded: Recorded[]): PlaybackLine[] => {
  const rows = recorded.map(([t, make], k) => ({ t, k, make })).sort((a, b) => a.t - b.t || a.k - b.k);
  return [
    { delayMs: 0, message: welcome(0, EPOCH, ["account.select", ...PLAYED_COMMANDS], "engine") },
    { delayMs: 0, message: snapshot(0, prs, [], [], EPOCH, showAccounts) },
    ...rows.map(({ t, make }, i): PlaybackLine => ({ delayMs: 20, message: make({ seq: i + 1, t }) })),
  ];
};

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
  evidenceCount: 5,
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
  "client.command.review.again": {
    type: "command",
    requestId: "req-cmd",
    command: "review.again",
    params: { prId: pr1.id, expectedHeadSha: pr1.headSha },
    idempotencyKey: `review.again:${pr1.id}:${pr1.headSha}`,
  },
  "client.command.review.add": {
    type: "command",
    requestId: "req-cmd",
    command: "review.add",
    params: { url: "https://github.com/acme/shop/pull/184" },
    idempotencyKey: "review.add:https://github.com/acme/shop/pull/184",
  },
  "client.command.review.cancel": { type: "command", requestId: "req-cmd", command: "review.cancel", params: { runId: run1.id }, idempotencyKey: `review.cancel:${run1.id}` },
  "client.command.review.retry": { type: "command", requestId: "req-cmd", command: "review.retry", params: { runId: run1.id }, idempotencyKey: `review.retry:${run1.id}` },
  "client.command.review.publish": publishCommand,
  "client.command.account-select": selectCommand(work),
  "client.command.daemon.stop": { type: "command", requestId: "req-cmd", command: "daemon.stop", params: { pauseReviews: false }, idempotencyKey: "daemon.stop" },
  "client.publication.get": { type: "publication.get", requestId: "req-pub", runId: run1.id },
  "client.settings.get": { type: "settings.get", requestId: "req-settings" },
  "client.command.settings.set": {
    type: "command",
    requestId: "req-cmd",
    command: "settings.set",
    params: { model: "openai/gpt-5.5" },
    idempotencyKey: "settings.set:openai/gpt-5.5",
  },
  "server.settings": {
    type: "settings",
    requestId: "req-settings",
    model: { ref: "anthropic/claude-opus-5-5", source: "config" },
    models: [
      { ref: "anthropic/claude-opus-5-5", provider: "anthropic", name: "Claude Opus 5.5", contextWindow: 1_000_000, authSource: "oauth", preferred: true },
      { ref: "anthropic/claude-sonnet-5", provider: "anthropic", name: "Claude Sonnet 5", contextWindow: 1_000_000, authSource: "oauth", preferred: false },
      { ref: "openai/gpt-5.5", provider: "openai", name: "GPT-5.5", contextWindow: 400_000, authSource: "OPENAI_API_KEY", preferred: false },
      { ref: "openrouter/google/gemini-3-pro", provider: "openrouter", name: "Gemini 3 Pro", contextWindow: 1_048_576, authSource: "OPENROUTER_API_KEY", preferred: false },
    ],
    configPath: "/Users/operator/.config/pr-review/pr-review.config.json",
  },
  "server.settings.env": {
    type: "settings",
    requestId: "req-settings",
    model: { ref: "openai/gpt-5.5", source: "PR_REVIEW_MODEL" },
    models: [{ ref: "openai/gpt-5.5", provider: "openai", name: "GPT-5.5", contextWindow: 400_000, authSource: "OPENAI_API_KEY", preferred: false }],
    configPath: "pr-review.config.json",
  },

  "server.welcome": welcome(108),
  "server.welcome.engine": welcome(108, EPOCH, ["account.select", ...REVIEW_COMMANDS], "engine"),
  "server.welcome.checks": welcomeChecks,
  "server.welcome.read-only": welcome(108, EPOCH, ["account.select"], "read-only", "another pr-review is running on /srv/pr-review/.data"),
  "server.publication.preview": preview,
  "server.publication.preview.published": { ...preview, status: "published", reviewId: 3141, url: "https://github.com/acme/shop/pull/184#pullrequestreview-3141" },
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
    attempt: 2,
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
  // A finding carried across three heads: posted on the first, found unchanged on the second, its cited code changed on the third.
  "server.finding.detail.tracked": {
    type: "finding.detail",
    requestId: "req-finding",
    finding: {
      ...detail,
      runId: run1Third.id,
      status: "POTENTIALLY_RESOLVED",
      history: [
        { runId: run1.id, disposition: "DETECTED", at: at(30) },
        { runId: run1.id, disposition: "PUBLISHED", at: at(45) },
        { runId: run1New.id, disposition: "STILL_PRESENT", at: at(95), basis: "located" },
        { runId: run1Third.id, disposition: "POTENTIALLY_RESOLVED", at: at(140), basis: "changed" },
      ],
    },
  },
  "server.finding.detail.minimal": {
    type: "finding.detail",
    requestId: "req-finding",
    finding: { ...f2, evidenceCount: 0, explanation: "e", reasoningSummary: "r", evidence: [], history: [] },
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
  "server.command.result.accepted.review-add": {
    type: "command.result",
    requestId: "req-cmd",
    status: "accepted",
    operationId: `review.add:${run1.id}`,
    prId: pr1.id,
    runId: run1.id,
  },
  "server.command.result.rejected.already-tracked": {
    type: "command.result",
    requestId: "req-cmd",
    status: "rejected",
    code: "CONFLICT",
    message: "acme/shop#184 is already tracked",
    prId: pr1.id,
  },
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
  ...Object.fromEntries(
    [...holdsDetails, ...tokensDetails].map((d, i) => [`server.finding.detail.tracking-${i + 1}`, { type: "finding.detail", requestId: "req-finding", finding: d }]),
  ),
  "server.diff.page.tracking-shop-1302": holdsDiff,
  "server.diff.page.tracking-web-1310": tokensDiff,
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
    "Only the reserved command names exist.",
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


// One finding crosses two runs: posted by the first, observed still present by the second, so it is never stale.
const run1Done = { ...run1, phase: "COMPLETED" as const, status: "COMPLETED" as const, completedAt: at(-30) };
const carryOver = lines(40, [
  welcome(10),
  snapshot(10, [pr1, pr2, pr3], [run1Done, run2], [{ ...f1, status: "PUBLISHED" }, f2]),
  ev({ seq: 11 }, "pr.updated", pr1New, { prId: pr1.id }),
  runEvent({ seq: 12 }, "run.started", run1New),
  findingUpdated({ seq: 13 }, { ...f1, status: "PUBLISHED", stale: true }),
  phase({ seq: 14 }, run1New, "COLLECT_CONTEXT", "TRIAGE", "CONTEXT_PERSISTED"),
  phase({ seq: 15 }, run1New, "TRIAGE", "ANALYZE", "ROUTING_PERSISTED"),
  phase({ seq: 16 }, run1New, "ANALYZE", "VERIFY", "CANDIDATES_PERSISTED"),
  findingUpdated({ seq: 17 }, { ...f1, runId: run1New.id, status: "STILL_PRESENT", line: 47 }),
  phase({ seq: 18 }, run1New, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  phase({ seq: 19 }, run1New, "PREPARE_PUBLICATION", "PUBLISH", "HEAD_VERIFIED"),
  phase({ seq: 20 }, run1New, "PUBLISH", "COMPLETED", "REVIEW_RECONCILED"),
  runEvent({ seq: 21 }, "review.completed", { ...run1New, phase: "COMPLETED", status: "COMPLETED", completedAt: at(21) }),
]);
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

// Nothing tracked yet on a machine that cannot review: the client shows the missing checks where the inbox would be.
const setup = lines(0, [welcomeChecks, snapshot(0, [], [], [], EPOCH, [])]);

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
  welcome(10, EPOCH, ["account.select", ...PLAYED_COMMANDS], "engine"),
  snapshot(10, [pr1, pr2, pr3], [run1, run2], [f1, f2], EPOCH, [personal, work]),
  phase({ seq: 11 }, run1, "VERIFY", "PREPARE_PUBLICATION", "CANDIDATES_DISPOSED"),
  phase({ seq: 12 }, run1, "PREPARE_PUBLICATION", "AWAITING_APPROVAL", "APPROVAL_REQUIRED"),
  publicationChanged({ seq: 13 }, run1, "awaiting_approval"),
  preview,
]).concat(
  // The approval plays once the reader sends it; playback answers the command itself, so the script carries no
  // command.result for a request the client never made.
  lines(40, [
    phase({ seq: 14 }, run1, "AWAITING_APPROVAL", "PUBLISH", "APPROVED"),
    publicationChanged({ seq: 15 }, run1, "publishing"),
    publicationChanged({ seq: 16 }, run1, "published", 3141),
    phase({ seq: 17 }, run1, "PUBLISH", "COMPLETED", "REVIEW_RECONCILED"),
    runEvent({ seq: 18 }, "review.completed", { ...run1, phase: "COMPLETED", status: "COMPLETED", completedAt: at(18) }),
  ]).map((l) => ({ ...l, after: "review.publish" })),
);

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
    { delayMs: 0, message: welcome(40, EPOCH, ["account.select", ...PLAYED_COMMANDS], "engine") },
    {
      delayMs: 0,
      message: snapshot(
        40,
        [orders, requestIds, signIn, drain, ledger, giftCards],
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
  "carry-over": {
    script: carryOver,
    expect: baseExpect({
      lastSequence: 21,
      prs: prRows(pr1New, pr2, pr3),
      runs: [runRow(run1Done), runRow(run2), runRow(run1New, "COMPLETED", "COMPLETED")],
      findings: [
        { id: f1.id, title: f1.title, status: "STILL_PRESENT", stale: false },
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
      lastSequence: 185,
      resyncRequested: false,
      toolActivityCount: 103,
      prs: prRows(orders, requestIds, signIn, drain, ledger, giftCards),
      runs: [
        runRow(ordersRun),
        runRow(requestIdsRun),
        runRow(signInRun, "COMPLETED", "COMPLETED"),
        runRow(drainOldRun),
        runRow(drainRun, "FAILED", "FAILED"),
        runRow(ledgerRun, "COMPLETED", "COMPLETED"),
        runRow(giftCardsRun, "COMPLETED", "COMPLETED"),
      ],
      findings: showDetails.map((f) => ({ id: f.id, title: f.title, status: f.runId === signInRun.id ? "PUBLISHED" : "DETECTED", stale: f.stale })),
      accounts: [
        { id: "github.com/acme-review-bot", state: "ok", ghActive: false, selected: false },
        { id: "github.com/dperalta", state: "ok", ghActive: true, selected: true },
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
  setup: {
    script: setup,
    expect: { streamEpoch: EPOCH, lastSequence: 0, resyncRequested: false, toolActivityCount: 0, prs: [], runs: [], findings: [], accounts: [] },
  },
  burst: { script: burst, expect: baseExpect({ lastSequence: 5010, toolActivityCount: 5000 }) },
  tracking: {
    script: replay([holdsBase], holdsRecorded),
    expect: {
      streamEpoch: EPOCH,
      lastSequence: holdsRecorded.length,
      resyncRequested: false,
      toolActivityCount: 7,
      prs: prRows(holdsHead),
      runs: holdsRuns.map((r) => runRow(r)),
      findings: [
        { ...holdsExpiry, status: "STILL_PRESENT" },
        { ...holdsZone, status: "POTENTIALLY_RESOLVED" },
        { ...holdsDeposit, status: "UNCERTAIN" },
        { ...holdsReopened, status: "DETECTED" },
        { ...holdsFixed, status: "RESOLVED" },
      ].map((f) => ({ id: f.id, title: f.title, status: f.status, stale: f.stale })),
      accounts: [...showAccounts].sort((a, b) => (a.id < b.id ? -1 : 1)).map((a) => ({ id: a.id, state: a.state, ghActive: a.ghActive, selected: a.selected })),
      publications: [{ runId: holdsFirst.id, status: "published" }],
    },
  },
  "big-diff": {
    script: replay([tokensPr], tokensRecorded),
    expect: {
      streamEpoch: EPOCH,
      lastSequence: tokensRecorded.length,
      resyncRequested: false,
      toolActivityCount: 1,
      prs: prRows(tokensPr),
      runs: [runRow(tokensRun)],
      findings: tokensDetails.map((f) => ({ id: f.id, title: f.title, status: "DETECTED", stale: false })),
      accounts: [...showAccounts].sort((a, b) => (a.id < b.id ? -1 : 1)).map((a) => ({ id: a.id, state: a.state, ghActive: a.ghActive, selected: a.selected })),
    },
  },
};
