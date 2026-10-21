import { requireUser } from "../auth.ts";
import { db } from "../db.ts";
import type { Handler, Router } from "../http.ts";

const getProfile: Handler = (req, res) => {
  const profile = db.profiles.get(req.user!.id);
  if (!profile) return res.status(404).end();
  res.json(profile);
};

const renameProfile: Handler = (req, res) => {
  const { name } = req.body as { name?: unknown };
  if (typeof name !== "string" || name.trim() === "") return res.status(400).end();
  const profile = db.profiles.get(req.user!.id);
  if (!profile) return res.status(404).end();
  profile.name = name.trim();
  res.json(profile);
};

export function registerUserRoutes(router: Router) {
  router.get("/me", requireUser, getProfile);
  router.post("/me/name", requireUser, renameProfile);
}
