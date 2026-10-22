import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeAll, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { type CandidateFinding, type ResolutionClaim, stableId } from "../src/domain/types.ts";
import { type EvidenceText, evidenceTexts, headReader, locate, normalize, type ObservedLocation, observe, type PriorFinding } from "../src/reasoning/track.ts";
import { checkResolutions, fingerprint, type VerifyContext, verifyCandidates } from "../src/reasoning/verify.ts";
import { parseDiff } from "../src/source/diff.ts";
import { PinnedRepo, syncRepo } from "../src/source/git.ts";
import { createFixture, DEFECT, type Fixture } from "./fixture.ts";

let fixture: Fixture;
let ctx: VerifyContext;

beforeAll(async () => {
  fixture = createFixture();
  const repo = new PinnedRepo(await syncRepo(mkdtempSync(join(tmpdir(), "cache-")), "acme/shop", fixture.repoDir, 1), fixture.baseSha, fixture.headSha);
  ctx = {
    prId: "pr-1",
    revisionId: "rev-1",
    baseSha: fixture.baseSha,
    headSha: fixture.headSha,
    files: parseDiff(await repo.diff()),
    read: (sha, path) => repo.show(sha, path),
    head: headReader(repo, await repo.changedFiles()),
    prior: [],
    thresholds: DEFAULT_CONFIG.verification,
  };
});

const candidate = (overrides: Partial<CandidateFinding>): CandidateFinding => ({
  id: "c1",
  category: "security",
  severity: "high",
  title: "Order deletion endpoint skips the admin check",
  explanation: "Every other order route is guarded by requireAdmin; DELETE /orders/:id is not.",
  file: DEFECT.file,
  newLine: DEFECT.line,
  codeEvidence: [{ path: DEFECT.file, start: 4, end: 5, refSha: fixture.headSha }],
  reasoningSummary: "Compared the new route with its sibling.",
  confidence: 0.9,
  evidenceKind: "confirmed_by_static_trace",
  ...overrides,
});

async function disposition(c: CandidateFinding) {
  const { dispositions } = await verifyCandidates([c], ctx);
  return dispositions[0]!;
}

it("verifies a candidate anchored on an added line with in-range evidence", async () => {
  const c = candidate({});
  const result = await verifyCandidates([c], ctx);
  expect(result.dispositions).toEqual([
    {
      candidateId: "c1",
      state: "VERIFIED",
      path: ["DISCOVERED", "INVESTIGATING", "EVIDENCE_READY", "VERIFIED"],
      reason: null,
      findingId: result.verified[0]!.findingId,
    },
  ]);
  expect(result.verified.map((v) => v.fingerprint)).toEqual([fingerprint(c)]);
});

it("discards an anchor outside every head-side hunk", async () => {
  expect(await disposition(candidate({ newLine: 1 }))).toMatchObject({
    state: "DISCARDED",
    path: ["DISCOVERED", "INVESTIGATING", "DISCARDED"],
    reason: "src/routes/orders.ts:1 is outside every hunk of the head-side diff",
  });
});

it("discards evidence past the end of the cited file", async () => {
  const d = await disposition(candidate({ codeEvidence: [{ path: DEFECT.file, start: 4, end: 40, refSha: fixture.headSha }] }));
  expect(d.reason).toBe(`evidence src/routes/orders.ts:4-40@${fixture.headSha.slice(0, 12)} is outside 1-6`);
});

it("discards a file outside the changed set", async () => {
  expect((await disposition(candidate({ file: "src/auth/require-admin.ts", newLine: 2 }))).reason).toBe(
    "src/auth/require-admin.ts is not in the changed set",
  );
});

it("rejects traversal in evidence paths", async () => {
  const d = await disposition(candidate({ codeEvidence: [{ path: "../../etc/passwd", start: 1, end: 1, refSha: fixture.headSha }] }));
  expect(d.reason).toBe("path traversal rejected: ../../etc/passwd");
});

