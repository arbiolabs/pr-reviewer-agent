import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { beforeAll, expect, it } from "vitest";
import type { CandidateFinding } from "../src/domain/types.ts";
import { evidenceTexts, findLongestRun, findOwnEvidence, findSnippet, headReader, linkPriors, locate, normalize, type ObservedLocation, observe, type PriorFinding, type Report, sameText, splitLines } from "../src/reasoning/track.ts";
import { fingerprint } from "../src/reasoning/verify.ts";
import { PinnedRepo, syncRepo } from "../src/source/git.ts";

const FILE = "src/routes/orders.ts";
const ROUTES = `import { requireUser, requireAdmin } from "../auth.ts";

export function refundOrder(req: Req, res: Res) {
  const order = db.orders.find(req.params.id);
  queueRefund(order, order.total);
  res.status(202).end();
}

export function registerOrderRoutes(router: Router) {
  router.post("/orders/:id/cancel", requireUser, requireAdmin, cancelOrder);
  router.post("/orders/:id/refund", requireUser, refundOrder);
}
`;
const EVIDENCE_LINES = ROUTES.split("\n").slice(8, 12);
const PAD = (n: number) => Array.from({ length: n }, (_, i) => `// moved down ${i + 1}`).join("\n");

/** One repository: base, the PR head, and one later head per scenario, each branched from the PR head. */
let repoDir: string;
let baseSha: string;
let headSha: string;
let cache: string;
const heads: Record<string, string> = {};

function git(...args: string[]) {
  return execFileSync("git", ["-C", repoDir, "-c", "user.name=T", "-c", "user.email=t@example.com", ...args], { encoding: "utf8" }).trim();
}
function write(path: string, text: string) {
  mkdirSync(dirname(join(repoDir, path)), { recursive: true });
  writeFileSync(join(repoDir, path), text);
}
function scenario(name: string, apply: () => void) {
  git("checkout", "-q", "-b", name, headSha);
  apply();
  git("add", "-A");
  git("commit", "-q", "-m", name);
  heads[name] = git("rev-parse", "HEAD");
  git("checkout", "-q", "main");
}

beforeAll(async () => {
  repoDir = join(mkdtempSync(join(tmpdir(), "track-")), "origin");
  mkdirSync(repoDir);
  git("init", "-q", "-b", "main");
  write("src/auth.ts", "export const requireUser = 1;\nexport const requireAdmin = 2;\n");
  write(FILE, ROUTES.replace('  router.post("/orders/:id/refund", requireUser, refundOrder);\n', ""));
  git("add", "-A");
  git("commit", "-q", "-m", "base");
  baseSha = git("rev-parse", "HEAD");
  git("checkout", "-q", "-b", "pr");
  write(FILE, ROUTES);
  git("commit", "-q", "-am", "add refund");
  headSha = git("rev-parse", "HEAD");
  git("update-ref", "refs/pull/1/head", headSha);
  git("checkout", "-q", "main");
  scenario("move", () => write(FILE, `${PAD(20)}\n${ROUTES}`));
  scenario("rename", () => {
    rmSync(join(repoDir, FILE));
    write("src/routes/order-routes.ts", `${PAD(5)}\n${ROUTES}`);
  });
  scenario("rewrite", () => write(FILE, ROUTES.replace('router.post("/orders/:id/refund", requireUser, refundOrder);', "router.post(REFUND, guard(refundOrder));")));
  scenario("delete", () => rmSync(join(repoDir, FILE)));
  scenario("copies", () => {
    rmSync(join(repoDir, FILE));
    write("src/a/orders.ts", ROUTES);
    write("src/b/orders.ts", ROUTES);
  });
  scenario("colon", () => {
    rmSync(join(repoDir, FILE));
    write("src/we:ird/orders.ts", ROUTES);
  });
  cache = mkdtempSync(join(tmpdir(), "track-cache-"));
  for (const sha of Object.values(heads)) git("update-ref", `refs/heads/keep-${sha.slice(0, 7)}`, sha);
});

async function at(name: string) {
  const repo = new PinnedRepo(await syncRepo(cache, "acme/track", repoDir, 1), baseSha, heads[name]!);
  return headReader(repo, await repo.changedFiles());
}

