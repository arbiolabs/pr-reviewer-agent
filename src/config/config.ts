import { existsSync, readFileSync, writeFileSync } from "node:fs";
import type { ReviewMode, Severity } from "../domain/types.ts";

/** Spec §16, the parts M1 reads, plus the verification thresholds §7 asks to make configurable. */
export type Config = {
  review: {
    publish_mode: "comment" | "dry_run" | "silent_no_findings";
    on_jev_failure: ReviewMode;
    max_retries_per_phase: number;
    max_changed_files: number;
    max_diff_bytes: number;
    base_policy_version: string;
    jev_timeout_ms: number;
    retry_backoff_ms: number;
    /** A PR's review starts this long after its last triggering event; a later push restarts the wait. */
    debounce_seconds: number;
    /** A webhook delivery that keeps failing is retried with exponential backoff, then recorded dead. */
    webhook_max_attempts: number;
    webhook_backoff_ms: number;
  };
  /** §9.4 rate limits and retries. `max_retries` bounds each request; the client never retries an ambiguous write. */
  github: {
    max_retries: number;
    backoff_ms: number;
    concurrency_per_installation: number;
    timeout_ms: number;
    /** The longest rate-limit wait the worker loop sits through; a longer one fails the delivery, which backs off. */
    max_rate_limit_sleep_ms: number;
    /** Our reviews' author for token auth that cannot read `/user`, such as an installation token; else `PR_REVIEW_BOT_LOGIN`. */
    bot_login: string;
  };
  routing: {
    mandatory_deep_paths: string[];
    // ponytail: uncalibrated placeholders; replace with values measured on a labeled PR corpus (spec §6.3).
    jev_deep_threshold: number;
    jev_critical_threshold: number;
  };
  verification: {
    min_severity: Severity;
    min_confidence: number;
  };
  jev: {
    /** Let a private repository's diff go to the keyless OpenCode Zen tier when TYPESAFE_API_KEY is unset. */
    allow_keyless_for_private: boolean;
  };
  models: {
    /** `provider/id`; empty means pick one. */
    default: string;
    /** Ordered preference for picking among available models; `provider/*` matches a whole provider. */
    preferred: string[];
  };
};

export const DEFAULT_CONFIG: Config = {
  review: {
    publish_mode: "dry_run",
    on_jev_failure: "standard",
    max_retries_per_phase: 3,
    max_changed_files: 300,
    max_diff_bytes: 2_000_000,
    base_policy_version: "1",
    jev_timeout_ms: 15_000,
    retry_backoff_ms: 1000,
    debounce_seconds: 30,
    webhook_max_attempts: 6,
    webhook_backoff_ms: 5000,
  },
  github: { max_retries: 3, backoff_ms: 500, concurrency_per_installation: 2, timeout_ms: 15_000, max_rate_limit_sleep_ms: 30_000, bot_login: "" },
  routing: {
    mandatory_deep_paths: ["**/auth/**", "**/payments/**", "**/booking/**", "**/migrations/**"],
    jev_deep_threshold: 0.7,
    jev_critical_threshold: 0.9,
  },
  verification: { min_severity: "medium", min_confidence: 0.5 },
  jev: { allow_keyless_for_private: false },
  models: {
    default: "",
    preferred: ["anthropic/claude-opus-5-5", "anthropic/claude-fable-5-1", "anthropic/claude-sonnet-5-5", "openai/*"],
  },
};

/** Overlay `pr-review.config.json` (or `PR_REVIEW_CONFIG`) onto the defaults; every key must exist in the defaults with the same JSON type. */
export const CONFIG_FILE = "pr-review.config.json";

export function loadConfig(path = process.env.PR_REVIEW_CONFIG || CONFIG_FILE): Config {
  if (!existsSync(path)) return DEFAULT_CONFIG;
  return mergeChecked(DEFAULT_CONFIG, JSON.parse(readFileSync(path, "utf8")), path) as Config;
}

function mergeChecked(base: unknown, override: unknown, at: string): unknown {
  const kind = (v: unknown) => (Array.isArray(v) ? "array" : v === null ? "null" : typeof v);
  if (kind(base) !== kind(override)) throw new Error(`config ${at}: expected ${kind(base)}, got ${kind(override)}`);
  if (kind(base) !== "object") return override;
  const result: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [key, value] of Object.entries(override as Record<string, unknown>)) {
    if (!(key in result)) throw new Error(`config ${at}: unknown key ${key}`);
    result[key] = mergeChecked(result[key], value, `${at}.${key}`);
  }
  return result;
}

/** Persist one model choice into the config file, keeping whatever else it holds. */
export function saveDefaultModel(ref: string, path = CONFIG_FILE): void {
  const current = existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as Record<string, Record<string, unknown>>) : {};
  current.models = { ...current.models, default: ref };
  writeFileSync(path, `${JSON.stringify(current, null, 2)}\n`);
}
