import { execFile } from "node:child_process";

export type KeychainItem = { service: string; account: string };
export type TypeSafeKey = { key: string; source: "env" | "keychain" };

/**
 * The generic password the desktop app saves the TypeSafe key as, in the login keychain. PR_REVIEW_KEYCHAIN_SERVICE
 * renames the service so a test never touches the operator's real item.
 */
export const keychainItem = (env: Record<string, string | undefined> = process.env): KeychainItem => ({
  service: env.PR_REVIEW_KEYCHAIN_SERVICE || "io.github.diego-arbio-dev.pr-review",
  account: "TYPESAFE_API_KEY",
});

/**
 * `/usr/bin/security`, the program the desktop app writes the item through, so the item's access list already trusts
 * it and no read prompts. Only the item's name goes in argv; the secret comes back on stdout. macOS only: elsewhere,
 * and on any failure (no item, a locked keychain), there is no key.
 */
function security(args: string[]): Promise<{ ok: boolean; stdout: string }> {
  if (process.platform !== "darwin") return Promise.resolve({ ok: false, stdout: "" });
  return new Promise((resolve) => execFile("/usr/bin/security", args, { timeout: 5000 }, (error, stdout) => resolve({ ok: !error, stdout })));
}

export async function readKeychain(item: KeychainItem): Promise<string | undefined> {
  const { ok, stdout } = await security(["find-generic-password", "-s", item.service, "-a", item.account, "-w"]);
  return (ok && stdout.replace(/\n$/, "")) || undefined;
}

/** Whether the item exists, from its attributes alone: the secret is never read. */
export async function inKeychain(item: KeychainItem): Promise<boolean> {
  return (await security(["find-generic-password", "-s", item.service, "-a", item.account])).ok;
}

/** TYPESAFE_API_KEY from the environment wins; else the Keychain item. Looked up on every call, so a saved key applies to the next review. */
export async function typeSafeKey(env: Record<string, string | undefined> = process.env, read = readKeychain): Promise<TypeSafeKey | undefined> {
  if (env.TYPESAFE_API_KEY) return { key: env.TYPESAFE_API_KEY, source: "env" };
  const key = await read(keychainItem(env));
  return key ? { key, source: "keychain" } : undefined;
}