it("sends suspected evidence through INCONCLUSIVE to DISCARDED", async () => {
  expect(await disposition(candidate({ evidenceKind: "suspected" }))).toMatchObject({
    state: "DISCARDED",
    path: ["DISCOVERED", "INVESTIGATING", "INCONCLUSIVE", "DISCARDED"],
    reason: "evidence is suspected",
  });
});

it("applies severity and confidence thresholds, and dedupes by fingerprint", async () => {
  expect((await disposition(candidate({ severity: "low" }))).reason).toBe("severity low is below medium");
  expect((await disposition(candidate({ confidence: 0.2 }))).reason).toBe("confidence 0.2 is below 0.5");
  const twice = await verifyCandidates([candidate({}), candidate({ id: "c2", title: "Admin check skips order deletion endpoint" })], ctx);
  expect(twice.dispositions.map((d) => [d.candidateId, d.state, d.reason])).toEqual([
    ["c1", "VERIFIED", null],
    ["c2", "DISCARDED", "duplicate of candidate c1"],
  ]);
});

it("links one candidate to an open finding and keeps a second one at the same lines as a fresh finding, discarding neither", async () => {
  const first = candidate({});
  const location: ObservedLocation = { file: DEFECT.file, newLine: DEFECT.line, headSha: fixture.headSha, evidence: await evidenceTexts(first, fixture.headSha, ctx.read) };
  const open: PriorFinding = { findingId: "f-open", status: "PUBLISHED", candidate: first, location, located: { kind: "same", location } };
  const second = candidate({ id: "c2", title: "Deleting an order never checks who owns it" });
  const { dispositions, verified, unverified } = await verifyCandidates([second, first], { ...ctx, prior: [open] });
  expect(dispositions.map((d) => [d.candidateId, d.state, d.reason])).toEqual([
    ["c2", "VERIFIED", null],
    ["c1", "VERIFIED", null],
  ]);
  expect(verified.map((v) => v.findingId === "f-open")).toEqual([false, true]);
  expect(unverified).toEqual([]);
});

it("links a candidate that names a doubted finding by the finding's text found in the candidate's file, and lists a named re-report that fails as unverified", async () => {
  const first = candidate({});
  const location: ObservedLocation = { file: DEFECT.file, newLine: DEFECT.line, headSha: "older", evidence: [{ path: DEFECT.file, start: 3, end: 5, text: "export function registerOrderRoutes(app: App) {\napp.get(\"/orders\", requireAdmin, listOrders);\napp.delete(\"/orders/:id\", deleteOrder);" }] };
  const doubted: PriorFinding = { findingId: "f-doubted", status: "PUBLISHED", candidate: first, location, located: { kind: "changed", file: DEFECT.file } };
  const named = candidate({ id: "c3", title: "Deleting an order never checks who owns it", priorFindingId: "f-doubted", codeEvidence: [{ path: DEFECT.file, start: 5, end: 5, refSha: fixture.headSha }] });
  const linked = await verifyCandidates([named], { ...ctx, prior: [doubted] });
  expect(linked.dispositions[0]!.findingId).toBe("f-doubted");
  expect(linked.unverified).toEqual([]);
  // A name no open finding has is ignored; the candidate is judged by the other rules, which make this one new.
  const stranger = candidate({ id: "c4", title: "Deleting an order never checks who owns it", priorFindingId: "f-nobody" });
  const failed = await verifyCandidates([{ ...named, evidenceKind: "suspected" }, stranger], { ...ctx, prior: [doubted] });
  expect(failed.dispositions.map((d) => [d.state, d.findingId])).toEqual([
    ["DISCARDED", null],
    ["VERIFIED", stableId("finding", "pr-1", "rev-1", fingerprint(stranger))],
  ]);
  expect(failed.unverified).toEqual(["f-doubted"]);
});

