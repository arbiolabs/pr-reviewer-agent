import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { Models } from "@earendil-works/pi-ai";
import { createRegistry, defineExtension, Harness, type ModelRef, type TaskId } from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import type { Config } from "../config/config.ts";
import { TERMINAL_REVIEW_STATES } from "../domain/machines.ts";
import type { PullInfo } from "../domain/types.ts";
import type { PullSource } from "../github/pulls.ts";
import type { DecisionEngine } from "../jev/decision.ts";
import { AppStore, type RevisionRow } from "../persistence/store.ts";
import { createReviewTools, type Workspace } from "../reasoning/tools.ts";
import { parseDiff } from "../source/diff.ts";
import { PinnedRepo } from "../source/git.ts";
import { createReviewTask, REVIEW_TASK_NAME, type ReviewInput, type ReviewResult, type ReviewTask } from "./task.ts";

export type AppOptions = {
  dataDir: string;
  config: Config;
  pulls: PullSource;
  engine: DecisionEngine | undefined;
  models: Models;
  model: ModelRef;
  /** Part of revision identity: a different reviewer (code or model) is a different review. */
  reviewerVersion: string;
  onReport?: (error: unknown) => void;
};

export type App = { store: AppStore; harness: Harness; task: ReviewTask; reviewerVersion: string; close(): Promise<void> };

const ctx = BACKGROUND_CONTEXT;

/** Open both stores and the Harness. Pi Durable owns checkpoints and conversations; the app DB owns business records. */
export async function openApp(options: AppOptions): Promise<App> {
  mkdirSync(options.dataDir, { recursive: true });
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
      await harness.close(ctx);
      store.close();
    },
  };
}

export type StartedReview = { revision: RevisionRow; taskId: TaskId<ReviewResult> | undefined };

/**
 * Converge on exactly one durable task per revision: an existing task for the revision is found by scanning in the
 * same commit that would create it. A terminal revision gets no task; its stored result is the answer.
 */
export async function startReview(app: App, pull: PullInfo, policyVersion: string): Promise<StartedReview> {
  const prId = app.store.upsertPull(pull);
  const revision = app.store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion, reviewerVersion: app.reviewerVersion });
  if (TERMINAL_REVIEW_STATES.includes(revision.state)) return { revision, taskId: undefined };
  const root = await app.harness.root(ctx);
  const taskId = await root.commit(async (tx) => {
    // ponytail: linear scan of every review task; index revision -> task in a session document if this grows.
    for (let page = await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100); ; ) {
      const found = page.items.find((t) => (t.input as ReviewInput).revisionId === revision.id);
      if (found) return found.id as TaskId<ReviewResult>;
      if (!page.next) break;
      page = await tx.scanTasks({ kind: REVIEW_TASK_NAME }, 100, page.next);
    }
    return tx.createTask(app.task, { revisionId: revision.id }, { ownership: { kind: "conversation" } });
  }, ctx);
  app.store.setDurableTask(revision.id, String(taskId));
  return { revision: app.store.revision(revision.id)!, taskId };
}

/** Wait for the revision's task; a task that ended without the revision reaching a terminal state fails it. */
export async function finishReview(app: App, started: StartedReview): Promise<RevisionRow> {
  if (started.taskId !== undefined) {
    const settled = await app.harness.waitForTask(started.taskId, ctx);
    const outcome = settled.state.outcome;
    if (!TERMINAL_REVIEW_STATES.includes(app.store.revision(started.revision.id)!.state)) {
      const why = outcome.status === "completed" ? "completed without a terminal revision state" : (outcome.error?.message ?? outcome.reason ?? outcome.status);
      app.store.advanceFromCurrent(started.revision.id, "TERMINAL_ERROR", `durable task ${outcome.status}: ${why}`);
    }
  }
  return app.store.revision(started.revision.id)!;
}
