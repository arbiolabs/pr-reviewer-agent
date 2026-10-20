import type { Table } from "./machine.ts";

// §4.2 per-revision review task machine.
export const REVIEW_STATES = [
  "COLLECT_CONTEXT",
  "TRIAGE",
  "ANALYZE",
  "VERIFY",
  "PREPARE_PUBLICATION",
  "PUBLISH",
  "RECONCILE",
  "COMPLETED",
  "FAILED",
  "SUPERSEDED",
] as const;
export type ReviewState = (typeof REVIEW_STATES)[number];

export const REVIEW_EVENTS = [
  "CONTEXT_PERSISTED",
  "ROUTING_PERSISTED",
  "CANDIDATES_PERSISTED",
  "CANDIDATES_DISPOSED",
  "HEAD_VERIFIED",
  "REVIEW_RECONCILED",
  "NO_PUBLICATION_RECORDED",
  "WRITE_UNCERTAIN",
  "CONFIRMED_NOT_POSTED",
  "NEWER_HEAD",
  "PR_NOT_OPEN",
  "TERMINAL_ERROR",
  "MANUAL_RETRY",
] as const;
export type ReviewEvent = (typeof REVIEW_EVENTS)[number];

// Whether a write may have reached GitHub is the outbox's status, not the revision's: the store refuses PUBLISH's
// NEWER_HEAD and PR_NOT_OPEN unless the outbox is still pending, and any TERMINAL_ERROR moves a `sending` outbox to
// needs_reconciliation (§2 invariants 6 and 10). FAILED + REVIEW_RECONCILED is a later reconcile finding the review.
export const REVIEW_MACHINE: Table<ReviewState, ReviewEvent> = {
  COLLECT_CONTEXT: { CONTEXT_PERSISTED: "TRIAGE", NEWER_HEAD: "SUPERSEDED", PR_NOT_OPEN: "SUPERSEDED", TERMINAL_ERROR: "FAILED" },
  TRIAGE: { ROUTING_PERSISTED: "ANALYZE", NEWER_HEAD: "SUPERSEDED", PR_NOT_OPEN: "SUPERSEDED", TERMINAL_ERROR: "FAILED" },
  ANALYZE: { CANDIDATES_PERSISTED: "VERIFY", NEWER_HEAD: "SUPERSEDED", PR_NOT_OPEN: "SUPERSEDED", TERMINAL_ERROR: "FAILED" },
  VERIFY: { CANDIDATES_DISPOSED: "PREPARE_PUBLICATION", NEWER_HEAD: "SUPERSEDED", PR_NOT_OPEN: "SUPERSEDED", TERMINAL_ERROR: "FAILED" },
  PREPARE_PUBLICATION: { HEAD_VERIFIED: "PUBLISH", NEWER_HEAD: "SUPERSEDED", PR_NOT_OPEN: "SUPERSEDED", TERMINAL_ERROR: "FAILED" },
  PUBLISH: {
    REVIEW_RECONCILED: "COMPLETED",
    NO_PUBLICATION_RECORDED: "COMPLETED",
    WRITE_UNCERTAIN: "RECONCILE",
    NEWER_HEAD: "SUPERSEDED",
    PR_NOT_OPEN: "SUPERSEDED",
    TERMINAL_ERROR: "FAILED",
  },
  RECONCILE: { REVIEW_RECONCILED: "COMPLETED", CONFIRMED_NOT_POSTED: "PUBLISH", TERMINAL_ERROR: "FAILED" },
  COMPLETED: {},
  // §4.1 manual override: only an explicit retry leaves FAILED for new work. COMPLETED and SUPERSEDED stay final.
  FAILED: { MANUAL_RETRY: "COLLECT_CONTEXT", REVIEW_RECONCILED: "COMPLETED" },
  SUPERSEDED: {},
};

/** Terminal for automatic progress; FAILED can still be reopened by the audited MANUAL_RETRY override. */
export const TERMINAL_REVIEW_STATES: readonly ReviewState[] = ["COMPLETED", "FAILED", "SUPERSEDED"];

// §9.3 publication outbox. `sending` is committed before the POST, so a crash leaves proof that a write may have
// happened; from there only reconciliation, never a second POST, decides. MANUAL_RETRY reopens a definite failure.
// A pending outbox is listed before its POST: FOUND adopts a review already there, UNDECIDED means the listing failed.
export const OUTBOX_STATES = ["pending", "sending", "published", "needs_reconciliation", "failed"] as const;
export type OutboxStatus = (typeof OUTBOX_STATES)[number];

export const OUTBOX_EVENTS = ["SEND", "SENT", "UNCERTAIN", "REJECTED", "ABANDON", "FOUND", "ABSENT", "UNDECIDED", "MANUAL_RETRY"] as const;
export type OutboxEvent = (typeof OUTBOX_EVENTS)[number];

export const OUTBOX_MACHINE: Table<OutboxStatus, OutboxEvent> = {
  pending: { SEND: "sending", ABANDON: "failed", FOUND: "published", UNDECIDED: "needs_reconciliation" },
  sending: { SENT: "published", UNCERTAIN: "needs_reconciliation", REJECTED: "failed" },
  needs_reconciliation: { FOUND: "published", ABSENT: "pending", UNDECIDED: "needs_reconciliation" },
  published: {},
  failed: { MANUAL_RETRY: "pending" },
};