const AUTH = "src/auth/require-admin.ts";
/** A finding seen on an older head whose lines 4-6 also registered a put route, which this head dropped: its cited block is nowhere at this head whole. */
const doubted = (located: PriorFinding["located"], status: PriorFinding["status"] = "PUBLISHED"): PriorFinding => ({
  findingId: "f-doubted",
  status,
  candidate: candidate({}),
  location: { file: DEFECT.file, newLine: DEFECT.line, headSha: "older", evidence: [{ path: DEFECT.file, start: 4, end: 6, text: 'app.get("/orders", requireAdmin, listOrders);\napp.delete("/orders/:id", deleteOrder);\napp.put("/orders/:id", updateOrder);' }] },
  located,
});
const claim = (over: Partial<ResolutionClaim> = {}): ResolutionClaim => ({
  findingId: "f-doubted",
  codeEvidence: [{ path: DEFECT.file, start: 5, end: 5, refSha: fixture.headSha }],
  reason: "the route now has the guard",
  evidenceKind: "confirmed_by_static_trace",
  ...over,
});
const none = { reported: new Map<string, string>(), cited: [] };
const verdict = async (c: ResolutionClaim, prior = doubted({ kind: "changed", file: DEFECT.file })) => (await checkResolutions([c], { ...ctx, prior: [prior] }, none))[0];

it("accepts a resolution claim only on a doubted finding, with head-side evidence in its file that is not a run of the finding's code unchanged", async () => {
  // Line 1 is outside the stored text, so citing it reads as changed code here.
  const outside = claim({ codeEvidence: [{ path: DEFECT.file, start: 1, end: 1, refSha: fixture.headSha }, { path: AUTH, start: 1, end: 2, refSha: fixture.headSha }] });
  expect(await verdict(outside)).toEqual({ findingId: "f-doubted", state: "ACCEPTED", reason: null });
  expect(await verdict(outside, doubted({ kind: "changed", file: DEFECT.file }, "UNCERTAIN"))).toMatchObject({ state: "ACCEPTED" });
  const sha = fixture.headSha.slice(0, 12);
  const cases: [string, ResolutionClaim, PriorFinding?][] = [
    [`evidence src/routes/orders.ts:5-5@${fixture.baseSha.slice(0, 12)} cites a SHA other than the head`, claim({ codeEvidence: [{ path: DEFECT.file, start: 5, end: 5, refSha: fixture.baseSha }] })],
    [`evidence src/routes/orders.ts:4-5@${sha} is the finding's code unchanged`, claim({ codeEvidence: [{ path: DEFECT.file, start: 4, end: 5, refSha: fixture.headSha }] })],
    [`evidence src/routes/orders.ts:5-5@${sha} is the finding's code unchanged`, claim()],
    [`no evidence in ${DEFECT.file}`, claim({ codeEvidence: [{ path: AUTH, start: 1, end: 2, refSha: fixture.headSha }] })],
    [`its cited code is unchanged at ${DEFECT.file}:5`, claim(), doubted({ kind: "same", location: { file: DEFECT.file, newLine: 5, headSha: fixture.headSha, evidence: [] } })],
    ["no open finding f-nobody", claim({ findingId: "f-nobody" })],
    ["evidence is suspected", claim({ evidenceKind: "suspected" })],
    ["no code evidence", claim({ codeEvidence: [] })],
    [`evidence src/routes/orders.ts:4-40@${sha} is outside 1-6`, claim({ codeEvidence: [{ path: DEFECT.file, start: 4, end: 40, refSha: fixture.headSha }] })],
    ["path traversal rejected: ../../etc/passwd", claim({ codeEvidence: [{ path: "../../etc/passwd", start: 1, end: 1, refSha: fixture.headSha }] })],
  ];
  for (const [reason, c, prior] of cases) expect((await verdict(c, prior))?.reason, reason).toBe(reason);
  // A finding whose file is gone is resolved by evidence in a file this pull request changes, not anywhere.
  expect(await verdict(outside, doubted({ kind: "file_gone" }, "OUTDATED"))).toMatchObject({ state: "ACCEPTED" });
  expect((await verdict(claim({ codeEvidence: [{ path: AUTH, start: 1, end: 2, refSha: fixture.headSha }] }), doubted({ kind: "file_gone" })))?.reason).toBe("no evidence in a file this pull request changes");
  expect((await checkResolutions([outside, outside], { ...ctx, prior: [doubted({ kind: "changed", file: DEFECT.file })] }, none)).map((v) => v.reason)).toEqual([null, "duplicate of an earlier claim"]);
});

