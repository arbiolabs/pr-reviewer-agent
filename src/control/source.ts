import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { PROTOCOL_DIR } from "../protocol/emit.ts";
import { fixtures, type PlaybackLine } from "../protocol/fixtures.ts";
import type { ClientMessage, ErrorCode, Finding, FindingDetail, PublicationPreview, ReviewCommand, SequencedEvent, ServerMessage } from "../protocol/v1.ts";
import type { DiffPage } from "./diff.ts";

export class ControlError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, message: string) {
    super(message);
    this.code = code;
  }
}

export type Snapshot = Omit<Extract<ServerMessage, { type: "snapshot" }>, "type" | "requestId">;
export type DiffRequest = Extract<ClientMessage, { type: "diff.get" }>;
export type TimelineRequest = Extract<ClientMessage, { type: "timeline.get" }>;
export type TimelinePage = { events: SequencedEvent[]; nextBeforeSequence?: number };

/**
 * What the control server streams from. Events come back with their own `prevSequence`, so a source whose journal
 * has a hole shows it to the client instead of the server papering over it.
 */
export interface ControlSource {
  epoch(): string;
  latestSequence(): number;
  snapshot(): Snapshot;
  eventsAfter(sequence: number, limit: number): SequencedEvent[];
  diff(request: DiffRequest): Promise<DiffPage>;
  finding(findingId: string): Promise<FindingDetail>;
  timeline(request: TimelineRequest): TimelinePage;
  /** Present on sources that can select a GitHub account; the server offers the account.select capability only then. */
  refreshAccounts?(maxAgeMs: number): Promise<void>;
  /** Journal the selection and return the operation id, or throw ControlError NOT_FOUND / CONFLICT. */
  selectAccount?(accountId: string): Promise<string>;
  /** `welcome.mode`: engine sources host the review engine; read-only ones only read its database. */
  mode?: "engine" | "read-only";
  /** `welcome.readOnlyReason`: why a read-only source is read-only, as display text. */
  readOnlyReason?: string;
  /** The review commands this source grants now; `command` runs one of them (its key already checked and replayed). */
  capabilities?(): string[];
  /** Why a review command is not granted, as the CAPABILITY_DENIED message. */
  unavailable?(command: string): string;
  /** Run a granted review command and return its operation id, or throw ControlError. */
  command?(request: ReviewCommand): Promise<string>;
  /** The stored payload of a run, or throw ControlError NOT_FOUND. */
  publication?(runId: string): Promise<PublicationPreview>;
  close(): void | Promise<void>;
}

const isSequenced = (m: ServerMessage): m is SequencedEvent => m.type === "event" && "sequence" in m;

/** Newest first, `limit` per page, filtered the way timeline.get asks. */
export function timelinePage(events: SequencedEvent[], request: TimelineRequest, findingIdOf: (e: SequencedEvent) => string | undefined): TimelinePage {
  const matching = events
    .filter(
      (e) =>
        (request.prId === undefined || ("prId" in e && e.prId === request.prId)) &&
        (request.runId === undefined || ("runId" in e && e.runId === request.runId)) &&
        (request.findingId === undefined || findingIdOf(e) === request.findingId) &&
        (request.beforeSequence === undefined || e.sequence < request.beforeSequence),
    )
    .sort((a, b) => b.sequence - a.sequence);
  const page = matching.slice(0, request.limit);
  return matching.length > page.length ? { events: page, nextBeforeSequence: page.at(-1)!.sequence } : { events: page };
}

/**
 * The M0 fixture server's world: a `protocol/playback` script read as a journal. Lines are released on the script's
 * clock, which starts at the first snapshot. The latest released snapshot is the source's state; the events after it
 * in its epoch are its journal, passed through verbatim (a scripted gap stays a gap) with repeats dropped. Strings are
 * not sanitized: the hostile script exists to test the client's own sanitizing.
 */
export class FixtureSource implements ControlSource {
  private readonly lines: { at: number; message: ServerMessage }[];
  private readonly now: () => number;
  private start: number | undefined;

  constructor(script: PlaybackLine[], now = () => Date.now()) {
    let at = 0;
    this.lines = script.map((l) => ({ at: (at += l.delayMs), message: l.message }));
    if (!this.lines.some((l) => l.message.type === "snapshot")) throw new Error("a playback script needs a snapshot");
    this.now = now;
  }

