import { chmodSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** gh's `auth status --json hosts` shape, tokenSource included: it can be a file path and must never leave the parser. */
export const GH_STATUS = {
  hosts: {
    "github.com": [
      { state: "success", active: true, host: "github.com", login: "diego-arbio-dev", tokenSource: "keyring", scopes: "gist, read:org, repo, workflow", gitProtocol: "https" },
      { state: "success", active: false, host: "github.com", login: "octo-work", tokenSource: "/Users/x/.config/gh/hosts.yml", scopes: "repo", gitProtocol: "https" },
      { state: "error", error: "The token in keyring is invalid.", active: false, host: "github.com", login: "old-bot", tokenSource: "keyring", gitProtocol: "https" },
    ],
    "ghe.acme.com": [{ state: "success", active: true, host: "ghe.acme.com", login: "diego", tokenSource: "keyring", scopes: "repo", gitProtocol: "ssh" }],
  },
};

export const FAKE_TOKEN = "ghp_FAKEtoken0123456789";

/**
 * A `gh` on its own PATH dir. `auth status` prints status.json and exits with exit-code; `auth token` prints
 * FAKE_TOKEN plus the user it was asked for. Every invocation is appended to calls.log.
 */
export function fakeGh(status: unknown = GH_STATUS, exitCode = 0) {
  const dir = mkdtempSync(join(tmpdir(), "fake-gh-"));
  const set = (s: unknown, code = exitCode) => {
    writeFileSync(join(dir, "status.json"), typeof s === "string" ? s : JSON.stringify(s));
    writeFileSync(join(dir, "exit-code"), String(code));
  };
  set(status);
  writeFileSync(
    join(dir, "gh"),
    `#!/bin/sh
echo "$*" >> "${dir}/calls.log"
echo "GITHUB_TOKEN=\${GITHUB_TOKEN-unset}" >> "${dir}/env.log"
if [ "$1 $2" = "auth status" ]; then /bin/cat "${dir}/status.json"; exit $(/bin/cat "${dir}/exit-code"); fi
if [ "$1 $2" = "auth token" ]; then
  if [ "$5" = "--user" ]; then echo "${FAKE_TOKEN}-$6"; else echo "${FAKE_TOKEN}-active"; fi
  exit 0
fi
exit 2
`,
  );
  chmodSync(join(dir, "gh"), 0o755);
  return { dir, set, envLog: () => readFileSync(join(dir, "env.log"), "utf8"), calls: () => (existsSync(join(dir, "calls.log")) ? readFileSync(join(dir, "calls.log"), "utf8").trimEnd().split("\n") : []) };
}
