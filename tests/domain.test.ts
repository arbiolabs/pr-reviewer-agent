import { describe, expect, it } from "vitest";
import { type Table, transition } from "../src/domain/machine.ts";
import {
  CANDIDATE_EVENTS,
  CANDIDATE_MACHINE,
  CANDIDATE_STATES,
  COORDINATOR_EVENTS,
  COORDINATOR_MACHINE,
  COORDINATOR_STATES,
  FINDING_EVENTS,
  FINDING_MACHINE,
  FINDING_STATES,
  OUTBOX_EVENTS,
  OUTBOX_MACHINE,
  OUTBOX_STATES,
  type OutboxStatus,
  publicationStatus,
  REVIEW_EVENTS,
  REVIEW_MACHINE,
  REVIEW_STATES,
  type ReviewState,
} from "../src/domain/machines.ts";

function matrix<S extends string, E extends string>(
  name: string,
  table: Table<S, E>,
  states: readonly S[],
  events: readonly E[],
  legal: readonly string[],
  terminal: readonly S[],
) {
  describe(name, () => {
    const expected = new Map(
      legal.map((edge) => {
        const [from, event, to] = edge.split(" ");
        return [`${from} ${event}`, to];
      }),
    );
    for (const state of states) {
      for (const event of events) {
        const to = expected.get(`${state} ${event}`);
        it(`${state} + ${event} -> ${to ?? "illegal"}`, () => {
          expect(transition(table, state, event)).toEqual(
            to === undefined ? { ok: false, reason: `${event} is not legal in ${state}` } : { ok: true, state: to },
          );
        });
      }
    }
    it("terminal states have no outgoing edges", () => {
      for (const state of terminal) {
        expect(events.filter((event) => transition(table, state, event).ok)).toEqual([]);
      }
    });
    it("every expected edge names known states and events", () => {
      for (const edge of legal) {
        const [from, event, to] = edge.split(" ");
        expect(states).toContain(from);
        expect(events).toContain(event);
        expect(states).toContain(to);
      }
    });
  });
}

matrix("review task machine", REVIEW_MACHINE, REVIEW_STATES, REVIEW_EVENTS, [
  "COLLECT_CONTEXT CONTEXT_PERSISTED TRIAGE",
  "COLLECT_CONTEXT NEWER_HEAD SUPERSEDED",
  "COLLECT_CONTEXT PR_NOT_OPEN SUPERSEDED",
  "COLLECT_CONTEXT TERMINAL_ERROR FAILED",
  "TRIAGE ROUTING_PERSISTED ANALYZE",
  "TRIAGE NEWER_HEAD SUPERSEDED",
  "TRIAGE PR_NOT_OPEN SUPERSEDED",
  "TRIAGE TERMINAL_ERROR FAILED",
  "ANALYZE CANDIDATES_PERSISTED VERIFY",
  "ANALYZE NEWER_HEAD SUPERSEDED",
  "ANALYZE PR_NOT_OPEN SUPERSEDED",
  "ANALYZE TERMINAL_ERROR FAILED",
  "VERIFY CANDIDATES_DISPOSED PREPARE_PUBLICATION",
  "VERIFY NEWER_HEAD SUPERSEDED",
  "VERIFY PR_NOT_OPEN SUPERSEDED",
  "VERIFY TERMINAL_ERROR FAILED",
  "PREPARE_PUBLICATION HEAD_VERIFIED PUBLISH",
  "PREPARE_PUBLICATION APPROVAL_REQUIRED AWAITING_APPROVAL",
  "PREPARE_PUBLICATION NEWER_HEAD SUPERSEDED",
  "PREPARE_PUBLICATION PR_NOT_OPEN SUPERSEDED",
  "PREPARE_PUBLICATION TERMINAL_ERROR FAILED",
  "AWAITING_APPROVAL APPROVED PUBLISH",
  "AWAITING_APPROVAL NEWER_HEAD SUPERSEDED",
  "AWAITING_APPROVAL PR_NOT_OPEN SUPERSEDED",
  "AWAITING_APPROVAL CANCELLED FAILED",
  "PUBLISH REVIEW_RECONCILED COMPLETED",
  "PUBLISH NO_PUBLICATION_RECORDED COMPLETED",
  "PUBLISH WRITE_UNCERTAIN RECONCILE",
  "PUBLISH NEWER_HEAD SUPERSEDED",
  "PUBLISH PR_NOT_OPEN SUPERSEDED",
  "PUBLISH TERMINAL_ERROR FAILED",
  "RECONCILE REVIEW_RECONCILED COMPLETED",
  "RECONCILE CONFIRMED_NOT_POSTED PUBLISH",
  "RECONCILE TERMINAL_ERROR FAILED",
  "FAILED MANUAL_RETRY COLLECT_CONTEXT",
  "FAILED REVIEW_RECONCILED COMPLETED",
], ["COMPLETED", "SUPERSEDED"]);

