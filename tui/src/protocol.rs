//! Protocol v1 wire types (protocol/README.md). Every server string is sanitized while it is parsed, so the rest of the
//! client only ever holds terminal-safe text. Unknown enum strings map to `Unknown`; unknown fields are ignored.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Deserializer, Serialize};

use crate::sanitize::sanitize;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 1 << 20;
const MAX_ACTIVITY_SUMMARY_CHARS: usize = 200;

/// Text that came from outside the client, already stripped of terminal control sequences.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Text(String);

impl Text {
    pub fn new(raw: &str) -> Self {
        Text(sanitize(raw))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Text::new(&s))
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! id {
    ($($name:ident),*) => {$(
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Text);
        impl $name {
            pub fn new(raw: &str) -> Self {
                Self(Text::new(raw))
            }
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    )*};
}

id!(PrId, RunId, FindingId, Epoch, Sha, RequestId);

impl Sha {
    pub fn short(&self) -> &str {
        let s = self.as_str();
        s.char_indices().nth(7).map_or(s, |(i, _)| &s[..i])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lifecycle {
    Active,
    Quiescing,
    Closed,
    Merged,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    CollectContext,
    Triage,
    Analyze,
    Verify,
    PreparePublication,
    Publish,
    Reconcile,
    Completed,
    Failed,
    Superseded,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Superseded,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Security,
    Concurrency,
    Correctness,
    DataIntegrity,
    Performance,
    General,
    Other,
    #[serde(other)]
    Unknown,
}

/// Declared most severe first, so `Ord` sorts a finding list by severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingStatus {
    Detected,
    Published,
    StillPresent,
    PotentiallyResolved,
    Outdated,
    Resolved,
    Uncertain,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    CodeRef,
    ToolSummary,
    TestResult,
    Scenario,
    Link,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Binary,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Add,
    Del,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Unauthorized,
    CapabilityDenied,
    InvalidTransition,
    RevisionStale,
    NotFound,
    Conflict,
    RateLimited,
    TemporarilyUnavailable,
    InternalError,
    ProtocolMismatch,
    InvalidRequest,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub id: PrId,
    pub repository: Text,
    pub number: u64,
    pub title: Text,
    pub author: Option<Text>,
    pub head_sha: Sha,
    pub base_sha: Sha,
    pub lifecycle: Lifecycle,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: RunId,
    pub pr_id: PrId,
    pub base_sha: Sha,
    pub head_sha: Sha,
    pub phase: Phase,
    pub status: RunStatus,
    pub model: Text,
    pub started_at: Text,
    pub completed_at: Option<Text>,
    pub failure_reason: Option<Text>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: FindingId,
    pub pr_id: PrId,
    pub run_id: RunId,
    pub fingerprint: Text,
    pub title: Text,
    pub category: Category,
    pub severity: Severity,
    pub status: FindingStatus,
    pub confidence: f64,
    pub file: Text,
    /// `None` is a file-level finding. Lines are 1-based, so a 0 fails to parse.
    pub line: Option<NonZeroU32>,
    pub evidence_count: u32,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDetail {
    #[serde(flatten)]
    pub finding: Finding,
    pub explanation: Text,
    pub reasoning_summary: Text,
    pub suggested_fix: Option<Text>,
    pub evidence: Vec<Evidence>,
    pub history: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub path: Option<Text>,
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub ref_sha: Option<Sha>,
    pub text: Text,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub run_id: RunId,
    pub disposition: FindingStatus,
    pub at: Text,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: Text,
    pub old_path: Option<Text>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: Text,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: Text,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub request_id: Option<RequestId>,
    pub stream_epoch: Epoch,
    pub sequence: u64,
    pub prs: Vec<Pr>,
    pub runs: Vec<Run>,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPage {
    pub request_id: Option<RequestId>,
    pub pr_id: PrId,
    pub base_sha: Sha,
    pub head_sha: Sha,
    pub files: Vec<DiffFile>,
    pub next_cursor: Option<Text>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub request_id: RequestId,
    #[serde(flatten)]
    pub outcome: CommandOutcome,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum CommandOutcome {
    #[serde(rename_all = "camelCase")]
    Accepted {
        operation_id: Text,
    },
    Rejected {
        code: ErrorCode,
        message: Text,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "welcome", rename_all = "camelCase")]
    Welcome {
        request_id: Option<RequestId>,
        protocol_version: u32,
        server_version: Text,
        session_id: Text,
        capabilities: Vec<Text>,
        stream_epoch: Epoch,
        latest_sequence: u64,
    },
    #[serde(rename = "snapshot")]
    Snapshot(Snapshot),
    #[serde(rename = "event")]
    Event(Event),
    #[serde(rename = "resync.required", rename_all = "camelCase")]
    ResyncRequired { request_id: Option<RequestId>, reason: Text },
    #[serde(rename = "diff.page")]
    DiffPage(DiffPage),
    #[serde(rename = "finding.detail", rename_all = "camelCase")]
    FindingDetail { request_id: Option<RequestId>, finding: FindingDetail },
    #[serde(rename = "timeline.page", rename_all = "camelCase")]
    TimelinePage { request_id: Option<RequestId>, events: Vec<SequencedEvent>, next_before_sequence: Option<u64> },
    #[serde(rename = "command.result")]
    CommandResult(CommandResult),
    #[serde(rename = "error", rename_all = "camelCase")]
    Error { request_id: Option<RequestId>, code: ErrorCode, message: Text, supported: Option<Vec<u32>> },
}

impl ServerMessage {
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawEvent")]
pub enum Event {
    /// Never durable and never sequenced: it only proves the connection is alive.
    Heartbeat {
        stream_epoch: Epoch,
        occurred_at: Text,
    },
    Sequenced(SequencedEvent),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawEvent")]
pub struct SequencedEvent {
    pub stream_epoch: Epoch,
    pub sequence: NonZeroU64,
    pub prev_sequence: u64,
    pub occurred_at: Text,
    pub pr_id: Option<PrId>,
    pub run_id: Option<RunId>,
    pub body: EventBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunChange {
    Started,
    Completed,
    Superseded,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventBody {
    PrUpdated(Pr),
    Run(RunChange, Run),
    PhaseChanged(PhaseChange),
    FindingUpdated(Finding),
    ToolActivity(ToolActivity),
    /// An event type this client does not know. It carries nothing, but still advances `lastApplied`.
    Other(Text),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhaseChange {
    pub run_id: RunId,
    pub from: Phase,
    pub to: Phase,
    pub event: Text,
    pub detail: Option<Text>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ToolActivity {
    pub tool: Text,
    pub summary: Text,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    stream_epoch: Epoch,
    sequence: Option<NonZeroU64>,
    prev_sequence: Option<u64>,
    occurred_at: Text,
    pr_id: Option<PrId>,
    run_id: Option<RunId>,
    event_type: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct RawPhaseChange {
    from: Phase,
    to: Phase,
    event: Text,
    detail: Option<Text>,
}

impl TryFrom<RawEvent> for Event {
    type Error = String;
    fn try_from(raw: RawEvent) -> Result<Self, String> {
        if raw.event_type == "heartbeat" {
            return Ok(Event::Heartbeat { stream_epoch: raw.stream_epoch, occurred_at: raw.occurred_at });
        }
        SequencedEvent::try_from(raw).map(Event::Sequenced)
    }
}

impl TryFrom<RawEvent> for SequencedEvent {
    type Error = String;
    fn try_from(raw: RawEvent) -> Result<Self, String> {
        let (Some(sequence), Some(prev_sequence)) = (raw.sequence, raw.prev_sequence) else {
            return Err(format!("{} event without sequence/prevSequence", raw.event_type));
        };
        let data = |name: &str| -> Result<serde_json::Value, String> {
            if raw.data.is_null() { Err(format!("{name} event without data")) } else { Ok(raw.data.clone()) }
        };
        let run = |change| -> Result<EventBody, String> {
            let run: Run = serde_json::from_value(data(&raw.event_type)?).map_err(|e| e.to_string())?;
            Ok(EventBody::Run(change, run))
        };
        let body = match raw.event_type.as_str() {
            "pr.updated" => EventBody::PrUpdated(from_value(data("pr.updated")?)?),
            "run.started" => run(RunChange::Started)?,
            "review.completed" => run(RunChange::Completed)?,
            "run.superseded" => run(RunChange::Superseded)?,
            "run.failed" => run(RunChange::Failed)?,
            "review.phase_changed" => {
                let run_id = raw.run_id.clone().ok_or("review.phase_changed without runId")?;
                let p: RawPhaseChange = from_value(data("review.phase_changed")?)?;
                EventBody::PhaseChanged(PhaseChange {
                    run_id,
                    from: p.from,
                    to: p.to,
                    event: p.event,
                    detail: p.detail,
                })
            }
            "finding.updated" => EventBody::FindingUpdated(from_value(data("finding.updated")?)?),
            "tool.activity" => {
                let a: ToolActivity = from_value(data("tool.activity")?)?;
                let summary: String = a.summary.as_str().chars().take(MAX_ACTIVITY_SUMMARY_CHARS).collect();
                EventBody::ToolActivity(ToolActivity { tool: a.tool, summary: Text(summary) })
            }
            other => EventBody::Other(Text::new(other)),
        };
        Ok(SequencedEvent {
            stream_epoch: raw.stream_epoch,
            sequence,
            prev_sequence,
            occurred_at: raw.occurred_at,
            pr_id: raw.pr_id,
            run_id: raw.run_id,
            body,
        })
    }
}

fn from_value<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// 1..=500, the only range the server accepts for `diff.get` and `timeline.get`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Limit(u16);

impl Limit {
    pub const DEFAULT: Limit = Limit(200);
}

impl TryFrom<u16> for Limit {
    type Error = String;
    fn try_from(n: u16) -> Result<Self, String> {
        if (1..=500).contains(&n) { Ok(Limit(n)) } else { Err(format!("limit {n} is outside 1..500")) }
    }
}

impl From<Limit> for u16 {
    fn from(l: Limit) -> u16 {
        l.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "hello", rename_all = "camelCase")]
    Hello { request_id: RequestId, protocol_version: u32, client: String, capabilities: Vec<String> },
    #[serde(rename = "snapshot.get", rename_all = "camelCase")]
    SnapshotGet { request_id: RequestId },
    #[serde(rename = "resume", rename_all = "camelCase")]
    Resume { request_id: RequestId, stream_epoch: Epoch, after_sequence: u64 },
    #[serde(rename = "diff.get", rename_all = "camelCase")]
    DiffGet {
        request_id: RequestId,
        pr_id: PrId,
        base_sha: Sha,
        head_sha: Sha,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<Text>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<Text>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<Limit>,
    },
    #[serde(rename = "finding.get", rename_all = "camelCase")]
    FindingGet { request_id: RequestId, finding_id: FindingId },
    #[serde(rename = "timeline.get", rename_all = "camelCase")]
    TimelineGet {
        request_id: RequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pr_id: Option<PrId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finding_id: Option<FindingId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_sequence: Option<u64>,
        limit: Limit,
    },
    /// Reserved this round: the client never sends one, because the server grants no write capability.
    #[serde(rename = "command")]
    Command(CommandRequest),
}

impl ClientMessage {
    pub fn request_id(&self) -> &RequestId {
        match self {
            ClientMessage::Hello { request_id, .. }
            | ClientMessage::SnapshotGet { request_id }
            | ClientMessage::Resume { request_id, .. }
            | ClientMessage::DiffGet { request_id, .. }
            | ClientMessage::FindingGet { request_id, .. }
            | ClientMessage::TimelineGet { request_id, .. } => request_id,
            ClientMessage::Command(c) => &c.request_id,
        }
    }
}

/// Commands are the protocol's only closed objects: an unknown field or param makes them invalid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawCommand", into = "RawCommand")]
pub struct CommandRequest {
    pub request_id: RequestId,
    pub idempotency_key: String,
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Start(StartParams),
    Cancel(RunParams),
    Retry(RunParams),
    Publish(PublishParams),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartParams {
    pub pr_id: PrId,
    pub expected_head_sha: Sha,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunParams {
    pub run_id: RunId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishParams {
    pub run_id: RunId,
    pub preview_id: Text,
    pub expected_head_sha: Sha,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawCommand {
    request_id: RequestId,
    command: String,
    params: serde_json::Value,
    idempotency_key: String,
}

impl TryFrom<RawCommand> for CommandRequest {
    type Error = String;
    fn try_from(raw: RawCommand) -> Result<Self, String> {
        let command = match raw.command.as_str() {
            "review.start" => Command::Start(from_value(raw.params)?),
            "review.cancel" => Command::Cancel(from_value(raw.params)?),
            "review.retry" => Command::Retry(from_value(raw.params)?),
            "review.publish" => Command::Publish(from_value(raw.params)?),
            other => return Err(format!("unknown command {other}")),
        };
        Ok(CommandRequest { request_id: raw.request_id, idempotency_key: raw.idempotency_key, command })
    }
}

impl From<CommandRequest> for RawCommand {
    fn from(c: CommandRequest) -> Self {
        let to = |v: Result<serde_json::Value, serde_json::Error>| v.unwrap_or_default();
        let (command, params) = match c.command {
            Command::Start(p) => ("review.start", to(serde_json::to_value(p))),
            Command::Cancel(p) => ("review.cancel", to(serde_json::to_value(p))),
            Command::Retry(p) => ("review.retry", to(serde_json::to_value(p))),
            Command::Publish(p) => ("review.publish", to(serde_json::to_value(p))),
        };
        RawCommand { request_id: c.request_id, command: command.into(), params, idempotency_key: c.idempotency_key }
    }
}