const candidate: CandidateFinding = {
  id: "c1",
  category: "security",
  severity: "high",
  title: "Refund route lacks requireAdmin",
  explanation: "e",
  file: FILE,
  newLine: 11,
  codeEvidence: [{ path: FILE, start: 9, end: 12, refSha: "" }],
  reasoningSummary: "r",
  confidence: 0.9,
  evidenceKind: "confirmed_by_static_trace",
};
const location = (): ObservedLocation => ({ file: FILE, newLine: 11, headSha, evidence: [{ path: FILE, start: 9, end: 12, text: normalize(EVIDENCE_LINES) }] });

it("builds the evidence text from the head-side ranges, or from the anchor's surroundings when none cite the head", async () => {
  const repo = new PinnedRepo(await syncRepo(cache, "acme/track", repoDir, 1), baseSha, headSha);
  const read = (sha: string, path: string) => repo.show(sha, path);
  expect(await evidenceTexts({ ...candidate, codeEvidence: [{ path: FILE, start: 9, end: 12, refSha: headSha }] }, headSha, read)).toEqual(location().evidence);
  expect(await evidenceTexts({ ...candidate, newLine: 10, codeEvidence: [{ path: FILE, start: 9, end: 12, refSha: baseSha }] }, headSha, read)).toEqual([
    { path: FILE, start: 8, end: 12, text: normalize(ROUTES.split("\n").slice(7, 12)) },
  ]);
});

it("keeps a finding whose lines moved inside the file, with the anchor shifted by the same offset", async () => {
  expect(await locate(location(), heads.move!, await at("move"))).toEqual({
    kind: "same",
    location: { file: FILE, newLine: 31, headSha: heads.move, evidence: [{ path: FILE, start: 29, end: 32, text: normalize(EVIDENCE_LINES) }] },
  });
});

it("follows a rename that the diff reports, and finds the same text by one tree-wide search when it does not", async () => {
  const renamed = { file: "src/routes/order-routes.ts", newLine: 16, headSha: heads.rename, evidence: [{ path: "src/routes/order-routes.ts", start: 14, end: 17, text: normalize(EVIDENCE_LINES) }] };
  const head = await at("rename");
  expect(head.renamedTo(FILE)).toBe("src/routes/order-routes.ts");
  expect(await locate(location(), heads.rename!, head)).toEqual({ kind: "same", location: renamed });
  expect(await locate(location(), heads.rename!, { ...head, renamedTo: () => undefined })).toEqual({ kind: "same", location: renamed });
});

it("doubts a finding whose cited code was rewritten while the file remains", async () => {
  expect(await locate(location(), heads.rewrite!, await at("rewrite"))).toEqual({ kind: "changed", file: FILE });
});

it("marks a finding outdated when its file is gone, and when its text now lives in two places the diff calls neither a rename", async () => {
  expect(await locate(location(), heads.delete!, await at("delete"))).toEqual({ kind: "file_gone" });
  expect(await locate(location(), heads.copies!, { ...(await at("copies")), renamedTo: () => undefined })).toEqual({ kind: "file_gone" });
});

it("never relocates on a snippet too small to be distinctive, but still finds it in place", async () => {
  const tiny: ObservedLocation = { ...location(), evidence: [{ path: FILE, start: 12, end: 12, text: "}" }] };
  expect(await locate(tiny, heads.delete!, await at("delete"))).toEqual({ kind: "file_gone" });
  // The `}` at line 12 is at line 32 after the move, with another `}` at line 27: in place it is found, moved it is not.
  expect(await locate(tiny, heads.move!, await at("move"))).toEqual({ kind: "changed", file: FILE });
  expect(await locate({ ...tiny, evidence: [{ path: FILE, start: 7, end: 7, text: "}" }] }, heads.rewrite!, await at("rewrite"))).toEqual({
    kind: "same",
    location: { file: FILE, newLine: 11, headSha: heads.rewrite, evidence: [{ path: FILE, start: 7, end: 7, text: "}" }] },
  });
  expect(findSnippet("}", ["a", "}", "b", "}"], 4)).toBe(4);
});

it("lists tree-wide hits by path alone, so a path with a colon survives, and refuses a list cut at the cap", async () => {
  const colon = { file: "src/we:ird/orders.ts", newLine: 11, headSha: heads.colon, evidence: [{ path: "src/we:ird/orders.ts", start: 9, end: 12, text: normalize(EVIDENCE_LINES) }] };
  expect(await locate(location(), heads.colon!, { ...(await at("colon")), renamedTo: () => undefined })).toEqual({ kind: "same", location: colon });
  const repo = new PinnedRepo(await syncRepo(cache, "acme/track", repoDir, 1), baseSha, heads.copies!);
  expect(await repo.grepPaths('router.post("/orders/:id/refund"')).toEqual(["src/a/orders.ts", "src/b/orders.ts"]);
  expect(await repo.grepPaths('router.post("/orders/:id/refund"', 16)).toBe(undefined);
  expect(await repo.grepPaths("no such line anywhere")).toEqual([]);
  // A cut list cannot show that exactly one file holds the text, so the finding stays where it was.
  const cut = { ...(await at("rename")), renamedTo: () => undefined, grep: async () => undefined };
  expect(await locate(location(), heads.rename!, cut)).toEqual({ kind: "file_gone" });
});