it("refuses a resolution claim when a candidate of the same revision reports the finding again, verified or not, or still cites its code", async () => {
  const prior = doubted({ kind: "changed", file: DEFECT.file });
  const fixed = claim({ codeEvidence: [{ path: DEFECT.file, start: 1, end: 1, refSha: fixture.headSha }] });
  // c1 is the finding's own title and lines: it links by rule 3 without naming the finding.
  const relinked = await verifyCandidates([candidate({})], { ...ctx, prior: [prior] }, [fixed]);
  expect(relinked.dispositions[0]!.findingId).toBe("f-doubted");
  expect(relinked.verdicts).toEqual([{ findingId: "f-doubted", state: "REFUSED", reason: "candidate c1 reports it again" }]);
  const named = await verifyCandidates([candidate({ id: "c2", title: "Deleting an order never checks who owns it", priorFindingId: "f-doubted", evidenceKind: "suspected" })], { ...ctx, prior: [prior] }, [fixed]);
  expect([named.dispositions[0]!.state, named.unverified, named.verdicts[0]!.reason]).toEqual(["DISCARDED", ["f-doubted"], "candidate c2 reports it again"]);
  // A second defect in the same handler: its verified report cites two of the finding's lines, so the handler is still live.
  const sibling = candidate({ id: "c3", title: "Deleting an order never checks who owns it", codeEvidence: [{ path: DEFECT.file, start: 4, end: 5, refSha: fixture.headSha }] });
  const cited = await verifyCandidates([sibling], { ...ctx, prior: [prior] }, [fixed]);
  expect([cited.dispositions[0]!.state, cited.dispositions[0]!.findingId === "f-doubted", cited.verdicts[0]!.reason]).toEqual(["VERIFIED", false, "candidate c3 still cites its code at src/routes/orders.ts:4-5"]);
  // A helper both cite is not the finding's code, and one shared line is below the floor.
  const helper = candidate({ id: "c4", title: "Deleting an order never checks who owns it", codeEvidence: [{ path: AUTH, start: 1, end: 3, refSha: fixture.headSha }, { path: DEFECT.file, start: 5, end: 5, refSha: fixture.headSha }] });
  const withHelper = { ...prior, location: { ...prior.location, evidence: [...prior.location.evidence, { path: AUTH, start: 1, end: 3, text: "export function requireAdmin(req: Req, res: Res, next: Next) {\nif (!req.user?.isAdmin) return res.status(403).end();\nnext();" }] } };
  expect((await verifyCandidates([helper], { ...ctx, prior: [withHelper] }, [fixed])).verdicts[0]).toMatchObject({ state: "ACCEPTED" });
  expect((await verifyCandidates([], { ...ctx, prior: [prior] }, [fixed])).verdicts[0]).toMatchObject({ state: "ACCEPTED" });
});

/** A head of in-memory files: every line is anchorable, and the tree-wide search scans them all for the line. */
function inMemory(files: Record<string, string[]>): Pick<VerifyContext, "files" | "read" | "head"> {
  const text = async (path: string) => (files[path] ? `${files[path].join("\n")}\n` : undefined);
  return {
    files: new Map(Object.entries(files).map(([path, lines]) => [path, { path, text: "", headLines: new Set(lines.map((_, i) => i + 1)) }])),
    read: (_sha, path) => text(path),
    head: { read: text, renamedTo: () => undefined, grep: async (line) => Object.keys(files).filter((p) => files[p]!.some((l) => l.includes(line))) },
  };
}

const A = "src/a.ts";
const HELPER = "src/helper.ts";
const BASE_A = ["import x;", "", "export function del(req, res) {", "  const id = req.params.id;", "  db.orders.delete(id);", "  res.status(204).end();", "}", "", "// end"];
const BASE_HELPER = ["export function auth(u) {", "  return u.admin === true;", "}"];
const item = (path: string, lines: string[], start: number, end: number): EvidenceText => ({ path, start, end, text: normalize(lines.slice(start - 1, end)) });

