// Runs the real CLI entry in its own process with a scripted model, so a test can SIGKILL it mid-ANALYZE.
import { createModels, fauxAssistantMessage, fauxProvider } from "@earendil-works/pi-ai";
import { main } from "../../src/cli/main.ts";
import type { Fixture } from "../fixture.ts";
import { FakePullSource } from "../fixture.ts";
import { readDiff, submitDefect } from "../helpers.ts";

const fixture = JSON.parse(process.env.FIXTURE_JSON!) as Fixture;
const faux = fauxProvider({ provider: "faux", models: [{ id: "reviewer", reasoning: true }] });
faux.setResponses(
  process.env.MODE === "block"
    ? [
        readDiff(),
        (_context, options) =>
          new Promise((resolve) => {
            process.stderr.write("IN_FLIGHT\n");
            options?.signal?.addEventListener("abort", () => resolve(fauxAssistantMessage("", { stopReason: "aborted" })));
          }),
      ]
    : [submitDefect(fixture)],
);
const models = createModels();
models.setProvider(faux.provider);
process.exitCode = await main(["review", "https://github.com/acme/shop/pull/1", "--dry-run", "--no-jev", "--model", "faux/reviewer"], {
  pulls: new FakePullSource(fixture.pull),
  models,
  isTTY: false,
});