it("skips the tree-wide search when told to, so an already doubted finding costs one file read", async () => {
  const head = { ...(await at("rename")), renamedTo: () => undefined, grep: () => Promise.reject(new Error("tree-wide search ran")) };
  expect(await locate(location(), heads.rename!, head, { treeWide: false })).toEqual({ kind: "file_gone" });
});

it("calls two texts the same only when both are substantial and equal", () => {
  expect(sameText("", "")).toBe(false);
  expect(sameText("}", "}")).toBe(false);
  expect(sameText("return res.json(user);", "return res.json(user);")).toBe(false);
  expect(sameText("}\n}", "x\n}\n}\ny")).toBe(false);
  expect(sameText(normalize(EVIDENCE_LINES.slice(0, 2)), normalize(EVIDENCE_LINES))).toBe(false);
  expect(sameText(normalize(EVIDENCE_LINES), normalize(EVIDENCE_LINES))).toBe(true);
});

const prior = (located: PriorFinding["located"], status: PriorFinding["status"] = "PUBLISHED"): PriorFinding => ({
  findingId: "f-1",
  status,
  candidate,
  location: location(),
  located,
});
const same = (file = FILE, shift = 0): PriorFinding["located"] => ({
  kind: "same",
  location: { file, newLine: 11 + shift, headSha: "new", evidence: [{ path: file, start: 9 + shift, end: 12 + shift, text: normalize(EVIDENCE_LINES) }] },
});
const reported = (over: Partial<CandidateFinding>, evidence: ObservedLocation["evidence"] = [], priorHits?: ObservedLocation["evidence"]): Report => {
  const c = { ...candidate, ...over };
  return { candidate: c, location: { file: c.file, newLine: c.newLine, headSha: "new", evidence }, ...(priorHits ? { priorHits } : {}) };
};
const linkOne = (report: ReturnType<typeof reported>, priors: PriorFinding[]) => linkPriors([report], fingerprint, priors)[0]?.findingId;
const cites = (start: number, end: number, file = FILE) => [{ path: file, start, end, text: normalize(ROUTES.split("\n").slice(start - 1, end)) }];

it("links a re-report in the finding's file and category by title words, by its cited text, or by a nearby anchor citing the same lines", () => {
  const f = prior(same());
  expect(linkOne(reported({ title: "Refund route lacks requireAdmin" }), [f])).toBe("f-1");
  expect(linkOne(reported({ title: "requireAdmin lacks on refund route", file: "src/routes/order-routes.ts" }), [prior(same("src/routes/order-routes.ts", 5))])).toBe("f-1");
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 10 }, cites(9, 12)), [prior({ kind: "changed", file: FILE })])).toBe("f-1");
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 13 }, cites(11, 14)), [f])).toBe("f-1");
  // Within LINK_DISTANCE but citing none of the finding's lines: a different defect that happens to sit nearby.
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 13 }, cites(13, 14)), [f])).toBe(undefined);
  // Overlapping lines, but too far from the anchor: a broad citation alone does not link.
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 30 }, cites(12, 30)), [f])).toBe(undefined);
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 13, category: "correctness" }, cites(11, 14)), [f])).toBe(undefined);
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 40 }), [f])).toBe(undefined);
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 11 }, cites(11, 12)), [prior({ kind: "changed", file: FILE })])).toBe(undefined);
});

it("never links across files while the finding's file exists, even on the same text or the same title", () => {
  for (const located of [same(), { kind: "changed" as const, file: FILE }]) {
    const f = prior(located);
    expect(linkOne(reported({ title: "Anyone can refund", file: "x.ts", newLine: 1 }, [{ path: "x.ts", start: 1, end: 4, text: normalize(EVIDENCE_LINES) }]), [f])).toBe(undefined);
    expect(linkOne(reported({ file: "x.ts", newLine: 1 }, cites(9, 12, "x.ts")), [f])).toBe(undefined);
  }
});

