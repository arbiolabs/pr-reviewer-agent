import { type Handler, requireAdmin, requireUser } from "../auth.ts";
import { db } from "../db.ts";

type Router = Record<"get" | "put" | "delete", (path: string, ...handlers: Handler[]) => void>;

const getProfile: Handler = (req, res) => res.json(db.users.get(req.user!.id));

const listUsers: Handler = (_req, res) => res.json([...db.users.values()]);

const deleteUser: Handler = (req, res) => {
  db.users.delete(req.params.id!);
  res.status(204).end();
};

export function registerUserRoutes(router: Router) {
  router.get("/me", requireUser, getProfile);
  router.get("/users", requireUser, requireAdmin, listUsers);
  router.delete("/users/:id", requireUser, requireAdmin, deleteUser);
}
