import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import type { Context } from "@earendil-works/chord";
import { Type } from "@earendil-works/pi-ai";
import {
  defineDoc,
  defineExtension,
  defineTool,
  type Extension,
  GenerationTask,
  hook,
  type HookApi,
  section,
  type ToolExecutionApi,
  type ToolExecutionResult,
} from "@earendil-works/pi-durable";
import type { CandidateFinding, ResolutionClaim } from "../domain/types.ts";
import type { AppStore, RevisionRow, Snapshot } from "../persistence/store.ts";
import type { FileDiff } from "../source/diff.ts";
import { isGenerated } from "./guidance.ts";
import { assertSafePath, capBytes, type PinnedRepo } from "../source/git.ts";
import { SubmitCandidatesSchema } from "./schema.ts";

const REVIEWER_PROMPT = readFileSync(new URL("../../prompts/reviewer.md", import.meta.url), "utf8");

/** Which revision a reviewer conversation works on; written in the commit that creates the conversation. */
export const TargetDoc = defineDoc<{ revisionId: string }>({
  kind: "pr-review.target",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "current",
  initial: () => ({ revisionId: "" }),
});

export type Workspace = { revision: RevisionRow; snapshot: Snapshot; repo: PinnedRepo; files: Map<string, FileDiff> };

export type ToolDeps = { store: AppStore; workspace(revisionId: string): Promise<Workspace>; generatedPaths?: string[] };

/** How long one search_code may run before it answers "timed out" instead. */
export const SEARCH_TIMEOUT_MS = 60_000;

/**
 * `work` under a signal that also aborts after `ms`: its result, or `undefined` when the time ran out first. An abort
 * of `outer` (the review stopping) still rejects, so a shutdown is never mistaken for a slow call.
 */
export async function withTimeout<T>(work: (signal: AbortSignal) => Promise<T>, ms: number, outer?: AbortSignal): Promise<T | undefined> {
  const timeout = AbortSignal.timeout(ms);
  try {
    return await work(outer ? AbortSignal.any([outer, timeout]) : timeout);
  } catch (error) {
    if (timeout.aborted && !outer?.aborted) return undefined;
    throw error;
  }
}

const text = (value: string): ToolExecutionResult => ({ output: [{ type: "text", text: value || "(empty)" }] });

const numbered = (source: string) =>
  source
    .replace(/\n$/, "")
    .split("\n")
    .map((line, i) => `${String(i + 1).padStart(5)}  ${line}`)
    .join("\n");


/** Seconds since `startedAt`, as an activity line says it: `0.4s`, `12.3s`, `2m 14s`. */
export const took = (startedAt: string) => {
  const s = Math.max(0, (Date.now() - Date.parse(startedAt)) / 1000);
  return s < 60 ? `${s.toFixed(1)}s` : `${Math.floor(s / 60)}m ${Math.floor(s % 60)}s`;
};

export const REVIEW_TOOLS_NAME = "pr-review-tools";

/**
 * The only tools the review model gets. All read pinned git objects or the app DB, so rerunning any of them after a
 * crash returns the same result; `submit_candidates` is a first-writer-wins upsert, so replay is safe there too.
 */
