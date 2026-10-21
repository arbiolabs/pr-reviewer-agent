import { requireAdmin, requireUser } from "./auth.ts";
import type { Router } from "./http.ts";
import { registerOrderRoutes } from "./routes/orders.ts";
import { registerUserRoutes } from "./routes/users.ts";

export function registerRoutes(router: Router) {
  router.use("/orders/:id/refund", requireUser, requireAdmin);
  registerOrderRoutes(router);
  registerUserRoutes(router);
}
