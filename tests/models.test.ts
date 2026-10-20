import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { detectModels } from "../src/models/detect.ts";
import { type AvailableModel, chooseModel } from "../src/models/select.ts";

function fakeEnvironment(auth: object | undefined) {
  const dir = mkdtempSync(join(tmpdir(), "detect-"));
  writeFileSync(join(dir, "pi"), "#!/bin/sh\nexit 0\n");
  chmodSync(join(dir, "pi"), 0o755);
  const authFile = join(dir, "auth.json");
  if (auth) writeFileSync(authFile, JSON.stringify(auth));
  return { pathVar: dir, authFile, env: {}, preferred: DEFAULT_CONFIG.models.preferred };
}

it("marks pi usable and lists its anthropic models when pi's auth file has a credential", async () => {
  const { harnesses, models } = await detectModels(fakeEnvironment({ anthropic: { type: "api_key", key: "sk-test" } }));
  expect(harnesses.find((h) => h.name === "pi")).toMatchObject({ installed: true, usable: true, note: "shares anthropic (api_key)" });
  expect(harnesses.find((h) => h.name === "codex")).toEqual({ name: "codex", installed: false, usable: false, note: "not on PATH" });
  expect(models[0]!.ref).toBe("anthropic/claude-opus-5-5");
  expect(models.map((m) => m.ref)).toContain("anthropic/claude-haiku-5-5");
  expect(new Set(models.map((m) => m.provider))).toEqual(new Set(["anthropic"]));
});

it("reports pi installed but not usable without an auth file, and lists no models", async () => {
  const { harnesses, models } = await detectModels(fakeEnvironment(undefined));
  expect(harnesses.find((h) => h.name === "pi")).toMatchObject({ installed: true, usable: false });
  expect(models).toEqual([]);
});

const available = ["anthropic/claude-opus-5-5", "anthropic/claude-haiku-5-5"].map(
  (ref): AvailableModel => ({ ref, provider: "anthropic", id: ref.split("/")[1]!, name: ref, contextWindow: 1, reasoning: true, authSource: "OAuth" }),
);

it("chooses the model by flag, then env, then config, then the first preferred without a TTY", () => {
  const all = { flag: "anthropic/claude-haiku-5-5", env: "anthropic/claude-opus-5-5", config: "anthropic/claude-opus-5-5" };
  expect(chooseModel(all, available, false)).toEqual({ kind: "chosen", ref: "anthropic/claude-haiku-5-5", source: "--model" });
  expect(chooseModel({ ...all, flag: undefined, env: "anthropic/claude-haiku-5-5" }, available, false)).toEqual({ kind: "chosen", ref: "anthropic/claude-haiku-5-5", source: "PR_REVIEW_MODEL" });
  expect(chooseModel({ config: "anthropic/claude-haiku-5-5" }, available, false)).toEqual({ kind: "chosen", ref: "anthropic/claude-haiku-5-5", source: "config" });
  expect(chooseModel({}, available, false)).toEqual({ kind: "chosen", ref: "anthropic/claude-opus-5-5", source: "default" });
  expect(chooseModel({}, available, true)).toEqual({ kind: "pick" });
});

it("rejects an unavailable model with the available list, and an empty list with login advice", () => {
  expect(chooseModel({ flag: "openai/gpt-6" }, available, false)).toEqual({
    kind: "error",
    message: "model openai/gpt-6 (from --model) is not available; available: anthropic/claude-opus-5-5, anthropic/claude-haiku-5-5",
  });
  expect(chooseModel({}, [], false)).toMatchObject({ kind: "error", message: expect.stringContaining("pr-review login anthropic") });
});
