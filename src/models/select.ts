import type { Models } from "@earendil-works/pi-ai";

export type AvailableModel = {
  ref: string;
  provider: string;
  id: string;
  name: string;
  contextWindow: number;
  reasoning: boolean;
  authSource: string;
};

/** Whether a `models.preferred` pattern (`provider/id`, or `provider/*`) names `ref`. */
export const matches = (pattern: string, ref: string) => (pattern.endsWith("/*") ? ref.startsWith(pattern.slice(0, -1)) : pattern === ref);

/**
 * Chat models whose provider has auth configured, preferred ones first. Uses pi-ai's `getAvailable` and `checkAuth`,
 * which read credentials without refreshing them, so listing never rotates an OAuth token.
 * Only reasoning models are listed: pi-ai's catalog has no tool-use flag, and the review needs multi-step tool use.
 */
export async function listAvailableModels(models: Models, preferred: readonly string[]): Promise<AvailableModel[]> {
  const chat = (await models.getAvailable()).filter((m) => m.reasoning);
  const sources = new Map<string, string>();
  for (const provider of new Set(chat.map((m) => m.provider))) {
    const check = await models.checkAuth(provider);
    sources.set(provider, check?.source ?? check?.type ?? "unknown");
  }
  const rank = (ref: string) => {
    const i = preferred.findIndex((p) => matches(p, ref));
    return i === -1 ? preferred.length : i;
  };
  return chat
    .map((m) => ({
      ref: `${m.provider}/${m.id}`,
      provider: m.provider,
      id: m.id,
      name: m.name,
      contextWindow: m.contextWindow,
      reasoning: m.reasoning,
      authSource: sources.get(m.provider)!,
    }))
    .sort((a, b) => rank(a.ref) - rank(b.ref) || a.ref.localeCompare(b.ref));
}

export type ModelChoice =
  | { kind: "chosen"; ref: string; source: "--model" | "PR_REVIEW_MODEL" | "config" | "default" }
  | { kind: "pick" }
  | { kind: "error"; message: string };

/** Precedence: flag, then environment, then config; with none set, a TTY picks and anything else takes the first. */
export function chooseModel(
  sources: { flag?: string | undefined; env?: string | undefined; config?: string | undefined },
  available: readonly AvailableModel[],
  isTTY: boolean,
): ModelChoice {
  if (available.length === 0) {
    return {
      kind: "error",
      message: "no models available: run `pr-review login anthropic`, or set ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENROUTER_API_KEY or AI_GATEWAY_API_KEY",
    };
  }
  const named = ([["--model", sources.flag], ["PR_REVIEW_MODEL", sources.env], ["config", sources.config]] as const).find(([, ref]) => ref);
  if (named) {
    const [source, ref] = named;
    if (available.some((m) => m.ref === ref)) return { kind: "chosen", ref: ref!, source };
    return { kind: "error", message: `model ${ref} (from ${source}) is not available; available: ${available.map((m) => m.ref).join(", ")}` };
  }
  return isTTY ? { kind: "pick" } : { kind: "chosen", ref: available[0]!.ref, source: "default" };
}

export function splitRef(ref: string): { provider: string; modelId: string } {
  const slash = ref.indexOf("/");
  return { provider: ref.slice(0, slash), modelId: ref.slice(slash + 1) };
}
