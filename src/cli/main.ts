import { mkdirSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { parseArgs } from "node:util";
import type { AuthInteraction, CredentialStore, Models } from "@earendil-works/pi-ai";
import { createTypeSafeClient, createZenClient } from "jev-systemone";
import { loadConfig, saveDefaultModel } from "../config/config.ts";
import { runDaemon, runTui } from "../control/daemon.ts";
import { type Account, ACCOUNT_KEY, chooseAccount, detectGh, listAccounts, planGithubAuth, resolveGithubAuth } from "../github/accounts.ts";
import { GitHubPullSource, parsePullRef, parsePullUrl, type PullSource } from "../github/pulls.ts";
import { type DecisionEngine, JevDecisionEngine, type JevRoute, jevSkipReason } from "../jev/decision.ts";
import { PiAuthFileStore } from "../models/credential-store.ts";
import { buildModels, detectModels } from "../models/detect.ts";
import { chooseModel, listAvailableModels, splitRef } from "../models/select.ts";
import { AppStore } from "../persistence/store.ts";
import { finishReview, openApp, startReview } from "../runtime/app.ts";
import { loadReport, renderJson, renderMarkdown } from "./report.ts";

export const REVIEWER_CODE_VERSION = "m1";

/** Seams the tests replace: the GitHub read side, the model collection, and where state lives. */
export type CliDeps = {
  pulls?: PullSource;
  models?: Models;
  credentials?: CredentialStore;
  engine?: DecisionEngine;
  isTTY?: boolean;
  out?: (text: string) => void;
  err?: (text: string) => void;
};

const USAGE = `usage:
  pr-review review <pr-url> --dry-run [--json] [--no-jev] [--model provider/id] [--retry]
  pr-review inspect --pr owner/repo#123 [--json]
  pr-review models [--json]
  pr-review accounts [--json]
  pr-review accounts use <host/login | login>
  pr-review login [provider] [--api-key]
  pr-review daemon [--socket PATH] [--fixture NAME] [--data DIR] [--heartbeat SECONDS]
  pr-review tui [args for pr-review-tui]`;

/** Text output; `json` writes the value unchanged, so a JSON consumer gets the raw strings (JSON escapes C0 itself). */
type Out = ((text: string) => void) & { json(value: unknown): void };

/** C0 and C1 controls except newline and tab: model and repository text must not drive the terminal. */
export const stripControl = (text: string) => text.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, "");

