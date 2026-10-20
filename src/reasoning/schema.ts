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
});

export const SubmitCandidatesSchema = Type.Object({ candidates: Type.Array(CandidateSchema, { maxItems: 25 }) });

export type CandidateInput = Static<typeof CandidateSchema>;
