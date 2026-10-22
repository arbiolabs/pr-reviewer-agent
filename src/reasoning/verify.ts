import type { Config } from "../config/config.ts";
import { transition } from "../domain/machine.ts";
import { CANDIDATE_MACHINE, type CandidateEvent, type CandidateState } from "../domain/machines.ts";
import { type CandidateFinding, type CodeEvidence, type Disposition, type ResolutionClaim, type ResolutionVerdict, SEVERITIES, sha256, stableId } from "../domain/types.ts";
import type { FileDiff } from "../source/diff.ts";
import { assertSafePath } from "../source/git.ts";
import { currentFile, evidenceTexts, findOwnEvidence, hasCode, type HeadReader, linkPriors, normalize, type ObservedLocation, type PriorFinding, type Report, sharesCode, splitLines, survivingCode, within } from "./track.ts";

export type VerifyContext = {
  prId: string;
  revisionId: string;
  baseSha: string;
  headSha: string;
  /** Head-side diff per changed file. */
  files: Map<string, FileDiff>;
  /** Contents of `path` at `sha`, `undefined` when it does not exist there. */
  read(sha: string, path: string): Promise<string | undefined>;
  /** The head, with its renames and a tree-wide line search, for finding an open finding's code wherever it went. */
  head: HeadReader;
  thresholds: Config["verification"];
  /** The PR's open findings, each already located at this head; a candidate that reports one again keeps its ID. */
  prior: PriorFinding[];
};

export type VerifiedFinding = { findingId: string; fingerprint: string; candidate: CandidateFinding; location: ObservedLocation };

// ponytail: category + file + title words is a crude stand-in for "violated invariant" (spec §7). Two phrasings of
// one defect get two fingerprints, so track.ts links a re-report by its cited text and location as well.
const STOPWORDS = new Set(["the", "and", "for", "with", "from", "that", "this", "into", "when", "are", "can"]);

export function fingerprint(candidate: Pick<CandidateFinding, "category" | "file" | "title">): string {
  const tokens = [...new Set(candidate.title.toLowerCase().split(/[^a-z0-9]+/).filter((t) => t.length > 2 && !STOPWORDS.has(t)))].sort();
  return sha256(`${candidate.category}|${candidate.file}|${tokens.join(" ")}`);
}

const lineCount = (text: string) => (text === "" ? 0 : text.split("\n").length - (text.endsWith("\n") ? 1 : 0));
const where = (e: CodeEvidence) => `${e.path}:${e.start}-${e.end}@${e.refSha.slice(0, 12)}`;

/** Why `path` must not be read, or `undefined` when it is safe to. */
function pathProblem(path: string): string | undefined {
  try {
    assertSafePath(path);
    return undefined;
  } catch (error) {
    return (error as Error).message;
  }
}

/** The first reason one evidence range cannot be read from pinned data at one of the `allowed` SHAs, or `undefined`. */
async function rangeProblem(evidence: CodeEvidence, ctx: VerifyContext, allowed: string[], outside: string): Promise<string | undefined> {
  const unsafe = pathProblem(evidence.path);
  if (unsafe !== undefined) return unsafe;
  if (!allowed.includes(evidence.refSha)) return `evidence ${where(evidence)} ${outside}`;
  const text = await ctx.read(evidence.refSha, evidence.path);
  if (text === undefined) return `evidence ${where(evidence)} does not exist`;
  const lines = lineCount(text);
  if (evidence.start < 1 || evidence.end < evidence.start || evidence.end > lines) return `evidence ${where(evidence)} is outside 1-${lines}`;
  return undefined;
}

/** The first structural reason a candidate cannot be anchored and reproduced from pinned data, or `undefined`. */
async function structuralProblem(candidate: CandidateFinding, ctx: VerifyContext): Promise<string | undefined> {
  const file = ctx.files.get(candidate.file);
  if (file === undefined) return `${candidate.file} is not in the changed set`;
  if (candidate.newLine === null) return "no line anchor";
  if (!file.headLines.has(candidate.newLine)) return `${candidate.file}:${candidate.newLine} is outside every hunk of the head-side diff`;
  if (candidate.codeEvidence.length === 0) return "no code evidence";
  for (const evidence of candidate.codeEvidence) {
    const problem = await rangeProblem(evidence, ctx, [ctx.baseSha, ctx.headSha], "cites a SHA outside this revision");
    if (problem !== undefined) return problem;
  }
  return undefined;
}

/** What this revision's candidates say: which open finding each reports again, and the code each cites. */
export type Contradictions = { reported: Map<string, string>; cited: Pick<VerifiedFinding, "candidate" | "location">[] };

