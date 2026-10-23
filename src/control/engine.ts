import type { Models } from "@earendil-works/pi-ai";
import { type Config, configPath, saveConfig } from "../config/config.ts";
import { type AvailableModel, listAvailableModels, matches } from "../models/select.ts";
import type { MODEL_SOURCES } from "../protocol/v1.ts";
import { reviewerVersion } from "../runtime/task.ts";
import type { Env } from "../github/accounts.ts";
import { type GitHubClient, GitHubError } from "../github/client.ts";
import { parsePullRef, parsePullUrl, type PullRef, type PullSource } from "../github/pulls.ts";
import type { ReviewCommand } from "../protocol/v1.ts";
import { type App, finishReview, recover, type StartedReview, startPolicy, startReview } from "../runtime/app.ts";
import { StateConflict } from "../persistence/store.ts";
import { approve, cancel, livePull, Refused, retry, startFromHead } from "../runtime/commands.ts";
import { AppDbSource, clean } from "./appdb.ts";
import { type Accepted, ControlError, type Settings } from "./source.ts";

export type EngineOptions = {
  config: Config;
  pulls: PullSource;
  github: (installationId: number) => GitHubClient;
  env?: Env;
  report?: (error: unknown) => void;
  /** The GitHub account the pull source reads as (`<host>/<login>` or `env/GITHUB_TOKEN`); absent when it reads anonymously. */
  account?: string;
  /** What `models` offers is what settings.set accepts. */
  models: Models;
  /** The model new reviews get, as `chooseModel` picked it when the engine opened. */
  model: { ref: string; source: ModelSource };
};

type ModelSource = (typeof MODEL_SOURCES)[number];

const CANCEL_REASON = "cancelled from the TUI";

/**
 * The daemon in engine mode (TUI_SPEC §2), which the daemon enters only with commands enabled: the app DB read side of
 * `AppDbSource`, plus the App it hosts. Commands run in process through the same code the CLI uses; each accepted one
 * is recorded under its idempotency key and attempt, so a replay answers the same operation and has no second effect.
 * Reviews it starts or resumes are followed to their end, and closing the source suspends them for the next start.
 */
export class EngineSource extends AppDbSource {
  override readonly mode = "engine" as const;
  override readonly readOnlyReason = undefined;
  private readonly app: App;
  private readonly options: EngineOptions;
  private readonly running = new Set<Promise<unknown>>();
  private model: EngineOptions["model"];

  private constructor(dataDir: string, app: App, options: EngineOptions) {
    super(dataDir, app.store, options.env ?? process.env);
    this.app = app;
    this.options = options;
    this.model = options.model;
  }

  /**
   * Host `app` (already holding the data-dir lock) and run its §11 startup recovery. On a recovery failure the app is
   * closed first, so the daemon's read-only fallback leaves no lock held and no task running.
   */
  static async host(dataDir: string, app: App, options: EngineOptions): Promise<EngineSource> {
    const source = new EngineSource(dataDir, app, options);
    try {
      await recover(app, options.github, (work) => source.track(work));
    } catch (error) {
      source.db.close();
      await app.close();
      throw error;
    }
    return source;
  }

  private track(work: Promise<unknown>): void {
    const p = work.catch((error) => this.options.report?.(error)).finally(() => this.running.delete(p));
    this.running.add(p);
  }

  private follow(started: StartedReview): void {
    if (started.taskId !== undefined) this.track(finishReview(this.app, started));
  }

  capabilities(): string[] {
    const publish = this.options.config.review.publish_mode === "approval";
    return ["review.add", "review.start", "review.retry", "review.cancel", ...(publish ? ["review.publish"] : [])];
  }

  override unavailable(command: string): string {
    return `${command} needs review.publish_mode "approval" (it is "${this.options.config.review.publish_mode}")`;
  }

