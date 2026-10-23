import { existsSync } from "node:fs";
import type { CredentialStore, Models } from "@earendil-works/pi-ai";
import { configPath, DEFAULT_CONFIG, loadConfig } from "../config/config.ts";
import { findTui } from "../control/daemon.ts";
import { type Account, ACCOUNT_KEY, detectGh, type Env, listAccounts, planGithubAuth } from "../github/accounts.ts";
import { inKeychain, type KeychainItem, keychainItem } from "../jev/key.ts";
import { PiAuthFileStore } from "../models/credential-store.ts";
import { buildModels, which } from "../models/detect.ts";
import { chooseModel, listAvailableModels } from "../models/select.ts";
import { withStore } from "../persistence/store.ts";

export const NO_TOKEN = "no token, public repositories only";

/** The GitHub identity `planGithubAuth` picks from `list`, as display text. Never fetches a token. */
export function using(list: Account[]): { ok: boolean; text: string } {
  try {
    const plan = planGithubAuth(list);
    if (plan.source === "env") return { ok: true, text: `${plan.account.login} from the environment (overrides gh accounts)` };
    if (plan.source === "none") return { ok: true, text: NO_TOKEN };
    return { ok: true, text: `${plan.account.id} (${plan.source === "selected" ? "selected" : "gh active account"})` };
  } catch (error) {
    return { ok: false, text: error instanceof Error ? error.message : String(error) };
  }
}

/** `missing` means reviews cannot run; `warn` means they run with a limitation. */
export type Check = { id: "git" | "config" | "model" | "github" | "jev" | "tui"; status: "ok" | "warn" | "missing"; detail: string; fix?: string };

/** What a review needs, checked without prompting, fetching a GitHub token, or reading the TypeSafe key's value. */
export async function gatherChecks(
  deps: { models?: Models; credentials?: CredentialStore; inKeychain?: (item: KeychainItem) => Promise<boolean> },
  dataDir: string,
  env: Env = process.env,
): Promise<Check[]> {
  const checks: Check[] = [];
  const git = which("git", env.PATH ?? "");
  checks.push(git ? { id: "git", status: "ok", detail: git } : { id: "git", status: "missing", detail: "not on PATH", fix: "install git" });

  const path = configPath(env);
  let config = DEFAULT_CONFIG;
  try {
    config = loadConfig(path);
    checks.push({ id: "config", status: "ok", detail: existsSync(path) ? path : `defaults, no ${path}` });
  } catch (error) {
    checks.push({ id: "config", status: "missing", detail: error instanceof Error ? error.message : String(error) });
  }

  const available = await listAvailableModels(deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore(), env), config.models.preferred);
  const choice = chooseModel({ env: env.PR_REVIEW_MODEL, config: config.models.default }, available, false);
  if (choice.kind === "chosen") {
    const from = choice.source === "config" ? `from ${path}` : choice.source === "default" ? "first available" : `from ${choice.source}`;
    checks.push({ id: "model", status: "ok", detail: `${choice.ref} (${from})` });
  } else {
    if (choice.kind === "error" && available.length > 0) checks.push({ id: "model", status: "missing", detail: choice.message, fix: "run `pr-review setup` to pick an available model" });
    else checks.push({ id: "model", status: "missing", detail: "no models available", fix: "run `pr-review login anthropic`, or set ANTHROPIC_API_KEY" });
  }

  const detection = await detectGh(env);
  const use = using(await withStore(dataDir, (store) => listAccounts(detection, store.getMeta(ACCOUNT_KEY), env)));
  if (!use.ok) checks.push({ id: "github", status: "missing", detail: use.text });
  else if (use.text === NO_TOKEN) checks.push({ id: "github", status: "warn", detail: use.text, fix: "run `gh auth login`, or set GITHUB_TOKEN" });
  else checks.push({ id: "github", status: "ok", detail: use.text });

  const keyless = config.jev.allow_keyless_for_private;
  if (env.TYPESAFE_API_KEY) checks.push({ id: "jev", status: "ok", detail: "TypeSafe (TYPESAFE_API_KEY set)" });
  else if (await (deps.inKeychain ?? inKeychain)(keychainItem(env))) checks.push({ id: "jev", status: "ok", detail: "TypeSafe (key in Keychain)" });
  else
    checks.push({
      id: "jev",
      status: "warn",
      detail: keyless ? "OpenCode Zen free tier for every repository (jev.allow_keyless_for_private)" : "OpenCode Zen free tier for public repositories; private repositories skip Jev",
      fix: keyless ? "set TYPESAFE_API_KEY, or save the key in the desktop app's Settings, to use TypeSafe" : "set TYPESAFE_API_KEY, or save the key in the desktop app's Settings, to use TypeSafe; without either, serve and daemon --allow-commands skip Jev for every repository",
    });

  const tui = findTui(env.PATH ?? "");
  checks.push(tui ? { id: "tui", status: "ok", detail: tui } : { id: "tui", status: "warn", detail: "pr-review-tui not built (optional terminal client)", fix: "(cd tui && cargo build --release)" });
  return checks;
}
