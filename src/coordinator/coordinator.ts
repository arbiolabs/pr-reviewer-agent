import { transition } from "../domain/machine.ts";
import { COORDINATOR_MACHINE, type CoordinatorEvent, type CoordinatorState, type ReviewState, TERMINAL_REVIEW_STATES } from "../domain/machines.ts";
import type { PullInfo } from "../domain/types.ts";
import { GitHubError } from "../github/client.ts";
import type { PullRef, PullSource } from "../github/pulls.ts";
import type { AppStore } from "../persistence/store.ts";

export type Trigger = { kind: "ignore"; reason: string } | { kind: "pr"; ref: PullRef & { installationId: number }; event: CoordinatorEvent };

type Payload = {
  action?: string;
  sender?: { type?: string };
  installation?: { id?: number };
  repository?: { full_name?: string };
  pull_request?: { number?: number; merged?: boolean };
  issue?: { number?: number; pull_request?: unknown };
  comment?: { body?: string; user?: { login?: string } };
};

const PR_ACTIONS: Record<string, CoordinatorEvent> = {
  opened: "PR_OPENED",
  reopened: "PR_REOPENED",
  synchronize: "PR_SYNCHRONIZED",
  closed: "PR_CLOSED",
  review_requested: "REVIEW_REQUESTED",
};

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * §9.2 step 4: which deliveries can start work. Bot senders and the app's own comments never do (no reply loops), and
 * a comment does only with an explicit `@<bot> review` on a PR (§17 item 7).
 */
export function routeEvent(eventType: string, payload: Payload, botLogin: string): Trigger {
  if (payload.sender?.type === "Bot") return { kind: "ignore", reason: "bot sender" };
  const [owner, repo] = (payload.repository?.full_name ?? "").split("/");
  const installationId = payload.installation?.id ?? 0;
  if (!owner || !repo) return { kind: "ignore", reason: "no repository" };
  if (eventType === "pull_request") {
    const event = PR_ACTIONS[payload.action ?? ""];
    const number = payload.pull_request?.number;
    if (!event || number === undefined) return { kind: "ignore", reason: `pull_request ${payload.action}` };
    return { kind: "pr", ref: { owner, repo, number, installationId }, event: event === "PR_CLOSED" && payload.pull_request?.merged ? "PR_MERGED" : event };
  }
  if (eventType === "issue_comment") {
    const number = payload.issue?.number;
    if (payload.action !== "created" || !payload.issue?.pull_request || number === undefined) return { kind: "ignore", reason: "not a new PR comment" };
    if (payload.comment?.user?.login === botLogin) return { kind: "ignore", reason: "own comment" };
    const handle = botLogin.replace(/\[bot\]$/, "");
    if (!new RegExp(`(^|\\s)@${escape(handle)}\\s+review\\b`, "i").test(payload.comment?.body ?? "")) return { kind: "ignore", reason: "no review mention" };
    return { kind: "pr", ref: { owner, repo, number, installationId }, event: "COMMENT_CREATED" };
  }
  return { kind: "ignore", reason: `unsupported event ${eventType}` };
}

const LIFECYCLE: readonly CoordinatorEvent[] = ["PR_OPENED", "PR_REOPENED", "PR_CLOSED", "PR_MERGED"];
const ASKS: readonly CoordinatorEvent[] = ["REVIEW_REQUESTED", "COMMENT_CREATED"];

/** The lifecycle event GitHub's current state implies, whatever the delivery said (§9.2 step 5). */
function lifecycleEvent(status: CoordinatorState, live: PullInfo): CoordinatorEvent | undefined {
  if (live.merged) return "PR_MERGED";
  if (live.state === "closed") return "PR_CLOSED";
  return status === "CLOSED" ? "PR_REOPENED" : undefined;
}

// Revisions the coordinator may abort; past PREPARE_PUBLICATION a write may be in flight and the publish head guard decides.
const ABORTABLE: readonly ReviewState[] = ["COLLECT_CONTEXT", "TRIAGE", "ANALYZE", "VERIFY", "PREPARE_PUBLICATION"];

export type CoordinatorDeps = {
  store: AppStore;
  pulls: PullSource;
  botLogin(installationId: number): Promise<string>;
  /** Start (or find) the review of this authoritative head. */
  start(pull: PullInfo): Promise<void>;
  abort(durableTaskId: string): Promise<void>;
  debounceMs: number;
  leaseMs: number;
  owner: string;
  /** A failing delivery waits `backoffMs * 2^(n-1)` after its n-th failure and is dead after `maxAttempts`. */
  webhookRetry: { maxAttempts: number; backoffMs: number };
  now(): number;
  report(error: unknown): void;
};

/** §4.1 and §9.2: drains the webhook inbox into the coordinator machine, then starts each PR's latest head once it is due. */
export class Coordinator {
  private readonly deps: CoordinatorDeps;
  /** Aborts in flight, per revision. They run beside the ticks, since an abort can wait on a task for a long time. */
  private readonly aborts = new Map<string, Promise<void>>();

  constructor(deps: CoordinatorDeps) {
    this.deps = deps;
  }

