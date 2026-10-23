import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createModels } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { gatherChecks } from "../src/cli/checks.ts";
import { type KeychainItem, typeSafeKey } from "../src/jev/key.ts";

const ITEM = { service: "io.github.diego-arbio-dev.pr-review", account: "TYPESAFE_API_KEY" };

function keychain(secret: string | undefined) {
  const asked: KeychainItem[] = [];
  return { asked, read: async (item: KeychainItem) => (asked.push(item), secret) };
}

it("takes TYPESAFE_API_KEY from the environment first, without asking the Keychain", async () => {
  const kc = keychain("from-keychain");
  expect(await typeSafeKey({ TYPESAFE_API_KEY: "from-env" }, kc.read)).toEqual({ key: "from-env", source: "env" });
  expect(kc.asked).toEqual([]);
});

it("falls back to the Keychain item, then to no key", async () => {
  const found = keychain("from-keychain");
  expect(await typeSafeKey({ TYPESAFE_API_KEY: "" }, found.read)).toEqual({ key: "from-keychain", source: "keychain" });
  expect(found.asked).toEqual([ITEM]);
  expect(await typeSafeKey({}, keychain(undefined).read)).toBeUndefined();
});

it("reads the service PR_REVIEW_KEYCHAIN_SERVICE names, so tests never touch the real item", async () => {
  const kc = keychain(undefined);
  await typeSafeKey({ PR_REVIEW_KEYCHAIN_SERVICE: "prsa.test" }, kc.read);
  expect(kc.asked).toEqual([{ service: "prsa.test", account: "TYPESAFE_API_KEY" }]);
});

it("reports a Keychain key in the setup check, and the environment's ahead of it", async () => {
  const dir = mkdtempSync(join(tmpdir(), "jev-key-"));
  const jev = async (env: Record<string, string>, present: boolean) =>
    (await gatherChecks({ models: createModels(), inKeychain: async () => present }, dir, { PATH: "", PR_REVIEW_CONFIG: join(dir, "none.json"), ...env })).find((c) => c.id === "jev");
  expect(await jev({}, true)).toEqual({ id: "jev", status: "ok", detail: "TypeSafe (key in Keychain)" });
  expect(await jev({ TYPESAFE_API_KEY: "x" }, true)).toEqual({ id: "jev", status: "ok", detail: "TypeSafe (TYPESAFE_API_KEY set)" });
  expect((await jev({}, false))?.status).toBe("warn");
});
