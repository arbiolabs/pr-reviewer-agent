import { requireUser } from "../auth.ts";
import { db } from "../db.ts";
import type { Handler, Router } from "../http.ts";

/** Support reopens an order that was cancelled by mistake, so it can ship again. */
const reopenOrder: Handler = (req, res) => {
  const order = db.orders.get(req.params.id!);
  if (!order) return res.status(404).end();
  if (order.status !== "cancelled") return res.status(409).end();
  order.status = "paid";
  res.json(order);
};

export function registerSupportRoutes(router: Router) {
  router.post("/support/orders/:id/reopen", requireUser, reopenOrder);
}