export async function main(argv: string[], deps: CliDeps = {}): Promise<number> {
  const stdout = deps.out ?? ((t) => process.stdout.write(t));
  const stderr = deps.err ?? ((t) => process.stderr.write(t));
  // The single output boundary: every text line the CLI prints passes through stripControl here.
  const out: Out = Object.assign((t: string) => stdout(stripControl(t)), { json: (value: unknown) => stdout(`${JSON.stringify(value, null, 2)}\n`) });
  const err = (t: string) => stderr(stripControl(t));
  const [command, ...rest] = argv;
  const dataDir = process.env.PR_REVIEW_DATA_DIR || ".data";
  try {
    switch (command) {
      case "review":
        return await review(rest, deps, dataDir, out, err);
      case "inspect":
        return inspect(rest, dataDir, out);
      case "models":
        return await models(rest, out);
      case "accounts":
        return await accounts(rest, dataDir, out);
      case "login":
        return await login(rest, deps, out);
      case "daemon":
        return await runDaemon(rest, dataDir, err);
      case "tui":
        return runTui(rest);
      default:
        err(`${USAGE}\n`);
        return 2;
    }
  } catch (error) {
    err(`pr-review: ${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

async function review(args: string[], deps: CliDeps, dataDir: string, out: Out, err: (t: string) => void): Promise<number> {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: { "dry-run": { type: "boolean" }, publish: { type: "boolean" }, json: { type: "boolean" }, "no-jev": { type: "boolean" }, model: { type: "string" }, retry: { type: "boolean" } },
  });
  if (values.publish) {
    err("pr-review: publishing lands in M2; run with --dry-run\n");
    return 2;
  }
  if (positionals.length !== 1) {
    err(`${USAGE}\n`);
    return 2;
  }
  const config = loadConfig();
  const ref = parsePullUrl(positionals[0]!);
  // A test's fake pull source serves local repositories; only the real GitHub source needs the token.
  const auth = deps.pulls ? undefined : await withStore(dataDir, (store) => resolveGithubAuth(store));
  if (auth) err(`github: ${auth.account ?? "no token, public repositories only"} (${auth.source})\n`);
  const githubToken = auth?.token;
  const pulls = deps.pulls ?? new GitHubPullSource(githubToken);
  const pull = await pulls.getPull(ref);

  const models = deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore());
  const available = await listAvailableModels(models, config.models.preferred);
  const isTTY = deps.isTTY ?? Boolean(process.stdin.isTTY && process.stdout.isTTY);
  let choice = chooseModel({ flag: values.model, env: process.env.PR_REVIEW_MODEL, config: config.models.default }, available, isTTY);
  if (choice.kind === "error") throw new Error(choice.message);
  if (choice.kind === "pick") choice = { kind: "chosen", ref: await pickModel(available.map((m) => `${m.ref}  (${m.authSource})`), available.map((m) => m.ref)), source: "--model" };
  if (choice.kind !== "chosen") throw new Error("no model chosen");
  err(`model: ${choice.ref} (${choice.source})\n`);

  const keyed = Boolean(process.env.TYPESAFE_API_KEY);
  const skipped = jevSkipReason({ noJev: values["no-jev"] ?? false, keyed, privateRepo: pull.private, allowKeylessForPrivate: config.jev.allow_keyless_for_private });
  const engine: JevRoute = skipped
    ? { skipped }
    : (deps.engine ?? new JevDecisionEngine(keyed ? createTypeSafeClient() : createZenClient()));
  const app = await openApp({
    dataDir,
    config,
    pulls,
    githubToken,
    engine,
    models,
    model: splitRef(choice.ref),
    reviewerVersion: `${REVIEWER_CODE_VERSION}+${choice.ref}`,
    onReport: (error) => err(`note: ${error instanceof Error ? error.message : String(error)}\n`),
  });
  try {
    app.harness.resume();
    const started = await startReview(app, pull, config.review.base_policy_version, { retry: values.retry ?? false });
    err(
      started.taskId === undefined
        ? `revision ${started.revision.id} already ${started.revision.state}; showing the stored result\n`
        : `revision ${started.revision.id} (${pull.fullName}#${pull.number} @ ${pull.headSha.slice(0, 12)}) durable task ${started.taskId}\n`,
    );
    const revision = await finishReview(app, started);
    const report = loadReport(app.store, revision.id);
    if (revision.state === "FAILED") err(`revision ${revision.id} FAILED: ${report.audit.at(-1)?.detail ?? "no reason recorded"}; rerun with --retry to try again\n`);
    if (values.json) out.json(renderJson(report));
    else out(renderMarkdown(report));
    return revision.state === "COMPLETED" ? 0 : 1;
  } finally {
    await app.close();
  }
}

async function pickModel(labels: string[], refs: string[]): Promise<string> {
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  try {
    labels.forEach((label, i) => process.stderr.write(`${String(i + 1).padStart(3)}. ${label}\n`));
    for (;;) {
      const answer = Number(await rl.question(`Pick a model [1-${refs.length}]: `));
      const ref = refs[answer - 1];
      if (Number.isInteger(answer) && ref) {
        if ((await rl.question(`Save ${ref} as the default in pr-review.config.json? [y/N] `)).trim().toLowerCase() === "y") saveDefaultModel(ref);
        return ref;
      }
    }
  } finally {
    rl.close();
  }
}

function inspect(args: string[], dataDir: string, out: Out): number {
  const { values } = parseArgs({ args, options: { pr: { type: "string" }, json: { type: "boolean" } } });
  if (!values.pr) throw new Error("inspect needs --pr owner/repo#123");
  const ref = parsePullRef(values.pr);
  const store = AppStore.open(`${dataDir}/app.sqlite`);
  try {
    const pr = store.pullByName(`${ref.owner}/${ref.repo}`, ref.number);
    if (!pr) throw new Error(`no reviews recorded for ${values.pr}`);
    const reports = store.revisionsForPr(pr.id).map((r) => loadReport(store, r.id));
    if (values.json) {
      out.json(reports.map((r) => ({ ...renderJson(r), durableTaskId: r.revision.durable_task_id, reviewer: r.revision.reviewer_version, transitions: r.audit })));
      return 0;
    }
    for (const r of reports) {
      out(`revision ${r.revision.id}  ${r.revision.state}\n`);
      out(`  head ${r.revision.head_sha}  base ${r.revision.base_sha}\n  reviewer ${r.revision.reviewer_version}  policy ${r.revision.policy_version}  durable task ${r.revision.durable_task_id ?? "-"}\n`);
      out(`  mode ${r.mode ?? "-"}${r.modeReasons.length ? ` (${r.modeReasons.join("; ")})` : ""}\n`);
      for (const e of r.audit) out(`  ${e.at}  ${e.from_state} --${e.event}--> ${e.to_state}${e.detail ? `  ${e.detail}` : ""}\n`);
      for (const f of r.findings) out(`  finding ${f.findingId.slice(0, 12)} ${f.status} ${f.severity}: ${f.candidate.title} (${f.candidate.file}:${f.candidate.newLine})\n`);
      for (const d of r.dispositions.filter((d) => d.state === "DISCARDED")) out(`  discarded ${d.candidateId}: ${d.reason}\n`);
      out("\n");
    }
    return 0;
  } finally {
    store.close();
  }
}

async function models(args: string[], out: Out): Promise<number> {
  const { values } = parseArgs({ args, options: { json: { type: "boolean" } } });
  const detected = await detectModels({ preferred: loadConfig().models.preferred });
  if (values.json) {
    out.json(detected);
    return 0;
  }
  out("Harnesses:\n");
  for (const h of detected.harnesses) out(`  ${h.name.padEnd(13)} ${(h.installed ? "installed" : "-").padEnd(10)} ${(h.usable ? "usable" : "-").padEnd(7)} ${h.note}\n`);
  out(`\nAvailable models (${detected.models.length}):\n`);
  for (const m of detected.models) out(`  ${m.ref.padEnd(44)} ${String(m.contextWindow).padStart(8)}  ${m.authSource}\n`);
  return 0;
}

async function withStore<T>(dataDir: string, fn: (store: AppStore) => Promise<T> | T): Promise<T> {
  mkdirSync(dataDir, { recursive: true });
  const store = AppStore.open(`${dataDir}/app.sqlite`);
  try {
    return await fn(store);
  } finally {
    store.close();
  }
}

function using(list: Account[]): { ok: boolean; text: string } {
  try {
    const plan = planGithubAuth(list);
    if (plan.source === "env") return { ok: true, text: "GITHUB_TOKEN from the environment (overrides gh accounts)" };
    if (plan.source === "none") return { ok: true, text: "no token, public repositories only" };
    return { ok: true, text: `${plan.account.id} (${plan.source === "selected" ? "selected" : "gh active account"})` };
  } catch (error) {
    return { ok: false, text: error instanceof Error ? error.message : String(error) };
  }
}

/** `pr-review accounts`: gh's accounts as this data dir sees them; `accounts use` selects one. Never fetches a token. */
async function accounts(args: string[], dataDir: string, out: Out): Promise<number> {
  const { values, positionals } = parseArgs({ args, allowPositionals: true, options: { json: { type: "boolean" } } });
  const [sub, requested, ...extra] = positionals;
  if ((sub !== undefined && sub !== "use") || (sub === "use" && (requested === undefined || extra.length > 0))) throw new Error("usage: pr-review accounts [--json] | pr-review accounts use <host/login | login>");
  const detection = await detectGh();
  return withStore(dataDir, (store) => {
    if (sub === "use") {
      const choice = chooseAccount(detection, requested!);
      if (!choice.ok) throw new Error(choice.message);
      store.journalMeta("ACCOUNT_SELECTED", choice.account.id, { [ACCOUNT_KEY]: choice.account.id });
      out(`selected ${choice.account.id} for GitHub reads and publishing in ${dataDir}\n`);
      return 0;
    }
    const list = listAccounts(detection, store.getMeta(ACCOUNT_KEY));
    const use = using(list);
    if (values.json) {
      out.json({ gh: detection.installed ? { installed: true, path: detection.path } : { installed: false }, accounts: list, using: use.ok ? use.text : null, error: use.ok ? undefined : use.text });
      return use.ok ? 0 : 1;
    }
    out(detection.installed ? `gh: ${detection.path}\n` : "gh: not on PATH\n");
    for (const a of list) out(`  ${a.id.padEnd(40)} ${a.state.padEnd(6)} ${a.ghActive ? "active" : "      "} ${a.selected ? "selected" : "        "} ${a.scopes.join(", ")}\n`.replace(/ +\n$/, "\n"));
    if (list.length === 0) out("  no accounts; run `gh auth login`\n");
    out(use.ok ? `using: ${use.text}\n` : `error: ${use.text}\n`);
    return use.ok ? 0 : 1;
  });
}

async function login(args: string[], deps: CliDeps, out: (t: string) => void): Promise<number> {
  const { values, positionals } = parseArgs({ args, allowPositionals: true, options: { "api-key": { type: "boolean" } } });
  const provider = positionals[0] ?? "anthropic";
  const models = deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore());
  const auth = models.getProvider(provider)?.auth;
  if (!auth) throw new Error(`unknown provider ${provider}`);
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const interaction: AuthInteraction = {
    prompt: async (p) => {
      if (p.type === "select") {
        p.options.forEach((o, i) => out(`${i + 1}. ${o.label}${o.description ? ` - ${o.description}` : ""}\n`));
        return p.options[Number(await rl.question(`${p.message} `)) - 1]?.id ?? p.options[0]!.id;
      }
      return rl.question(`${p.message} `);
    },
    notify: (e) => {
      if (e.type === "auth_url") out(`Open: ${e.url}\n${e.instructions ?? ""}\n`);
      else if (e.type === "device_code") out(`Visit ${e.verificationUri} and enter ${e.userCode}\n`);
      else out(`${e.message}\n`);
    },
  };
  try {
    const type = auth.oauth && !values["api-key"] ? "oauth" : "api_key";
    await models.login(provider, type, interaction);
    out(`Logged in to ${provider} (${type}); credential stored in pi's auth file.\n`);
    return 0;
  } finally {
    rl.close();
  }
}

if (import.meta.main) process.exitCode = await main(process.argv.slice(2));
