import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createModels, fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { main } from "../src/cli/main.ts";
import { AppStore } from "../src/persistence/store.ts";
import { createFixture } from "./fixture.ts";

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