/** The verdict on a claim for a finding last seen on `BASE_A`, and the state the finding ends in, at a head of `files`, whose diff hunks cover `hunks` (every line when absent). */
async function resolve(files: Record<string, string[]>, evidence: EvidenceText[], ranges: [number, number][], candidates: CandidateFinding[] = [], head: Partial<VerifyContext["head"]> = {}, hunks: Record<string, number[]> = {}) {
  const at = { ...inMemory(files), prior: [] };
  at.head = { ...at.head, ...head };
  for (const [path, lines] of Object.entries(hunks)) at.files.get(path)!.headLines = new Set(lines);
  const location: ObservedLocation = { file: A, newLine: 5, headSha: "older", evidence };
  const f: PriorFinding = { findingId: "f-a", status: "PUBLISHED", candidate: candidate({ file: A, newLine: 5, codeEvidence: [] }), location, located: await locate(location, ctx.headSha, at.head, { treeWide: false }) };
  const result = await verifyCandidates(candidates, { ...ctx, ...at, prior: [f] }, [claim({ findingId: "f-a", codeEvidence: ranges.map(([start, end]) => ({ path: A, start, end, refSha: ctx.headSha })) })]);
  return [result.verdicts[0]!.reason, observe([f], result.verified, result).find((o) => o.findingId === "f-a")!.path.at(-1)];
}

it("refuses a resolution while any of the finding's own-file code survives at head, whatever made locate call it changed", async () => {
  const sha = ctx.headSha.slice(0, 12);
  // Only a helper the finding also cited changed; its own lines are byte-identical.
  const helperChanged = { [A]: BASE_A, [HELPER]: ["export function auth(u) {", "  return Boolean(u?.admin);", "}"] };
  expect(await resolve(helperChanged, [item(A, BASE_A, 3, 6), item(HELPER, BASE_HELPER, 1, 3)], [[3, 7]])).toEqual(["its code survives at src/a.ts:3", "POTENTIALLY_RESOLVED"]);
  // One line shifted down by an insert above it: with six tokens it is found where it went; with two it is found in place only, and then anywhere in its file by the survival check.
  const shifted = { [A]: ["import x;", "import y;", ...BASE_A.slice(1)], [HELPER]: BASE_HELPER };
  expect(await resolve(shifted, [item(A, BASE_A, 5, 5)], [[5, 6]])).toEqual(["its cited code is unchanged at src/a.ts:6", "STILL_PRESENT"]);
  const headed = { [A]: ["// orders", ...BASE_A], [HELPER]: BASE_HELPER };
  expect(await resolve(headed, [item(A, BASE_A, 1, 1)], [[5, 6]])).toEqual(["its code survives at src/a.ts:2", "POTENTIALLY_RESOLVED"]);
  // A comment inserted inside the cited block is not a change: the block's code is found in place, so the finding is still present.
  const commented = { [A]: [...BASE_A.slice(0, 4), "  // checked upstream", ...BASE_A.slice(4)], [HELPER]: BASE_HELPER };
  expect(await resolve(commented, [item(A, BASE_A, 3, 6)], [[3, 8]])).toEqual(["its cited code is unchanged at src/a.ts:5", "STILL_PRESENT"]);
  // The file is gone and its code reappears verbatim in another file: a claim on any changed file does not resolve it.
  const moved = { "src/b.ts": BASE_A, "src/c.ts": ["x"] };
  expect(await resolve(moved, [item(A, BASE_A, 3, 6)], [[1, 1]])).toEqual(["its code survives at src/b.ts:3", "OUTDATED"]);
  expect(await resolve(moved, [item(A, BASE_A, 3, 6)], [[1, 1]], [], { grep: async () => undefined })).toEqual(["its code may survive, in more files than the search lists", "OUTDATED"]);
  // A guard inserted where the delete was: nothing of the block is found whole, and a claim that cites the guard, alone or with the block around it, is new code.
  const guarded = { [A]: [...BASE_A.slice(0, 4), "  if (!req.user.admin) return res.status(403).end();", ...BASE_A.slice(4)], [HELPER]: BASE_HELPER };
  expect(await resolve(guarded, [item(A, BASE_A, 3, 6)], [[5, 5]])).toEqual([null, "RESOLVED"]);
  expect(await resolve(guarded, [item(A, BASE_A, 3, 6)], [[3, 8]])).toEqual([null, "RESOLVED"]);
  // A claim that cites the old block without the guard cites the finding's code unchanged.
  expect(await resolve(guarded, [item(A, BASE_A, 3, 6)], [[6, 7]])).toEqual([`evidence src/a.ts:6-7@${sha} is the finding's code unchanged`, "POTENTIALLY_RESOLVED"]);
});