  /**
   * The run attempt a command's key is bound to, counted in reopenings: a cancel ends the current attempt, a retry or
   * an approval opens the next one, and a start names a head rather than an attempt. A key recorded under another
   * attempt is a new request, so the same retry acts again once the attempt it opened has failed, and the same cancel
   * or approval acts on an attempt opened since.
   */
  private attemptOf(request: ReviewCommand): number | null {
    if (!("runId" in request.params)) return null;
    const revision = this.app.store.revision(request.params.runId);
    if (!revision) return null;
    const n = this.app.store.reopenings(revision.id).length;
    if (request.command === "review.retry") return revision.state === "FAILED" ? n + 1 : n;
    if (request.command === "review.publish") return revision.state === "AWAITING_APPROVAL" ? n + 1 : n;
    return n;
  }

  async command(request: ReviewCommand): Promise<Accepted> {
    if (request.command === "review.add") return this.add(request.params.url, request.idempotencyKey);
    return { operationId: await this.runCommand(request) };
  }

  /**
   * review.add: track the PR a URL or `owner/repo#N` names and start its review, as `pr-review review` does under
   * this daemon's config (`publish_mode` decides whether it parks or posts). A PR already tracked is a CONFLICT that
   * names it and starts nothing; its key replayed answers the PR and run it added.
   */
  private async add(url: string, key: string): Promise<Accepted> {
    const { app, options } = this;
    const accepted = (runId: string): Accepted => ({ operationId: `review.add:${runId}`, prId: app.store.revision(runId)!.pr_id, runId });
    const replay = app.store.commandResult(key);
    if (replay) return accepted(replay.operationId.slice("review.add:".length));
    const ref = pullTarget(url);
    const tracked = app.store.pullByName(`${ref.owner}/${ref.repo}`, ref.number);
    if (tracked) throw new ControlError("CONFLICT", `${tracked.repo_full_name}#${tracked.number} is already tracked`, tracked.id);
    const pull = await options.pulls.getPull(ref, { maxSleepMs: 5000 }).catch((error: unknown) => {
      throw unreadable(error, `${ref.owner}/${ref.repo}#${ref.number}`, options.account);
    });
    if (pull.state !== "open") throw new ControlError("CONFLICT", `${pull.fullName}#${pull.number} is ${pull.merged ? "merged" : "closed"}; only open PRs are reviewed`);
    const publish = options.config.review.publish_mode !== "dry_run";
    const started = await startReview(app, pull, options.config.review.base_policy_version, { publish });
    this.follow(started);
    const result = accepted(started.revision.id);
    app.store.recordCommand(key, { command: "review.add", operationId: result.operationId, attempt: null });
    return result;
  }

  private async runCommand(request: Exclude<ReviewCommand, { command: "review.add" }>): Promise<string> {
    const { app, options } = this;
    const key = request.idempotencyKey;
    const attempt = this.attemptOf(request);
    const replay = app.store.commandResult(key);
    const record = (operationId: string) => () => app.store.recordCommand(key, { command: request.command, operationId, attempt });
    try {
      const publish = options.config.review.publish_mode !== "dry_run";
      // A start replays only for the revision it would start now: the same base, head, policy and reviewer, with the
      // base read from GitHub. Otherwise it starts anew, which refuses a reviewed head. When GitHub can't be read, a
      // new start couldn't run either, so the recorded result stands.
      const recorded = app.store.revision(replay?.operationId.slice("review.start:".length) ?? "");
      const base =
        request.command === "review.start" && recorded
          ? await livePull(app, options.pulls, request.params.prId).then((pull) => pull.baseSha, () => recorded.base_sha)
          : undefined;
      const replays =
        request.command !== "review.start" ||
        (recorded?.head_sha === request.params.expectedHeadSha &&
          recorded.base_sha === base &&
          recorded.policy_version === startPolicy(options.config.review.base_policy_version, publish) &&
          recorded.reviewer_version === app.reviewerVersion);
      if (replay && replay.attempt === attempt && replays) return replay.operationId;
      switch (request.command) {
        case "review.start": {
          const started = await startFromHead(app, options.pulls, request.params, { basePolicyVersion: options.config.review.base_policy_version, publish });
          this.follow(started);
          const operationId = `review.start:${started.revision.id}`;
          record(operationId)();
          return operationId;
        }
        case "review.retry": {
          const operationId = `review.retry:${request.params.runId}`;
          this.follow(await retry(app, request.params.runId, "tui via daemon", record(operationId)));
          return operationId;
        }
        case "review.cancel": {
          await cancel(app, request.params.runId, CANCEL_REASON);
          const operationId = `review.cancel:${request.params.runId}`;
          record(operationId)();
          return operationId;
        }
        case "review.publish": {
          const operationId = `review.publish:${request.params.runId}`;
          const params = { revisionId: request.params.runId, previewId: request.params.previewId, expectedHeadSha: request.params.expectedHeadSha };
          this.follow(await approve(app, options.pulls, params, "tui via daemon", record(operationId)));
          return operationId;
        }
      }
    } catch (error) {
      if (error instanceof Refused) throw new ControlError(error.code, error.message);
      // The state the command checked moved during its await (a cancel from another connection, say).
      if (error instanceof StateConflict) throw new ControlError("CONFLICT", error.message);
      throw error;
    }
  }

