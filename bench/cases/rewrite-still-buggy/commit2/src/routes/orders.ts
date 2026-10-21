import { requireAdmin, requireUser } from "../auth.ts";
import { db, queueRefund } from "../db.ts";
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

/** Refunds `cents` of the order, or everything not yet refunded when the body names no amount. */
const refundOrder: Handler = (req, res) => {
  const order = db.orders.get(req.params.id!);
  if (!order) return res.status(404).end();
  if (order.status !== "paid") return res.status(409).end();
  const refundable = order.totalCents - order.refundedCents;
  const { cents = refundable } = req.body as { cents?: number };
  if (!Number.isInteger(cents) || cents <= 0 || cents > refundable) return res.status(400).end();
  queueRefund(order.id, cents);
  order.refundedCents += cents;
  if (order.refundedCents === order.totalCents) order.status = "refunded";
  res.json(order);
};

export function registerOrderRoutes(router: Router) {
  router.get("/orders/:id", requireUser, getOrder);
  router.post("/orders/:id/cancel", requireUser, requireAdmin, cancelOrder);
  router.post("/orders/:id/refund", requireUser, refundOrder);
}