it("reads code as tokens: an edit that adds a comment, a blank line, whitespace or drops a semicolon leaves the finding present, and a claim on a comment cites no code", async () => {
  const sha = ctx.headSha.slice(0, 12);
  const block = [item(A, BASE_A, 3, 6)];
  const commented = { [A]: [...BASE_A.slice(0, 4), "  // auth checked upstream", ...BASE_A.slice(4)], [HELPER]: BASE_HELPER };
  const blank = { [A]: [...BASE_A.slice(0, 4), "", ...BASE_A.slice(4)], [HELPER]: BASE_HELPER };
  const noSemicolon = { [A]: BASE_A.with(4, "  db.orders.delete(id)"), [HELPER]: BASE_HELPER };
  const crlf = { [A]: BASE_A.map((l) => `\t${l}\r`), [HELPER]: BASE_HELPER };
  const cases: [Record<string, string[]>, [number, number][]][] = [
    [commented, [[5, 5]]],
    [blank, [[5, 5]]],
    [blank, [[5, 6]]],
    [noSemicolon, [[5, 5]]],
    [crlf, [[5, 5]]],
  ];
  for (const [files, ranges] of cases) expect(await resolve(files, block, ranges), JSON.stringify(ranges)).toEqual(["its cited code is unchanged at src/a.ts:5", "STILL_PRESENT"]);
  // A helper the finding also cited changed, so locate doubts it, and the block gained a comment: its code still survives whole.
  const helperAndComment = { ...commented, [HELPER]: ["export function auth(u) {", "  return Boolean(u?.admin);", "}"] };
  expect(await resolve(helperAndComment, [...block, item(HELPER, BASE_HELPER, 1, 3)], [[5, 5]])).toEqual(["its code survives at src/a.ts:3", "POTENTIALLY_RESOLVED"]);
  // A real guard went in, but the claim cites only the comment above it.
  const guardedCommented = { [A]: [...BASE_A.slice(0, 4), "  // admins only", "  if (!req.user.admin) return res.status(403).end();", ...BASE_A.slice(4)], [HELPER]: BASE_HELPER };
  expect(await resolve(guardedCommented, block, [[5, 5]])).toEqual([`evidence src/a.ts:5-5@${sha} cites no code`, "POTENTIALLY_RESOLVED"]);
  expect(await resolve(guardedCommented, block, [[6, 6]])).toEqual([null, "RESOLVED"]);
});

it("searches the tree for a short item with enough tokens, and finds a moved block whose raw lines hold runs of spaces", async () => {
  // One line of six tokens moved verbatim into a helper the route now calls.
  const removeOrder = ["export function removeOrder(id) {", "  db.orders.delete(id);", "  return db.get(id);", "}"];
  const callsHelper = { [A]: BASE_A.with(4, "  removeOrder(id);"), [HELPER]: removeOrder };
  expect(await resolve(callsHelper, [item(A, BASE_A, 5, 5)], [[5, 5]])).toEqual(["its code survives at src/helper.ts:2", "POTENTIALLY_RESOLVED"]);
  // Five tokens are not distinctive: the line is looked for in the finding's file only, and the helper's copy is not it.
  expect(await resolve(callsHelper, [item(A, removeOrder, 3, 3)], [[5, 5]])).toEqual([null, "RESOLVED"]);
  // The file was renamed with edits the diff does not call a rename, and the line is in the new file.
  const renamed = { "src/b.ts": ["import x;", "import y;", "export function del(req, res) {", "  log(req);", "  db.orders.delete(id);", "}"], "src/c.ts": ["x"] };
  expect(await resolve(renamed, [item(A, BASE_A, 5, 5)], [[1, 1]])).toEqual(["its code survives at src/b.ts:5", "OUTDATED"]);
  // Aligned assignments: the raw lines hold runs of spaces that the stored text collapsed, which a literal search for a stored line never finds.
  const aligned = ["import x;", "", "export function del(req, res) {", "  const id      =  req.params.id;", "  const reason  =  req.body.reason;", "  db.orders.delete(id, reason);", "}"];
  const moved = { [A]: ["import x;", "", "export function del(req, res) {", "  removeOrder(req);", "}"], [HELPER]: ["export function removeOrder(req) {", ...aligned.slice(3, 6), "}"] };
  expect(await resolve(moved, [item(A, aligned, 4, 6)], [[4, 4]])).toEqual(["its code survives at src/helper.ts:2", "POTENTIALLY_RESOLVED"]);
});

