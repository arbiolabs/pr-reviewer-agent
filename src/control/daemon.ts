import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { AppDbSource } from "./appdb.ts";
import { defaultSocketPath, startControlServer } from "./server.ts";
import { FixtureSource } from "./source.ts";

/**
 * `pr-review daemon`: serve the control socket in the foreground until SIGINT or SIGTERM. It takes no data-dir lock, so
 * it runs beside `pr-review review`; its only app DB writes are account rows, and no client action reaches a durable task.
 */
export async function runDaemon(args: string[], dataDir: string, err: (text: string) => void): Promise<number> {
  const { values } = parseArgs({ args, options: { socket: { type: "string" }, fixture: { type: "string" }, data: { type: "string" }, heartbeat: { type: "string" } } });
  const heartbeat = Number(values.heartbeat ?? 15);
  if (!(heartbeat > 0)) throw new Error("--heartbeat takes a positive number of seconds");
  const source = values.fixture ? FixtureSource.load(values.fixture) : AppDbSource.open(values.data ?? dataDir);
  try {
    const server = await startControlServer({ socketPath: values.socket ?? defaultSocketPath(), source, heartbeatMs: heartbeat * 1000 });
    err(`pr-review daemon: listening on ${server.path} (${values.fixture ? `fixture ${values.fixture}` : `data dir ${values.data ?? dataDir}`})\n`);
    await new Promise((resolve) => {
      process.once("SIGINT", resolve);
      process.once("SIGTERM", resolve);
    });
    await server.close();
    return 0;
  } finally {
    source.close();
  }
}

const RELEASE_BINARY = fileURLToPath(new URL("../../tui/target/release/pr-review-tui", import.meta.url));

/** `pr-review tui`: run the Rust client from PATH, else from this checkout's release build, with the same args. */
export function runTui(args: string[]): number {
  for (const bin of ["pr-review-tui", RELEASE_BINARY]) {
    if (bin === RELEASE_BINARY && !existsSync(bin)) break;
    const result = spawnSync(bin, args, { stdio: "inherit" });
    if ((result.error as NodeJS.ErrnoException | undefined)?.code === "ENOENT") continue;
    if (result.error) throw result.error;
    return result.status ?? 1;
  }
  throw new Error(`pr-review-tui not found on PATH or at ${RELEASE_BINARY}; build it with: cargo build --release --manifest-path tui/Cargo.toml`);
}
