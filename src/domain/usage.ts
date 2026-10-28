/** One recorded model response, as `model_usage` holds it: `at` is an ISO instant, `model` is provider/id, cost in USD. */
export type UsageRow = {
  revisionId: string;
  at: string;
  model: string;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
};

export const USAGE_BY = ["day", "week", "month"] as const;
export type UsageBy = (typeof USAGE_BY)[number];

/** How many buckets a report covers when the caller does not say. */
export const DEFAULT_PERIODS: Record<UsageBy, number> = { day: 14, week: 12, month: 12 };

/** `reviews` counts distinct revisions; every other count is a sum over responses. */
export type UsageTotals = { reviews: number; responses: number; input: number; output: number; cacheRead: number; cacheWrite: number; cost: number };
/** `start` and `end` are local calendar dates (YYYY-MM-DD), both inside the bucket. */
export type UsageBucket = { start: string; end: string } & UsageTotals;
export type UsageModel = { model: string } & UsageTotals;
export type UsageReport = { by: UsageBy; timeZone: string; buckets: UsageBucket[]; totals: UsageTotals; models: UsageModel[] };

export type UsageOptions = { by: UsageBy; tz: string; now: number; periods?: number };

const DAY_MS = 86_400_000;

/** The calendar date `ms` falls on in `tz`, as YYYY-MM-DD. */
export function localDate(ms: number, tz: string): string {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", { timeZone: tz, year: "numeric", month: "2-digit", day: "2-digit" }).formatToParts(ms).map((p) => [p.type, p.value]),
  );
  return `${parts.year}-${parts.month}-${parts.day}`;
}

// Calendar arithmetic on plain dates, done in UTC so no clock change can move a date.
const toMs = (date: string) => Date.parse(`${date}T00:00:00Z`);
const toDate = (ms: number) => new Date(ms).toISOString().slice(0, 10);
const addDays = (date: string, n: number) => toDate(toMs(date) + n * DAY_MS);
const addMonths = (date: string, n: number) => {
  const d = new Date(toMs(date));
  return toDate(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + n, 1));
};

/** The first date of the `by` bucket `date` is in; weeks start on Monday (ISO 8601). */
function bucketStart(date: string, by: UsageBy): string {
  if (by === "day") return date;
  if (by === "month") return `${date.slice(0, 7)}-01`;
  const weekday = (new Date(toMs(date)).getUTCDay() + 6) % 7;
  return addDays(date, -weekday);
}

const step = (start: string, by: UsageBy, n: number) => (by === "month" ? addMonths(start, n) : addDays(start, by === "week" ? 7 * n : n));

/** The first local date the report covers. */
export function usageStart({ by, tz, now, periods = DEFAULT_PERIODS[by] }: UsageOptions): string {
  return step(bucketStart(localDate(now, tz), by), by, -(periods - 1));
}

/** An instant no later than the report's first local midnight in any time zone: what to read rows from. */
export function usageSince(options: UsageOptions): string {
  return new Date(toMs(usageStart(options)) - DAY_MS).toISOString();
}

const zero = (): UsageTotals => ({ reviews: 0, responses: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 });

type Tally = { totals: UsageTotals; revisions: Set<string> };
const tally = (): Tally => ({ totals: zero(), revisions: new Set() });

function add(t: Tally, row: UsageRow) {
  t.revisions.add(row.revisionId);
  t.totals.responses += 1;
  t.totals.input += row.input;
  t.totals.output += row.output;
  t.totals.cacheRead += row.cacheRead;
  t.totals.cacheWrite += row.cacheWrite;
  t.totals.cost += row.cost;
}

// Sums of float dollars carry noise past the sixth decimal; nothing priced per token needs more.
const settle = (t: Tally): UsageTotals => ({ ...t.totals, reviews: t.revisions.size, cost: Math.round(t.totals.cost * 1e6) / 1e6 });

/**
 * Token usage and cost over the last `periods` buckets up to the one `now` is in, in `tz`'s calendar, oldest first.
 * Every bucket is listed, empty ones included. Rows outside the range are ignored.
 */
export function usageReport(rows: readonly UsageRow[], options: UsageOptions): UsageReport {
  const { by, tz, periods = DEFAULT_PERIODS[by] } = options;
  if (!Number.isInteger(periods) || periods < 1) throw new RangeError(`periods must be a positive integer, got ${periods}`);
  const first = usageStart(options);
  const buckets = new Map<string, Tally>();
  for (let i = 0; i < periods; i++) buckets.set(step(first, by, i), tally());
  const all = tally();
  const models = new Map<string, Tally>();
  for (const row of rows) {
    const bucket = buckets.get(bucketStart(localDate(Date.parse(row.at), tz), by));
    if (!bucket) continue;
    add(bucket, row);
    add(all, row);
    if (!models.has(row.model)) models.set(row.model, tally());
    add(models.get(row.model)!, row);
  }
  return {
    by,
    timeZone: tz,
    buckets: [...buckets].map(([start, t]) => ({ start, end: addDays(step(start, by, 1), -1), ...settle(t) })),
    totals: settle(all),
    models: [...models]
      .map(([model, t]) => ({ model, ...settle(t) }))
      .sort((a, b) => b.cost - a.cost || b.input + b.output - (a.input + a.output) || (a.model < b.model ? -1 : 1)),
  };
}