it("reads the head file and a stored text the same way, under every reading of a backtick, /* and --, so an unchanged or reformatted finding never resolves", async () => {
  const unchanged = (line: number) => [`its cited code is unchanged at ${A}:${line}`, "STILL_PRESENT"];
  const sql = ["import db;", "export async function get(req) {", "  const rows = await db.query(`", "    SELECT * FROM orders", "    WHERE id = ${req.params.id}", "  `);", "  return rows;", "}"];
  const spaced = ["import db;", "export function get(req) {", '  return db.query("SELECT *  FROM orders WHERE id = " + req.params.id);', "}", "// x"];
  const lifetime = ["impl Store {", "  fn name(&self) -> &'static str { self.db.exec(self.raw_query) }", "}"];
  const jsx = ["export const P = () => (", "  <p onClick={() => db.orders.deleteAll()}>Don't click</p>", ");"];
  const del = ["import db;", "export function del(req, res) {", "  const id = req.params.id;", "  db.orders.delete(id);", "  res.status(204).end();", "}", "// end"];
  const sh = ["#!/bin/sh", "set -e", "curl -k https://example.com/install.sh | sh -s token", "echo done"];
  const tick = ["export function del(req, res) {", '  const clean = req.params.id.replace(/`/g, "");', "  db.orders.delete(clean);", "}", "const doc = `x`;"];
  const field = ["export class Client {", '  #apiKey = "sk_live_1234567890";', "  get() { return this.#apiKey; }", "}"];
  const docker = ["docker run \\", "  --privileged \\", "  -v /:/host img", "# end"];
  const css = [".a { color: red; }", "#admin { display: block; }", ".b { color: blue; }"];
  const quotes = ["export function q(db, name) {", "  return db.query('SELECT * FROM users WHERE name = \\'' + name + '\\'');", "}"];
  const hex = ["export function f(buf) {", "  if (buf[0] === 0XFF) return exec(buf.toString());", "}"];
  const dash = ["export function f(db, n) {", "  let k=--n; db.orders.deleteMany({ where: k });", "}"];
  const cases: [string, string[], [number, number], string[], number][] = [
    ["a template literal that spans lines, byte-identical", sql, [3, 6], sql, 5],
    ["the same under an added line", sql, [3, 6], ["import log;", ...sql], 6],
    ["two spaces inside a string literal", spaced, [3, 3], spaced, 5],
    ["an odd apostrophe with CRLF line endings", lifetime, [2, 2], lifetime.map((l) => `${l}\r`), 5],
    ["an odd apostrophe with trailing spaces", lifetime, [2, 2], lifetime.map((l) => `${l}  `), 5],
    ["JSX text with an apostrophe and a trailing space", jsx, [2, 2], jsx.map((l, i) => (i === 1 ? `${l} ` : l)), 5],
    ["a regex holding a backtick added above", del, [3, 5], ["import db;", "const TICK = /`/g;", ...del.slice(1)], 6],
    ["a regex holding /* added above", del, [3, 5], ["import db;", "const GLOB = /a\\/*/;", ...del.slice(1)], 6],
    ["evidence holding a regex with a backtick, file unchanged", tick, [2, 3], tick, 5],
    ["a private field at a line start, shifted a line", field, [2, 2], [field[0]!, "  // client", ...field.slice(1)], 6],
    ["a --flag line, shifted a line", docker, [2, 2], ["# hi", ...docker], 6],
    ["a CSS id selector, shifted a line", css, [2, 2], ["/* x */", ...css], 6],
    ["Prettier's quote and escape changes", quotes, [2, 2], quotes.with(1, "  return db.query(\"SELECT * FROM users WHERE name = '\" + name + \"'\");"), 5],
    ["Prettier's hex case", hex, [2, 2], hex.map((l) => l.replace("0XFF", "0xff")), 5],
    ["a space before -- that turns it into a comment", dash, [2, 2], dash.with(1, "  let k = --n; db.orders.deleteMany({ where: k });"), 5],
  ];
  for (const [name, base, range, head, line] of cases) expect(await resolve({ [A]: head }, [item(A, base, ...range)], [[1, 1]]), name).toEqual(unchanged(line));
  // Two copies of a line with no code tokens: locate cannot pick one, so the finding is doubted, and the survival check still finds the line as written.
  expect(await resolve({ [A]: [field[0]!, "  // client", field[1]!, field[1]!, ...field.slice(2)] }, [item(A, field, 2, 2)], [[1, 1]])).toEqual([`its code survives at ${A}:3`, "POTENTIALLY_RESOLVED"]);
  // A shell glob holding /* added above a curl line: `//` in the URL drops the rest of the line, so the item has five tokens and is not relocated, but it is found in the file and the claim is refused.
  expect(await resolve({ [A]: [...sh.slice(0, 2), "cp /src/* /dst", ...sh.slice(2)] }, [item(A, sh, 3, 3)], [[1, 1]])).toEqual([`its code survives at ${A}:4`, "POTENTIALLY_RESOLVED"]);
});

