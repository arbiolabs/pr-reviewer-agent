import { createModels, type FauxResponseStep, fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai";
import { DEFAULT_CONFIG } from "../src/config/config.ts";
import { type App, openApp } from "../src/runtime/app.ts";
import { createFixture, DEFECT, type Fixture } from "./fixture.ts";

export function submitDefect(fixture: Fixture) {
  return fauxAssistantMessage(
    [
      fauxToolCall("submit_candidates", {
        candidates: [
          {
            id: "c1",
            category: "security",
            severity: "high",
            title: "Order deletion endpoint skips the admin check",
            explanation: "GET /orders requires requireAdmin; the new DELETE /orders/:id has no guard, so any caller can delete orders.",
            file: DEFECT.file,
            newLine: DEFECT.line,
            codeEvidence: [{ path: DEFECT.file, start: 4, end: 5, refSha: fixture.headSha }],
            reasoningSummary: "Compared the new route registration with its guarded sibling.",
            confidence: 0.9,
            evidenceKind: "confirmed_by_static_trace",
          },
        ],
      }),
    ],
    { stopReason: "toolUse" },
  );
}

export const readDiff = () => fauxAssistantMessage([fauxToolCall("read_diff", { file: DEFECT.file })], { stopReason: "toolUse" });

export async function open(dataDir: string, fixture: Fixture, responses: FauxResponseStep[]): Promise<App> {
  const faux = fauxProvider({ provider: "faux", models: [{ id: "reviewer" }] });
  faux.setResponses(responses);
  const models = createModels();
  models.setProvider(faux.provider);
  return openApp({
    dataDir,
    config: DEFAULT_CONFIG,
    pulls: fixture.pulls,
    engine: undefined,
    models,
    model: { provider: "faux", modelId: "reviewer" },
    reviewerVersion: "m1+faux/reviewer",
  });
}

