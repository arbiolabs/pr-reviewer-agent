export type Role = "admin" | "member";
export type Req = { user?: { id: string; role: Role }; params: Record<string, string>; body: unknown };
export type Res = { status(code: number): Res; json(body: unknown): void; end(): void };
export type Handler = (req: Req, res: Res, next: () => void) => void;

export const requireUser: Handler = (req, res, next) => {
  if (!req.user) return res.status(401).end();
  next();
};

export const requireAdmin: Handler = (req, res, next) => {
  if (req.user?.role !== "admin") return res.status(403).end();
  next();
};