it("reads what every candidate cites before any other check, so a discarded one still contradicts a claim", async () => {
  // Line 4 changed; a re-report cites the surviving lines 5-7 without naming the finding, and fails VERIFY on its own terms.
  const partial = { [A]: BASE_A.with(3, "  const id = String(req.params.id);"), [HELPER]: BASE_HELPER };
  const block = [item(A, BASE_A, 3, 6)];
  const again = (over: Partial<CandidateFinding>) => candidate({ id: "c9", title: "Delete route lacks authorization", file: A, newLine: 5, codeEvidence: [{ path: A, start: 5, end: 7, refSha: ctx.headSha }], ...over });
  const refused = ["candidate c9 still cites its code at src/a.ts:5-7", "POTENTIALLY_RESOLVED"];
  for (const over of [{}, { evidenceKind: "suspected" as const }, { severity: "low" as const, confidence: 0.1 }]) {
    expect(await resolve(partial, block, [[4, 4]], [again(over)])).toEqual(refused);
  }
  // Anchored outside every hunk, or in a file the PR does not change: the citation still reads.
  expect(await resolve(partial, block, [[4, 4]], [again({})], {}, { [A]: [4] })).toEqual(refused);
  expect(await resolve(partial, block, [[4, 4]], [again({ file: "src/route2.ts", newLine: 2 })])).toEqual(refused);
  // Citing the base commit, the head lines around its anchor are what it cites; citing past the end of the file, the lines that exist are.
  expect(await resolve(partial, block, [[4, 4]], [again({ newLine: 4, codeEvidence: [{ path: A, start: 5, end: 7, refSha: ctx.baseSha }] })])).toEqual(["candidate c9 still cites its code at src/a.ts:2-6", "POTENTIALLY_RESOLVED"]);
  expect(await resolve(partial, block, [[4, 4]], [again({ codeEvidence: [{ path: A, start: 5, end: 70, refSha: ctx.headSha }] })])).toEqual(["candidate c9 still cites its code at src/a.ts:5-70", "POTENTIALLY_RESOLVED"]);
  // Only a candidate whose paths are unsafe says nothing.
  expect(await resolve(partial, block, [[4, 4]], [again({ codeEvidence: [{ path: "../../etc/passwd", start: 1, end: 1, refSha: ctx.headSha }] })])).toEqual([null, "RESOLVED"]);
});
