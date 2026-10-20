import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { PiAuthFileStore } from "../src/models/credential-store.ts";

const tempAuth = () => join(mkdtempSync(join(tmpdir(), "auth-")), "auth.json");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

it("lists provider ids and types only, and an absent file is an empty store", async () => {
  const path = tempAuth();
  expect(await new PiAuthFileStore(path).list()).toEqual([]);
  writeFileSync(path, JSON.stringify({ anthropic: { type: "oauth", access: "sk-secret", refresh: "rt-secret", expires: 1 }, openai: { type: "api_key", key: "sk-2" } }));
  const listed = await new PiAuthFileStore(path).list();
  expect(listed).toEqual([
    { providerId: "anthropic", type: "oauth" },
    { providerId: "openai", type: "api_key" },
  ]);
  expect(JSON.stringify(listed)).not.toContain("secret");
});

it("serializes concurrent modifies so both changes survive, and creates the file 0600", async () => {
  const path = tempAuth();
  const store = new PiAuthFileStore(path);
  const order: string[] = [];
  const slow = (id: string, key: string) =>
    store.modify(id, async () => {
      order.push(`start ${id}`);
      await sleep(50);
      order.push(`end ${id}`);
      return { type: "api_key", key };
    });
  await Promise.all([slow("anthropic", "a"), slow("openai", "o")]);
  expect(order).toEqual(["start anthropic", "end anthropic", "start openai", "end openai"]);
  expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({ anthropic: { type: "api_key", key: "a" }, openai: { type: "api_key", key: "o" } });
  expect(statSync(path).mode & 0o777).toBe(0o600);
});

it("waits for a lock held by another process the way pi takes it, then re-reads that process's write", async () => {
  const path = tempAuth();
  writeFileSync(path, "{}");
  const lockfilePath = createRequire(import.meta.url).resolve("proper-lockfile");
  const holder = spawn(process.execPath, [
    "-e",
    `const l = require(${JSON.stringify(lockfilePath)}); const fs = require("fs");
     l.lock(${JSON.stringify(path)}, { realpath: false, stale: 30000 }).then(async (release) => {
       console.log("locked");
       await new Promise((r) => setTimeout(r, 400));
       fs.writeFileSync(${JSON.stringify(path)}, JSON.stringify({ anthropic: { type: "oauth", access: "new", refresh: "rotated", expires: 2 } }));
       await release();
     });`,
  ]);
  await new Promise<void>((resolve) => holder.stdout.once("data", () => resolve()));
  const seen = await new PiAuthFileStore(path).modify("openai", async (current) => {
    expect(current).toBe(undefined);
    return { type: "api_key", key: "o" };
  });
  expect(seen).toEqual({ type: "api_key", key: "o" });
  expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({
    anthropic: { type: "oauth", access: "new", refresh: "rotated", expires: 2 },
    openai: { type: "api_key", key: "o" },
  });
}, 10_000);

it("refreshes the lock mtime often enough that pi's 10 s sync-path stale check never steals it", async () => {
  const path = tempAuth();
  const mtime = () => statSync(`${path}.lock`).mtimeMs;
  let refreshedAfter = Number.NaN;
  await new PiAuthFileStore(path).modify("openai", async () => {
    // Both times are the lock's own mtimes, so a late timer or a slow stat in this process cannot skew the interval.
    const taken = mtime();
    while (mtime() === taken) await sleep(50);
    refreshedAfter = mtime() - taken;
    return { type: "api_key", key: "o" };
  });
  expect(refreshedAfter).toBeLessThan(4500);
}, 15_000);
