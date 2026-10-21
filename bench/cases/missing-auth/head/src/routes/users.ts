import { type Handler, type Role, requireAdmin, requireUser } from "../auth.ts";
import { db } from "../db.ts";

type Router = Record<"get" | "put" | "delete", (path: string, ...handlers: Handler[]) => void>;

const getProfile: Handler = (req, res) => res.json(db.users.get(req.user!.id));

const listUsers: Handler = (_req, res) => res.json([...db.users.values()]);

const deleteUser: Handler = (req, res) => {
  db.users.delete(req.params.id!);
  res.status(204).end();
};

const setRole: Handler = (req, res) => {
  const { role } = req.body as { role: Role };
  const user = db.users.get(req.params.id!);
  if (!user) return res.status(404).end();
  user.role = role;
  res.json(user);
};

export function registerUserRoutes(router: Router) {
  router.get("/me", requireUser, getProfile);
  router.get("/users", requireUser, requireAdmin, listUsers);
  router.delete("/users/:id", requireUser, requireAdmin, deleteUser);
  router.put("/users/:id/role", requireUser, setRole);
}
