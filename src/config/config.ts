import { existsSync, readFileSync, writeFileSync } from "node:fs";
import type { ReviewMode, Severity } from "../domain/types.ts";
import { templateProblem } from "../publish/template.ts";

/** Spec §16, the parts M1 reads, plus the verification thresholds §7 asks to make configurable. */
export type Config = {
  review: {
    /** `approval` stores the payload and parks the revision in AWAITING_APPROVAL until `pr-review approve` or the TUI approves it. */
    publish_mode: "comment" | "dry_run" | "silent_no_findings" | "approval";
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
    /** The review summary's `{{variable}}` template (src/publish/template.ts); empty means the built-in default. */
    summary_template: string;
    /** The engine-mode daemon polls GitHub for PRs that request the selected account's review and starts them. */
    auto_review: AutoReview;
    /** Repository files with instructions for the reviewer, read from the PR's base commit; [] reads none. */
    guidance_files: string[];
    /** Globs for lock files, generated and vendored code: the reviewer is told they changed and skips them. */
    generated_paths: string[];
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
  control: {
    /** Let the daemon accept review.start, review.retry, review.cancel and review.publish from the TUI (TUI_SPEC §15.2). */
    allow_commands: boolean;
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
    summary_template: "",
    auto_review: { enabled: false, interval_minutes: 5, include_team_requests: false, repos: { include: [], exclude: [] }, daily_cap: 20 },
    guidance_files: ["AGENTS.md", "CLAUDE.md", ".github/copilot-instructions.md"],
    generated_paths: [
      "**/package-lock.json",
      "**/npm-shrinkwrap.json",
      "**/pnpm-lock.yaml",
      "**/yarn.lock",
      "**/bun.lock",
      "**/bun.lockb",
      "**/Cargo.lock",
      "**/go.sum",
      "**/poetry.lock",
      "**/uv.lock",
      "**/Pipfile.lock",
      "**/Gemfile.lock",
      "**/composer.lock",
      "**/Podfile.lock",
      "**/pubspec.lock",
      "**/mix.lock",
      "**/*.pb.go",
      "**/*_pb2.py",
      "**/*_pb2.pyi",
      "**/*.pb.swift",
      "**/*.generated.*",
      "**/__generated__/**",
      "**/*.min.js",
      "**/*.min.css",
      "**/*.js.map",
      "**/*.css.map",
      "**/vendor/**",
      "**/node_modules/**",
    ],
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
  control: { allow_commands: false },
};

export type AutoReview = {
  enabled: boolean;
  interval_minutes: number;
  /** Also PRs that request a team the account is on, not only the account itself. */
  include_team_requests: boolean;
  /** `owner/name`, matched exactly but for case; an empty include list means every repository. */
  repos: { include: string[]; exclude: string[] };
  /** Automatic starts allowed per local day (the daemon's time zone); 0 means no cap. Manual reviews never count. */
  daily_cap: number;
};

export const AUTO_REVIEW_INTERVALS = [5, 15, 30] as const;
export const REPO_NAME = /^[\w.-]+\/[\w.-]+$/;

export function autoReviewProblem(a: AutoReview): string | undefined {
  if (!(AUTO_REVIEW_INTERVALS as readonly number[]).includes(a.interval_minutes)) return `interval_minutes must be 5, 15 or 30, not ${a.interval_minutes}`;
  if (!Number.isInteger(a.daily_cap) || a.daily_cap < 0) return `daily_cap must be a whole number, 0 for no cap, not ${a.daily_cap}`;
  const bad = [...a.repos.include, ...a.repos.exclude].find((r) => !REPO_NAME.test(r));
  return bad === undefined ? undefined : `repos must list owner/name, not ${JSON.stringify(bad)}`;
}

/** Overlay `pr-review.config.json` (or `PR_REVIEW_CONFIG`) onto the defaults; every key must exist in the defaults with the same JSON type. */
export const CONFIG_FILE = "pr-review.config.json";

export const configPath = (env: Record<string, string | undefined> = process.env) => env.PR_REVIEW_CONFIG || CONFIG_FILE;

/** A fresh copy each time: the daemon changes its own in place when settings.set saves a summary template. */
export function loadConfig(path = configPath()): Config {
  if (!existsSync(path)) return structuredClone(DEFAULT_CONFIG);
  const config = mergeChecked(structuredClone(DEFAULT_CONFIG), JSON.parse(readFileSync(path, "utf8")), path) as Config;
  const problem = templateProblem(config.review.summary_template);
  if (problem) throw new Error(`config ${path}.review.summary_template: ${problem}`);
  const auto = autoReviewProblem(config.review.auto_review);
  if (auto) throw new Error(`config ${path}.review.auto_review: ${auto}`);
  return config;
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

export type ConfigPatch = { [K in keyof Config]?: Partial<Config[K]> };

/** Merge `patch` into the config file, keeping whatever else it holds. */
export function saveConfig(patch: ConfigPatch, path = configPath()): void {
  const current: unknown = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : {};
  writeFileSync(path, `${JSON.stringify(mergePatch(current, patch), null, 2)}\n`);
}

function mergePatch(base: unknown, patch: unknown): unknown {
  const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
  if (!isObject(base) || !isObject(patch)) return patch;
  const result = { ...base };
  for (const [key, value] of Object.entries(patch)) result[key] = mergePatch(base[key], value);
  return result;
}
