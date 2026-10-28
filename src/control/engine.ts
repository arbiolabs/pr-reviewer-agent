import type { Models } from "@earendil-works/pi-ai";
import { type AutoReview, autoReviewProblem, type Config, configPath, saveConfig } from "../config/config.ts";
import type { PullInfo } from "../domain/types.ts";
import { type AvailableModel, listAvailableModels, matches } from "../models/select.ts";
import type { MODEL_SOURCES } from "../protocol/v1.ts";
import { reviewerVersion } from "../runtime/task.ts";
import type { Env } from "../github/accounts.ts";
import { type GitHubClient, GitHubError } from "../github/client.ts";
import { parsePullRef, parsePullUrl, type PullRef, type PullSource } from "../github/pulls.ts";
import { accessibleRepos, reviewRequests } from "../github/requests.ts";
import { type AutoReviewPoll, type ReposAnswer, type ReviewCommand, settingsKey } from "../protocol/v1.ts";
import { DEFAULT_SUMMARY_TEMPLATE, effectiveTemplate, TEMPLATE_VARIABLES, templateProblem } from "../publish/template.ts";
import { type App, finishReview, generationOf, recover, type StartedReview, startPolicy, startReview } from "../runtime/app.ts";
import { StateConflict } from "../persistence/store.ts";
import { againTarget, approve, cancel, livePull, Refused, retry, reviewAgain, startFromHead } from "../runtime/commands.ts";
import { AppDbSource, clean } from "./appdb.ts";
import { type Accepted, ControlError, type Settings, type SettingsParams } from "./source.ts";

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
  /** Tests shorten the auto-review clock: the first poll after start, and the interval in place of the config's. */
  autoReviewMs?: { first: number; interval?: number };
  /** Tests move the day: the daily cap counts from local midnight of this clock, and repos.list's cache ages by it. */
  now?: () => number;
};

type ModelSource = (typeof MODEL_SOURCES)[number];

const CANCEL_REASON = "cancelled from the TUI";

