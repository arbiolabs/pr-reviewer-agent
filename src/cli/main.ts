import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { parseArgs } from "node:util";
import type { AuthInteraction, CredentialStore, Models } from "@earendil-works/pi-ai";
import { createTypeSafeClient, createZenClient } from "jev-systemone";
import { type Config, configPath, DEFAULT_CONFIG, loadConfig, saveConfig } from "../config/config.ts";
import { runDaemon, runTui } from "../control/daemon.ts";
import { EngineSource } from "../control/engine.ts";
import { ACCOUNT_KEY, chooseAccount, detectGh, listAccounts, resolveGithubAuth } from "../github/accounts.ts";
import { clientFor, type GitHubClient } from "../github/client.ts";
import { GitHubPullSource, parsePullRef, parsePullUrl, type PullSource } from "../github/pulls.ts";
import { type DecisionEngine, JevDecisionEngine, type JevRoute, jevSkipReason } from "../jev/decision.ts";
import { typeSafeKey } from "../jev/key.ts";
import { PiAuthFileStore } from "../models/credential-store.ts";
import { buildModels, detectModels } from "../models/detect.ts";
import { type AvailableModel, chooseModel, listAvailableModels, type ModelChoice } from "../models/select.ts";
import { USAGE_BY, USAGE_GROUPS, type UsageBy, type UsageGroup, type UsageGroupBy, type UsageTotals, usageReport, usageSince } from "../domain/usage.ts";
import { AppStore, usageRows, withStore } from "../persistence/store.ts";
import { reconcileOutbox } from "../publish/publisher.ts";
import { finishReview, lockDataDir, openApp, startReview } from "../runtime/app.ts";
import { approve as approvePublication, Refused, resumeApproved, reviewAgain } from "../runtime/commands.ts";
import { reviewerVersion } from "../runtime/task.ts";
import { startService } from "../server/service.ts";
import { type Check, gatherChecks, NO_TOKEN, using } from "./checks.ts";
import { loadReport, renderJson, renderMarkdown } from "./report.ts";


/** Seams the tests replace: the GitHub read side, the model collection, and where state lives. */
export type CliDeps = {
  pulls?: PullSource;
  /** The GitHub client `--publish` and `reconcile` use; a test points it at a fake server. */
  github?: GitHubClient;
  models?: Models;
  credentials?: CredentialStore;
  engine?: DecisionEngine;
  isTTY?: boolean;
  out?: (text: string) => void;
  err?: (text: string) => void;
};

