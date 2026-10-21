import type { Router } from "./http.ts";
import { registerOrderRoutes } from "./routes/orders.ts";
import { registerSupportRoutes } from "./routes/support.ts";
import { registerUserRoutes } from "./routes/users.ts";

export function registerRoutes(router: Router) {
  registerOrderRoutes(router);
  registerSupportRoutes(router);
  registerUserRoutes(router);
}