/** Reviews one poll for review requests may start; the rest wait for the next poll, oldest first. */
export const MAX_STARTS_PER_POLL = 5;
/** The longest rate-limit wait a poll sits through; past it the poll fails and is retried later. */
const POLL_MAX_SLEEP_MS = 5000;
/** Failed polls back off by doubling the interval, up to this. */
const POLL_BACKOFF_CAP_MS = 60 * 60_000;
/** The attribution a poll's start carries; the daily cap counts these lines, so the count survives a restart. */
const AUTO_TOOL = "auto-review";
const AUTO_STARTED = "started automatically: review requested";
const REPOS_TTL_MS = 10 * 60_000;
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

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
  private lastPoll: AutoReviewPoll | undefined;
  private polling: Promise<AutoReviewPoll> | undefined;
  private pollFailures = 0;
  private pollTimer: NodeJS.Timeout | undefined;
  private closed = false;
  private reposCache: { account: string | undefined; at: number; answer: Promise<ReposAnswer> } | undefined;

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
    source.schedulePoll(options.autoReviewMs?.first ?? 10_000);
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
    return ["review.add", "review.start", "review.again", "review.retry", "review.cancel", ...(publish ? ["review.publish"] : [])];
  }

  override unavailable(command: string): string {
    return `${command} needs review.publish_mode "approval" (it is "${this.options.config.review.publish_mode}")`;
  }

  /**
   * The run attempt a command's key is bound to, counted in reopenings: a cancel ends the current attempt, a retry or
   * an approval opens the next one, and a start names a head rather than an attempt. A key recorded under another
   * attempt is a new request, so the same retry acts again once the attempt it opened has failed, and the same cancel
   * or approval acts on an attempt opened since. A review again is bound the same way to the head's review-again
   * generation: the one it would open now, or while that one runs, the current one.
   */
  private attemptOf(request: ReviewCommand): number | null {
    if (request.command === "review.again") {
      const { generation, refusal } = againTarget(this.app, request.params.prId, request.params.expectedHeadSha);
      return refusal ? generation - 1 : generation;
    }
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
    const operationId = await this.runCommand(request);
    // Starting a review brings an archived PR back; archiving never touched the run.
    const { params } = request;
    if ("prId" in params) this.writer.setArchived(params.prId, false);
    else if (request.command === "review.retry") this.writer.setArchived(this.app.store.revision(params.runId)!.pr_id, false);
    return { operationId };
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
    // Adding a PR again is asking to see it, so an archived one comes back to the lists.
    if (replay) {
      const result = accepted(replay.operationId.slice("review.add:".length));
      this.writer.setArchived(result.prId!, false);
      return result;
    }
    const ref = pullTarget(url);
    const tracked = app.store.pullByName(`${ref.owner}/${ref.repo}`, ref.number);
    if (tracked) {
      this.writer.setArchived(tracked.id, false);
      throw new ControlError("CONFLICT", `${tracked.repo_full_name}#${tracked.number} is already tracked`, tracked.id);
    }
    const pull = await options.pulls.getPull(ref, { maxSleepMs: 5000 }).catch((error: unknown) => {
      throw unreadable(error, `${ref.owner}/${ref.repo}#${ref.number}`, options.account);
    });
    if (pull.state !== "open") throw new ControlError("CONFLICT", `${pull.fullName}#${pull.number} is ${pull.merged ? "merged" : "closed"}; only open PRs are reviewed`);
    const result = accepted((await this.start(pull)).revision.id);
    app.store.recordCommand(key, { command: "review.add", operationId: result.operationId, attempt: null });
    return result;
  }

  /**
   * Start reviewing `pull` at its head under this daemon's config, as review.add does, and follow the review. A poll's
   * start is attributed before the task runs, so the task's own activity can't crowd the line out of the rate limit.
   */
  private async start(pull: PullInfo, auto = false): Promise<StartedReview> {
    const { config } = this.options;
    const started = await startReview(this.app, pull, config.review.base_policy_version, { publish: config.review.publish_mode !== "dry_run" });
    if (auto) this.app.store.appendActivity(started.revision.id, AUTO_TOOL, AUTO_STARTED);
    this.follow(started);
    return started;
  }

  /**
   * One poll for the PRs that request the account's review (`review.auto_review`). Overlapping triggers share the poll
   * in flight. It never throws: a failure is the poll's `error`, and the next one waits longer.
   */
  pollRequests(): Promise<AutoReviewPoll> {
    this.polling ??= this.poll().finally(() => (this.polling = undefined));
    return this.polling;
  }

  private async poll(): Promise<AutoReviewPoll> {
    const at = new Date().toISOString();
    let found = 0;
    let started = 0;
    let today = 0;
    let error: string | undefined;
    const cap = this.options.config.review.auto_review.daily_cap;
    try {
      today = this.startedToday();
      if (this.options.account === undefined) throw new Error("no GitHub account to search as; run gh auth login or set GITHUB_TOKEN, then restart the daemon");
      const search = await reviewRequests(this.options.github(0), this.options.config.review.auto_review, POLL_MAX_SLEEP_MS);
      found = search.found;
      for (const ref of search.pulls) {
        if (started >= MAX_STARTS_PER_POLL || (cap > 0 && today >= cap)) break;
        try {
          if (await this.startRequested(ref)) {
            started++;
            today++;
          }
        } catch (failure) {
          // A rate limit or an outage stops the poll; anything else is this PR's own problem.
          if (failure instanceof GitHubError && failure.class === "retryable") throw failure;
          this.options.report?.(failure);
          error ??= `${ref.owner}/${ref.repo}#${ref.number}: ${message(failure)}`;
        }
      }
      this.pollFailures = 0;
    } catch (failure) {
      this.pollFailures++;
      error = message(failure);
      this.options.report?.(new Error(`auto-review poll failed: ${error}`));
    }
    const capped = cap > 0 && today >= cap ? { capReached: true } : {};
    this.lastPoll = { at, found, started, skipped: found - started, startedToday: today, ...capped, ...(error === undefined ? {} : { error }) };
    return this.lastPoll;
  }

  /** Automatic starts since local midnight, read back from their attribution lines, so a restart keeps the count. */
  private startedToday(): number {
    const midnight = new Date(this.options.now?.() ?? Date.now());
    midnight.setHours(0, 0, 0, 0);
    return this.app.store.countActivity(AUTO_TOOL, AUTO_STARTED, midnight.toISOString());
  }

  /**
   * repos.list: what the account can reach on GitHub, unarchived, most recently pushed first. One answer serves every
   * client for ten minutes, or until the account changes; a failure is the answer's `error` and is not kept.
   */
  repos(): Promise<ReposAnswer> {
    const now = this.options.now?.() ?? Date.now();
    const { account } = this.options;
    const hit = this.reposCache;
    if (hit && hit.account === account && now - hit.at < REPOS_TTL_MS) return hit.answer;
    const fetchedAt = new Date(now).toISOString();
    const fetch =
      account === undefined
        ? Promise.reject(new Error("no GitHub account to list repositories as; run gh auth login or set GITHUB_TOKEN, then restart the daemon"))
        : accessibleRepos(this.options.github(0), POLL_MAX_SLEEP_MS);
    const answer: Promise<ReposAnswer> = fetch.then(
      (repos) => ({ repos, fetchedAt }),
      (failure: unknown) => {
        if (this.reposCache?.answer === answer) this.reposCache = undefined;
        return { repos: [], fetchedAt, error: message(failure) };
      },
    );
    this.reposCache = { account, at: now, answer };
    return answer;
  }

  /**
   * Start a requested PR at its live head, unless it is archived, no longer open, or that head has a review already
   * (any state, any model). That last check is what keeps polls, restarts and a crash mid-poll from starting a head
   * twice: a start creates the head's revision before anything else. True when it started one.
   */
  private async startRequested(ref: PullRef): Promise<boolean> {
    const { store } = this.app;
    if (store.pullByName(`${ref.owner}/${ref.repo}`, ref.number)?.archived_at) return false;
    const pull = await this.options.pulls.getPull(ref, { maxSleepMs: POLL_MAX_SLEEP_MS });
    if (pull.state !== "open") return false;
    const prId = store.upsertPull(pull);
    if (store.revisionsForPr(prId).some((r) => r.head_sha === pull.headSha)) return false;
    await this.start(pull, true);
    return true;
  }

  /** Poll in `ms`, then each interval (doubling after failures), while auto-review is on. */
  private schedulePoll(ms: number): void {
    clearTimeout(this.pollTimer);
    const auto = this.options.config.review.auto_review;
    if (this.closed || !auto.enabled) return;
    const every = this.options.autoReviewMs?.interval ?? auto.interval_minutes * 60_000;
    const next = () => Math.min(every * 2 ** Math.max(0, this.pollFailures - 1), Math.max(every, POLL_BACKOFF_CAP_MS));
    this.pollTimer = setTimeout(() => void this.pollRequests().then(() => this.schedulePoll(next())), ms);
    this.pollTimer.unref();
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
          recorded.policy_version === startPolicy(options.config.review.base_policy_version, publish, generationOf(recorded)) &&
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
        case "review.again": {
          const again = { basePolicyVersion: options.config.review.base_policy_version, publish, source: "tui via daemon" };
          const started = await reviewAgain(app, options.pulls, request.params, again, (r) => record(`review.again:${r.id}`)());
          this.follow(started);
          return `review.again:${started.revision.id}`;
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
    const saved = this.options.config.review.summary_template;
    const summaryTemplate = { value: effectiveTemplate(saved), isDefault: saved === "", default: DEFAULT_SUMMARY_TEMPLATE, variables: [...TEMPLATE_VARIABLES] };
    const a = this.options.config.review.auto_review;
    const autoReview = {
      enabled: a.enabled,
      intervalMinutes: a.interval_minutes as 5 | 15 | 30,
      includeTeamRequests: a.include_team_requests,
      repos: { include: [...a.repos.include], exclude: [...a.repos.exclude] },
      dailyCap: a.daily_cap,
      ...(this.lastPoll ? { lastPoll: this.lastPoll } : {}),
    };
    const publishMode = this.options.config.review.publish_mode;
    return clean({ model: this.model, models, configPath: configPath(this.options.env ?? process.env), summaryTemplate, publishMode, autoReview });
  }

  /**
   * settings.set: save the model as `models.default`, the summary template as `review.summary_template` and the
   * auto-review fields given under `review.auto_review` (other config keys kept), each checked before any is written.
   * A review in flight keeps the model its revision names; a payload already stored keeps its text. The model is
   * refused while the daemon's own `--model` or PR_REVIEW_MODEL outranks the config, since the saved choice would not
   * survive a restart. Turning auto-review on polls at once; any other change to it restarts the interval.
   */
  async setSettings(params: SettingsParams): Promise<string> {
    const { model: ref, summaryTemplate, autoReview } = params;
    if (ref === undefined && summaryTemplate === undefined && autoReview === undefined) {
      throw new ControlError("INVALID_REQUEST", "settings.set needs model, summaryTemplate or autoReview");
    }
    if (ref !== undefined) {
      const { model } = this;
      if (model.source === "--model" || model.source === "PR_REVIEW_MODEL") {
        throw new ControlError("CONFLICT", `the daemon's ${model.source} (${model.ref}) overrides the config; remove it and restart the daemon to choose the model here`);
      }
      const available = await this.available();
      if (!available.some((m) => m.ref === ref)) throw new ControlError("NOT_FOUND", `model ${ref} is not available; choose one of ${available.map((m) => m.ref).join(", ")}`);
    }
    const template = summaryTemplate ?? "";
    const problem = summaryTemplate === undefined ? undefined : templateProblem(template);
    if (problem) throw new ControlError("INVALID_REQUEST", problem);
    const was = this.options.config.review.auto_review;
    const auto: AutoReview | undefined = autoReview && {
      enabled: autoReview.enabled ?? was.enabled,
      interval_minutes: autoReview.intervalMinutes ?? was.interval_minutes,
      include_team_requests: autoReview.includeTeamRequests ?? was.include_team_requests,
      repos: { include: autoReview.repos?.include ?? was.repos.include, exclude: autoReview.repos?.exclude ?? was.repos.exclude },
      daily_cap: autoReview.dailyCap ?? was.daily_cap,
    };
    const autoProblem = auto && autoReviewProblem(auto);
    if (autoProblem) throw new ControlError("INVALID_REQUEST", `autoReview: ${autoProblem}`);
    // A reset removes the key, so the file says nothing rather than repeating the default.
    const review = { ...(summaryTemplate !== undefined ? { summary_template: template || undefined } : {}), ...(auto ? { auto_review: auto } : {}) };
    saveConfig({ ...(ref !== undefined ? { models: { default: ref } } : {}), ...(Object.keys(review).length > 0 ? { review } : {}) }, configPath(this.options.env ?? process.env));
    if (ref !== undefined) {
      this.model = { ref, source: "config" };
      this.app.reviewerVersion = reviewerVersion(ref);
    }
    // The review task reads this same config object when it prepares a payload.
    if (summaryTemplate !== undefined) this.options.config.review.summary_template = template;
    if (auto) {
      this.options.config.review.auto_review = auto;
      this.schedulePoll(auto.enabled && !was.enabled ? 0 : this.options.autoReviewMs?.interval ?? auto.interval_minutes * 60_000);
    }
    return settingsKey(params);
  }

  /** Suspends the reviews in flight (they resume on the next engine start) and releases the data-dir lock. */
  override async close(): Promise<void> {
    this.closed = true;
    clearTimeout(this.pollTimer);
    await this.polling;
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
