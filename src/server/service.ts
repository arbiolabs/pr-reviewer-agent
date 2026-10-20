import { accessSync, constants } from "node:fs";
import type { AddressInfo } from "node:net";
import { randomUUID } from "node:crypto";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { TaskId } from "@earendil-works/pi-durable";
import { loadReport, renderJson } from "../cli/report.ts";
import type { Config } from "../config/config.ts";
import { Coordinator } from "../coordinator/coordinator.ts";
import type { GitHubClient } from "../github/client.ts";
import type { PullSource } from "../github/pulls.ts";
import { reconcileOutbox } from "../publish/publisher.ts";
import { type App, ensureTask, finishReview, publishes, startReview } from "../runtime/app.ts";
import type { ReviewResult } from "../runtime/task.ts";
import { createHttpServer } from "./http.ts";

export type ServiceOptions = {
  app: App;
  config: Config;
  dataDir: string;
  pulls: PullSource;
  github: (installationId: number) => GitHubClient;
  webhookSecret: string;
  adminToken: string | undefined;
  port: number;
  /** Publish reviews (publish_mode comment or silent_no_findings) or only record them (dry_run). */
  publish: boolean;
  tickMs?: number;
  now?: () => number;
  report: (error: unknown) => void;
};

export type Service = {
  url: string;
  coordinator: Coordinator;
  /** Resolves once every review this process started or resumed has settled. */
  idle(): Promise<void>;
  close(): Promise<void>;
};

const ctx = BACKGROUND_CONTEXT;

/**
 * One process: webhook receiver, worker loop and durable reviews. Startup follows §11 recovery in order: the stores are
 * already open, durable tasks resume, unfinished revisions get their task back, and outboxes whose write may have
 * reached GitHub are reconciled before anything new is posted. Expired leases are taken over as PRs come due.
 */
export async function startService(options: ServiceOptions): Promise<Service> {
  const { app, config } = options;
  const store = app.store;
  const now = options.now ?? Date.now;
  const tickMs = options.tickMs ?? 1000;
  const running = new Set<Promise<unknown>>();
  const track = (promise: Promise<unknown>) => {
    const p = promise.catch(options.report).finally(() => running.delete(p));
    running.add(p);
  };
  const follow = (revisionId: string, taskId: TaskId<ReviewResult> | undefined) => {
    if (taskId !== undefined) track(finishReview(app, { revision: store.revision(revisionId)!, taskId }));
  };

  app.harness.resume();
  for (const revision of store.unfinishedRevisions()) follow(revision.id, await ensureTask(app, revision, publishes(revision)));
  for (const outbox of store.uncertainOutboxes()) {
    const revision = store.revision(outbox.revision_id)!;
    // An unfinished revision's own task reconciles in its PUBLISH or RECONCILE phase.
    if (store.unfinishedRevisions(revision.pr_id).some((r) => r.id === revision.id)) continue;
    const pr = store.pullById(revision.pr_id)!;
    track(reconcileOutbox(store, options.github(pr.installation_id), { fullName: pr.repo_full_name, number: pr.number }, outbox));
  }

  // The worker loop's own GitHub reads never sit through a long rate-limit wait: the delivery backs off instead.
  const maxSleepMs = config.github.max_rate_limit_sleep_ms;
  const coordinator = new Coordinator({
    store,
    pulls: { getPull: (ref) => options.pulls.getPull(ref, { maxSleepMs }) },
    botLogin: (installationId) => options.github(installationId).viewerLogin({ maxSleepMs }),
    start: async (pull) => {
      const started = await startReview(app, pull, config.review.base_policy_version, { publish: options.publish });
      follow(started.revision.id, started.taskId);
    },
    abort: async (taskId) => void (await app.harness.abortTask(Number(taskId) as TaskId, ctx)),
    debounceMs: config.review.debounce_seconds * 1000,
    leaseMs: 5 * 60_000,
    webhookRetry: { maxAttempts: config.review.webhook_max_attempts, backoffMs: config.review.webhook_backoff_ms },
    owner: `pr-review-${process.pid}-${randomUUID()}`,
    now,
    report: options.report,
  });

  let lastTick = now();
  let stopped = false;
  let wake: () => void = () => {};
  const loop = (async () => {
    while (!stopped) {
      await coordinator.tick().catch(options.report);
      lastTick = now();
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, tickMs);
        wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
    }
  })();

  const server = createHttpServer({
    store,
    webhookSecret: options.webhookSecret,
    adminToken: options.adminToken,
    wake: () => wake(),
    report: options.report,
    ready: () => {
      const check = (fn: () => unknown) => {
        try {
          fn();
          return true;
        } catch {
          return false;
        }
      };
      const checks = {
        db: check(() => store.db.prepare("SELECT 1").get()),
        worker: !stopped && now() - lastTick < tickMs * 5 + 30_000,
        storage: check(() => accessSync(options.dataDir, constants.W_OK)),
      };
      return { ok: Object.values(checks).every(Boolean), checks };
    },
    inspect: (revisionId) => {
      if (!store.revision(revisionId)) return undefined;
      const report = loadReport(store, revisionId);
      return {
        ...renderJson(report),
        transitions: report.audit,
        outboxEvents: report.outbox ? store.outboxEvents(report.outbox.id) : [],
      };
    },
    retry: async (revisionId) => {
      const revision = store.revision(revisionId);
      if (!revision) return { status: 404, body: { error: "no such revision" } };
      if (revision.state !== "FAILED") return { status: 409, body: { error: `MANUAL_RETRY is not legal in ${revision.state}` } };
      const n = store.retryCount(revisionId) + 1;
      store.advance(revisionId, "FAILED", "MANUAL_RETRY", `manual override: POST /reviews/${revisionId}/retry (retry ${n})`);
      follow(revisionId, await ensureTask(app, store.revision(revisionId)!, publishes(revision)));
      return { status: 202, body: { revisionId, state: store.revision(revisionId)!.state } };
    },
  });
  await new Promise<void>((resolve) => server.listen(options.port, resolve));

  return {
    url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`,
    coordinator,
    async idle() {
      while (running.size > 0) await Promise.all(running);
    },
    async close() {
      stopped = true;
      wake();
      await loop;
      await coordinator.drain();
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
