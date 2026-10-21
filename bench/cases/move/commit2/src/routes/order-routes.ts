import { requireAdmin, requireUser } from "../auth.ts";
import { db, type Order, queueRefund } from "../db.ts";
import type { Handler, Router } from "../http.ts";

const getOrder: Handler = (req, res) => {
  const order = db.orders.get(req.params.id!);
  if (!order || order.userId !== req.user!.id) return res.status(404).end();
  res.json(order);
};

const cancelOrder: Handler = (req, res) => {
  const order = db.orders.get(req.params.id!);
  if (!order) return res.status(404).end();
  order.status = "cancelled";
  res.json(order);
};

/** What the order list shows for each order: no payment details. */
type OrderSummary = { id: string; totalCents: number; status: Order["status"] };

const summarize = (order: Order): OrderSummary => ({ id: order.id, totalCents: order.totalCents, status: order.status });

const listMyOrders: Handler = (req, res) => {
  const mine: OrderSummary[] = [];
  for (const order of db.orders.values()) {
    if (order.userId === req.user!.id) mine.push(summarize(order));
  }
  mine.sort((a, b) => a.id.localeCompare(b.id));
  res.json(mine);
};

const countMyOrders: Handler = (req, res) => {
  let count = 0;
  for (const order of db.orders.values()) {
    if (order.userId === req.user!.id) count++;
  }
  res.json({ count });
};

const refundOrder: Handler = (req, res) => {
  const order = db.orders.get(req.params.id!);
  if (!order) return res.status(404).end();
  if (order.status !== "paid") return res.status(409).end();
  queueRefund(order.id, order.totalCents - order.refundedCents);
  order.refundedCents = order.totalCents;
  order.status = "refunded";
  res.json(order);
};

export function registerOrderRoutes(router: Router) {
  router.get("/orders", requireUser, listMyOrders);
  router.get("/orders/count", requireUser, countMyOrders);
  router.get("/orders/:id", requireUser, getOrder);
  router.post("/orders/:id/cancel", requireUser, requireAdmin, cancelOrder);
  router.post("/orders/:id/refund", requireUser, refundOrder);
}
