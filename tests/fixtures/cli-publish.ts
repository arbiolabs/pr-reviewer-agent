// Runs a CLI command (default `review --publish`) in its own process against a fake GitHub; KILL makes it SIGKILL
// itself at a review POST. CLI_ARGS (a JSON array) replaces the default arguments, for `approve`.
import { main } from "../../src/cli/main.ts";
import { GitHubClient } from "../../src/github/client.ts";
import type { Fixture } from "../fixture.ts";
import { fauxModels, readDiff, submitDefect } from "../helpers.ts";

const fixture = JSON.parse(process.env.FIXTURE_JSON!) as Fixture;
const client = new GitHubClient({ auth: { kind: "token", token: "ghp_test" }, baseUrl: process.env.FAKE_URL!, maxRetries: 1, backoffMs: 1, concurrency: 2, timeoutMs: 5000 });
const request = client.request.bind(client);
client.request = (async (method: string, path: string, options?: Parameters<typeof request>[2]) => {
  const post = method === "POST" && path.endsWith("/reviews");
  if (post && process.env.KILL === "before-post") process.kill(process.pid, "SIGKILL");
  const result = await request(method, path, options);
  if (post && process.env.KILL === "after-response") process.kill(process.pid, "SIGKILL");
  return result;
}) as typeof client.request;
const args = process.env.CLI_ARGS ? (JSON.parse(process.env.CLI_ARGS) as string[]) : ["review", "https://github.com/acme/shop/pull/1", "--publish", "--no-jev", "--model", "faux/reviewer"];
process.exitCode = await main(args, {
  github: client,
  models: fauxModels([readDiff(), submitDefect(fixture)]),
  isTTY: false,
});
