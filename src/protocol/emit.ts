// `pnpm protocol:emit`: writes protocol/schema/v1, protocol/fixtures and protocol/playback from the TypeBox source.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { TSchema } from "typebox";
import { DEFAULT_SUMMARY_TEMPLATE, fillTemplate, templateSamples } from "../publish/template.ts";
import { fixtures, invalidFixtures, playback } from "./fixtures.ts";
import { ClientMessage, ENTITIES, ServerMessage } from "./v1.ts";

export const PROTOCOL_DIR = join(dirname(fileURLToPath(import.meta.url)), "../../protocol");
export const GENERATED_DIRS = ["schema", "fixtures", "playback"];

// JSON.stringify leaves DEL and C1 controls raw; fixtures must carry them as \u escapes so they survive any tooling.
const json = (value: unknown, indent?: number) =>
  JSON.stringify(value, null, indent).replace(/[\u007f-\u009f]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`);

const schema = (name: string, s: TSchema) =>
  json({ $schema: "https://json-schema.org/draft/2020-12/schema", $id: `pr-review/protocol/v1/${name}.json`, ...s }, 2) + "\n";

const byId = <T extends { id: string }>(rows: T[]) => [...rows].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

/** Relative path under protocol/ -> file content. */
export function protocolFiles(): Map<string, string> {
  const files = new Map<string, string>();
  files.set("schema/v1/client-message.json", schema("client-message", ClientMessage));
  files.set("schema/v1/server-message.json", schema("server-message", ServerMessage));
  for (const [name, s] of Object.entries(ENTITIES)) files.set(`schema/v1/${name}.json`, schema(name, s));
  for (const [name, message] of Object.entries(fixtures)) files.set(`fixtures/${name}.json`, json(message, 2) + "\n");
  // What a client's preview must show for the default template filled with the samples, byte for byte.
  files.set("fixtures/summary-template.preview.md", fillTemplate(DEFAULT_SUMMARY_TEMPLATE, templateSamples()));
  for (const [name, [message, why]] of Object.entries(invalidFixtures)) {
    files.set(`fixtures/invalid/${name}.json`, json(message, 2) + "\n");
    files.set(`fixtures/invalid/${name}.why.txt`, why + "\n");
  }
  for (const [name, { script, expect }] of Object.entries(playback)) {
    files.set(`playback/${name}.ndjson`, script.map((line) => json(line) + "\n").join(""));
    const sorted = { ...expect, prs: byId(expect.prs), runs: byId(expect.runs), findings: byId(expect.findings) };
    files.set(`playback/${name}.expect.json`, json(sorted, 2) + "\n");
  }
  return files;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  for (const dir of GENERATED_DIRS) rmSync(join(PROTOCOL_DIR, dir), { recursive: true, force: true });
  for (const [path, content] of protocolFiles()) {
    mkdirSync(dirname(join(PROTOCOL_DIR, path)), { recursive: true });
    writeFileSync(join(PROTOCOL_DIR, path), content);
  }
}