/**
 * Why a resolution claim fails, or `undefined` when code supports it. A claim must name an open finding none of whose
 * own-file code is found at this head, compared as code tokens (a changed line alone never resolves, and code still in
 * place never does: a changed helper, a comment or blank line inserted in the block, a dropped semicolon, or a short
 * item shifted a line is not a fix), which no candidate of this revision reports again or still cites: a candidate
 * whose evidence shares a substantial run of tokens with the finding's own code says that code is still part of a
 * live defect, as when a handler stays reachable through a second unguarded route. The claim's evidence must be
 * confirmed_by_static_trace, exist at the head SHA, include a range in the finding's file at this head (or, when that
 * file is gone, in a file this pull request changes), and cite code: a range with no code tokens, such as a comment
 * or a blank line, that sits whole in the finding's own code unchanged, or that holds all of it, is not a fix. Every
 * code comparison reads only the finding's own-file evidence: a helper both cite says nothing about the defect.
 */
async function refusal(claim: ResolutionClaim, ctx: VerifyContext, { reported, cited }: Contradictions, seen: Set<string>): Promise<string | undefined> {
  const f = ctx.prior.find((p) => p.findingId === claim.findingId);
  if (!f) return `no open finding ${claim.findingId}`;
  if (seen.has(claim.findingId)) return "duplicate of an earlier claim";
  const by = reported.get(claim.findingId);
  if (by !== undefined) return `candidate ${by} reports it again`;
  const own = f.location.evidence.filter((e) => e.path === f.location.file);
  for (const c of cited) {
    const cites = c.location.evidence.find((e) => own.some((old) => sharesCode(old.text, e.text)));
    if (cites) return `candidate ${c.candidate.id} still cites its code at ${cites.path}:${cites.start}-${cites.end}`;
  }
  if (f.located.kind === "same") return `its cited code is unchanged at ${f.located.location.file}:${f.located.location.newLine ?? "?"}`;
  const survives = await survivingCode(f, ctx.head);
  if (survives !== undefined) return `its code survives ${survives}`;
  if (claim.evidenceKind !== "confirmed_by_static_trace") return `evidence is ${claim.evidenceKind}`;
  if (claim.codeEvidence.length === 0) return "no code evidence";
  for (const evidence of claim.codeEvidence) {
    const problem = await rangeProblem(evidence, ctx, [ctx.headSha], "cites a SHA other than the head");
    if (problem !== undefined) return problem;
    const text = normalize(splitLines(await ctx.read(ctx.headSha, evidence.path))!.slice(evidence.start - 1, evidence.end));
    if (!hasCode(text)) return `evidence ${where(evidence)} cites no code`;
    if (own.some((old) => within(text, old.text))) return `evidence ${where(evidence)} is the finding's code unchanged`;
    if (own.some((old) => within(old.text, text))) return `evidence ${where(evidence)} contains the finding's code unchanged`;
  }
  const file = currentFile(f);
  const inScope = (path: string) => (file === undefined ? ctx.files.has(path) : path === file);
  if (!claim.codeEvidence.some((e) => inScope(e.path))) return file === undefined ? "no evidence in a file this pull request changes" : `no evidence in ${file}`;
  return undefined;
}

/** One verdict per claim, in order; only an ACCEPTED one moves its finding to RESOLVED (spec §4.3). */
export async function checkResolutions(claims: ResolutionClaim[], ctx: VerifyContext, contradictions: Contradictions): Promise<ResolutionVerdict[]> {
  const verdicts: ResolutionVerdict[] = [];
  const seen = new Set<string>();
  for (const claim of claims) {
    const reason = await refusal(claim, ctx, contradictions, seen);
    seen.add(claim.findingId);
    verdicts.push({ findingId: claim.findingId, state: reason === undefined ? "ACCEPTED" : "REFUSED", reason: reason ?? null });
  }
  return verdicts;
}

function thresholdProblem(candidate: CandidateFinding, thresholds: VerifyContext["thresholds"]): string | undefined {
  if (SEVERITIES.indexOf(candidate.severity) > SEVERITIES.indexOf(thresholds.min_severity)) {
    return `severity ${candidate.severity} is below ${thresholds.min_severity}`;
  }
  if (candidate.confidence < thresholds.min_confidence) return `confidence ${candidate.confidence} is below ${thresholds.min_confidence}`;
  return undefined;
}

const step = (path: CandidateState[], event: CandidateEvent): CandidateState[] => {
  const result = transition(CANDIDATE_MACHINE, path.at(-1)!, event);
  if (!result.ok) throw new Error(result.reason);
  return [...path, result.state];
};

