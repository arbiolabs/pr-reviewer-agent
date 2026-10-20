import { createHash } from "node:crypto";

export const CATEGORIES = ["security", "concurrency", "correctness", "data_integrity", "performance", "general", "other"] as const;
export type Category = (typeof CATEGORIES)[number];

export const SEVERITIES = ["critical", "high", "medium", "low"] as const;
export type Severity = (typeof SEVERITIES)[number];

export const EVIDENCE_KINDS = ["confirmed_by_static_trace", "suspected", "unreproducible"] as const;
export type EvidenceKind = (typeof EVIDENCE_KINDS)[number];

export const REVIEW_MODES = ["standard", "deep", "critical"] as const;
export type ReviewMode = (typeof REVIEW_MODES)[number];

export type CodeEvidence = { path: string; start: number; end: number; refSha: string };

export type CandidateFinding = {
  id: string;
  category: Category;
  severity: Severity;
  title: string;
  explanation: string;
  file: string;
  newLine: number | null;
  codeEvidence: CodeEvidence[];
  reasoningSummary: string;
  suggestedFix?: string;
  confidence: number;
  evidenceKind: EvidenceKind;
};

export type PullInfo = {
  installationId: number;
  repositoryId: number;
  fullName: string;
  number: number;
  title: string;
  body: string;
  baseSha: string;
  headSha: string;
  state: "open" | "closed";
  merged: boolean;
  cloneUrl: string;
};

export type ChangedFile = { path: string; status: string };

// §5, limited to the phases M1 runs. `attempt` counts retries of the current phase.
export type ReviewCheckpoint =
  | { phase: "collect"; revisionId: string; attempt?: number }
  | { phase: "triage"; revisionId: string; contextSnapshotId: string; attempt?: number }
  | { phase: "analyze"; revisionId: string; triageDecisionId: string; attempt?: number }
  | { phase: "verify"; revisionId: string; candidateSetId: string; attempt?: number }
  | { phase: "prepare"; revisionId: string; verifiedSetId: string; attempt?: number };

export function stableId(...parts: (string | number)[]): string {
  return createHash("sha256").update(parts.map(String).join("\u0000")).digest("hex").slice(0, 32);
}

export function sha256(text: string): string {
  return createHash("sha256").update(text).digest("hex");
}

/** How VERIFY disposed of one candidate: the candidate-machine states it passed through and why it ended there. */
export type Disposition = {
  candidateId: string;
  state: "VERIFIED" | "DISCARDED";
  path: string[];
  reason: string | null;
  findingId: string | null;
};
