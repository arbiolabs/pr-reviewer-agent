import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { Models } from "@earendil-works/pi-ai";
import { createRegistry, defineExtension, Harness, type ModelRef, type TaskId } from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import lockfile from "proper-lockfile";
import type { Config } from "../config/config.ts";
import { IDLE_REVIEW_STATES, TERMINAL_REVIEW_STATES } from "../domain/machines.ts";
import type { PullInfo } from "../domain/types.ts";
import type { GitHubClient } from "../github/client.ts";
import type { PullSource } from "../github/pulls.ts";
import type { JevRoute } from "../jev/decision.ts";
import { AppStore, type RevisionRow } from "../persistence/store.ts";
import { reconcileOutbox } from "../publish/publisher.ts";
import { createReviewTools, type Workspace } from "../reasoning/tools.ts";
import { parseDiff } from "../source/diff.ts";
import { PinnedRepo } from "../source/git.ts";
import { createReviewTask, REVIEW_TASK_NAME, type ReviewInput, type ReviewResult, type ReviewTask } from "./task.ts";

export type AppOptions = {
  dataDir: string;
  config: Config;
  pulls: PullSource;
  gitToken?: ((installationId: number) => Promise<string | undefined>) | undefined;
  github?: ((installationId: number) => GitHubClient) | undefined;
  engine: JevRoute;
  models: Models;
  model: ModelRef;
  /** Part of revision identity: a different reviewer (code or model) is a different review. */
  reviewerVersion: string;
  onReport?: (error: unknown) => void;
  /** How long to wait for another process's data-dir lock before giving up; see `lockDataDir`. */
  lockWaitMs?: number;
};

export type App = { store: AppStore; harness: Harness; task: ReviewTask; reviewerVersion: string; close(): Promise<void> };

const ctx = BACKGROUND_CONTEXT;

/**
 * Open both stores and the Harness. Pi Durable owns checkpoints and conversations; the app DB owns business records.
 * The data dir is locked for the App's lifetime.
 */
export async function openApp(options: AppOptions): Promise<App> {
  mkdirSync(options.dataDir, { recursive: true });
  const release = await lockDataDir(options.dataDir, options.lockWaitMs);
  try {
    return await openLocked(options, release);
  } catch (error) {
    await release();
    throw error;
  }
}

/**
 * One pr-review per data dir: two would run the same durable tasks and share the bare clones. The retries outlast
 * `stale`, so a rerun after kill -9 takes over the dead process's lock. proper-lockfile removes the lock on exit and
 * on signals.
 */
export async function lockDataDir(dir: string, waitMs = 6000): Promise<() => Promise<void>> {
  try {
    // Inside the dir, not proper-lockfile's default sibling `<dir>.lock`: a volume's parent (say `/`) is often read-only.
    const retries = { retries: Math.ceil(waitMs / 500), minTimeout: 500, maxTimeout: 500 };
    return await lockfile.lock(dir, { lockfilePath: join(dir, "lock"), stale: 5000, update: 1000, retries });
  } catch (error) {
    if ((error as { code?: unknown }).code === "ELOCKED") throw new Error(`another pr-review is running on ${dir}`);
    throw error;
  }
}

async function openLocked(options: AppOptions, release: () => Promise<void>): Promise<App> {
  const cacheDir = join(options.dataDir, "repos");
  const store = AppStore.open(join(options.dataDir, "app.sqlite"));
  const workspaces = new Map<string, Workspace>();
  async function workspace(revisionId: string): Promise<Workspace> {
    const cached = workspaces.get(revisionId);
    if (cached) return cached;
    const revision = store.revision(revisionId)!;
    const snapshot = store.snapshot(revisionId);
    if (!snapshot) throw new Error(`revision ${revisionId} has no context snapshot`);
    const repo = new PinnedRepo(join(cacheDir, `${snapshot.pr.fullName}.git`), revision.base_sha, revision.head_sha);
    const ws = { revision, snapshot, repo, files: parseDiff(snapshot.diff) };
    workspaces.set(revisionId, ws);
    return ws;
  }
  const tools = createReviewTools({ store, workspace });
  const task = createReviewTask({ ...options, store, cacheDir, tools, workspace });
  const registry = createRegistry();
  registry.install(tools);
  registry.install(defineExtension({ name: "pr-review-runtime", tasks: [task] }));
  const storage = await openNodeSqliteStorage(join(options.dataDir, "durable.sqlite"));
  const harness = await Harness.open(storage, { models: options.models, registry, ...(options.onReport ? { onReport: options.onReport } : {}) }, ctx);
  return {
    store,
    harness,
    task,
    reviewerVersion: options.reviewerVersion,
    async close() {
      try {
        await harness.close(ctx);
        store.close();
      } finally {
        await release();
      }
    },
  };
}

const PUBLISH_POLICY = "+publish";

/** Whether a revision was started to publish; recovery recreates its task the same way. */
export const publishes = (revision: RevisionRow) => revision.policy_version.endsWith(PUBLISH_POLICY);

/** Publishing is part of the policy: a dry run of a head never blocks publishing that head later (§2 invariant 3). */
export const startPolicy = (basePolicyVersion: string, publish: boolean) => (publish ? `${basePolicyVersion}${PUBLISH_POLICY}` : basePolicyVersion);