  static names(): string[] {
    return readdirSync(join(PROTOCOL_DIR, "playback"))
      .filter((f) => f.endsWith(".ndjson"))
      .map((f) => f.slice(0, -".ndjson".length));
  }

  static load(name: string, now?: () => number): FixtureSource {
    const path = join(PROTOCOL_DIR, "playback", `${name}.ndjson`);
    if (!/^[\w-]+$/.test(name) || !existsSync(path)) throw new Error(`no fixture ${name}; choose one of ${FixtureSource.names().join(", ")}`);
    const script = readFileSync(path, "utf8").trimEnd().split("\n").map((l) => JSON.parse(l) as PlaybackLine);
    return new FixtureSource(script, now);
  }

  private released(): ServerMessage[] {
    const elapsed = this.start === undefined ? 0 : this.now() - this.start;
    // Everything up to the first snapshot is the starting state, there before the clock runs.
    const first = this.lines.findIndex((l) => l.message.type === "snapshot");
    return this.lines.filter((l, i) => i <= first || l.at <= elapsed).map((l) => l.message);
  }

  private state(): { snapshot: Snapshot; journal: SequencedEvent[] } {
    const released = this.released();
    const at = released.findLastIndex((m) => m.type === "snapshot");
    const { type: _, requestId: __, ...snapshot } = released[at] as Extract<ServerMessage, { type: "snapshot" }>;
    const journal: SequencedEvent[] = [];
    let last = snapshot.sequence;
    for (const m of released.slice(at + 1)) {
      if (!isSequenced(m) || m.streamEpoch !== snapshot.streamEpoch || m.sequence <= last) continue;
      journal.push(m);
      last = m.sequence;
    }
    return { snapshot, journal };
  }

  epoch(): string {
    return this.state().snapshot.streamEpoch;
  }

  latestSequence(): number {
    const { snapshot, journal } = this.state();
    return journal.at(-1)?.sequence ?? snapshot.sequence;
  }

  snapshot(): Snapshot {
    this.start ??= this.now();
    return this.state().snapshot;
  }

  eventsAfter(sequence: number, limit: number): SequencedEvent[] {
    return this.state().journal.filter((e) => e.sequence > sequence).slice(0, limit);
  }

  async diff(request: DiffRequest): Promise<DiffPage> {
    if (!this.state().snapshot.prs.some((p) => p.id === request.prId)) throw new ControlError("NOT_FOUND", `no PR ${request.prId}`);
    const pages = [fixtures["server.diff.page"], fixtures["server.diff.page.last"]] as Extract<ServerMessage, { type: "diff.page" }>[];
    const { files, nextCursor, truncated } = pages[request.cursor === undefined ? 0 : 1]!;
    const wanted = request.file === undefined ? files : files.filter((f) => f.path === request.file);
    return nextCursor === undefined ? { files: wanted, truncated } : { files: wanted, nextCursor, truncated };
  }

  async finding(findingId: string): Promise<FindingDetail> {
    const { snapshot, journal } = this.state();
    const updates = journal.flatMap((e) => (e.eventType === "finding.updated" ? [e.data as Finding] : []));
    const finding = [...snapshot.findings, ...updates].findLast((f) => f.id === findingId);
    if (finding === undefined) throw new ControlError("NOT_FOUND", `no finding ${findingId}`);
    const scripted = (fixtures["server.finding.detail"] as Extract<ServerMessage, { type: "finding.detail" }>).finding;
    if (scripted.id === findingId) return { ...scripted, ...finding };
    return { ...finding, explanation: "Fixture finding.", reasoningSummary: "Fixture finding.", evidence: [], history: [] };
  }

  timeline(request: TimelineRequest): TimelinePage {
    return timelinePage(this.state().journal, request, (e) => (e.eventType === "finding.updated" ? (e.data as Finding).id : undefined));
  }

  unavailable(command: string): string {
    return `${command} is not available from a fixture server`;
  }

  async publication(runId: string): Promise<PublicationPreview> {
    const { type: _, requestId: __, ...scripted } = fixtures["server.publication.preview"] as Extract<ServerMessage, { type: "publication.preview" }>;
    if (scripted.runId !== runId) throw new ControlError("NOT_FOUND", `no stored publication for run ${runId}`);
    return scripted;
  }

  close(): void {}
}