const USAGE = `usage:
  pr-review setup [--check] [--json]
  pr-review review <pr-url> [--dry-run | --publish] [--json] [--no-jev] [--model provider/id] [--retry | --again]
  pr-review reconcile --revision <revision-id>
  pr-review approve --revision <revision-id> --preview <payload-hash>
  pr-review serve
  pr-review inspect --pr owner/repo#123 [--json]
  pr-review models [--json]
  pr-review usage [--by day|week|month] [--group time|repo|author|pr] [--json]
  pr-review accounts [--json]
  pr-review accounts use <host/login | login>
  pr-review login [provider] [--api-key]
  pr-review daemon [--socket PATH] [--fixture NAME] [--data DIR] [--heartbeat SECONDS] [--allow-commands]
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
      case "setup":
        return await setup(rest, deps, dataDir, out, err);
      case "review":
        return await review(rest, deps, dataDir, out, err);
      case "reconcile":
        return await reconcile(rest, deps, dataDir, out);
      case "approve":
        return await approve(rest, deps, dataDir, out, err);
      case "serve":
        return await serve(deps, dataDir, err);
      case "inspect":
        return inspect(rest, dataDir, out);
      case "models":
        return await models(rest, out);
      case "usage":
        return await usage(rest, dataDir, out);
      case "accounts":
        return await accounts(rest, dataDir, out);
      case "login":
        return await login(rest, deps, out);
      case "daemon":
        return await runDaemon(rest, dataDir, err, { openEngine: (dir) => openEngine(dir, deps, err), commandsConfigured: () => loadConfig().control.allow_commands, checks: (dir) => gatherChecks(deps, dir) });
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
    options: { "dry-run": { type: "boolean" }, publish: { type: "boolean" }, json: { type: "boolean" }, "no-jev": { type: "boolean" }, model: { type: "string" }, retry: { type: "boolean" }, again: { type: "boolean" } },
  });
  if (positionals.length !== 1 || (values.publish && values["dry-run"]) || (values.retry && values.again)) {
    err(`${USAGE}\n`);
    return 2;
  }
  const config = loadConfig();
  const ref = parsePullUrl(positionals[0]!);
  // A test's fake pull source serves local repositories; only the real GitHub source needs the token.
  const auth = deps.pulls || deps.github ? undefined : await withStore(dataDir, (store) => resolveGithubAuth(store));
  if (auth) err(`github: ${auth.account ?? NO_TOKEN} (${auth.source})\n`);
  const githubToken = auth?.token;
  if (values.publish && !deps.github && githubToken === undefined) throw new Error("--publish needs a GitHub token: set GITHUB_TOKEN, run gh auth login, or pick one with pr-review accounts use");
  const github = deps.github ?? clientFor(config.github, { kind: "token", token: githubToken });
  const pulls = deps.pulls ?? new GitHubPullSource(() => github);
  const pull = await pulls.getPull(ref);

  const { models, ref: modelRef } = await resolveModel(config, deps, values.model, deps.isTTY ?? Boolean(process.stdin.isTTY && process.stdout.isTTY), err);

  const key = await typeSafeKey();
  const skipped = jevSkipReason({ noJev: values["no-jev"] ?? false, keyed: key !== undefined, privateRepo: pull.private, allowKeylessForPrivate: config.jev.allow_keyless_for_private });
  const engine: JevRoute = skipped
    ? { skipped }
    : (deps.engine ?? new JevDecisionEngine(key ? createTypeSafeClient({ apiKey: key.key }) : createZenClient()));
  const app = await openApp({
    dataDir,
    config,
    pulls,
    gitToken: async () => githubToken,
    github: () => github,
    engine: async () => engine,
    models,
    reviewerVersion: reviewerVersion(modelRef),
    onReport: (error) => err(`note: ${error instanceof Error ? error.message : String(error)}\n`),
  });
  try {
    app.harness.resume();
    const publish = values.publish ?? false;
    const prId = app.store.upsertPull(pull);
    const again = { basePolicyVersion: config.review.base_policy_version, publish, source: "pr-review review --again" };
    const started = values.again
      ? await reviewAgain(app, pulls, { prId, expectedHeadSha: pull.headSha }, again).catch((error: unknown) => {
          throw error instanceof Refused ? new Error(`${error.code}: ${error.message}`) : error;
        })
      : await startReview(app, pull, config.review.base_policy_version, { retry: values.retry ?? false, publish });
    err(
      started.taskId === undefined
        ? `revision ${started.revision.id} already ${started.revision.state}; showing the stored result\n`
        : `revision ${started.revision.id} (${pull.fullName}#${pull.number} @ ${pull.headSha.slice(0, 12)}) durable task ${started.taskId}\n`,
    );
    const revision = await finishReview(app, started);
    const report = loadReport(app.store, revision.id);
    if (report.outbox) err(`publication ${report.outbox.status}${report.outbox.github_review_id ? `: review ${report.outbox.github_review_id}` : ""}${report.outbox.last_error ? ` (${report.outbox.last_error})` : ""}\n`);
    if (revision.state === "FAILED") err(`revision ${revision.id} FAILED: ${report.audit.at(-1)?.detail ?? "no reason recorded"}; rerun with --retry to try again\n`);
    if (revision.state === "AWAITING_APPROVAL") err(`revision ${revision.id} awaits approval: pr-review approve --revision ${revision.id} --preview ${report.outbox?.payload_hash}\n`);
    if (values.json) out.json(renderJson(report));
    else out(renderMarkdown(report));
    return revision.state === "COMPLETED" || revision.state === "AWAITING_APPROVAL" ? 0 : 1;
  } finally {
    await app.close();
  }
}

/**
 * `approve --revision <id> --preview <hash>`: the headless twin of the TUI's review.publish. Publishes exactly the
 * stored payload through the exactly-once outbox. A rerun after a crash resumes the approved attempt instead.
 */
