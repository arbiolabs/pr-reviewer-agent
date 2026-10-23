import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { parseDiff } from "../src/source/diff.ts";
import { assertSafePath, PinnedRepo, syncRepo } from "../src/source/git.ts";
import { createFixture, DEFECT, type Fixture } from "./fixture.ts";

let fixture: Fixture;
let repo: PinnedRepo;

beforeAll(async () => {
  fixture = createFixture();
  const dir = await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/shop", fixture.repoDir, 1);
  repo = new PinnedRepo(dir, fixture.baseSha, fixture.headSha);
  await repo.assertCommitsPresent();
});

describe("pinned repo", () => {
  it("lists changed files and anchors only head-side hunk lines", async () => {
    expect(await repo.changedFiles()).toEqual([{ path: DEFECT.file, status: "M" }]);
    const file = parseDiff(await repo.diff()).get(DEFECT.file)!;
    expect([...file.headLines].sort((a, b) => a - b)).toEqual([2, 3, 4, 5, 6]);
  });

  it("reads files at a pinned SHA, not the working tree", async () => {
    expect(await repo.show(fixture.baseSha, DEFECT.file)).not.toContain("app.delete");
    expect((await repo.show(fixture.headSha, DEFECT.file))?.split("\n")[4]).toBe('  app.delete("/orders/:id", deleteOrder);');
    expect(await repo.show(fixture.headSha, "missing.ts")).toBe(undefined);
    expect(await repo.show("0".repeat(40), DEFECT.file)).toBe(undefined);
  });

  it("rethrows git failures other than a missing path, so the phase retries instead of discarding", async () => {
    const gone = new PinnedRepo(join(tmpdir(), "no-such-repo.git"), fixture.baseSha, fixture.headSha);
    await expect(gone.show(fixture.headSha, DEFECT.file)).rejects.toThrow("cannot change to");
  });

  it("searches the head tree with literal queries and scopes", async () => {
    expect(await repo.grep("requireAdmin", "src/routes")).toBe(
      'src/routes/orders.ts:1:import { requireAdmin } from "../auth/require-admin.ts";\nsrc/routes/orders.ts:4:  app.get("/orders", requireAdmin, listOrders);\n',
    );
    expect(await repo.grep("(", ":(top)")).toBe("");
  });

  it("stops git when the review that asked is cancelled, so a shutdown never waits on a search", async () => {
    const stopped = AbortSignal.abort();
    await expect(repo.grep("requireAdmin", undefined, undefined, stopped)).rejects.toMatchObject({ name: "AbortError" });
    await expect(repo.show(fixture.headSha, DEFECT.file, { signal: stopped })).rejects.toMatchObject({ name: "AbortError" });
    await expect(repo.log(DEFECT.file, 5, stopped)).rejects.toMatchObject({ name: "AbortError" });
    expect(await repo.grep("requireAdmin", "src/routes", undefined, new AbortController().signal)).toContain("requireAdmin");
  });

  it("fetches a blob-less clone's missing blobs for a search in one batch, not one round trip each", async () => {
    const origin = mkdtempSync(join(tmpdir(), "origin-"));
    const sh = (dir: string, ...args: string[]) => execFileSync("git", ["-C", dir, "-c", "user.name=F", "-c", "user.email=f@x", ...args], { encoding: "utf8" }).trim();
    sh(origin, "init", "-q");
    mkdirSync(join(origin, "src"));
    for (let i = 0; i < 40; i++) writeFileSync(join(origin, "src", `f${i}.ts`), `export const tel${i} = "tel:${i}";\n`);
    writeFileSync(join(origin, "README.md"), "outside the scope\n");
    sh(origin, "add", ".");
    sh(origin, "commit", "-q", "-m", "files");
    sh(origin, "config", "uploadpack.allowFilter", "true");
    const head = sh(origin, "rev-parse", "HEAD");
    sh(origin, "update-ref", "refs/pull/1/head", head);
    // A file:// clone honours --filter=blob:none, so it starts with no blobs at all.
    const dir = await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/lazy", `file://${origin}`, 1);
    // Every fetch writes its own pack, so the pack count is the number of round trips.
    const packs = () => readdirSync(join(dir, "objects", "pack")).filter((f) => f.endsWith(".pack")).length;
    const missing = (scope: string) => {
      const blobs = sh(dir, "ls-tree", "-r", "--object-only", head, "--", scope).split("\n");
      const out = execFileSync("git", ["-C", dir, "cat-file", "--batch-check"], { input: `${blobs.join("\n")}\n`, encoding: "utf8", env: { ...process.env, GIT_NO_LAZY_FETCH: "1" } });
      return out.split("\n").filter((l) => l.endsWith(" missing")).length;
    };
    expect(missing("src")).toBe(40);
    const before = packs();
    const lazy = new PinnedRepo(dir, head, head);
    expect((await lazy.grep("tel:", "src")).split("\n").filter(Boolean)).toHaveLength(40);
    expect(missing("src")).toBe(0);
    expect(packs() - before).toBe(1);
    expect(missing("README.md")).toBe(1);
  });

  it("rejects traversal, absolute and NUL paths before running git", async () => {
    await expect(repo.show(fixture.headSha, "../../etc/passwd")).rejects.toThrow("path traversal rejected: ../../etc/passwd");
    expect(() => assertSafePath("/etc/passwd")).toThrow("absolute path rejected: /etc/passwd");
    expect(() => assertSafePath("src/a\0b")).toThrow("path must be non-empty and contain no NUL");
    expect(assertSafePath("./src/x.ts")).toBe("src/x.ts");
  });
});

describe("GitHub token on clone and fetch", () => {
  const token = "ghs_secretTokenValue123";
  const basic = Buffer.from(`x-access-token:${token}`).toString("base64");

  it("sends the token as a per-command header and keeps it out of errors", async () => {
    const seen: (string | undefined)[] = [];
    const server = createServer((req, res) => {
      seen.push(req.headers.authorization);
      res.writeHead(404).end();
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const { port } = server.address() as AddressInfo;
    try {
      const error = await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/private", `http://127.0.0.1:${port}/acme/private.git`, 1, token).then(
        () => undefined,
        (e: Error) => e,
      );
      expect(seen[0]).toBe(`Basic ${basic}`);
      expect(error?.message).toContain("Authorization: Basic ***");
      expect(error?.message).not.toContain(token);
      expect(error?.message).not.toContain(basic);
    } finally {
      server.close();
    }
  });

  it("never writes the token into the bare repository's config", async () => {
    const dir = await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/shop", fixture.repoDir, 1, token);
    const config = readFileSync(join(dir, "config"), "utf8");
    expect(config).not.toContain(token);
    expect(config).not.toContain(basic);
  });
});
