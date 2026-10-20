import type { Config } from "../config/config.ts";
import type { Env } from "../github/accounts.ts";
import type { GitHubClient } from "../github/client.ts";
import type { PullSource } from "../github/pulls.ts";
import type { ReviewCommand } from "../protocol/v1.ts";
import { type App, finishReview, recover, type StartedReview } from "../runtime/app.ts";
import { StateConflict } from "../persistence/store.ts";
import { approve, cancel, Refused, retry, startFromHead } from "../runtime/commands.ts";
import { AppDbSource } from "./appdb.ts";
import { ControlError } from "./source.ts";

export type EngineOptions = {
  config: Config;
  pulls: PullSource;
  github: (installationId: number) => GitHubClient;
  env?: Env;
  report?: (error: unknown) => void;
};

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

  private constructor(dataDir: string, app: App, options: EngineOptions) {
    super(dataDir, app.store, options.env ?? process.env);
    this.app = app;
    this.options = options;
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
    return ["review.start", "review.retry", "review.cancel", ...(publish ? ["review.publish"] : [])];
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

  async command(request: ReviewCommand): Promise<string> {
    const { app, options } = this;
    const key = request.idempotencyKey;
    const attempt = this.attemptOf(request);
    const replay = app.store.commandResult(key);
    if (replay && replay.attempt === attempt) return replay.operationId;
    const record = (operationId: string) => () => app.store.recordCommand(key, { command: request.command, operationId, attempt });
    try {
      switch (request.command) {
        case "review.start": {
          const publish = options.config.review.publish_mode !== "dry_run";
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

  /** Suspends the reviews in flight (they resume on the next engine start) and releases the data-dir lock. */
  override async close(): Promise<void> {
    this.db.close();
    await this.app.close();
    await Promise.allSettled(this.running);
  }
}