async function approve(args: string[], deps: CliDeps, dataDir: string, out: Out, err: (t: string) => void): Promise<number> {
  const { values } = parseArgs({ args, options: { revision: { type: "string" }, preview: { type: "string" } } });
  if (!values.revision || !values.preview) throw new Error("approve needs --revision <revision-id> --preview <payload-hash>");
  const config = loadConfig();
  const auth = deps.github ? undefined : await withStore(dataDir, (store) => resolveGithubAuth(store));
  if (!deps.github && auth?.token === undefined) throw new Error("approve needs a GitHub token: set GITHUB_TOKEN, run gh auth login, or pick one with pr-review accounts use");
  const github = deps.github ?? clientFor(config.github, { kind: "token", token: auth!.token });
  const pulls = deps.pulls ?? new GitHubPullSource(() => github);
  // The approved attempt only publishes, so the revision's own reviewer identity serves; no model is resolved.
  const identity = await withStore(dataDir, (store) => {
    const revision = store.revision(values.revision!);
    if (!revision) throw new Error(`no revision ${values.revision}`);
    return revision.reviewer_version;
  });
  const app = await openApp({
    dataDir,
    config,
    pulls,
    gitToken: async () => auth?.token,
    github: () => github,
    engine: async () => ({ skipped: "approve does not triage" }),
    models: deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore()),
    reviewerVersion: identity,
    onReport: (error) => err(`note: ${error instanceof Error ? error.message : String(error)}\n`),
  });
  try {
    app.harness.resume();
    const revision = app.store.revision(values.revision)!;
    let started = await resumeApproved(app, revision);
    if (started) err(`revision ${revision.id} was already approved; waiting for its publication\n`);
    else {
      try {
        started = await approvePublication(app, pulls, { revisionId: revision.id, previewId: values.preview }, "pr-review approve");
      } catch (error) {
        if (error instanceof Refused) throw new Error(`${error.code}: ${error.message}`);
        throw error;
      }
      err(`revision ${revision.id} approved; durable task ${started.taskId}\n`);
    }
    const finished = await finishReview(app, started);
    const outbox = app.store.outboxForRevision(finished.id);
    out(`publication ${outbox?.status}${outbox?.github_review_id ? `: review ${outbox.github_review_id}` : ""}${outbox?.last_error ? ` (${outbox.last_error})` : ""}; revision ${finished.state}\n`);
    return finished.state === "COMPLETED" ? 0 : 1;
  } finally {
    await app.close();
  }
}

async function resolveModel(config: Config, deps: CliDeps, flag: string | undefined, isTTY: boolean, err: (t: string) => void): Promise<{ models: Models; ref: string; source: Extract<ModelChoice, { kind: "chosen" }>["source"] }> {
  const models = deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore());
  const available = await listAvailableModels(models, config.models.preferred);
  let choice = chooseModel({ flag, env: process.env.PR_REVIEW_MODEL, config: config.models.default }, available, isTTY);
  if (choice.kind === "error") throw new Error(choice.message);
  if (choice.kind === "pick") {
    const ref = await pickModel(available);
    if (await confirm(`Save ${ref} as the default in ${configPath()}?`, false)) saveConfig({ models: { default: ref } });
    choice = { kind: "chosen", ref, source: "--model" };
  }
  if (choice.kind !== "chosen") throw new Error("no model chosen");
  err(`model: ${choice.ref} (${choice.source})\n`);
  return { models, ref: choice.ref, source: choice.source };
}

/**
 * GitHub clients per installation from the environment: a GitHub App (`GITHUB_APP_ID` plus `GITHUB_APP_PRIVATE_KEY`
 * or `GITHUB_APP_PRIVATE_KEY_FILE`), else one token client resolved by `resolveGithubAuth` (`GITHUB_TOKEN`, the selected
 * gh account, or gh's active account).
 */
async function githubFromEnv(
  config: Config,
  store: { getMeta(key: string): string | undefined },
): Promise<{ clientFor: (installationId: number) => GitHubClient; kind: "app" | "token"; account?: string }> {
  const appId = process.env.GITHUB_APP_ID;
  if (!appId) {
    const auth = await resolveGithubAuth(store);
    const client = clientFor(config.github, { kind: "token", token: auth.token });
    return { clientFor: () => client, kind: "token", ...(auth.account ? { account: auth.account } : {}) };
  }
  const pem = process.env.GITHUB_APP_PRIVATE_KEY?.replaceAll("\\n", "\n") ?? (process.env.GITHUB_APP_PRIVATE_KEY_FILE ? readFileSync(process.env.GITHUB_APP_PRIVATE_KEY_FILE, "utf8") : undefined);
  if (!pem) throw new Error("GITHUB_APP_ID is set but neither GITHUB_APP_PRIVATE_KEY nor GITHUB_APP_PRIVATE_KEY_FILE is");
  const clients = new Map<number, GitHubClient>();
  return {
    kind: "app",
    clientFor: (installationId) => {
      if (installationId === 0) throw new Error("this PR was recorded without a GitHub App installation; use token auth for it");
      if (!clients.has(installationId)) clients.set(installationId, clientFor(config.github, { kind: "app", appId, privateKeyPem: pem, installationId }));
      return clients.get(installationId)!;
    },
  };
}

