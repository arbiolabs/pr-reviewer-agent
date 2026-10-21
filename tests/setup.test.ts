import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { createModels } from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { gatherChecks } from "../src/cli/checks.ts";
import { main } from "../src/cli/main.ts";
import { AppDbSource } from "../src/control/appdb.ts";
import { lockDataDir } from "../src/runtime/app.ts";
import { saveConfig } from "../src/config/config.ts";
import { which } from "../src/models/detect.ts";
import { fakeGh } from "./fake-gh.ts";
import { fauxModels } from "./helpers.ts";

let dir: string;
let configPath: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "setup-"));
  configPath = join(dir, "pr-review.config.json");
  const git = which("git", process.env.PATH ?? "")!;
  vi.stubEnv("PATH", [fakeGh().dir, dirname(git)].join(delimiter));
  vi.stubEnv("GITHUB_TOKEN", "");
  vi.stubEnv("GH_TOKEN", "");
  vi.stubEnv("PR_REVIEW_MODEL", "");
  vi.stubEnv("TYPESAFE_API_KEY", "");
  vi.stubEnv("PR_REVIEW_DATA_DIR", join(dir, "data"));
  vi.stubEnv("PR_REVIEW_CONFIG", configPath);
});
afterEach(() => vi.unstubAllEnvs());

async function run(args: string[], models = fauxModels([]), isTTY?: boolean) {
  let out = "";
  const code = await main(["setup", ...args], { models, isTTY, out: (t) => (out += t), err: (t) => (out += t) });
  return { code, lines: out.split("\n") };
}

it("passes with a model, a gh account and git, and names the model and where it came from", async () => {
  writeFileSync(configPath, JSON.stringify({ models: { default: "faux/reviewer" } }));
  const { code, lines } = await run(["--check"]);
  expect(code).toBe(0);
  expect(lines).toContain(`  ✓ config  ${configPath}`);
  expect(lines).toContain(`  ✓ model   faux/reviewer (from ${configPath})`);
  expect(lines).toContain("  ✓ github  github.com/diego-arbio-dev (gh active account)");
  expect(lines.some((l) => l.startsWith("  ✓ git     "))).toBe(true);
});

it("fails without model credentials and says how to log in", async () => {
  const { code, lines } = await run(["--check"], createModels());
  expect(code).toBe(1);
  expect(lines).toContain("  ✗ model   no models available");
  expect(lines).toContain("            run `pr-review login anthropic`, or set ANTHROPIC_API_KEY");
  expect(lines).toContain("  ✓ config  defaults, no " + configPath);
});

it("reports a config file that does not load", async () => {
  writeFileSync(configPath, JSON.stringify({ nope: 1 }));
  const { code, lines } = await run(["--check"]);
  expect(code).toBe(1);
  expect(lines).toContain(`  ✗ config  config ${configPath}: unknown key nope`);
});

it("warns without TYPESAFE_API_KEY, and with it set never prints the key", async () => {
  const keyless = await run(["--check"]);
  expect(keyless.code).toBe(0);
  expect(keyless.lines).toContain("  ! jev     OpenCode Zen free tier for public repositories; private repositories skip Jev");
  vi.stubEnv("TYPESAFE_API_KEY", "tsk-very-secret-value");
  const keyed = await run(["--check"]);
  expect(keyed.code).toBe(0);
  expect(keyed.lines).toContain("  ✓ jev     TypeSafe (TYPESAFE_API_KEY set)");
  expect(keyed.lines.join("\n")).not.toContain("tsk-very-secret-value");
  const json = await run(["--check", "--json"]);
  expect(json.lines.join("\n")).not.toContain("tsk-very-secret-value");
});

it("prints the checks as JSON", async () => {
  const { code, lines } = await run(["--check", "--json"], createModels());
  expect(code).toBe(1);
  const { checks } = JSON.parse(lines.join("\n"));
  expect(checks.map((c: { id: string }) => c.id)).toEqual(["git", "config", "model", "github", "jev", "tui"]);
  expect(checks[2]).toEqual({ id: "model", status: "missing", detail: "no models available", fix: "run `pr-review login anthropic`, or set ANTHROPIC_API_KEY" });
  expect(checks[3]).toEqual({ id: "github", status: "ok", detail: "github.com/diego-arbio-dev (gh active account)" });
});

it("checks only, without prompting, when not on a terminal", async () => {
  const { code, lines } = await run([], fauxModels([]), false);
  expect(code).toBe(0);
  expect(lines).toContain("  ✓ model   faux/reviewer (first available)");
  expect(lines).toContain("setup: not a terminal, so checking only (same as setup --check)");
});

it("saveConfig merges a nested patch and keeps every other key", () => {
  writeFileSync(configPath, JSON.stringify({ review: { publish_mode: "comment", debounce_seconds: 5 }, models: { preferred: ["openai/*"] } }));
  saveConfig({ models: { default: "faux/reviewer" } });
  saveConfig({ review: { publish_mode: "approval" } });
  expect(JSON.parse(readFileSync(configPath, "utf8"))).toEqual({
    review: { publish_mode: "approval", debounce_seconds: 5 },
    models: { preferred: ["openai/*"], default: "faux/reviewer" },
  });
});

it("fails when models.default is not available and points at setup instead of login", async () => {
  writeFileSync(configPath, JSON.stringify({ models: { default: "openai/nope" } }));
  const { code, lines } = await run(["--check"]);
  expect(code).toBe(1);
  expect(lines).toContain("  ✗ model   model openai/nope (from config) is not available; available: faux/reviewer");
  expect(lines).toContain("            run `pr-review setup` to pick an available model");
});

it("checks a data dir that a running daemon holds locked and open", async () => {
  const data = join(dir, "data");
  const source = AppDbSource.open(data);
  const release = await lockDataDir(data);
  try {
    const checks = await gatherChecks({ models: fauxModels([]) }, data);
    expect(checks.find((c) => c.id === "github")).toEqual({ id: "github", status: "ok", detail: "github.com/diego-arbio-dev (gh active account)" });
  } finally {
    await release();
    source.close();
  }
});
