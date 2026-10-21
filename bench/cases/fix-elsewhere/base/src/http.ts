export type User = { id: string; isAdmin: boolean };
export type Req = { user?: User; params: Record<string, string>; body: unknown };
export type Res = { status(code: number): Res; json(body: unknown): void; end(): void };
export type Handler = (req: Req, res: Res, next: () => void) => void | Promise<void>;
/** Express-style: `use` runs its handlers before any route whose path matches. */
export type Router = Record<"get" | "post" | "use", (path: string, ...handlers: Handler[]) => void>;