/** `reconcile --revision <id>`: decide an uncertain publication by listing the PR's reviews. Never posts. */
async function reconcile(args: string[], deps: CliDeps, dataDir: string, out: Out): Promise<number> {
  const { values } = parseArgs({ args, options: { revision: { type: "string" } } });
  if (!values.revision) throw new Error("reconcile needs --revision <revision-id>");
  const config = loadConfig();
  const release = await lockDataDir(dataDir);
  const store = AppStore.open(`${dataDir}/app.sqlite`);
  try {
    const revision = store.revision(values.revision);
    if (!revision) throw new Error(`no revision ${values.revision}`);
    const outbox = store.outboxForRevision(revision.id);
    if (!outbox) throw new Error(`revision ${revision.id} has no publication`);
    if (outbox.status !== "sending" && outbox.status !== "needs_reconciliation") {
      out(`publication ${outbox.status}${outbox.github_review_id ? `: review ${outbox.github_review_id}` : ""}; nothing to reconcile\n`);
      return 0;
    }
    const pr = store.pullById(revision.pr_id)!;
    const client = deps.github ?? (await githubFromEnv(config, store)).clientFor(pr.installation_id);
    const verdict = await reconcileOutbox(store, client, { fullName: pr.repo_full_name, number: pr.number }, outbox);
    const state = store.revision(revision.id)!.state;
    if (verdict.kind === "found") out(`found review ${verdict.reviewId}; publication published, revision ${state}\n`);
    else if (verdict.kind === "absent") out(`no review with the marker on GitHub; publication pending, revision ${state}${state === "FAILED" ? "; rerun review --publish --retry to post it" : ""}\n`);
    else out(`cannot safely decide: ${verdict.reason}; publication stays needs_reconciliation\n`);
    return verdict.kind === "undecided" ? 1 : 0;
  } finally {
    store.close();
    await release();
  }
}

/**
 * One Jev engine serves every repository, so the keyless tier is used only when private diffs may go there. The key is
 * looked up each time (`typeSafeKey`), so a key saved to the Keychain applies to the next review without a restart.
 */
export async function jevRoute(config: Config, deps: CliDeps): Promise<JevRoute> {
  const key = await typeSafeKey();
  if (key) return deps.engine ?? new JevDecisionEngine(createTypeSafeClient({ apiKey: key.key }));
  return config.jev.allow_keyless_for_private ? (deps.engine ?? new JevDecisionEngine(createZenClient())) : { skipped: "jev_skipped_no_key" };
}

/**
 * The daemon's engine, configured like `serve` from the environment: the GitHub auth `resolveGithubAuth` picks, the
 * model resolved without a prompt, and the data-dir lock. Throws while another pr-review holds that lock.
 */
async function openEngine(dataDir: string, deps: CliDeps, err: (t: string) => void): Promise<EngineSource> {
  const config = loadConfig();
  const github = await withStore(dataDir, (store) => githubFromEnv(config, store));
  const pulls = deps.pulls ?? new GitHubPullSource(github.clientFor);
  const { models, ref, source } = await resolveModel(config, deps, undefined, false, err);
  const report = (error: unknown) => err(`note: ${error instanceof Error ? error.message : String(error)}\n`);
  const app = await openApp({
    dataDir,
    config,
    pulls,
    gitToken: (installationId) => github.clientFor(installationId).token(),
    github: github.clientFor,
    engine: () => jevRoute(config, deps),
    models,
    reviewerVersion: reviewerVersion(ref),
    onReport: report,
  });
  return EngineSource.host(dataDir, app, { config, pulls, github: github.clientFor, report, models, model: { ref, source }, ...(github.account ? { account: github.account } : {}) });
}

