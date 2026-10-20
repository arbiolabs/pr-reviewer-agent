import { readFileSync } from "node:fs";
import type { Context } from "@earendil-works/chord";
import { Type } from "@earendil-works/pi-ai";
import { defineDoc, defineExtension, defineTool, type Extension, section, type ToolExecutionApi, type ToolExecutionResult } from "@earendil-works/pi-durable";
import type { CandidateFinding } from "../domain/types.ts";
import type { AppStore, RevisionRow, Snapshot } from "../persistence/store.ts";
import type { FileDiff } from "../source/diff.ts";
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

export type ToolDeps = { store: AppStore; workspace(revisionId: string): Promise<Workspace> };

const text = (value: string): ToolExecutionResult => ({ output: [{ type: "text", text: value || "(empty)" }] });

const numbered = (source: string) =>
  source
    .replace(/\n$/, "")
    .split("\n")
    .map((line, i) => `${String(i + 1).padStart(5)}  ${line}`)
    .join("\n");

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

  return defineExtension({
    name: REVIEW_TOOLS_NAME,
    sections: [section("reviewer", () => REVIEWER_PROMPT, { tag: false })],
    tools: [
      defineTool({
        name: "get_changed_files",
        description: "List the files this pull request changes, with git status letters (A, M, D, R).",
        parameters: Type.Object({}),
        replay: "safe",
        execute: async (_args, api, context) => {
          const ws = await workspace(api, context);
          const lines = ws.snapshot.changedFiles.map((f) => `${f.status} ${f.path}`);
          return text([...lines, ...ws.snapshot.limitations.map((l) => `note: ${l}`)].join("\n"));
        },
      }),
      defineTool({
        name: "read_diff",
        description: "Read the unified diff of one changed file between the base and head commits.",
        parameters: Type.Object({ file: Type.String() }),
        replay: "safe",
        execute: async (args, api, context) => {
          const ws = await workspace(api, context);
          const file = ws.files.get(assertSafePath(args.file));
          if (file === undefined) throw new Error(`${args.file} is not a changed file with a head side`);
          deps.store.markCodeRead(ws.revision.id);
          return text(capBytes(file.text));
        },
      }),
      defineTool({
        name: "read_file",
        description: "Read a file at the base or head commit, with 1-based line numbers.",
        parameters: Type.Object({ path: Type.String(), ref: Type.Union([Type.Literal("base"), Type.Literal("head")]) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const ws = await workspace(api, context);
          const sha = args.ref === "base" ? ws.repo.baseSha : ws.repo.headSha;
          const source = await ws.repo.show(sha, args.path);
          if (source === undefined) throw new Error(`${args.path} does not exist at ${args.ref} (${sha})`);
          deps.store.markCodeRead(ws.revision.id);
          return text(capBytes(`${args.path} @ ${sha}\n${numbered(source)}`));
        },
      }),
      defineTool({
        name: "search_code",
        description: "Search the head commit for a literal string. Optional scope is a directory or file path.",
        parameters: Type.Object({ query: Type.String({ minLength: 1, maxLength: 200 }), scope: Type.Optional(Type.String()) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const ws = await workspace(api, context);
          const found = await ws.repo.grep(args.query, args.scope);
          deps.store.markCodeRead(ws.revision.id);
          return text(found);
        },
      }),
      defineTool({
        name: "git_history",
        description: "Show recent commits touching a path, newest first, up to the head commit.",
        parameters: Type.Object({ path: Type.String(), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })) }),
        replay: "safe",
        execute: async (args, api, context) => {
          const ws = await workspace(api, context);
          return text(capBytes(await ws.repo.log(args.path, args.limit ?? 10)));
        },
      }),
      defineTool({
        name: "previous_findings",
        description: "List findings recorded on earlier revisions of this pull request.",
        parameters: Type.Object({}),
        replay: "safe",
        execute: async (_args, api, context) => {
          const ws = await workspace(api, context);
          const findings = deps.store.previousFindings(ws.revision.pr_id, ws.revision.id);
          return text(findings.length === 0 ? "No earlier findings." : JSON.stringify(findings, null, 2));
        },
      }),
      defineTool({
        name: "submit_candidates",
        description:
          "Submit the final list of candidate defects for verification. Call exactly once. An empty list is valid after you have read the diff or code.",
        parameters: SubmitCandidatesSchema,
        replay: "safe",
        execute: async (args, api, context) => {
          const ws = await workspace(api, context);
          // §2 invariant 9: "no findings" is a result only when the code was looked at.
          if (args.candidates.length === 0 && !deps.store.revision(ws.revision.id)?.code_read_at) {
            throw new Error("An empty submission needs a look at the code first: call read_diff, read_file or search_code, then submit again. Nothing was stored.");
          }
          const { stored } = deps.store.saveCandidateSet(ws.revision.id, args.candidates as CandidateFinding[]);
          return {
            ...text(stored ? `Stored ${args.candidates.length} candidate(s) for verification.` : "Candidates were already submitted for this revision; this call changed nothing."),
            control: { terminate: true },
          };
        },
      }),
    ],
  });
}
