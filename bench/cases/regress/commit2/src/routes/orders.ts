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
  router.get("/orders/:id", requireUser, getOrder);
  router.post("/orders/:id/cancel", requireUser, requireAdmin, cancelOrder);
  router.post("/orders/:id/refund", requireUser, requireAdmin, refundOrder);
}