  async tick(): Promise<void> {
    const { store, webhookRetry } = this.deps;
    for (const event of store.dueWebhooks(this.deps.now())) {
      try {
        await this.handle(event.delivery_id, event.event_type, JSON.parse(event.payload_json));
      } catch (error) {
        this.deps.report(error);
        // A definite GitHub refusal (the PR is gone or unreadable) never gets better; anything else backs off.
        const attempts = event.attempts + 1;
        const permanent = error instanceof GitHubError && error.class === "fatal";
        const retryAt = permanent || attempts >= webhookRetry.maxAttempts ? null : this.deps.now() + webhookRetry.backoffMs * 2 ** (attempts - 1);
        store.failWebhook(event.delivery_id, error instanceof Error ? error.message : String(error), retryAt);
      }
    }
    for (const pr of this.deps.store.duePulls(this.deps.now())) {
      const [owner, repo] = pr.repo_full_name.split("/") as [string, string];
      await this.withLease(pr.id, async () => {
        const live = await this.deps.pulls.getPull({ owner, repo, number: pr.number, installationId: pr.installation_id });
        const status = this.apply(pr.id, pr.status, live, "DEBOUNCE_ELAPSED");
        // Cleared only after the start succeeds; a failed start is due again next tick, and starting is idempotent.
        if (status === "ACTIVE") await this.deps.start(live);
        this.deps.store.setPullSchedule(pr.id, status, null);
      }).catch((error) => this.deps.report(error));
    }
  }

  private async handle(deliveryId: string, eventType: string, payload: Payload): Promise<void> {
    const store = this.deps.store;
    const bot = eventType === "issue_comment" ? await this.deps.botLogin(payload.installation?.id ?? 0) : "";
    const trigger = routeEvent(eventType, payload, bot);
    if (trigger.kind === "ignore") return store.markWebhookProcessed(deliveryId, `ignored: ${trigger.reason}`);
    const live = await this.deps.pulls.getPull(trigger.ref);
    const prId = store.upsertPull(live);
    const done = await this.withLease(prId, async () => {
      const status = this.apply(prId, store.pullById(prId)!.status, live, trigger.event);
      const open = status === "ACTIVE" || status === "QUIESCING";
      // Debounce: each event restarts the wait, so a burst of pushes reviews only the head GitHub has when it ends.
      store.setPullSchedule(prId, status, open ? this.deps.now() + this.deps.debounceMs : null);
      // A closed or merged PR supersedes every in-flight revision, whatever its head.
      this.supersedeOlder(prId, open ? live.headSha : undefined);
      // A request to review a head that already has a final review starts nothing; say so. Only an admin retry reopens it.
      const reviewed = ASKS.includes(trigger.event) ? store.revisionsForPr(prId).filter((r) => r.head_sha === live.headSha && TERMINAL_REVIEW_STATES.includes(r.state)).at(-1) : undefined;
      store.markWebhookProcessed(deliveryId, reviewed ? `already_reviewed:${reviewed.state}` : `${trigger.event} -> ${status}`);
    });
    if (!done) throw new Error(`PR ${prId} is leased by another worker; delivery ${deliveryId} waits`);
  }

  /**
   * GitHub's live state alone decides open, closed and merged; a delivery adds only what it asks for (a push, a review
   * request, a mention). Each event goes through the machine, and an illegal one changes nothing.
   */
  private apply(prId: string, status: CoordinatorState, live: PullInfo, trigger: CoordinatorEvent): CoordinatorState {
    const headMoved = this.deps.store.unfinishedRevisions(prId).some((r) => r.head_sha !== live.headSha);
    const events: (CoordinatorEvent | undefined)[] = [lifecycleEvent(status, live), headMoved ? "HEAD_CHANGED" : undefined, LIFECYCLE.includes(trigger) ? undefined : trigger];
    let state = status;
    for (const event of events) {
      if (event === undefined) continue;
      const next = transition(COORDINATOR_MACHINE, state, event);
      if (next.ok) state = next.state;
    }
    return state;
  }

  /**
   * §11: in-flight revisions of other heads (all, with no head) are marked superseded_requested; those not yet publishing
   * are aborted. A revision parked for approval has no task and nothing sent, so it is superseded on the spot.
   */
  private supersedeOlder(prId: string, headSha: string | undefined): void {
    for (const revision of this.deps.store.unfinishedRevisions(prId)) {
      if (revision.head_sha === headSha) continue;
      if (revision.state === "AWAITING_APPROVAL") {
        this.deps.store.advanceFromCurrent(revision.id, headSha === undefined ? "PR_NOT_OPEN" : "NEWER_HEAD", headSha === undefined ? "pr_not_open" : `head moved to ${headSha}`);
        continue;
      }
      this.deps.store.requestSupersede(revision.id);
      if (!revision.durable_task_id || !ABORTABLE.includes(revision.state) || this.aborts.has(revision.id)) continue;
      const abort = this.deps
        .abort(revision.durable_task_id)
        .catch((error) => this.deps.report(error))
        .finally(() => this.aborts.delete(revision.id));
      this.aborts.set(revision.id, abort);
    }
  }

  /** Resolves once every abort started so far has settled; the service awaits it on shutdown. */
  async drain(): Promise<void> {
    while (this.aborts.size > 0) await Promise.all(this.aborts.values());
  }

  private async withLease(prId: string, fn: () => Promise<void>): Promise<boolean> {
    const { store, owner, leaseMs, now } = this.deps;
    if (!store.acquireLease(prId, owner, now(), leaseMs)) return false;
    try {
      await fn();
    } finally {
      store.releaseLease(prId, owner);
    }
    return true;
  }
}