  private available(): Promise<AvailableModel[]> {
    return listAvailableModels(this.options.models, this.options.config.models.preferred);
  }

  async settings(): Promise<Settings> {
    const { preferred } = this.options.config.models;
    const models = (await this.available()).map((m) => ({
      ref: m.ref,
      provider: m.provider,
      name: m.name,
      contextWindow: m.contextWindow,
      authSource: m.authSource,
      preferred: preferred.some((p) => matches(p, m.ref)),
    }));
    return clean({ model: this.model, models, configPath: configPath(this.options.env ?? process.env) });
  }

  /**
   * settings.set: save the model as `models.default` (other config keys kept) and give it to reviews started from now
   * on. A review in flight keeps the model its revision names. Refused while the daemon's own `--model` or
   * PR_REVIEW_MODEL outranks the config, since the saved choice would not survive a restart.
   */
  async setModel(ref: string): Promise<string> {
    const { model } = this;
    const { env } = this.options;
    if (model.source === "--model" || model.source === "PR_REVIEW_MODEL") {
      throw new ControlError("CONFLICT", `the daemon's ${model.source} (${model.ref}) overrides the config; remove it and restart the daemon to choose the model here`);
    }
    const available = await this.available();
    if (!available.some((m) => m.ref === ref)) throw new ControlError("NOT_FOUND", `model ${ref} is not available; choose one of ${available.map((m) => m.ref).join(", ")}`);
    saveConfig({ models: { default: ref } }, configPath(env ?? process.env));
    this.model = { ref, source: "config" };
    this.app.reviewerVersion = reviewerVersion(ref);
    return `settings.set:${ref}`;
  }

  /** Suspends the reviews in flight (they resume on the next engine start) and releases the data-dir lock. */
  override async close(): Promise<void> {
    this.db.close();
    await this.app.close();
    await Promise.allSettled(this.running);
  }
}

/** review.add's target: a GitHub pull request URL or `owner/repo#N`. */
function pullTarget(url: string): PullRef {
  for (const parse of [parsePullUrl, parsePullRef]) {
    try {
      return parse(url);
    } catch {}
  }
  throw new ControlError("INVALID_REQUEST", `${url} is not a GitHub pull request; paste its URL (https://github.com/owner/repo/pull/123) or owner/repo#123`);
}

/** A PR GitHub would not serve, in words that say what to fix: a definite refusal names the account it read as. */
function unreadable(error: unknown, name: string, account: string | undefined): unknown {
  if (!(error instanceof GitHubError)) return error;
  if (error.class === "retryable") return new ControlError("TEMPORARILY_UNAVAILABLE", `GitHub did not answer for ${name} just now (${error.message}); try again`);
  if (error.class !== "fatal" || ![401, 403, 404].includes(error.status ?? 0)) return error;
  return new ControlError(
    "NOT_FOUND",
    account
      ? `cannot read ${name}: it does not exist, or ${account} has no access to it (HTTP ${error.status})`
      : `cannot read ${name}: it does not exist, or it is private and the daemon has no GitHub token; run gh auth login or set GITHUB_TOKEN, then restart the daemon (HTTP ${error.status})`,
  );
}
