import { accessSync, constants, existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import type { AuthContext, CredentialStore, MutableModels } from "@earendil-works/pi-ai";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { PiAuthFileStore, ReadOnlyCredentialStore } from "./credential-store.ts";
import { type AvailableModel, listAvailableModels } from "./select.ts";

export type HarnessInfo = { name: string; installed: boolean; path?: string; usable: boolean; note: string };

const NOT_SHARED = "credentials not shared; run `pr-review login <provider>`";

/** Agent CLIs we know about. Only pi shares its credential file; the others keep tokens we must never read or refresh. */
const KNOWN_HARNESSES: readonly { name: string; bin: string; sharesCredentials: boolean }[] = [
  { name: "pi", bin: "pi", sharesCredentials: true },
  { name: "claude", bin: "claude", sharesCredentials: false },
  { name: "codex", bin: "codex", sharesCredentials: false },
  { name: "opencode", bin: "opencode", sharesCredentials: false },
  { name: "gemini", bin: "gemini", sharesCredentials: false },
  { name: "copilot", bin: "copilot", sharesCredentials: false },
  { name: "cursor-agent", bin: "cursor-agent", sharesCredentials: false },
];

function which(bin: string, pathVar: string): string | undefined {
  for (const dir of pathVar.split(delimiter).filter(Boolean)) {
    const candidate = join(dir, bin);
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {}
  }
  return undefined;
}

export type DetectOptions = {
  pathVar?: string;
  authFile?: string;
  env?: Record<string, string | undefined>;
  preferred: readonly string[];
};

function authContext(env: Record<string, string | undefined>): AuthContext {
  return {
    env: async (name) => env[name],
    fileExists: async (path) => existsSync(path.startsWith("~") ? join(homedir(), path.slice(1)) : path),
  };
}

/** Models over pi's credential file plus ambient env-var credentials. */
export function buildModels(credentials: CredentialStore, env: Record<string, string | undefined> = process.env): MutableModels {
  return builtinModels({ credentials, authContext: authContext(env) });
}

/** Read-only by construction: the store refuses writes, so detection cannot refresh or rotate any token. */
export async function detectModels(options: DetectOptions): Promise<{ harnesses: HarnessInfo[]; models: AvailableModel[] }> {
  const store = new ReadOnlyCredentialStore(new PiAuthFileStore(options.authFile));
  const stored = await store.list();
  const harnesses = KNOWN_HARNESSES.map(({ name, bin, sharesCredentials }): HarnessInfo => {
    const path = which(bin, options.pathVar ?? process.env.PATH ?? "");
    if (path === undefined) return { name, installed: false, usable: false, note: "not on PATH" };
    if (!sharesCredentials) return { name, installed: true, path, usable: false, note: NOT_SHARED };
    return stored.length > 0
      ? { name, installed: true, path, usable: true, note: `shares ${stored.map((c) => `${c.providerId} (${c.type})`).join(", ")}` }
      : { name, installed: true, path, usable: false, note: "no credential in pi's auth file; run `pr-review login <provider>`" };
  });
  const models = await listAvailableModels(buildModels(store, options.env), options.preferred);
  return { harnesses, models };
}
