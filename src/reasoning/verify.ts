import type { Config } from "../config/config.ts";
import { transition } from "../domain/machine.ts";
import { CANDIDATE_MACHINE, type CandidateEvent, type CandidateState } from "../domain/machines.ts";
import { type CandidateFinding, type Disposition, SEVERITIES, sha256, stableId } from "../domain/types.ts";
import type { FileDiff } from "../source/diff.ts";
import { assertSafePath } from "../source/git.ts";

export type VerifyContext = {
  prId: string;
  baseSha: string;
  headSha: string;
  /** Head-side diff per changed file. */
  files: Map<string, FileDiff>;
  /** Line count of `path` at `sha`, `undefined` when it does not exist there. */
  lineCount(sha: string, path: string): Promise<number | undefined>;
  thresholds: Config["verification"];
};

export type VerifiedFinding = { findingId: string; fingerprint: string; candidate: CandidateFinding };

// ponytail: category + file + title words is a crude stand-in for "violated invariant" (spec §7). Two phrasings of
// one defect get two fingerprints, and a moved defect gets a new one; replace with a semantic key in M3.
const STOPWORDS = new Set(["the", "and", "for", "with", "from", "that", "this", "into", "when", "are", "can"]);

export function fingerprint(candidate: Pick<CandidateFinding, "category" | "file" | "title">): string {
  const tokens = [...new Set(candidate.title.toLowerCase().split(/[^a-z0-9]+/).filter((t) => t.length > 2 && !STOPWORDS.has(t)))].sort();
  return sha256(`${candidate.category}|${candidate.file}|${tokens.join(" ")}`);
}

/** The first structural reason a candidate cannot be anchored and reproduced from pinned data, or `undefined`. */
async function structuralProblem(candidate: CandidateFinding, ctx: VerifyContext): Promise<string | undefined> {
  try {
    assertSafePath(candidate.file);
    for (const evidence of candidate.codeEvidence) assertSafePath(evidence.path);
  } catch (error) {
    return (error as Error).message;
  }
  const file = ctx.files.get(candidate.file);
  if (file === undefined) return `${candidate.file} is not in the changed set`;
  if (candidate.newLine === null) return "no line anchor";
  if (!file.headLines.has(candidate.newLine)) return `${candidate.file}:${candidate.newLine} is outside every hunk of the head-side diff`;
  if (candidate.codeEvidence.length === 0) return "no code evidence";
  for (const evidence of candidate.codeEvidence) {
    const where = `${evidence.path}:${evidence.start}-${evidence.end}@${evidence.refSha.slice(0, 12)}`;
    if (evidence.refSha !== ctx.baseSha && evidence.refSha !== ctx.headSha) return `evidence ${where} cites a SHA outside this revision`;
    const lines = await ctx.lineCount(evidence.refSha, evidence.path);
    if (lines === undefined) return `evidence ${where} does not exist`;
    if (evidence.start < 1 || evidence.end < evidence.start || evidence.end > lines) return `evidence ${where} is outside 1-${lines}`;
  }
  return undefined;
}

function thresholdProblem(candidate: CandidateFinding, thresholds: VerifyContext["thresholds"]): string | undefined {
  if (SEVERITIES.indexOf(candidate.severity) > SEVERITIES.indexOf(thresholds.min_severity)) {
    return `severity ${candidate.severity} is below ${thresholds.min_severity}`;
  }
  if (candidate.confidence < thresholds.min_confidence) return `confidence ${candidate.confidence} is below ${thresholds.min_confidence}`;
  return undefined;
}

/**
 * Deterministic VERIFY (spec §7): every candidate walks the candidate submachine to VERIFIED or DISCARDED.
 * There is no LLM second-pass verifier in M1; a candidate passes on anchoring, evidence, and policy alone.
 */
export async function verifyCandidates(candidates: CandidateFinding[], ctx: VerifyContext): Promise<{ dispositions: Disposition[]; verified: VerifiedFinding[] }> {
  const dispositions: Disposition[] = [];
  const verified: VerifiedFinding[] = [];
  const seen = new Map<string, string>();
  for (const candidate of candidates) {
    const path: CandidateState[] = ["DISCOVERED"];
    const step = (event: CandidateEvent) => {
      const result = transition(CANDIDATE_MACHINE, path.at(-1)!, event);
      if (!result.ok) throw new Error(result.reason);
      path.push(result.state);
    };
    const discard = (reason: string) => {
      step("REJECT");
      dispositions.push({ candidateId: candidate.id, state: "DISCARDED", path, reason, findingId: null });
    };
    step("INVESTIGATE");
    const structural = await structuralProblem(candidate, ctx);
    if (structural !== undefined) {
      discard(structural);
      continue;
    }
    if (candidate.evidenceKind !== "confirmed_by_static_trace") {
      step("EVIDENCE_INSUFFICIENT");
      discard(`evidence is ${candidate.evidenceKind}`);
      continue;
    }
    step("EVIDENCE_FOUND");
    const print = fingerprint(candidate);
    const threshold = thresholdProblem(candidate, ctx.thresholds) ?? (seen.has(print) ? `duplicate of candidate ${seen.get(print)}` : undefined);
    if (threshold !== undefined) {
      discard(threshold);
      continue;
    }
    step("CONFIRM");
    seen.set(print, candidate.id);
    const findingId = stableId("finding", ctx.prId, print);
    dispositions.push({ candidateId: candidate.id, state: "VERIFIED", path, reason: null, findingId });
    verified.push({ findingId, fingerprint: print, candidate });
  }
  return { dispositions, verified };
}