/**
 * Deterministic VERIFY (spec §7): every candidate walks the candidate submachine to VERIFIED or DISCARDED.
 * There is no LLM second-pass verifier in M1; a candidate passes on anchoring, evidence, and policy alone.
 * The candidates that pass are then linked to the PR's open findings one-to-one (track.ts): a linked candidate
 * takes that finding's ID, and any other gets a new one. Two candidates that get the same new ID are one defect
 * reported twice, so the second is a duplicate. Two that read as the same open finding are not: the second is new.
 * `unverified` names the open findings a candidate reported again (`priorFindingId`) without a verified link to it.
 * The resolution claims are checked last, because a finding any candidate reports again or still cites cannot be
 * resolved. What a candidate cites is read before any other check, as soon as its paths are safe to read, so a
 * candidate still cites the finding's code when its anchor, file, kind or severity discards it.
 */
export async function verifyCandidates(
  candidates: CandidateFinding[],
  ctx: VerifyContext,
  resolutions: ResolutionClaim[] = [],
): Promise<{ dispositions: Disposition[]; verified: VerifiedFinding[]; unverified: string[]; verdicts: ResolutionVerdict[] }> {
  const dispositions: Disposition[] = [];
  const passed: (Report & { index: number; path: CandidateState[] })[] = [];
  const cited: Contradictions["cited"] = [];
  const named = (candidate: CandidateFinding) => ctx.prior.find((f) => f.findingId === candidate.priorFindingId);
  const discard = (index: number, candidate: CandidateFinding, path: CandidateState[], reason: string) => {
    dispositions[index] = { candidateId: candidate.id, state: "DISCARDED", path: step(path, "REJECT"), reason, findingId: null };
  };
  for (const [index, candidate] of candidates.entries()) {
    let path = step(["DISCOVERED"], "INVESTIGATE");
    const unsafe = [candidate.file, ...candidate.codeEvidence.map((e) => e.path)].map(pathProblem).find((p) => p !== undefined);
    if (unsafe !== undefined) {
      discard(index, candidate, path, unsafe);
      continue;
    }
    const location: ObservedLocation = { file: candidate.file, newLine: candidate.newLine, headSha: ctx.headSha, evidence: await evidenceTexts(candidate, ctx.headSha, ctx.read) };
    cited.push({ candidate, location });
    const structural = await structuralProblem(candidate, ctx);
    if (structural !== undefined) {
      discard(index, candidate, path, structural);
      continue;
    }
    if (candidate.evidenceKind !== "confirmed_by_static_trace") {
      discard(index, candidate, step(path, "EVIDENCE_INSUFFICIENT"), `evidence is ${candidate.evidenceKind}`);
      continue;
    }
    path = step(path, "EVIDENCE_FOUND");
    const threshold = thresholdProblem(candidate, ctx.thresholds);
    if (threshold !== undefined) {
      discard(index, candidate, path, threshold);
      continue;
    }
    // A candidate that names a finding whose code was not located says which file holds it now: look for its text there.
    const prior = named(candidate);
    const priorHits = prior && prior.located.kind !== "same" ? findOwnEvidence(prior, candidate.file, splitLines(await ctx.read(ctx.headSha, candidate.file)) ?? []) : [];
    passed.push({ index, candidate, path, location, priorHits });
  }
  const linked = linkPriors(passed, fingerprint, ctx.prior);
  const verified: VerifiedFinding[] = [];
  const seen = new Map<string, string>();
  for (const [i, { index, candidate, path, location }] of passed.entries()) {
    const print = fingerprint(candidate);
    const findingId = linked[i]?.findingId ?? stableId("finding", ctx.prId, ctx.revisionId, print);
    const first = seen.get(findingId);
    if (first !== undefined) {
      discard(index, candidate, path, `duplicate of candidate ${first}`);
      continue;
    }
    seen.set(findingId, candidate.id);
    dispositions[index] = { candidateId: candidate.id, state: "VERIFIED", path: step(path, "CONFIRM"), reason: null, findingId };
    verified.push({ findingId, fingerprint: print, candidate, location });
  }
  const unverified = candidates.filter((c, i) => named(c) && dispositions[i]!.findingId !== c.priorFindingId).map((c) => c.priorFindingId!);
  // Every open finding a candidate reports again, by a verified link or by name: the model says it is still there.
  const reported = new Map<string, string>();
  for (const [i, c] of candidates.entries()) {
    for (const id of [dispositions[i]!.findingId, c.priorFindingId]) if (id && ctx.prior.some((p) => p.findingId === id) && !reported.has(id)) reported.set(id, c.id);
  }
  return { dispositions, verified, unverified, verdicts: await checkResolutions(resolutions, ctx, { reported, cited }) };
}
