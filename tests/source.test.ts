import { mkdtempSync } from "node:fs";
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
  });

  it("searches the head tree with literal queries and scopes", async () => {
    expect(await repo.grep("requireAdmin", "src/routes")).toBe(
      'src/routes/orders.ts:1:import { requireAdmin } from "../auth/require-admin.ts";\nsrc/routes/orders.ts:4:  app.get("/orders", requireAdmin, listOrders);\n',
    );
    expect(await repo.grep("(", ":(top)")).toBe("");
  });

  it("rejects traversal, absolute and NUL paths before running git", async () => {
    await expect(repo.show(fixture.headSha, "../../etc/passwd")).rejects.toThrow("path traversal rejected: ../../etc/passwd");
    expect(() => assertSafePath("/etc/passwd")).toThrow("absolute path rejected: /etc/passwd");
    expect(() => assertSafePath("src/a\0b")).toThrow("path must be non-empty and contain no NUL");
    expect(assertSafePath("./src/x.ts")).toBe("src/x.ts");
  });
});
