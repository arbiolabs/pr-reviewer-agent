import { requireUser } from "../auth.ts";
import { db } from "../db.ts";
import type { Handler, Router } from "../http.ts";

const getProfile: Handler = (req, res) => {
  const profile = db.profiles.get(req.user!.id);
  if (!profile) return res.status(404).end();
  res.json(profile);
};

export function registerUserRoutes(router: Router) {
  router.get("/me", requireUser, getProfile);
}