// §4.1 PR coordinator machine. DEBOUNCE_ELAPSED is not in the spec's event list; QUIESCING needs a way back.
export const COORDINATOR_STATES = ["ACTIVE", "QUIESCING", "CLOSED", "MERGED"] as const;
export type CoordinatorState = (typeof COORDINATOR_STATES)[number];

export const COORDINATOR_EVENTS = [
  "PR_OPENED",
  "PR_REOPENED",
  "PR_SYNCHRONIZED",
  "REVIEW_REQUESTED",
  "COMMENT_CREATED",
  "PR_CLOSED",
  "PR_MERGED",
  "HEAD_CHANGED",
  "FORCE_REVIEW",
  "DEBOUNCE_ELAPSED",
] as const;
export type CoordinatorEvent = (typeof COORDINATOR_EVENTS)[number];

export const COORDINATOR_MACHINE: Table<CoordinatorState, CoordinatorEvent> = {
  ACTIVE: {
    PR_OPENED: "ACTIVE",
    PR_SYNCHRONIZED: "QUIESCING",
    HEAD_CHANGED: "QUIESCING",
    REVIEW_REQUESTED: "ACTIVE",
    COMMENT_CREATED: "ACTIVE",
    FORCE_REVIEW: "ACTIVE",
    PR_CLOSED: "CLOSED",
    PR_MERGED: "MERGED",
  },
  QUIESCING: {
    PR_SYNCHRONIZED: "QUIESCING",
    HEAD_CHANGED: "QUIESCING",
    REVIEW_REQUESTED: "QUIESCING",
    COMMENT_CREATED: "QUIESCING",
    FORCE_REVIEW: "QUIESCING",
    DEBOUNCE_ELAPSED: "ACTIVE",
    PR_CLOSED: "CLOSED",
    PR_MERGED: "MERGED",
  },
  CLOSED: { PR_REOPENED: "ACTIVE" },
  MERGED: {},
};

// §4.3 candidate-finding submachine. INCONCLUSIVE may only be discarded: prefer no finding over speculation.
export const CANDIDATE_STATES = ["DISCOVERED", "INVESTIGATING", "EVIDENCE_READY", "INCONCLUSIVE", "DISCARDED", "VERIFIED"] as const;
export type CandidateState = (typeof CANDIDATE_STATES)[number];

export const CANDIDATE_EVENTS = ["INVESTIGATE", "EVIDENCE_FOUND", "EVIDENCE_INSUFFICIENT", "REJECT", "CONFIRM"] as const;
export type CandidateEvent = (typeof CANDIDATE_EVENTS)[number];

export const CANDIDATE_MACHINE: Table<CandidateState, CandidateEvent> = {
  DISCOVERED: { INVESTIGATE: "INVESTIGATING", REJECT: "DISCARDED" },
  INVESTIGATING: { EVIDENCE_FOUND: "EVIDENCE_READY", EVIDENCE_INSUFFICIENT: "INCONCLUSIVE", REJECT: "DISCARDED" },
  EVIDENCE_READY: { CONFIRM: "VERIFIED", REJECT: "DISCARDED" },
  INCONCLUSIVE: { REJECT: "DISCARDED" },
  DISCARDED: {},
  VERIFIED: {},
};

// §4.3 persistent finding lifecycle. DETECTED is the M1 addition: a verified finding not yet published.
export const FINDING_STATES = [
  "DETECTED",
  "PUBLISHED",
  "STILL_PRESENT",
  "POTENTIALLY_RESOLVED",
  "OUTDATED",
  "RESOLVED",
  "UNCERTAIN",
] as const;
export type FindingState = (typeof FINDING_STATES)[number];

export const FINDING_EVENTS = [
  "PUBLISHED_TO_GITHUB",
  "OBSERVED_PRESENT",
  "OBSERVED_POSSIBLY_FIXED",
  "OBSERVED_OUTDATED",
  "CONFIRMED_RESOLVED",
  "OBSERVED_UNCERTAIN",
] as const;
export type FindingEvent = (typeof FINDING_EVENTS)[number];

// RESOLVED needs CONFIRMED_RESOLVED from a state that already doubted the finding: a changed line alone never resolves.
export const FINDING_MACHINE: Table<FindingState, FindingEvent> = {
  DETECTED: { PUBLISHED_TO_GITHUB: "PUBLISHED" },
  PUBLISHED: { OBSERVED_PRESENT: "STILL_PRESENT", OBSERVED_POSSIBLY_FIXED: "POTENTIALLY_RESOLVED", OBSERVED_OUTDATED: "OUTDATED" },
  STILL_PRESENT: { OBSERVED_PRESENT: "STILL_PRESENT", OBSERVED_POSSIBLY_FIXED: "POTENTIALLY_RESOLVED", OBSERVED_OUTDATED: "OUTDATED" },
  POTENTIALLY_RESOLVED: { CONFIRMED_RESOLVED: "RESOLVED", OBSERVED_PRESENT: "STILL_PRESENT", OBSERVED_UNCERTAIN: "UNCERTAIN" },
  OUTDATED: { CONFIRMED_RESOLVED: "RESOLVED", OBSERVED_PRESENT: "STILL_PRESENT", OBSERVED_UNCERTAIN: "UNCERTAIN" },
  UNCERTAIN: { OBSERVED_PRESENT: "STILL_PRESENT", OBSERVED_POSSIBLY_FIXED: "POTENTIALLY_RESOLVED", OBSERVED_OUTDATED: "OUTDATED" },
  RESOLVED: {},
};
