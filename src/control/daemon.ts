import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { which } from "../models/detect.ts";
import type { SetupCheck } from "../protocol/v1.ts";
import { AppDbSource } from "./appdb.ts";
import type { EngineSource } from "./engine.ts";
import { defaultSocketPath, startControlServer } from "./server.ts";
import { type ControlSource, FixtureSource } from "./source.ts";

export type DaemonDeps = {
  /** Open the engine for `dataDir`; throws `another pr-review is running on ...` while another process holds the lock. */
  openEngine?: (dataDir: string) => Promise<EngineSource>;
  /** `control.allow_commands` from the config: the other way, besides `--allow-commands`, to enable the engine. */
  commandsConfigured?: () => boolean;
  /** `pr-review setup --check` for `dataDir`, which each welcome carries. */
  checks?: (dataDir: string) => Promise<SetupCheck[]>;
};

export const COMMANDS_OFF = "review commands are off; start the daemon with --allow-commands or set control.allow_commands";

/**
 * Read-only by default (TUI_SPEC §15.2): the daemon takes no lock, runs no task and grants only `account.select`.
 * With commands enabled it hosts the engine when it can take the data-dir lock, and stays read-only otherwise (another
 * pr-review holds the lock, or the engine fails to open for another reason, no model say), saying why in `welcome` and
 * on every command.
 */
export async function openSource(dataDir: string, allowCommands: boolean, deps: DaemonDeps, err: (text: string) => void): Promise<ControlSource> {
  if (!allowCommands || !deps.openEngine) return AppDbSource.open(dataDir, process.env, allowCommands ? undefined : COMMANDS_OFF);
  try {
    return await deps.openEngine(dataDir);
  } catch (error) {
    const why = error instanceof Error ? error.message : String(error);
    err(`pr-review daemon: read-only mode (${why})\n`);
    return AppDbSource.open(dataDir, process.env, why);
  }
}

/**
 * `pr-review daemon`: serve the control socket in the foreground until SIGINT, SIGTERM or an accepted `daemon.stop`.
 * With commands enabled it hosts the review engine and runs the commands in process; stopping it suspends the reviews
 * in flight, which resume on the next engine start. Quitting a client never touches it.
 */
export async function runDaemon(args: string[], dataDir: string, err: (text: string) => void, deps: DaemonDeps = {}): Promise<number> {
  const { values } = parseArgs({
    args,
    options: { socket: { type: "string" }, fixture: { type: "string" }, data: { type: "string" }, heartbeat: { type: "string" }, "allow-commands": { type: "boolean" } },
  });
  const heartbeat = Number(values.heartbeat ?? 15);
  if (!(heartbeat > 0)) throw new Error("--heartbeat takes a positive number of seconds");
  const dir = values.data ?? dataDir;
  const allowCommands = (values["allow-commands"] ?? false) || (deps.commandsConfigured?.() ?? false);
  const source: ControlSource = values.fixture ? FixtureSource.load(values.fixture) : await openSource(dir, allowCommands, deps, err);
  try {
    // The client asking is the TUI, so whether it is built says nothing; a fixture has no environment to check.
    const checks = values.fixture || !deps.checks ? undefined : async () => (await deps.checks!(dir)).filter((c) => c.id !== "tui");
    // SIGINT, SIGTERM and an accepted daemon.stop all end here, so each one shuts down the same way.
    let stop!: () => void;
    const stopped = new Promise<void>((resolve) => (stop = resolve));
    process.once("SIGINT", stop);
    process.once("SIGTERM", stop);
    try {
      const server = await startControlServer({ socketPath: values.socket ?? defaultSocketPath(), source, heartbeatMs: heartbeat * 1000, checks, stop });
      const where = values.fixture ? `fixture ${values.fixture}` : `data dir ${dir}, ${source.mode ?? "read-only"} mode`;
      err(`pr-review daemon: listening on ${server.path} (${where}${source.capabilities?.().length ? ", commands enabled" : ""})\n`);
      await stopped;
      await server.close();
      return 0;
    } finally {
      process.off("SIGINT", stop);
      process.off("SIGTERM", stop);
    }
  } finally {
    await source.close();
  }
}

const RELEASE_BINARY = fileURLToPath(new URL("../../tui/target/release/pr-review-tui", import.meta.url));

/** The Rust client on PATH, else this checkout's release build. */
export function findTui(pathVar = process.env.PATH ?? ""): string | undefined {
  return which("pr-review-tui", pathVar) ?? (existsSync(RELEASE_BINARY) ? RELEASE_BINARY : undefined);
}

/** `pr-review tui`: run the client `findTui` finds, with the same args. */
export function runTui(args: string[]): number {
  const bin = findTui();
  if (!bin) throw new Error(`pr-review-tui not found on PATH or at ${RELEASE_BINARY}; build it with: cargo build --release --manifest-path tui/Cargo.toml`);
  const result = spawnSync(bin, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  return result.status ?? 1;
}
