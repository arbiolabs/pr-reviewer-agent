import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { which } from "../models/detect.ts";
import type { Account } from "../protocol/v1.ts";

export type { Account };
export type Env = Record<string, string | undefined>;

/** `meta` key holding this data dir's selected account, as `<host>/<login>`. */
export const ACCOUNT_KEY = "github.account";
/** The only host pr-review talks to; a token for another host must never be sent to api.github.com. */
export const GITHUB_HOST = "github.com";

/** What gh reports, before the selection is applied. `selected` is always false here. */
export type GhDetection = { installed: false } | { installed: true; path: string; accounts: Account[] };

type GhStatus = { hosts?: Record<string, { state?: string; active?: boolean; host?: string; login?: string; scopes?: string }[]> };

const run = promisify(execFile);

/** The env tokens, in the order they win. Either one is the daemon's env pseudo-account `env/<name>`. */
const ENV_TOKENS = ["GITHUB_TOKEN", "GH_TOKEN"] as const;

function envToken(env: Env): { name: (typeof ENV_TOKENS)[number]; token: string } | undefined {
  for (const name of ENV_TOKENS) {
    const token = env[name];
    if (token) return { name, token };
  }
  return undefined;
}

/** gh reads GH_TOKEN and GITHUB_TOKEN before its stored accounts; strip both so gh lists and hands out only those. */
const ghEnv = (env: Env): Env => ({ ...env, GITHUB_TOKEN: undefined, GH_TOKEN: undefined });

/**
 * `gh auth status --json hosts`, which never prints a token. gh exits 1 when any account has an auth problem but still
 * prints the JSON, so stdout is parsed whatever the exit code.
 */
export async function detectGh(env: Env = process.env): Promise<GhDetection> {
  const path = which("gh", env.PATH ?? "");
  if (path === undefined) return { installed: false };
  let stdout: string;
  try {
    ({ stdout } = await run(path, ["auth", "status", "--json", "hosts"], { env: ghEnv(env), timeout: 5000 }));
  } catch (error) {
    stdout = String((error as { stdout?: unknown }).stdout ?? "");
  }
  let status: GhStatus;
  try {
    status = JSON.parse(stdout) as GhStatus;
  } catch {
    return { installed: true, path, accounts: [] };
  }
  const accounts = Object.entries(status.hosts ?? {}).flatMap(([host, entries]) =>
    entries.flatMap((a): Account[] =>
      a.login
        ? [
            {
              id: `${a.host ?? host}/${a.login}`,
              host: a.host ?? host,
              login: a.login,
              state: a.state === "success" ? "ok" : "error",
              ghActive: a.active === true,
              selected: false,
              scopes: (a.scopes ?? "").split(",").map((s) => s.trim()).filter(Boolean),
            },
          ]
        : [],
    ),
  );
  return { installed: true, path, accounts };
}

/**
 * The account list clients see: the env pseudo-account first when GITHUB_TOKEN or GH_TOKEN is set, then gh's accounts
 * with the selection marked. A selection gh no longer lists stays in the list as an error, so its absence is visible.
 */
export function listAccounts(detection: GhDetection, selected: string | undefined, env: Env = process.env): Account[] {
  const gh = (detection.installed ? detection.accounts : []).map((a) => ({ ...a, selected: a.id === selected }));
  const name = envToken(env)?.name;
  const envAccount: Account[] = name ? [{ id: `env/${name}`, host: "env", login: name, state: "ok", ghActive: false, selected: false, scopes: [] }] : [];
  const slash = selected?.indexOf("/") ?? -1;
  const vanished: Account[] =
    selected !== undefined && !gh.some((a) => a.selected)
      ? [{ id: selected, host: selected.slice(0, slash), login: selected.slice(slash + 1), state: "error", ghActive: false, selected: true, scopes: [] }]
      : [];
  return [...envAccount, ...gh, ...vanished];
}

export type AuthPlan = { source: "env" | "selected" | "active"; account: Account } | { source: "none" };

/** Precedence: GITHUB_TOKEN > GH_TOKEN > the selected account > gh's active github.com account > none. Never falls back past a bad selection. */
export function planGithubAuth(accounts: Account[]): AuthPlan {
  const env = accounts.find((a) => a.host === "env");
  if (env) return { source: "env", account: env };
  const selected = accounts.find((a) => a.selected);
  if (selected) {
    if (selected.state !== "ok") {
      throw new Error(`selected GitHub account ${selected.id} is not usable in gh; run \`gh auth login --hostname ${selected.host}\` or \`pr-review accounts use <account>\``);
    }
    return { source: "selected", account: selected };
  }
  const active = accounts.find((a) => a.ghActive && a.host === GITHUB_HOST && a.state === "ok");
  return active ? { source: "active", account: active } : { source: "none" };
}

export type GithubAuth = { token: string | undefined; source: AuthPlan["source"]; account?: string };

/** The one place a GitHub token is resolved. The token is returned to the caller only: never logged or stored. */
export async function resolveGithubAuth(store: { getMeta(key: string): string | undefined }, env: Env = process.env): Promise<GithubAuth> {
  const fromEnv = envToken(env);
  if (fromEnv) return { token: fromEnv.token, source: "env", account: `env/${fromEnv.name}` };
  const detection = await detectGh(env);
  const plan = planGithubAuth(listAccounts(detection, store.getMeta(ACCOUNT_KEY), env));
  if (plan.source === "none" || plan.source === "env" || !detection.installed) return { token: undefined, source: "none" };
  const { host, login, id } = plan.account;
  const { stdout } = await run(detection.path, ["auth", "token", "--hostname", host, "--user", login], { env: ghEnv(env), timeout: 5000 });
  const token = stdout.trim();
  if (!token) throw new Error(`gh returned no token for ${id}; run \`gh auth login --hostname ${host}\``);
  return { token, source: plan.source, account: id };
}

export type Choice = { ok: true; account: Account } | { ok: false; code: "NOT_FOUND" | "CONFLICT"; message: string };

/** Validate a selection against accounts detected just now. `requested` is `<host>/<login>`, or a login unique across hosts. */
export function chooseAccount(detection: GhDetection, requested: string, env: Env = process.env): Choice {
  if (!detection.installed) return { ok: false, code: "NOT_FOUND", message: "gh is not installed; install the GitHub CLI and run `gh auth login`" };
  const matches = detection.accounts.filter((a) => a.id === requested || (!requested.includes("/") && a.login === requested));
  if (matches.length === 0) return { ok: false, code: "NOT_FOUND", message: `gh has no account ${requested}; see \`pr-review accounts\`` };
  if (matches.length > 1) return { ok: false, code: "CONFLICT", message: `${requested} is logged in on several hosts; name one of ${matches.map((a) => a.id).join(", ")}` };
  const account = matches[0]!;
  if (account.host !== GITHUB_HOST) return { ok: false, code: "CONFLICT", message: `${account.id} is not a ${GITHUB_HOST} account; pr-review only talks to ${GITHUB_HOST}` };
  if (account.state !== "ok") return { ok: false, code: "CONFLICT", message: `${account.id} has an auth problem in gh; run \`gh auth login --hostname ${account.host}\`` };
  const fromEnv = envToken(env);
  if (fromEnv) return { ok: false, code: "CONFLICT", message: `${fromEnv.name} is set and overrides any selected account; unset it first` };
  return { ok: true, account };
}
