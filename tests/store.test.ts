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
  expect(reopened.db.prepare("SELECT name FROM schema_migrations").all()).toEqual([{ name: "001_init.sql" }]);
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