it("derives the publication status the TUI shows from the revision and its outbox", () => {
  const cases: [string, string, string | null][] = [
    ["AWAITING_APPROVAL", "pending", "awaiting_approval"],
    ["PUBLISH", "pending", "publishing"],
    ["PUBLISH", "sending", "publishing"],
    ["RECONCILE", "needs_reconciliation", "uncertain"],
    ["FAILED", "needs_reconciliation", "uncertain"],
    ["COMPLETED", "published", "published"],
    ["SUPERSEDED", "failed", "failed"],
    ["FAILED", "pending", "failed"],
    ["COLLECT_CONTEXT", "pending", null],
  ];
  expect(cases.map(([state, outbox]) => publicationStatus(state as ReviewState, outbox as OutboxStatus))).toEqual(cases.map((c) => c[2]));
});

matrix("publication outbox", OUTBOX_MACHINE, OUTBOX_STATES, OUTBOX_EVENTS, [
  "pending SEND sending",
  "pending ABANDON failed",
  "pending FOUND published",
  "pending UNDECIDED needs_reconciliation",
  "sending SENT published",
  "sending NOT_SENT pending",
  "sending UNCERTAIN needs_reconciliation",
  "sending REJECTED failed",
  "needs_reconciliation FOUND published",
  "needs_reconciliation ABSENT pending",
  "needs_reconciliation UNDECIDED needs_reconciliation",
  "failed MANUAL_RETRY pending",
], ["published"]);

matrix("coordinator machine", COORDINATOR_MACHINE, COORDINATOR_STATES, COORDINATOR_EVENTS, [
  "ACTIVE PR_OPENED ACTIVE",
  "ACTIVE PR_SYNCHRONIZED QUIESCING",
  "ACTIVE HEAD_CHANGED QUIESCING",
  "ACTIVE REVIEW_REQUESTED ACTIVE",
  "ACTIVE COMMENT_CREATED ACTIVE",
  "ACTIVE FORCE_REVIEW ACTIVE",
  "ACTIVE PR_CLOSED CLOSED",
  "ACTIVE PR_MERGED MERGED",
  "QUIESCING PR_SYNCHRONIZED QUIESCING",
  "QUIESCING HEAD_CHANGED QUIESCING",
  "QUIESCING REVIEW_REQUESTED QUIESCING",
  "QUIESCING COMMENT_CREATED QUIESCING",
  "QUIESCING FORCE_REVIEW QUIESCING",
  "QUIESCING DEBOUNCE_ELAPSED ACTIVE",
  "QUIESCING PR_CLOSED CLOSED",
  "QUIESCING PR_MERGED MERGED",
  "CLOSED PR_REOPENED ACTIVE",
], ["MERGED"]);

matrix("candidate submachine", CANDIDATE_MACHINE, CANDIDATE_STATES, CANDIDATE_EVENTS, [
  "DISCOVERED INVESTIGATE INVESTIGATING",
  "DISCOVERED REJECT DISCARDED",
  "INVESTIGATING EVIDENCE_FOUND EVIDENCE_READY",
  "INVESTIGATING EVIDENCE_INSUFFICIENT INCONCLUSIVE",
  "INVESTIGATING REJECT DISCARDED",
  "EVIDENCE_READY CONFIRM VERIFIED",
  "EVIDENCE_READY REJECT DISCARDED",
  "INCONCLUSIVE REJECT DISCARDED",
], ["DISCARDED", "VERIFIED"]);

matrix("finding lifecycle", FINDING_MACHINE, FINDING_STATES, FINDING_EVENTS, [
  "DETECTED PUBLISHED_TO_GITHUB PUBLISHED",
  "PUBLISHED OBSERVED_PRESENT STILL_PRESENT",
  "PUBLISHED OBSERVED_POSSIBLY_FIXED POTENTIALLY_RESOLVED",
  "PUBLISHED OBSERVED_OUTDATED OUTDATED",
  "STILL_PRESENT OBSERVED_PRESENT STILL_PRESENT",
  "STILL_PRESENT OBSERVED_POSSIBLY_FIXED POTENTIALLY_RESOLVED",
  "STILL_PRESENT OBSERVED_OUTDATED OUTDATED",
  "POTENTIALLY_RESOLVED CONFIRMED_RESOLVED RESOLVED",
  "POTENTIALLY_RESOLVED OBSERVED_PRESENT STILL_PRESENT",
  "POTENTIALLY_RESOLVED OBSERVED_UNCERTAIN UNCERTAIN",
  "OUTDATED CONFIRMED_RESOLVED RESOLVED",
  "OUTDATED OBSERVED_PRESENT STILL_PRESENT",
  "OUTDATED OBSERVED_UNCERTAIN UNCERTAIN",
  "UNCERTAIN OBSERVED_PRESENT STILL_PRESENT",
  "UNCERTAIN OBSERVED_POSSIBLY_FIXED POTENTIALLY_RESOLVED",
  "UNCERTAIN OBSERVED_OUTDATED OUTDATED",
], ["RESOLVED"]);
