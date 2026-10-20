import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { AuthOperationOptions, Credential, CredentialInfo, CredentialStore } from "@earendil-works/pi-ai";
import lockfile from "proper-lockfile";

export const DEFAULT_AUTH_FILE = join(homedir(), ".pi", "agent", "auth.json");

export function authFilePath(): string {
  return process.env.PR_REVIEW_AUTH_FILE || DEFAULT_AUTH_FILE;
}

/**
 * pi's credential file, shared with the `pi` CLI. OAuth refresh tokens rotate, so every write must happen under the
 * same lock pi takes (`proper-lockfile` on the file path, `realpath: false`) and re-read the file inside it; a write
 * that skips either can store a stale token and log the user out of pi.
 */
export class PiAuthFileStore implements CredentialStore {
  readonly path: string;

  constructor(path = authFilePath()) {
    this.path = path;
  }

  private readAll(): Record<string, Credential> {
    if (!existsSync(this.path)) return {};
    const text = readFileSync(this.path, "utf8").trim();
    return text === "" ? {} : (JSON.parse(text) as Record<string, Credential>);
  }

  async read(providerId: string): Promise<Credential | undefined> {
    return this.readAll()[providerId];
  }

  async list(): Promise<readonly CredentialInfo[]> {
    return Object.entries(this.readAll()).map(([providerId, credential]) => ({ providerId, type: credential.type }));
  }

  async modify(
    providerId: string,
    fn: (current: Credential | undefined) => Promise<Credential | undefined>,
    options?: AuthOperationOptions,
  ): Promise<Credential | undefined> {
    return this.withLock(async (all) => {
      const next = await fn(all[providerId]);
      if (next === undefined) return { result: all[providerId] };
      return { result: next, write: { ...all, [providerId]: next } };
    }, options);
  }

  async delete(providerId: string, options?: AuthOperationOptions): Promise<void> {
    await this.withLock(async (all) => {
      const { [providerId]: _removed, ...rest } = all;
      return { result: undefined, write: rest };
    }, options);
  }

  private async withLock<T>(
    fn: (all: Record<string, Credential>) => Promise<{ result: T; write?: Record<string, Credential> }>,
    options?: AuthOperationOptions,
  ): Promise<T> {
    options?.signal?.throwIfAborted();
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    if (!existsSync(this.path)) writeFileSync(this.path, "{}", { mode: 0o600, flag: "wx" });
    let compromised: Error | undefined;
    // Same lock file and staleness as pi's FileAuthStorageBackend, so pi and this process exclude each other.
    const release = await lockfile.lock(this.path, {
      realpath: false,
      stale: 30_000,
      retries: { retries: 60, minTimeout: 10, maxTimeout: 1000, randomize: true },
      onCompromised: (error) => {
        compromised = error;
      },
    });
    try {
      const { result, write } = await fn(this.readAll());
      if (compromised) throw compromised;
      if (write !== undefined) {
        const temp = `${this.path}.${process.pid}.tmp`;
        writeFileSync(temp, JSON.stringify(write, null, 2), { mode: 0o600 });
        renameSync(temp, this.path);
      }
      return result;
    } finally {
      await release();
    }
  }
}

/** A store that can be read but never written, for runs that must not risk touching a shared credential. */
export class ReadOnlyCredentialStore implements CredentialStore {
  private readonly inner: CredentialStore;

  constructor(inner: CredentialStore) {
    this.inner = inner;
  }

  read(providerId: string) {
    return this.inner.read(providerId);
  }

  list() {
    return this.inner.list();
  }

  async modify(providerId: string): Promise<Credential | undefined> {
    throw new Error(`refusing to write credential for ${providerId}: read-only credential store`);
  }

  async delete(providerId: string): Promise<void> {
    throw new Error(`refusing to delete credential for ${providerId}: read-only credential store`);
  }
}
