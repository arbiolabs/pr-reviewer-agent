import { type Static, Type } from "@earendil-works/pi-ai";
import { CATEGORIES, EVIDENCE_KINDS, SEVERITIES } from "../domain/types.ts";

const literals = <T extends readonly string[]>(values: T) => Type.Union(values.map((v) => Type.Literal(v)));

/** Spec §7 `CandidateFinding` plus `evidenceKind`; the Harness validates model arguments against it. */
export const CandidateSchema = Type.Object({
  id: Type.String({ description: "Short id unique within this submission, such as c1" }),
  category: literals(CATEGORIES),
  severity: literals(SEVERITIES),
  title: Type.String({ maxLength: 200 }),
  explanation: Type.String({ maxLength: 4000 }),
  file: Type.String({ description: "Head-side path of the changed file" }),
  newLine: Type.Union([Type.Integer({ minimum: 1 }), Type.Null()], { description: "Head-side line the comment anchors to" }),
  codeEvidence: Type.Array(
    Type.Object({
      path: Type.String(),
      start: Type.Integer({ minimum: 1 }),
      end: Type.Integer({ minimum: 1 }),
      refSha: Type.String({ description: "Base or head SHA of this revision" }),
    }),
    { maxItems: 10 },
  ),
  reasoningSummary: Type.String({ maxLength: 2000 }),
  suggestedFix: Type.Optional(Type.String({ maxLength: 2000 })),
  confidence: Type.Number({ minimum: 0, maximum: 1 }),
  evidenceKind: literals(EVIDENCE_KINDS),
  priorFindingId: Type.Optional(Type.String({ description: "The id of the open finding from an earlier review that this candidate reports again, if any" })),
});

const CodeEvidenceSchema = Type.Object({
  path: Type.String(),
  start: Type.Integer({ minimum: 1 }),
  end: Type.Integer({ minimum: 1 }),
  refSha: Type.String({ description: "The head SHA of this revision" }),
});

/** A claim that an open finding is fixed at the head; the verifier checks it like a candidate. */
export const ResolutionSchema = Type.Object({
  findingId: Type.String({ description: "The id of the open finding" }),
  codeEvidence: Type.Array(CodeEvidenceSchema, { minItems: 1, maxItems: 10, description: "Head-side ranges whose code shows the defect is gone" }),
  reason: Type.String({ maxLength: 2000 }),
  evidenceKind: literals(EVIDENCE_KINDS),
});

export const SubmitCandidatesSchema = Type.Object({
  candidates: Type.Array(CandidateSchema, { maxItems: 25 }),
  resolutions: Type.Optional(Type.Array(ResolutionSchema, { maxItems: 25, description: "Open findings from earlier reviews that this head's code shows fixed" })),
  focus: Type.Optional(
    Type.Array(
      Type.Object({
        file: Type.String({ description: "A changed file, as get_changed_files lists it" }),
        reason: Type.String({ maxLength: 200, description: "One short sentence: what a person should check there" }),
      }),
      { maxItems: 3, description: "The changed files a human reviewer should read first, most important first" },
    ),
  ),
});

export type CandidateInput = Static<typeof CandidateSchema>;
