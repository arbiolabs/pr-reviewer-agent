import type { Role } from "./auth.ts";

export type User = { id: string; email: string; role: Role };

export const db = { users: new Map<string, User>() };
