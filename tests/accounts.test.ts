import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { main } from "../src/cli/main.ts";
import { ACCOUNT_KEY, chooseAccount, detectGh, listAccounts, resolveGithubAuth } from "../src/github/accounts.ts";
import { AppStore } from "../src/persistence/store.ts";
import { FAKE_TOKEN, fakeGh, GH_STATUS } from "./fake-gh.ts";

const meta = (selected?: string) => ({ getMeta: (key: string) => (key === ACCOUNT_KEY ? selected : undefined) });
const PERSONAL = { id: "github.com/diego-arbio-dev", host: "github.com", login: "diego-arbio-dev", state: "ok", ghActive: true, selected: false, scopes: ["gist", "read:org", "repo", "workflow"] };
const WORK = { id: "github.com/octo-work", host: "github.com", login: "octo-work", state: "ok", ghActive: false, selected: false, scopes: ["repo"] };
const OLD_BOT = { id: "github.com/old-bot", host: "github.com", login: "old-bot", state: "error", ghActive: false, selected: false, scopes: [] };
const GHE = { id: "ghe.acme.com/diego", host: "ghe.acme.com", login: "diego", state: "ok", ghActive: true, selected: false, scopes: ["repo"] };

describe("detectGh", () => {
  it("parses every account on every host, maps state, splits scopes and drops tokenSource", async () => {
    const gh = fakeGh();
    expect(await detectGh({ PATH: gh.dir })).toEqual({ installed: true, path: join(gh.dir, "gh"), accounts: [PERSONAL, WORK, OLD_BOT, GHE] });
    expect(gh.calls()).toEqual(["auth status --json hosts"]);
  });

  it("still parses stdout when gh exits 1 because one account has an auth problem", async () => {
    const gh = fakeGh(GH_STATUS, 1);
    expect(await detectGh({ PATH: gh.dir })).toEqual({ installed: true, path: join(gh.dir, "gh"), accounts: [PERSONAL, WORK, OLD_BOT, GHE] });
  });

  it("reports gh missing, and installed with no accounts when its output is not JSON", async () => {
    expect(await detectGh({ PATH: mkdtempSync(join(tmpdir(), "no-gh-")) })).toEqual({ installed: false });
    const gh = fakeGh("You are not logged into any GitHub hosts. To log in, run: gh auth login", 1);
    expect(await detectGh({ PATH: gh.dir })).toEqual({ installed: true, path: join(gh.dir, "gh"), accounts: [] });
  });

  it("hides GITHUB_TOKEN from gh so it lists only its own accounts", async () => {
    const gh = fakeGh();
    await detectGh({ PATH: gh.dir, GITHUB_TOKEN: "env-secret" });
    expect(gh.envLog()).toBe("GITHUB_TOKEN=unset\n");
  });
});

describe("resolveGithubAuth precedence", () => {
  it("env GITHUB_TOKEN > selected > gh active (github.com) > none", async () => {
    const gh = fakeGh();
    expect(await resolveGithubAuth(meta("github.com/octo-work"), { PATH: gh.dir, GITHUB_TOKEN: "env-token" })).toEqual({ token: "env-token", source: "env", account: "env/GITHUB_TOKEN" });
    expect(await resolveGithubAuth(meta("github.com/octo-work"), { PATH: gh.dir })).toEqual({ token: `${FAKE_TOKEN}-octo-work`, source: "selected", account: "github.com/octo-work" });
    expect(await resolveGithubAuth(meta(), { PATH: gh.dir })).toEqual({ token: `${FAKE_TOKEN}-diego-arbio-dev`, source: "active", account: "github.com/diego-arbio-dev" });
    expect(await resolveGithubAuth(meta(), { PATH: mkdtempSync(join(tmpdir(), "no-gh-")) })).toEqual({ token: undefined, source: "none" });
    gh.set({ hosts: { "ghe.acme.com": GH_STATUS.hosts["ghe.acme.com"] } });
    expect(await resolveGithubAuth(meta(), { PATH: gh.dir })).toEqual({ token: undefined, source: "none" });
    expect(gh.calls().filter((c) => c.startsWith("auth token"))).toEqual(["auth token --hostname github.com --user octo-work", "auth token --hostname github.com --user diego-arbio-dev"]);
  });

  it("fails on a selected account that vanished or is in error, and never falls back to another identity", async () => {
    const gh = fakeGh();
    await expect(resolveGithubAuth(meta("github.com/gone"), { PATH: gh.dir })).rejects.toThrow("selected GitHub account github.com/gone is not usable in gh; run `gh auth login --hostname github.com`");
    await expect(resolveGithubAuth(meta("github.com/old-bot"), { PATH: gh.dir })).rejects.toThrow("github.com/old-bot");
    await expect(resolveGithubAuth(meta("github.com/octo-work"), { PATH: mkdtempSync(join(tmpdir(), "no-gh-")) })).rejects.toThrow("github.com/octo-work");
    expect(gh.calls().filter((c) => c.startsWith("auth token"))).toEqual([]);
  });
});

