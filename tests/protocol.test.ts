import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { Compile } from "typebox/compile";
import { describe, expect, it } from "vitest";
import { GENERATED_DIRS, PROTOCOL_DIR, protocolFiles } from "../src/protocol/emit.ts";
import type { Expect } from "../src/protocol/fixtures.ts";
import { sanitize } from "../src/protocol/sanitize.ts";
import { type Account, ClientMessage, KNOWN_EVENT_TYPES, ServerMessage, type Finding, type Pr, type Run } from "../src/protocol/v1.ts";

const client = Compile(ClientMessage);
const server = Compile(ServerMessage);
const read = (path: string) => readFileSync(join(PROTOCOL_DIR, path), "utf8");
const list = (dir: string): string[] =>
  readdirSync(join(PROTOCOL_DIR, dir), { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile())
    .map((e) => relative(PROTOCOL_DIR, join(e.parentPath, e.name)));
const validatorFor = (name: string) => (name.startsWith("client.") ? client : server);

it("committed protocol files match `pnpm protocol:emit`", () => {
  const emitted = protocolFiles();
  const committed = GENERATED_DIRS.flatMap(list).sort();
  expect(committed).toEqual([...emitted.keys()].sort());
  for (const [path, content] of emitted) expect(read(path), path).toBe(content);
});

describe("fixtures", () => {
  const names = (dir: string, ext: string) =>
    readdirSync(join(PROTOCOL_DIR, dir)).filter((f) => f.endsWith(ext)).map((f) => f.slice(0, -ext.length));

  it.each(names("fixtures", ".json"))("%s is valid", (name) => {
    const v = validatorFor(name);
    const message = JSON.parse(read(`fixtures/${name}.json`));
    expect(v.Check(message), JSON.stringify([...v.Errors(message)].slice(0, 3))).toBe(true);
  });

  it.each(names("fixtures/invalid", ".json"))("invalid/%s is rejected and explained", (name) => {
    expect(validatorFor(name).Check(JSON.parse(read(`fixtures/invalid/${name}.json`)))).toBe(false);
    expect(read(`fixtures/invalid/${name}.why.txt`).trim()).not.toBe("");
  });

  it("covers every known event type", () => {
    const seen = new Set(names("fixtures", ".json").map((n) => JSON.parse(read(`fixtures/${n}.json`)).eventType));
    for (const t of KNOWN_EVENT_TYPES) expect(seen.has(t), t).toBe(true);
  });

  it("carries control bytes as JSON escapes", () => {
    const raw = read("fixtures/server.snapshot.hostile.json");
    expect(raw).toContain("\\u001b]8;;");
    expect(raw).toContain("\\u009b2J");
    expect(raw).not.toMatch(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/);
  });
});

it("sanitize strips terminal control sequences and keeps text, newlines and tabs", () => {
  expect(sanitize("\u001b[1;31mred\u001b[0m")).toBe("red");
  expect(sanitize("\u001b]8;;https://x\u0007link\u001b]8;;\u001b\\")).toBe("link");
  expect(sanitize("\u001b]52;c;ZXZpbA==\u0007ok")).toBe("ok");
  expect(sanitize("a\u009b2Jb\u009d0;title\u009cc")).toBe("abc");
  expect(sanitize("x\u0090q\u009cy\u001bcz\u0085")).toBe("xyz");
  expect(sanitize("tab\there\nnext\rline\u0000\u007f")).toBe("tab\there\nnextline");
  expect(sanitize("修复 🔥 👩‍💻")).toBe("修复 🔥 👩‍💻");
});

// The reference client: the rules protocol/README.md states, which the Rust reducer must match on every script.
function replay(ndjson: string): Expect {
  let epoch = "";
  let last = -1;
  let inSync = false;
  let resyncRequested = false;
  let toolActivityCount = 0;
  let prs = new Map<string, Pr>();
  let runs = new Map<string, Run>();
  let findings = new Map<string, Finding>();
  let accounts: Account[] | undefined;
  const needResync = () => {
    inSync = false;
    resyncRequested = true;
  };

  for (const line of ndjson.trimEnd().split("\n")) {
    const { delayMs, message } = JSON.parse(line);
    expect(Number.isInteger(delayMs) && delayMs >= 0).toBe(true);
    expect(server.Check(message), line.slice(0, 200)).toBe(true);
    const m = message as ServerMessage;
    if (m.type === "snapshot") {
      [epoch, last, inSync] = [m.streamEpoch, m.sequence, true];
      prs = new Map(m.prs.map((p) => [p.id, p]));
      runs = new Map(m.runs.map((r) => [r.id, r]));
      findings = new Map(m.findings.map((f) => [f.id, f]));
      accounts = m.accounts;
    } else if (m.type === "resync.required") {
      needResync();
    } else if (m.type === "event" && "sequence" in m && inSync) {
      if (m.streamEpoch !== epoch) needResync();
      else if (m.sequence <= last) continue;
      else if (m.prevSequence !== last) needResync();
      else {
        last = m.sequence;
        const e = m as Extract<ServerMessage, { type: "event"; eventType: (typeof KNOWN_EVENT_TYPES)[number] }>;
        switch (e.eventType) {
          case "pr.updated":
            prs.set(e.data.id, e.data);
            break;
          case "run.started":
          case "review.completed":
          case "run.superseded":
          case "run.failed":
            runs.set(e.data.id, e.data);
            break;
          case "review.phase_changed": {
            const r = runs.get(e.runId);
            if (r) runs.set(r.id, { ...r, phase: e.data.to });
            break;
          }
          case "finding.updated":
            findings.set(e.data.id, e.data);
            break;
          case "tool.activity":
            toolActivityCount++;
            break;
          case "accounts.updated":
            accounts = e.data.accounts;
            break;
        }
      }
    }
  }
  const sorted = <T extends { id: string }>(m: Map<string, T>) => [...m.values()].sort((a, b) => (a.id < b.id ? -1 : 1));
  return {
    streamEpoch: epoch,
    lastSequence: last,
    resyncRequested,
    toolActivityCount,
    prs: sorted(prs).map((p) => ({ id: p.id, headSha: p.headSha, title: sanitize(p.title) })),
    runs: sorted(runs).map((r) => ({ id: r.id, phase: r.phase, status: r.status })),
    findings: sorted(findings).map((f) => ({ id: f.id, title: sanitize(f.title), status: f.status, stale: f.stale })),
    ...(accounts ? { accounts: [...accounts].sort((a, b) => (a.id < b.id ? -1 : 1)).map(({ id, state, ghActive, selected }) => ({ id, state, ghActive, selected })) } : {}),
  };
}

const scripts = readdirSync(join(PROTOCOL_DIR, "playback"))
  .filter((f) => f.endsWith(".ndjson"))
  .map((f) => f.slice(0, -".ndjson".length));

it.each(scripts)("playback %s reaches its expected client state", (name) => {
  expect(replay(read(`playback/${name}.ndjson`))).toEqual(JSON.parse(read(`playback/${name}.expect.json`)));
});