/** `serve`: webhook receiver, worker and reviews in one process, configured from the environment. */
async function serve(deps: CliDeps, dataDir: string, err: (t: string) => void): Promise<number> {
  const config = loadConfig();
  const webhookSecret = process.env.GITHUB_WEBHOOK_SECRET;
  if (!webhookSecret) throw new Error("serve needs GITHUB_WEBHOOK_SECRET");
  const github = await withStore(dataDir, (store) => githubFromEnv(config, store));
  const pulls = new GitHubPullSource(github.clientFor);
  const { models, ref } = await resolveModel(config, deps, undefined, false, err);
  const report = (error: unknown) => err(`note: ${error instanceof Error ? error.message : String(error)}\n`);
  const app = await openApp({
    dataDir,
    config,
    pulls,
    gitToken: (installationId) => github.clientFor(installationId).token(),
    github: github.clientFor,
    engine: () => jevRoute(config, deps),
    models,
    reviewerVersion: reviewerVersion(ref),
    onReport: report,
  });
  const service = await startService({
    app,
    config,
    dataDir,
    pulls,
    github: github.clientFor,
    webhookSecret,
    adminToken: process.env.PR_REVIEW_ADMIN_TOKEN || undefined,
    port: Number(process.env.PORT ?? 8080),
    publish: config.review.publish_mode !== "dry_run",
    report,
  });
  err(`serving on ${service.url} (${github.kind} auth, publish_mode ${config.review.publish_mode})\n`);
  await new Promise<void>((resolve) => {
    process.once("SIGTERM", resolve);
    process.once("SIGINT", resolve);
  });
  await service.close();
  await app.close();
  return 0;
}

async function ask(question: string): Promise<string> {
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  try {
    return (await rl.question(question)).trim();
  } finally {
    rl.close();
  }
}

async function confirm(question: string, yes: boolean): Promise<boolean> {
  const answer = (await ask(`${question} [${yes ? "Y/n" : "y/N"}] `)).toLowerCase();
  return answer ? answer === "y" || answer === "yes" : yes;
}

async function pickModel(available: AvailableModel[]): Promise<string> {
  available.forEach((m, i) => process.stderr.write(`${String(i + 1).padStart(3)}. ${m.ref}  (${m.authSource})\n`));
  for (;;) {
    const ref = available[Number(await ask(`Pick a model [1-${available.length}]: `)) - 1]?.ref;
    if (ref) return ref;
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

const compact = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });
const COLUMNS = ["Reviews", "Responses", "Input", "Output", "Cache read", "Cache write", "Cost"];
const cells = (t: UsageTotals) => [String(t.reviews), String(t.responses), ...[t.input, t.output, t.cacheRead, t.cacheWrite].map((n) => compact.format(n)), `$${t.cost.toFixed(2)}`];

const GROUP_NAMES: Record<UsageGroupBy, string> = { time: "Time", repo: "Repository", author: "Author", pr: "PR" };
const clip = (s: string) => (s.length > 64 ? `${s.slice(0, 63)}…` : s);
const groupLabel = (g: UsageGroup) => clip(g.prId ? `${g.repository}#${g.number} ${g.label}` : g.label);

