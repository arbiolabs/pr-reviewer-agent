export type Table<S extends string, E extends string> = Record<S, Partial<Record<E, S>>>;

export type TransitionResult<S extends string> = { ok: true; state: S } | { ok: false; reason: string };

export function transition<S extends string, E extends string>(table: Table<S, E>, state: S, event: E): TransitionResult<S> {
  const next = table[state][event];
  return next === undefined ? { ok: false, reason: `${event} is not legal in ${state}` } : { ok: true, state: next };
}