export function createReviewTools(deps: ToolDeps): Extension {
  async function workspace(api: ToolExecutionApi, context: Context): Promise<Workspace> {
    const target = await api.snapshot(TargetDoc, api.conversationId, context);
    if (!target?.revisionId) throw new Error("this conversation is not bound to a review revision");
    return deps.workspace(target.revisionId);
  }

  // The model turn in flight per revision: its tool calls are its children on the waterfall. In memory only: after a
  // restart the count starts again, and the ids stay distinct.
  type Turn = { n: number; id: string; startedAt: string; outputTokens?: number; toolsFrom?: string; open: boolean };
  const turns = new Map<string, Turn>();
  const revisionOf = async (api: HookApi, context: Context) => (await api.snapshot(TargetDoc, api.conversationId, context))?.revisionId || undefined;
  const turnLine = (revisionId: string, turn: Turn, summary: string, ended: boolean) => {
    if (ended) turn.open = false;
    deps.store.appendActivity(revisionId, "model", summary, {
      id: turn.id,
      parent: "phase:analyze",
      kind: "model",
      startedAt: turn.startedAt,
      ...(ended ? { endedAt: new Date().toISOString() } : {}),
      ...(turn.outputTokens === undefined ? {} : { outputTokens: turn.outputTokens }),
    });
  };
  const log = (ws: Workspace, tool: string, summary: string, startedAt: string) =>
    deps.store.appendActivity(ws.revision.id, tool, summary, {
      id: `tool:${randomUUID().slice(0, 8)}`,
      ...(turns.has(ws.revision.id) ? { parent: turns.get(ws.revision.id)!.id } : {}),
      kind: "tool",
      startedAt,
      endedAt: new Date().toISOString(),
    });

  return defineExtension({
    name: REVIEW_TOOLS_NAME,
    sections: [section("reviewer", () => REVIEWER_PROMPT, { tag: false })],
    // Each model request opens a turn span. Its answer updates the span with the timing, output tokens and the
    // tools it called, and the span closes once those tools are done, so they sit inside it. Neither the request
    // nor the answer's text is read.
    hooks: [
      hook(GenerationTask, {
        beforeRequest: async (_request, api, context) => {
          const revisionId = await revisionOf(api, context);
          if (!revisionId) return undefined;
          const last = turns.get(revisionId);
          if (last?.open) turnLine(revisionId, last, `turn ${last.n} ended`, true);
          const n = (last?.n ?? 0) + 1;
          const turn = { n, id: `turn:${n}:${randomUUID().slice(0, 8)}`, startedAt: new Date().toISOString(), open: true };
          turns.set(revisionId, turn);
          turnLine(revisionId, turn, `turn ${n} started`, false);
          return undefined;
        },
        afterResponse: async (message, api, context) => {
          const revisionId = await revisionOf(api, context);
          // Recorded before the turn check: a response polled after a restart has no open turn but was still billed.
          // The harness calls this hook only on a live provider answer, never on a replayed entry, so it counts once.
          const u = message.usage;
          if (revisionId && u && u.input + u.output + u.cacheRead + u.cacheWrite + u.cost.total > 0) {
            deps.store.recordUsage(revisionId, `${message.provider}/${message.model}`, u);
          }
          const turn = revisionId ? turns.get(revisionId) : undefined;
          if (!revisionId || !turn?.open) return;
          const calls = new Map<string, number>();
          for (const block of message.content) if (block.type === "toolCall") calls.set(block.name, (calls.get(block.name) ?? 0) + 1);
          const called = [...calls].map(([name, k]) => (k > 1 ? `${name} x${k}` : name)).join(", ");
          turn.outputTokens = message.usage?.output ?? 0;
          const tokens = turn.outputTokens >= 1000 ? `${(turn.outputTokens / 1000).toFixed(1)}k` : String(turn.outputTokens);
          const stopped = message.stopReason === "error" || message.stopReason === "aborted";
          const end = stopped ? `stopped: ${message.stopReason}` : called ? `called ${called}` : "answered";
          turn.toolsFrom = new Date().toISOString();
          turnLine(revisionId, turn, `turn ${turn.n}: ${took(turn.startedAt)}, ${tokens} output tokens, ${end}`, stopped || !called);
        },
        afterTools: async (_assistant, results, api, context) => {
          const revisionId = await revisionOf(api, context);
          const turn = revisionId ? turns.get(revisionId) : undefined;
          if (!revisionId || !turn?.open) return;
          turnLine(revisionId, turn, `turn ${turn.n}: ${results.length} tool result(s) in ${took(turn.toolsFrom ?? turn.startedAt)}`, true);
        },
      }),
    ],
    tools: [
      defineTool({
        name: "get_changed_files",
        description: "List the files this pull request changes, with git status letters (A, M, D, R). Lock files, generated and vendored code are marked (generated).",
        parameters: Type.Object({}),
        replay: "safe",
        execute: async (_args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          const lines = ws.snapshot.changedFiles.map((f) => `${f.status} ${f.path}${isGenerated(f.path, deps.generatedPaths ?? []) ? " (generated)" : ""}`);
          log(ws, "get_changed_files", `${lines.length} files`, startedAt);
          return text([...lines, ...ws.snapshot.limitations.map((l) => `note: ${l}`)].join("\n"));
        },
      }),
      defineTool({
        name: "read_diff",
        description: "Read the unified diff of one changed file between the base and head commits.",
        parameters: Type.Object({ file: Type.String() }),
        replay: "safe",
        execute: async (args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          const file = ws.files.get(assertSafePath(args.file));
          if (file === undefined) throw new Error(`${args.file} is not a changed file with a head side`);
          deps.store.markCodeRead(ws.revision.id);
          log(ws, "read_diff", file.path, startedAt);
          return text(capBytes(file.text));
        },
      }),
      defineTool({
        name: "read_file",
        description: "Read a file at the base or head commit, with 1-based line numbers.",
        parameters: Type.Object({ path: Type.String(), ref: Type.Union([Type.Literal("base"), Type.Literal("head")]) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          const sha = args.ref === "base" ? ws.repo.baseSha : ws.repo.headSha;
          const source = await ws.repo.show(sha, args.path, { signal: context.abortSignal });
          if (source === undefined) throw new Error(`${args.path} does not exist at ${args.ref} (${sha})`);
          deps.store.markCodeRead(ws.revision.id);
          log(ws, "read_file", `${args.path} @${args.ref}`, startedAt);
          return text(capBytes(`${args.path} @ ${sha}\n${numbered(source)}`));
        },
      }),
      defineTool({
        name: "search_code",
        description: "Search the head commit for a literal string. Optional scope is a directory or file path.",
        parameters: Type.Object({ query: Type.String({ minLength: 1, maxLength: 200 }), scope: Type.Optional(Type.String()) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          // The tree fetch is outside the limit: on a resumed review it has not run yet, and cutting it short would leave
          // the search to fetch blob by blob. A search that runs long answers the model instead of holding the review.
          await ws.repo.prefetch(context.abortSignal);
          const found = await withTimeout((signal) => ws.repo.grep(args.query, args.scope, undefined, signal), SEARCH_TIMEOUT_MS, context.abortSignal);
          if (found === undefined) {
            deps.store.appendActivity(ws.revision.id, "search_code", `timed out after ${SEARCH_TIMEOUT_MS / 1000}s: ${args.query}`);
            return text(`search_code timed out after ${SEARCH_TIMEOUT_MS / 1000} s. Narrow the scope to a smaller directory or file, or search for a more specific string.`);
          }
          deps.store.markCodeRead(ws.revision.id);
          log(ws, "search_code", args.scope ? `${args.query} in ${args.scope}` : args.query, startedAt);
          return text(found);
        },
      }),
      defineTool({
        name: "git_history",
        description: "Show recent commits touching a path, newest first, up to the head commit.",
        parameters: Type.Object({ path: Type.String(), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          const history = await ws.repo.log(args.path, args.limit ?? 10, context.abortSignal);
          log(ws, "git_history", `${args.path} limit ${args.limit ?? 10}`, startedAt);
          return text(capBytes(history));
        },
      }),
      defineTool({
        name: "previous_findings",
        description: "List findings recorded on earlier revisions of this pull request.",
        parameters: Type.Object({}),
        replay: "safe",
        execute: async (_args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          const findings = deps.store.previousFindings(ws.revision.pr_id, ws.revision.id);
          log(ws, "previous_findings", `${findings.length} found`, startedAt);
          return text(findings.length === 0 ? "No earlier findings." : JSON.stringify(findings, null, 2));
        },
      }),
      defineTool({
        name: "submit_candidates",
        description:
          "Submit the final list of candidate defects for verification, and the open findings from earlier reviews that this head's code shows fixed. Call exactly once. An empty list is valid after you have read the diff or code.",
        parameters: SubmitCandidatesSchema,
        replay: "safe",
        execute: async (args, api, context) => {
          const startedAt = new Date().toISOString();
          const ws = await workspace(api, context);
          // §2 invariant 9: "no findings" is a result only when the code was looked at.
          if (args.candidates.length === 0 && !deps.store.revision(ws.revision.id)?.code_read_at) {
            throw new Error("An empty submission needs a look at the code first: call read_diff, read_file or search_code, then submit again. Nothing was stored.");
          }
          const resolutions = (args.resolutions ?? []) as ResolutionClaim[];
          const { stored } = deps.store.saveCandidateSet(ws.revision.id, args.candidates as CandidateFinding[], resolutions);
          log(ws, "submit_candidates", `${args.candidates.length} candidate(s), ${resolutions.length} resolution claim(s)`, startedAt);
          return {
            ...text(stored ? `Stored ${args.candidates.length} candidate(s) and ${resolutions.length} resolution claim(s) for verification.` : "Candidates were already submitted for this revision; this call changed nothing."),
            control: { terminate: true },
          };
        },
      }),
    ],
  });
}