it("links across files once the finding's file is gone, but only on a substantial equal cited text in the same category", () => {
  const gone = prior({ kind: "file_gone" });
  const moved = [{ path: "x.ts", start: 1, end: 4, text: normalize(EVIDENCE_LINES) }];
  expect(linkOne(reported({ title: "Anyone can refund", file: "x.ts", newLine: 1 }, moved), [gone])).toBe("f-1");
  expect(linkOne(reported({ title: "Anyone can refund", file: "x.ts", newLine: 1, category: "correctness" }, moved), [gone])).toBe(undefined);
  expect(linkOne(reported({ title: "Anyone can refund", file: "x.ts", newLine: 1 }, [{ path: "x.ts", start: 1, end: 1, text: normalize([EVIDENCE_LINES[2]!]) }]), [gone])).toBe(undefined);
  expect(linkOne(reported({ file: "x.ts", newLine: 1 }), [gone])).toBe(undefined);
});

it("reads a cited text only from the finding's own file, so two defects that cite one shared helper stay apart", () => {
  const helper = { path: "src/lib/auth.ts", start: 20, end: 22, text: normalize(["export function checkToken(t: string) {", "  return t.length > 0 && verify(t);", "}"]) };
  const withHelper = (f: PriorFinding): PriorFinding => ({ ...f, location: { ...f.location, evidence: [...f.location.evidence, helper] } });
  const gone = withHelper(prior({ kind: "file_gone" }));
  const other = { path: "src/new/router.ts", start: 78, end: 82, text: normalize(["router.post('/hook', (req) => {", "  handle(req.body);", "});"]) };
  expect(linkOne(reported({ title: "Webhook route accepts unsigned payloads", file: "src/new/router.ts", newLine: 80 }, [other, helper]), [gone])).toBe(undefined);
  const changed = withHelper(prior({ kind: "changed", file: FILE }));
  expect(linkOne(reported({ title: "Webhook route accepts unsigned payloads", newLine: 80 }, [{ ...other, path: FILE }, helper]), [changed])).toBe(undefined);
  // The helper's text cited from the finding's own file is still that file's code.
  expect(linkOne(reported({ title: "Anyone can refund", newLine: 10 }, cites(9, 12)), [changed])).toBe("f-1");
});

it("links by title words only where the finding was: in place, or near its last anchor or lines once its code changed", () => {
  const changed = prior({ kind: "changed", file: FILE });
  // The same title on a second instance far down the file, after the first was fixed, is a new finding.
  expect(linkOne(reported({ newLine: 300 }, [{ path: FILE, start: 298, end: 302, text: "x" }]), [changed])).toBe(undefined);
  expect(linkOne(reported({ newLine: 12 }, [{ path: FILE, start: 10, end: 14, text: "rewritten" }]), [changed])).toBe("f-1");
  expect(linkOne(reported({ newLine: 14 }, [{ path: FILE, start: 20, end: 22, text: "x" }]), [changed])).toBe("f-1");
  expect(linkOne(reported({ newLine: 40 }, [{ path: FILE, start: 9, end: 12, text: "x" }]), [changed])).toBe("f-1");
  expect(linkOne(reported({ newLine: 300 }), [prior(same())])).toBe("f-1");
});

it("finds a finding's own text in the file a named candidate reports, by the longest substantial run that occurs there once", async () => {
  const repo = new PinnedRepo(await syncRepo(cache, "acme/track", repoDir, 1), baseSha, heads.rename!);
  const lines = splitLines(await repo.show(heads.rename!, "src/routes/order-routes.ts"))!;
  expect(findOwnEvidence(prior({ kind: "file_gone" }), "src/routes/order-routes.ts", lines)).toEqual([{ path: "src/routes/order-routes.ts", start: 14, end: 17, text: normalize(EVIDENCE_LINES) }]);
  const tiny = { ...prior({ kind: "file_gone" }), location: { ...location(), evidence: [{ path: FILE, start: 12, end: 12, text: "}" }] } };
  expect(findOwnEvidence(tiny, "src/routes/order-routes.ts", lines)).toEqual([]);
  expect(findOwnEvidence(prior({ kind: "file_gone" }), "x.ts", ["nothing", "here"])).toEqual([]);
  // A block that spans the handler and the registration survives the rewrite of its last line as the handler's lines.
  const block = { ...prior({ kind: "changed", file: FILE }), location: { ...location(), evidence: [{ path: FILE, start: 3, end: 11, text: normalize(ROUTES.split("\n").slice(2, 11)) }] } };
  const rewritten = splitLines(await repo.show(heads.rewrite!, FILE))!;
  expect(findOwnEvidence(block, FILE, rewritten)).toEqual([{ path: FILE, start: 3, end: 10, text: normalize(ROUTES.split("\n").slice(2, 10)) }]);
  // A run that occurs twice names no place, however long; a shorter run elsewhere in the block still can.
  const twice = ["const order = db.orders.find(req.params.id);", "queueRefund(order, order.total);", "res.status(202).end();"];
  expect(findLongestRun(twice.join("\n"), [...twice, "x", ...twice])).toBe(undefined);
  expect(findLongestRun([...twice, "router.post(\"/orders/:id/refund\", requireUser, refundOrder);", "}"].join("\n"), [...twice, "x", ...twice, "y", 'router.post("/orders/:id/refund", requireUser, refundOrder);', "}"])).toEqual({ path: "", start: 9, end: 10, text: 'router.post("/orders/:id/refund", requireUser, refundOrder);\n}' });
  expect(findLongestRun("}\n}", ["}", "}"])).toBe(undefined);
});

