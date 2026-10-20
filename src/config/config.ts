import { existsSync, readFileSync } from "node:fs";
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
  },
  routing: {
    mandatory_deep_paths: ["**/auth/**", "**/payments/**", "**/booking/**", "**/migrations/**"],
    jev_deep_threshold: 0.7,
    jev_critical_threshold: 0.9,
  },
  verification: { min_severity: "medium", min_confidence: 0.5 },
};

/** Overlay `pr-review.config.json` onto the defaults; every key must exist in the defaults with the same JSON type. */
export function loadConfig(path = "pr-review.config.json"): Config {
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