/** `pr-review usage`: recorded model usage from this data dir, the way the daemon's usage.get reports it. */
async function usage(args: string[], dataDir: string, out: Out): Promise<number> {
  const { values } = parseArgs({ args, options: { by: { type: "string", default: "day" }, group: { type: "string", default: "time" }, json: { type: "boolean" } } });
  if (!(USAGE_BY as readonly string[]).includes(values.by)) throw new Error(`--by must be one of ${USAGE_BY.join(", ")}`);
  if (!(USAGE_GROUPS as readonly string[]).includes(values.group)) throw new Error(`--group must be one of ${USAGE_GROUPS.join(", ")}`);
  const options = { by: values.by as UsageBy, group: values.group as UsageGroupBy, tz: Intl.DateTimeFormat().resolvedOptions().timeZone, now: Date.now() };
  const report = usageReport(await withStore(dataDir, (store) => usageRows(store.db, usageSince(options))), options);
  if (values.json) {
    out.json({ ...report, currency: "USD", priceSource: "catalog" });
    return 0;
  }
  const first = report.buckets[0]!.start;
  const last = report.buckets.at(-1)!.end;
  const split = report.groups ? `, split by ${GROUP_NAMES[report.group].toLowerCase()}` : "";
  out(`Usage by ${report.by} from ${first} to ${last}${split}, ${report.timeZone}. Estimated at the model's list price in USD; a subscription login is billed differently.\n`);
  if (report.totals.responses === 0) {
    out("No usage recorded in this range. Usage is recorded from this version on.\n");
    return 0;
  }
  const label = { day: "Day", week: "Week of", month: "Month" }[report.by];
  const rows = report.groups
    ? [
        [GROUP_NAMES[report.group], ...COLUMNS, "Share"],
        ...report.groups.map((g) => [groupLabel(g), ...cells(g), `${(g.share * 100).toFixed(1)}%`]),
        ["Total", ...cells(report.totals), "100%"],
      ]
    : [
        [label, ...COLUMNS],
        ...report.buckets.map((b) => [report.by === "month" ? b.start.slice(0, 7) : b.start, ...cells(b)]),
        ["Total", ...cells(report.totals)],
      ];
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)));
  const line = (r: string[]) => `${r.map((c, i) => (i === 0 ? c.padEnd(widths[i]!) : c.padStart(widths[i]!))).join("  ")}\n`;
  for (const r of rows) out(line(r));
  out("\nBy model:\n");
  const named = Math.max(...report.models.map((m) => m.model.length));
  for (const m of report.models) out(`  ${m.model.padEnd(named)}  ${m.reviews} review(s)  ${m.responses} response(s)  $${m.cost.toFixed(2)}\n`);
  return 0;
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

const MARK = { ok: "✓", warn: "!", missing: "✗" } as const;

function printChecks(out: Out, checks: Check[]): void {
  for (const c of checks) out(`  ${MARK[c.status]} ${c.id.padEnd(7)} ${c.detail}\n${c.fix ? `${" ".repeat(12)}${c.fix}\n` : ""}`);
}

const PUBLISH_MODES = ["dry_run", "comment", "silent_no_findings", "approval"] as const satisfies readonly Config["review"]["publish_mode"][];

/** `pr-review setup`: print the checks, then offer each fix setup can make. `--check` only prints, and never prompts. */
async function setup(args: string[], deps: CliDeps, dataDir: string, out: Out, err: (t: string) => void): Promise<number> {
  const { values } = parseArgs({ args, options: { check: { type: "boolean" }, json: { type: "boolean" } } });
  let checks = await gatherChecks(deps, dataDir);
  const exitCode = () => (checks.some((c) => c.status === "missing") ? 1 : 0);
  const status = (id: Check["id"]) => checks.find((c) => c.id === id)!.status;
  if (values.json) {
    out.json({ checks });
    return exitCode();
  }
  printChecks(out, checks);
  if (values.check) return exitCode();
  if (!(deps.isTTY ?? Boolean(process.stdin.isTTY && process.stdout.isTTY))) {
    err("setup: not a terminal, so checking only (same as setup --check)\n");
    return exitCode();
  }

  // A config file that fails to load is never rewritten; the user fixes it by hand first.
  const config = status("config") === "ok" ? loadConfig() : undefined;
  const available = () => listAvailableModels(deps.models ?? buildModels(deps.credentials ?? new PiAuthFileStore()), (config ?? DEFAULT_CONFIG).models.preferred);
  if (status("model") === "missing" && (await available()).length === 0 && (await confirm("Log in to anthropic now?", true))) await login(["anthropic"], deps, out);
  if (config) {
    const models = await available();
    const why = !config.models.default ? `No models.default in ${configPath()}.` : status("model") === "missing" ? `models.default ${config.models.default} is not available.` : undefined;
    if (models.length > 0 && why && (await confirm(`${why} Pick one now?`, true))) saveConfig({ models: { default: await pickModel(models) } });
    const current = config.review.publish_mode;
    let mode: Config["review"]["publish_mode"] | undefined;
    while (!mode) {
      const answer = (await ask(`Publish mode (${PUBLISH_MODES.join(" / ")}) [${current}]: `)) || current;
      mode = PUBLISH_MODES.find((m) => m === answer);
    }
    if (mode !== current) saveConfig({ review: { publish_mode: mode } });
  }

  checks = await gatherChecks(deps, dataDir);
  out("\n");
  printChecks(out, checks);
  if (exitCode() === 0) out("\nnext: pr-review review <pr-url>\n");
  return exitCode();
}

if (import.meta.main) process.exitCode = await main(process.argv.slice(2));