it("links a candidate that names its prior finding only where code sees the report: near the finding's anchor or on its lines at this head", () => {
  const named = { priorFindingId: "f-1", title: "Anyone can refund" };
  // The code was rewritten in place: the anchor drifted past LINK_DISTANCE, but the citation overlaps the lines last seen.
  const changed = prior({ kind: "changed", file: FILE });
  expect(linkOne(reported({ ...named, newLine: 15 }, [{ path: FILE, start: 10, end: 15, text: "rewritten" }]), [changed])).toBe("f-1");
  expect(linkOne(reported({ ...named, newLine: 15 }, [{ path: FILE, start: 14, end: 15, text: "rewritten" }]), [changed])).toBe(undefined);
  expect(linkOne(reported({ ...named, newLine: 13 }, [{ path: FILE, start: 14, end: 15, text: "rewritten" }]), [changed])).toBe("f-1");
  expect(linkOne(reported({ ...named, newLine: 300 }), [changed])).toBe(undefined);
  expect(linkOne(reported({ ...named, newLine: 15, category: "correctness" }, [{ path: FILE, start: 10, end: 15, text: "x" }]), [changed])).toBe(undefined);
  expect(linkOne(reported({ ...named, newLine: 15, file: "x.ts" }, [{ path: "x.ts", start: 10, end: 15, text: "x" }]), [changed])).toBe(undefined);
  // The file is gone and the model says which file holds the code now: the finding's text found there must meet the citation.
  const gone = prior({ kind: "file_gone" });
  const hits = [{ path: "src/routes/order-routes.ts", start: 14, end: 17, text: normalize(EVIDENCE_LINES) }];
  const moved = (start: number, end: number, priorHits = hits) => reported({ ...named, file: "src/routes/order-routes.ts", newLine: 20 }, [{ path: "src/routes/order-routes.ts", start, end, text: "shifted by one" }], priorHits);
  expect(linkOne(moved(15, 18), [gone])).toBe("f-1");
  expect(linkOne(moved(18, 20), [gone])).toBe(undefined);
  expect(linkOne(moved(15, 18, []), [gone])).toBe(undefined);
  // Naming a finding whose code is found in place still needs the report at that place.
  expect(linkOne(reported({ ...named, newLine: 13 }, cites(13, 14)), [prior(same())])).toBe("f-1");
  expect(linkOne(reported({ ...named, newLine: 40 }, cites(40, 41)), [prior(same())])).toBe(undefined);
  expect(linkOne(reported({ priorFindingId: "f-9", title: "Anyone can refund", newLine: 13 }, cites(13, 14)), [prior(same())])).toBe(undefined);
});

it("assigns each open finding to at most one candidate, the best-scoring one, and leaves the rest fresh", () => {
  const f = prior(same());
  const byTitle = reported({ id: "c-title", title: "Refund route lacks requireAdmin", newLine: 13 }, cites(13, 14));
  const nearby = reported({ id: "c-near", title: "Anyone can refund", newLine: 11 }, cites(9, 12));
  const far = reported({ id: "c-far", title: "Unrelated", newLine: 40 });
  expect(linkPriors([nearby, byTitle, far], fingerprint, [f]).map((p) => p?.findingId)).toEqual([undefined, "f-1", undefined]);
  // Two findings, two nearby candidates: the closer anchor takes each.
  const g = { ...prior(same(FILE, 2)), findingId: "f-2" };
  const atEleven = reported({ id: "c11", title: "A", newLine: 11 }, cites(9, 14));
  const atThirteen = reported({ id: "c13", title: "B", newLine: 13 }, cites(9, 14));
  expect(linkPriors([atThirteen, atEleven], fingerprint, [f, g]).map((p) => p?.findingId)).toEqual(["f-2", "f-1"]);
});

