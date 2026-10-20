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
  expect(reopened.db.prepare("SELECT name FROM schema_migrations").all()).toEqual([{ name: "001_init.sql" }, { name: "002_code_read.sql" }, { name: "050_stream_meta.sql" }, { name: "051_journal_without_revision.sql" }]);
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
  expect(store.db.prepare("SELECT key, length(value) AS n FROM meta").all()).toEqual([{ key: "stream_epoch", n: 32 }]);
});