export type StartedReview = { revision: RevisionRow; taskId: TaskId<ReviewResult> | undefined };

/**
 * Converge on exactly one durable task per revision attempt: an existing task for the attempt is found by scanning in
 * the same commit that would create it. A terminal revision gets no task; its stored result is the answer. `retry`
 * reopens a FAILED revision through the audited MANUAL_RETRY override (§4.1); the machine refuses it from any other
 * terminal state.
 */
export async function startReview(
  app: App,
  pull: PullInfo,
  basePolicyVersion: string,
  options: { retry?: boolean; publish?: boolean; retrySource?: string } = {},
): Promise<StartedReview> {
  const prId = app.store.upsertPull(pull);
  const policyVersion = startPolicy(basePolicyVersion, options.publish ?? false);
  const revision = app.store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion, reviewerVersion: app.reviewerVersion });
  if (options.retry && TERMINAL_REVIEW_STATES.includes(revision.state)) return retryReview(app, revision, options.retrySource ?? "pr-review review --retry");
  const taskId = await ensureTask(app, revision, options.publish ?? false);
  return { revision: app.store.revision(revision.id)!, taskId };
}

/**
 * Reopen a revision through the audited MANUAL_RETRY override (§4.1), which the machine allows only from FAILED, and
 * give the new attempt its task. `onCommit` runs in the override's transaction.
 */
export async function retryReview(app: App, revision: RevisionRow, retrySource: string, onCommit?: () => void): Promise<StartedReview> {
  const n = app.store.retryCount(revision.id) + 1;
  app.store.advance(revision.id, revision.state, "MANUAL_RETRY", `manual override: ${retrySource} (retry ${n})`, onCommit);
  const reopened = app.store.revision(revision.id)!;
  return { revision: reopened, taskId: await ensureTask(app, reopened, publishes(reopened)) };
}

/**
 * The durable task for the revision's current attempt, created only if no task for it exists; `undefined` for an
 * idle revision (terminal, or parked for approval). Recovery calls this for every unfinished revision. An attempt
 * opened by APPROVED starts at the publish phase of the stored outbox.
 */
export async function ensureTask(app: App, revision: RevisionRow, publish: boolean): Promise<TaskId<ReviewResult> | undefined> {
  if (IDLE_REVIEW_STATES.includes(revision.state)) return undefined;
  const reopenings = app.store.reopenings(revision.id);
  const retry = reopenings.length;
  const root = await app.harness.root(ctx);
  const taskId = await root.commit(async (tx) => {
    // ponytail: linear scan of every review task; index revision -> task in a session document if this grows.
    for (let page = await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100); ; ) {
      const found = page.items.find((t) => {
        const input = t.input as ReviewInput;
        return input.revisionId === revision.id && (input.retry ?? 0) === retry;
      });
      if (found) return found.id as TaskId<ReviewResult>;
      if (!page.next) break;
      page = await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100, page.next);
    }
    const approved = reopenings.at(-1) === "APPROVED" ? { approvedOutboxId: app.store.outboxForRevision(revision.id)!.id } : {};
    return tx.createTask(app.task, { revisionId: revision.id, retry, publish, ...approved }, { ownership: { kind: "conversation" } });
  }, ctx);
  app.store.setDurableTask(revision.id, String(taskId));
  return taskId;
}

/**
 * Wait for the revision's task; a task that ended without the revision reaching an idle state fails it, unless a later
 * attempt (an approval) has taken the revision over.
 */
export async function finishReview(app: App, started: StartedReview): Promise<RevisionRow> {
  if (started.taskId !== undefined) {
    const settled = await app.harness.waitForTask(started.taskId, ctx);
    const outcome = settled.state.outcome;
    const current = app.store.revision(started.revision.id)!;
    if (!IDLE_REVIEW_STATES.includes(current.state) && current.durable_task_id === String(started.taskId)) {
      const why = outcome.status === "completed" ? "completed without a terminal revision state" : (outcome.error?.message ?? outcome.reason ?? outcome.status);
      app.store.advanceFromCurrent(started.revision.id, "TERMINAL_ERROR", `durable task ${outcome.status}: ${why}`);
    }
  }
  return app.store.revision(started.revision.id)!;
}

/**
 * §11 startup recovery, in order: durable tasks resume, unfinished revisions get their task back, and outboxes whose
 * write may have reached GitHub are reconciled before anything new is posted. `track` follows each piece of work.
 */
export async function recover(app: App, github: (installationId: number) => GitHubClient, track: (work: Promise<unknown>) => void): Promise<void> {
  const store = app.store;
  app.harness.resume();
  for (const revision of store.unfinishedRevisions()) {
    const taskId = await ensureTask(app, revision, publishes(revision));
    if (taskId !== undefined) track(finishReview(app, { revision, taskId }));
  }
  for (const outbox of store.uncertainOutboxes()) {
    const revision = store.revision(outbox.revision_id)!;
    // An unfinished revision's own task reconciles in its PUBLISH or RECONCILE phase.
    if (store.unfinishedRevisions(revision.pr_id).some((r) => r.id === revision.id)) continue;
    const pr = store.pullById(revision.pr_id)!;
    track(reconcileOutbox(store, github(pr.installation_id), { fullName: pr.repo_full_name, number: pr.number }, outbox));
  }
}
