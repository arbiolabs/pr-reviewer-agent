import { expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { isGenerated, loadGuidance } from "../src/reasoning/guidance.ts";
import { focusFiles } from "../src/reasoning/tools.ts";
import { BlobTooLargeError } from "../src/source/git.ts";

const files = (tree: Record<string, string>) => async (path: string) => {
  if (path === "huge.md") throw new BlobTooLargeError("over");
  return tree[path];
};

it("reads the guidance files in order, skipping missing, symlinked, repeated and oversized ones", async () => {
  const read = files({ "AGENTS.md": "Use pnpm.\nNo default exports.\n", "CLAUDE.md": "AGENTS.md", "COPY.md": "Use pnpm.\nNo default exports." });
  const got = await loadGuidance(read, ["missing.md", "AGENTS.md", "CLAUDE.md", "COPY.md", "huge.md"]);
  expect(got).toEqual([{ path: "AGENTS.md", text: "Use pnpm.\nNo default exports.", truncated: false }]);
});

it("cuts the files to one shared line budget", async () => {
  const read = files({ "a.md": "1\n2\n3", "b.md": "4\n5\n6", "c.md": "7" });
  expect(await loadGuidance(read, ["a.md", "b.md", "c.md"], 4)).toEqual([
    { path: "a.md", text: "1\n2\n3", truncated: false },
    { path: "b.md", text: "4", truncated: true },
  ]);
});

it("passes other read failures on", async () => {
  await expect(loadGuidance(async () => Promise.reject(new Error("git died")), ["AGENTS.md"])).rejects.toThrow("git died");
});

it("treats lock files, generated and vendored code as generated, and leaves source and migrations alone", () => {
  const globs = DEFAULT_CONFIG.review.generated_paths;
  for (const path of ["pnpm-lock.yaml", "web/package-lock.json", "Cargo.lock", "go.sum", "api/user_pb2.py", "src/schema.generated.ts", "app/__generated__/q.ts", "public/app.min.js", "vendor/github.com/x/y.go"]) {
    expect(isGenerated(path, globs), path).toBe(true);
  }
  for (const path of ["src/lock.ts", "migrations/001_init.sql", "package.json", "src/vendors.ts", "docs/generated-code.md"]) {
    expect(isGenerated(path, globs), path).toBe(false);
  }
});

it("keeps only focus files the PR changes, not generated ones, each once, on one line, at most three", () => {
  const changed = ["a.ts", "b.ts", "c.ts", "d.ts", "pnpm-lock.yaml"].map((path) => ({ path }));
  const focus = [
    { file: "pnpm-lock.yaml", reason: "lock" },
    { file: "elsewhere.ts", reason: "not in the PR" },
    { file: "a.ts", reason: "first\n  line" },
    { file: "a.ts", reason: "again" },
    { file: "b.ts", reason: "b" },
    { file: "c.ts", reason: "c" },
    { file: "d.ts", reason: "d" },
  ];
  expect(focusFiles(focus, changed, ["**/pnpm-lock.yaml"])).toEqual([
    { file: "a.ts", reason: "first line" },
    { file: "b.ts", reason: "b" },
    { file: "c.ts", reason: "c" },
  ]);
});