describe("listAccounts and chooseAccount", () => {
  it("lists the env pseudo-account first and keeps a vanished selection as an error", async () => {
    const detection = await detectGh({ PATH: fakeGh().dir });
    expect(listAccounts(detection, "github.com/gone", { GITHUB_TOKEN: "t" }).map((a) => [a.id, a.state, a.selected])).toEqual([
      ["env/GITHUB_TOKEN", "ok", false],
      [PERSONAL.id, "ok", false],
      [WORK.id, "ok", false],
      [OLD_BOT.id, "error", false],
      [GHE.id, "ok", false],
      ["github.com/gone", "error", true],
    ]);
  });

  it("accepts a github.com account in state ok by id or unique login, and refuses the rest", async () => {
    const detection = await detectGh({ PATH: fakeGh().dir });
    expect(chooseAccount(detection, "octo-work", {})).toEqual({ ok: true, account: WORK });
    expect(chooseAccount(detection, "github.com/octo-work", {})).toEqual({ ok: true, account: WORK });
    expect(chooseAccount(detection, "github.com/nobody", {})).toMatchObject({ ok: false, code: "NOT_FOUND" });
    expect(chooseAccount(detection, "env/GITHUB_TOKEN", {})).toMatchObject({ ok: false, code: "NOT_FOUND" });
    expect(chooseAccount(detection, "github.com/old-bot", {})).toMatchObject({ ok: false, code: "CONFLICT" });
    expect(chooseAccount(detection, "ghe.acme.com/diego", {})).toMatchObject({ ok: false, code: "CONFLICT", message: expect.stringContaining("only talks to github.com") });
    expect(chooseAccount(detection, "octo-work", { GITHUB_TOKEN: "t" })).toMatchObject({ ok: false, code: "CONFLICT", message: expect.stringContaining("GITHUB_TOKEN") });
    expect(chooseAccount({ installed: false }, "octo-work", {})).toMatchObject({ ok: false, code: "NOT_FOUND" });
  });
});

describe("pr-review accounts", () => {
  afterEach(() => vi.unstubAllEnvs());

  it("`accounts use` stores the selection and journals ACCOUNT_SELECTED; the list then marks it", async () => {
    const gh = fakeGh();
    const dataDir = mkdtempSync(join(tmpdir(), "accounts-cli-"));
    vi.stubEnv("PATH", gh.dir);
    vi.stubEnv("GITHUB_TOKEN", "");
    vi.stubEnv("PR_REVIEW_DATA_DIR", dataDir);
    let out = "";
    const io = { out: (t: string) => (out += t), err: (t: string) => (out += t) };

    expect(await main(["accounts", "use", "octo-work"], io)).toBe(0);
    expect(out).toBe(`selected github.com/octo-work for GitHub reads and publishing in ${dataDir}\n`);
    const store = AppStore.open(join(dataDir, "app.sqlite"));
    expect(store.getMeta(ACCOUNT_KEY)).toBe("github.com/octo-work");
    expect(store.db.prepare("SELECT revision_id, from_state, event, to_state, detail FROM audit_events").all()).toEqual([
      { revision_id: null, from_state: "-", event: "ACCOUNT_SELECTED", to_state: "-", detail: "github.com/octo-work" },
    ]);
    store.close();

    out = "";
    expect(await main(["accounts"], io)).toBe(0);
    expect(out).toBe(
      [
        `gh: ${join(gh.dir, "gh")}`,
        "  github.com/diego-arbio-dev               ok     active          gist, read:org, repo, workflow",
        "  github.com/octo-work                     ok            selected repo",
        "  github.com/old-bot                       error",
        "  ghe.acme.com/diego                       ok     active          repo",
        "using: github.com/octo-work (selected)",
        "",
      ].join("\n"),
    );

    out = "";
    expect(await main(["accounts", "use", "ghe.acme.com/diego"], io)).toBe(1);
    expect(out).toContain("only talks to github.com");
    expect(gh.calls().some((c) => c.includes("token"))).toBe(false);
  });
});
