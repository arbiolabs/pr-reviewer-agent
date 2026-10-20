import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import type { PullInfo } from "../src/domain/types.ts";
import { AppStore } from "../src/persistence/store.ts";

const pull: PullInfo = {
  installationId: 0,
  repositoryId: 42,
  fullName: "acme/shop",
  number: 7,
  title: "t",
  body: "",
  baseSha: "b".repeat(40),
  headSha: "h".repeat(40),
  state: "open",
  merged: false,
  private: false,
  cloneUrl: "file:///nowhere",
};

function openFresh() {
  const path = join(mkdtempSync(join(tmpdir(), "store-")), "app.sqlite");
  return { path, store: AppStore.open(path) };
}

it("derives the same revision id on reopen and applies migrations once", () => {
  const { path, store } = openFresh();
  const prId = store.upsertPull(pull);
  const key = { prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" };
  const first = store.ensureRevision(key);
  store.close();
  const reopened = AppStore.open(path);
  expect(reopened.ensureRevision(key).id).toBe(first.id);
  expect(reopened.db.prepare("SELECT name FROM schema_migrations").all()).toEqual([{ name: "001_init.sql" }, { name: "002_code_read.sql" }, { name: "003_publication.sql" }, { name: "004_coordinator.sql" }, { name: "005_webhook_retry.sql" }, { name: "006_meta.sql" }, { name: "050_stream_meta.sql" }, { name: "051_journal_without_revision.sql" }, { name: "052_control_commands.sql" }]);
  expect(reopened.revisionsForPr(prId).length).toBe(1);
});

it("advances through the domain machine with one audit row per transition and converges on retry", () => {
  const { store } = openFresh();
  const prId = store.upsertPull(pull);
  const rev = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
  expect(store.advance(rev.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED")).toBe("TRIAGE");
  expect(store.advance(rev.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED")).toBe("TRIAGE");
  expect(() => store.advance(rev.id, "ANALYZE", "CANDIDATES_PERSISTED")).toThrow(
    `revision ${rev.id} is TRIAGE, expected ANALYZE for CANDIDATES_PERSISTED`,
  );
  expect(() => store.advance(rev.id, "TRIAGE", "HEAD_VERIFIED")).toThrow("HEAD_VERIFIED is not legal in TRIAGE");
  expect(store.auditEvents(rev.id).map((e) => [e.from_state, e.event, e.to_state])).toEqual([["COLLECT_CONTEXT", "CONTEXT_PERSISTED", "TRIAGE"]]);
});

it("rolls back the transition when a write in the same transaction fails", () => {
  const { store } = openFresh();
  const prId = store.upsertPull(pull);
  const rev = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
  expect(() =>
    store.advance(rev.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED", null, () => {
      throw new Error("disk full");
    }),
  ).toThrow("disk full");
  expect(store.revision(rev.id)!.state).toBe("COLLECT_CONTEXT");
  expect(store.auditEvents(rev.id)).toEqual([]);
});

function revisionInPublish(store: AppStore) {
  const prId = store.upsertPull(pull);
  const revision = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m2" });
  store.db.prepare("UPDATE review_revisions SET state = 'PUBLISH' WHERE id = ?").run(revision.id);
  const outbox = store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:p1`, payloadHash: "h", payloadJson: "{}" });
  return { revision, outbox };
}

it("inserts one outbox row per idempotency key and returns the winner on a rerun", () => {
  const { store } = openFresh();
  const { revision, outbox } = revisionInPublish(store);
  const again = store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:p1`, payloadHash: "other", payloadJson: "[]" });
  expect([again.id, again.payload_hash, store.db.prepare("SELECT count(*) AS n FROM publication_outbox").get()]).toEqual([outbox.id, "h", { n: 1 }]);
});

it("refuses to supersede a PUBLISH whose POST may have been sent, and fails it into reconciliation instead", () => {
  const { store } = openFresh();
  const { revision, outbox } = revisionInPublish(store);
  store.advanceOutbox(outbox.id, "SEND");
  expect(store.advanceFromCurrent(revision.id, "NEWER_HEAD", "head moved")).toBe(undefined);
  expect(store.revision(revision.id)!.state).toBe("PUBLISH");
  expect(store.advanceFromCurrent(revision.id, "TERMINAL_ERROR", "db down")).toBe("FAILED");
  expect(store.outbox(outbox.id)).toMatchObject({ status: "needs_reconciliation", attempt_count: 1, last_error: "revision failed while the POST was in flight" });
});

it("abandons a pending outbox with its superseded revision", () => {
  const { store } = openFresh();
  const { revision, outbox } = revisionInPublish(store);
  expect(store.advanceFromCurrent(revision.id, "NEWER_HEAD", "head moved")).toBe("SUPERSEDED");
  expect(store.outboxEvents(outbox.id)).toEqual([{ from_status: "pending", event: "ABANDON", to_status: "failed", detail: "superseded" }]);
});

function revisionAwaitingApproval(store: AppStore) {
  const prId = store.upsertPull(pull);
  const revision = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1+publish", reviewerVersion: "m3" });
  store.db.prepare("UPDATE review_revisions SET state = 'PREPARE_PUBLICATION' WHERE id = ?").run(revision.id);
  store.advance(revision.id, "PREPARE_PUBLICATION", "APPROVAL_REQUIRED", "head still h", () => {
    store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:p1`, payloadHash: "h", payloadJson: "{}" });
  });
  return { revision, outbox: store.outboxForRevision(revision.id)! };
}

const publicationJournal = (store: AppStore, revisionId: string) =>
  (store.db.prepare("SELECT detail FROM audit_events WHERE revision_id = ? AND event = 'PUBLICATION_CHANGED' ORDER BY seq").all(revisionId) as { detail: string }[]).map((r) => r.detail);

it("journals one publication status row per change, from the outbox insert to the posted review", () => {
  const { store } = openFresh();
  const { revision, outbox } = revisionAwaitingApproval(store);
  expect(store.revision(revision.id)!.state).toBe("AWAITING_APPROVAL");
  expect(publicationJournal(store, revision.id)).toEqual(["publishing", "awaiting_approval"]);
  store.advance(revision.id, "AWAITING_APPROVAL", "APPROVED", "approved");
  store.advanceOutbox(outbox.id, "SEND");
  store.advanceOutbox(outbox.id, "NOT_SENT", "429");
  store.advanceOutbox(outbox.id, "SEND");
  store.tx(() => {
    store.advanceOutbox(outbox.id, "SENT", "review 7", 7);
    store.advance(revision.id, "PUBLISH", "REVIEW_RECONCILED", "review 7 posted");
  });
  expect(publicationJournal(store, revision.id)).toEqual(["publishing", "awaiting_approval", "publishing", "published"]);
  expect(store.reopenings(revision.id)).toEqual(["APPROVED"]);
  // Status rows are stream-only: the audit trail keeps the machine's transitions.
  expect(store.auditEvents(revision.id).map((e) => e.event)).toEqual(["APPROVAL_REQUIRED", "APPROVED", "REVIEW_RECONCILED"]);
});

it("abandons the unsent outbox when an awaiting revision is superseded or cancelled", () => {
  const { store } = openFresh();
  const superseded = revisionAwaitingApproval(store);
  expect(store.advanceFromCurrent(superseded.revision.id, "NEWER_HEAD", "head moved")).toBe("SUPERSEDED");
  expect(store.outbox(superseded.outbox.id)).toMatchObject({ status: "failed", last_error: "superseded" });
  expect(publicationJournal(store, superseded.revision.id)).toEqual(["publishing", "awaiting_approval", "failed"]);

  const other = { ...pull, number: 8 };
  const prId = store.upsertPull(other);
  const revision = store.ensureRevision({ prId, baseSha: other.baseSha, headSha: other.headSha, policyVersion: "1+publish", reviewerVersion: "m3" });
  store.db.prepare("UPDATE review_revisions SET state = 'AWAITING_APPROVAL' WHERE id = ?").run(revision.id);
  const outbox = store.ensureOutbox({ revisionId: revision.id, idempotencyKey: `${revision.id}:p1`, payloadHash: "h", payloadJson: "{}" });
  expect(store.advance(revision.id, "AWAITING_APPROVAL", "CANCELLED", "cancelled from the TUI")).toBe("FAILED");
  expect(store.outboxEvents(outbox.id)).toEqual([{ from_status: "pending", event: "ABANDON", to_status: "failed", detail: "cancelled" }]);
  expect(store.advance(revision.id, "FAILED", "MANUAL_RETRY", "retry 1")).toBe("COLLECT_CONTEXT");
  expect([store.outbox(outbox.id)!.status, store.reopenings(revision.id)]).toEqual(["pending", ["MANUAL_RETRY"]]);
});

it("records an accepted command once per idempotency key and keeps a cancel reason", () => {
  const { store } = openFresh();
  expect(store.commandResult("review.start:p:h")).toBe(undefined);
  store.recordCommand("review.start:p:h", { command: "review.start", operationId: "review.start:rev-1" });
  store.recordCommand("review.start:p:h", { command: "review.start", operationId: "review.start:rev-2" });
  expect(store.commandResult("review.start:p:h")).toEqual({ command: "review.start", operationId: "review.start:rev-1" });
  const prId = store.upsertPull(pull);
  const rev = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
  expect(store.revision(rev.id)!.cancel_reason).toBe(null);
  store.requestCancel(rev.id, "cancelled from the TUI");
  expect(store.revision(rev.id)!.cancel_reason).toBe("cancelled from the TUI");
});

it("journals tool activity and revision creation without adding to the audit trail", () => {
  const { store } = openFresh();
  const prId = store.upsertPull(pull);
  const rev = store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
  store.ensureRevision({ prId, baseSha: pull.baseSha, headSha: pull.headSha, policyVersion: "1", reviewerVersion: "m1" });
  store.advance(rev.id, "COLLECT_CONTEXT", "CONTEXT_PERSISTED");
  store.appendActivity(rev.id, "read_file", `src/\u001b[31mx.ts\n${"y".repeat(300)}`);
  const journal = store.db.prepare("SELECT seq, from_state, event, to_state, detail FROM audit_events ORDER BY seq").all();
  expect(journal).toEqual([
    { seq: 1, from_state: "COLLECT_CONTEXT", event: "PR_UPDATED", to_state: "COLLECT_CONTEXT", detail: null },
    { seq: 2, from_state: "COLLECT_CONTEXT", event: "REVISION_CREATED", to_state: "COLLECT_CONTEXT", detail: null },
    { seq: 3, from_state: "COLLECT_CONTEXT", event: "CONTEXT_PERSISTED", to_state: "TRIAGE", detail: null },
    { seq: 4, from_state: "TRIAGE", event: "TOOL_ACTIVITY", to_state: "TRIAGE", detail: `read_file src/x.ts ${"y".repeat(181)}` },
  ]);
  expect(store.auditEvents(rev.id).map((e) => e.seq)).toEqual([3]);
  expect(store.db.prepare("SELECT key, length(value) AS n FROM meta WHERE key = 'stream_epoch'").all()).toEqual([{ key: "stream_epoch", n: 32 }]);
});
