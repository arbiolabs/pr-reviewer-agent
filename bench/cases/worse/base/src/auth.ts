import type { Handler } from "./http.ts";

export const requireUser: Handler = (req, res, next) => {
  if (!req.user) return res.status(401).end();
  next();
};

export const requireAdmin: Handler = (req, res, next) => {
  if (!req.user?.isAdmin) return res.status(403).end();
  next();
};