it("observes every open finding once: re-reported, located, changed or gone, and a candidate linked to nothing is new", () => {
  const fresh = { findingId: "f-new", candidate: { ...candidate, id: "c9", title: "Other" }, location: location() };
  const link = { findingId: "f-1", candidate: { ...candidate, newLine: 12 }, location: { ...location(), newLine: 12 } };
  const [moved, changed, gone] = [{ ...prior(same(FILE, 20)), findingId: "f-2" }, { ...prior({ kind: "changed", file: FILE }, "STILL_PRESENT"), findingId: "f-3" }, { ...prior({ kind: "file_gone" }, "DETECTED"), findingId: "f-4" }];
  const rows = observe([prior(same()), moved, changed, gone], [fresh, link]);
  expect(rows.map((o) => [o.findingId, o.path.join(">"), o.basis, o.candidate.newLine, o.location.headSha])).toEqual([
    ["f-new", "DETECTED", null, 11, headSha],
    ["f-1", "PUBLISHED>STILL_PRESENT", "reverified", 12, headSha],
    ["f-2", "PUBLISHED>STILL_PRESENT", "located", 31, "new"],
    ["f-3", "STILL_PRESENT>POTENTIALLY_RESOLVED", "changed", 11, headSha],
    ["f-4", "DETECTED>OUTDATED", "file_gone", 11, headSha],
  ]);
  expect(rows[2]!.candidate.codeEvidence).toEqual([{ path: FILE, start: 29, end: 32, refSha: "new" }]);
  // A doubted finding observed as changed again has no legal event and stays where it is.
  expect(observe([prior({ kind: "changed", file: FILE }, "POTENTIALLY_RESOLVED")], [])[0]!.path).toEqual(["POTENTIALLY_RESOLVED"]);
});

it("resolves a doubted finding on an accepted claim, leaves one re-reported without verification uncertain, and records a refused claim", () => {
  const rows = (f: PriorFinding, claims: Parameters<typeof observe>[2]) => observe([f], [], claims).map((o) => [o.path.join(">"), o.basis]);
  const accepted = { verdicts: [{ findingId: "f-1", state: "ACCEPTED" as const, reason: null }], unverified: [] };
  const refused = { verdicts: [{ findingId: "f-1", state: "REFUSED" as const, reason: "x" }], unverified: [] };
  const unverified = { verdicts: [], unverified: ["f-1"] };
  expect(rows(prior({ kind: "changed", file: FILE }), accepted)).toEqual([["PUBLISHED>POTENTIALLY_RESOLVED>RESOLVED", "resolution_checked"]]);
  expect(rows(prior({ kind: "file_gone" }, "OUTDATED"), accepted)).toEqual([["OUTDATED>RESOLVED", "resolution_checked"]]);
  expect(rows(prior({ kind: "file_gone" }, "UNCERTAIN"), accepted)).toEqual([["UNCERTAIN>OUTDATED>RESOLVED", "resolution_checked"]]);
  expect(rows(prior({ kind: "changed", file: FILE }), refused)).toEqual([["PUBLISHED>POTENTIALLY_RESOLVED", "resolution_refused"]]);
  expect(rows(prior({ kind: "changed", file: FILE }), unverified)).toEqual([["PUBLISHED>POTENTIALLY_RESOLVED>UNCERTAIN", "reported_unverified"]]);
  expect(rows(prior({ kind: "changed", file: FILE }, "UNCERTAIN"), unverified)).toEqual([["UNCERTAIN>POTENTIALLY_RESOLVED>UNCERTAIN", "reported_unverified"]]);
  // Code found the finding's code in place: no claim or unverified report moves it off STILL_PRESENT.
  expect(rows(prior(same()), unverified)).toEqual([["PUBLISHED>STILL_PRESENT", "located"]]);
  expect(rows(prior(same()), accepted)).toEqual([["PUBLISHED>STILL_PRESENT", "located"]]);
  // A verified re-report wins over any claim about the finding.
  expect(observe([prior({ kind: "changed", file: FILE })], [{ findingId: "f-1", candidate, location: location() }], accepted).map((o) => [o.path.join(">"), o.basis])).toEqual([["PUBLISHED>STILL_PRESENT", "reverified"]]);
});
