import { createInterface } from "node:readline/promises";
import { parseArgs } from "node:util";
import type { AuthInteraction, CredentialStore, Models } from "@earendil-works/pi-ai";
import { createTypeSafeClient, createZenClient } from "jev-systemone";
import { loadConfig, saveDefaultModel } from "../config/config.ts";
import { GitHubPullSource, parsePullRef, parsePullUrl, type PullSource, resolveGitHubToken } from "../github/pulls.ts";
import { type DecisionEngine, JevDecisionEngine } from "../jev/decision.ts";
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
  pr-review login [provider]`;

export async function main(argv: string[], deps: CliDeps = {}): Promise<number> {
  const out = deps.out ?? ((t) => process.stdout.write(t));
  const err = deps.err ?? ((t) => process.stderr.write(t));
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
      case "login":
        return await login(rest, deps, out);
      default:
        err(`${USAGE}\n`);
        return 2;
    }
  } catch (error) {
    err(`pr-review: ${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

async function review(args: string[], deps: CliDeps, dataDir: string, out: (t: string) => void, err: (t: string) => void): Promise<number> {
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
  const pulls = deps.pulls ?? new GitHubPullSource(await resolveGitHubToken());
  const pull = await pulls.getPull(ref);

  const models = deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore());
  const available = await listAvailableModels(models, config.models.preferred);
  const isTTY = deps.isTTY ?? Boolean(process.stdin.isTTY && process.stdout.isTTY);
  let choice = chooseModel({ flag: values.model, env: process.env.PR_REVIEW_MODEL, config: config.models.default }, available, isTTY);
  if (choice.kind === "error") throw new Error(choice.message);
  if (choice.kind === "pick") choice = { kind: "chosen", ref: await pickModel(available.map((m) => `${m.ref}  (${m.authSource})`), available.map((m) => m.ref)), source: "--model" };
  if (choice.kind !== "chosen") throw new Error("no model chosen");
  err(`model: ${choice.ref} (${choice.source})\n`);

  const engine = values["no-jev"]
    ? undefined
    : (deps.engine ?? new JevDecisionEngine(process.env.TYPESAFE_API_KEY ? createTypeSafeClient() : createZenClient()));
  const app = await openApp({
    dataDir,
    config,
    pulls,
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
    out(values.json ? `${JSON.stringify(renderJson(report), null, 2)}\n` : renderMarkdown(report));
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

function inspect(args: string[], dataDir: string, out: (t: string) => void): number {
  const { values } = parseArgs({ args, options: { pr: { type: "string" }, json: { type: "boolean" } } });
  if (!values.pr) throw new Error("inspect needs --pr owner/repo#123");
  const ref = parsePullRef(values.pr);
  const store = AppStore.open(`${dataDir}/app.sqlite`);
  try {
    const pr = store.pullByName(`${ref.owner}/${ref.repo}`, ref.number);
    if (!pr) throw new Error(`no reviews recorded for ${values.pr}`);
    const reports = store.revisionsForPr(pr.id).map((r) => loadReport(store, r.id));
    if (values.json) {
      out(`${JSON.stringify(reports.map((r) => ({ ...renderJson(r), durableTaskId: r.revision.durable_task_id, reviewer: r.revision.reviewer_version, transitions: r.audit })), null, 2)}\n`);
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

async function models(args: string[], out: (t: string) => void): Promise<number> {
  const { values } = parseArgs({ args, options: { json: { type: "boolean" } } });
  const detected = await detectModels({ preferred: loadConfig().models.preferred });
  if (values.json) {
    out(`${JSON.stringify(detected, null, 2)}\n`);
    return 0;
  }
  out("Harnesses:\n");
  for (const h of detected.harnesses) out(`  ${h.name.padEnd(13)} ${(h.installed ? "installed" : "-").padEnd(10)} ${(h.usable ? "usable" : "-").padEnd(7)} ${h.note}\n`);
  out(`\nAvailable models (${detected.models.length}):\n`);
  for (const m of detected.models) out(`  ${m.ref.padEnd(44)} ${String(m.contextWindow).padStart(8)}  ${m.authSource}\n`);
  return 0;
}

async function login(args: string[], deps: CliDeps, out: (t: string) => void): Promise<number> {
  const provider = args[0] ?? "anthropic";
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
    const type = auth.oauth ? "oauth" : "api_key";
    await models.login(provider, type, interaction);
    out(`Logged in to ${provider} (${type}); credential stored in pi's auth file.\n`);
    return 0;
  } finally {
    rl.close();
  }
}

if (import.meta.main) process.exitCode = await main(process.argv.slice(2));
