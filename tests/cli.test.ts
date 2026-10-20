import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createModels, fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { AppStore } from "../src/persistence/store.ts";
import { createFixture } from "./fixture.ts";
import { fauxModels, readDiff, submitDefect } from "./helpers.ts";

const SCRIPT = new URL("./fixtures/cli-faux.ts", import.meta.url).pathname;

function runCli(env: Record<string, string>) {
  const child = spawn(process.execPath, [SCRIPT], { env: { ...process.env, ...env } });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (d) => (stdout += d));
  child.stderr.on("data", (d) => (stderr += d));
  const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)));
  return { child, exited, output: () => ({ stdout, stderr }) };
}

it("survives SIGKILL of the CLI process mid-ANALYZE and completes on rerun", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "kill-"));
  const env = { FIXTURE_JSON: JSON.stringify(fixture), PR_REVIEW_DATA_DIR: dataDir };

  const first = runCli({ ...env, MODE: "block" });
  await new Promise<void>((resolve) => first.child.stderr.on("data", (d) => String(d).includes("IN_FLIGHT") && resolve()));
  first.child.kill("SIGKILL");
  expect(await first.exited).toBe(null);
  const killed = AppStore.open(join(dataDir, "app.sqlite"));
  expect(killed.db.prepare("SELECT state FROM review_revisions").all()).toEqual([{ state: "ANALYZE" }]);
  killed.close();

  const second = runCli({ ...env, MODE: "finish" });
  expect(await second.exited).toBe(0);
  expect(second.output().stdout).toContain("- High: Order deletion endpoint skips the admin check (`src/routes/orders.ts:5`)");
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  expect(store.db.prepare("SELECT count(*) AS n FROM candidate_sets").get()).toEqual({ n: 1 });
  expect(store.db.prepare("SELECT count(*) AS n FROM findings").get()).toEqual({ n: 1 });
  store.close();
}, 60_000);

it("treats the model as part of reviewer identity: two models give two revisions", async () => {
  const fixture = createFixture();
  const dataDir = mkdtempSync(join(tmpdir(), "identity-"));
  const faux = fauxProvider({ provider: "faux", models: [{ id: "one", reasoning: true }, { id: "two", reasoning: true }] });
  const empty = () => fauxAssistantMessage([fauxToolCall("submit_candidates", { candidates: [] })], { stopReason: "toolUse" });
  faux.setResponses([empty(), empty()]);
  const models = createModels();
  models.setProvider(faux.provider);
  process.env.PR_REVIEW_DATA_DIR = dataDir;
  const quiet = { pulls: fixture.pulls, models, isTTY: false, out: () => {}, err: () => {} };
  try {
    for (const model of ["faux/one", "faux/two", "faux/one"]) {
      expect(await main(["review", "https://github.com/acme/shop/pull/1", "--no-jev", "--model", model], quiet)).toBe(0);
    }
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
  }
  const store = AppStore.open(join(dataDir, "app.sqlite"));
  expect(store.db.prepare("SELECT reviewer_version FROM review_revisions ORDER BY reviewer_version").all()).toEqual([
    { reviewer_version: "m1+faux/one" },
    { reviewer_version: "m1+faux/two" },
  ]);
  store.close();
}, 60_000);

it("refuses --publish in M1", async () => {
  const errors: string[] = [];
  expect(await main(["review", "https://github.com/acme/shop/pull/1", "--publish"], { err: (t) => errors.push(t) })).toBe(2);
  expect(errors).toEqual(["pr-review: publishing lands in M2; run with --dry-run\n"]);
});

it("strips control characters from model text in Markdown and inspect output, and keeps JSON raw", async () => {
  const fixture = createFixture();
  const title = "Order deletion\u001b[2J\u009b31m skips the admin check\r\u0007";
  const message = submitDefect(fixture);
  const call = message.content.find((c) => c.type === "toolCall")!;
  (call.arguments as { candidates: { title: string }[] }).candidates[0]!.title = title;
  process.env.PR_REVIEW_DATA_DIR = mkdtempSync(join(tmpdir(), "control-"));
  const outputs: string[] = [];
  const deps = { pulls: fixture.pulls, models: fauxModels([readDiff(), message]), isTTY: false, out: (t: string) => outputs.push(t), err: (t: string) => outputs.push(t) };
  const review = ["review", "https://github.com/acme/shop/pull/1", "--no-jev", "--model", "faux/reviewer"];
  try {
    expect(await main(review, deps)).toBe(0);
    expect(await main(["inspect", "--pr", "acme/shop#1"], deps)).toBe(0);
    const text = outputs.join("");
    expect(text).toContain("- High: Order deletion[2J31m skips the admin check (`src/routes/orders.ts:5`)");
    expect(text).not.toMatch(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/);
    outputs.length = 0;
    expect(await main([...review, "--json"], { ...deps, err: () => {} })).toBe(0);
    expect(JSON.parse(outputs.join("")).findings[0].title).toBe(title);
  } finally {
    delete process.env.PR_REVIEW_DATA_DIR;
  }
}, 60_000);
