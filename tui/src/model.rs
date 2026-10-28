//! Client state. The domain cache is keyed by stable IDs; the UI stores selected IDs, never list indexes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::protocol::*;
use crate::waterfall::{Span, SpanKind};

pub const ACTIVITY_CAP: usize = 2_000;
/// How long an accepted command may show nothing before the panel says so.
pub const ACCEPTED_QUIET_MS: u64 = 15_000;
const ERROR_CAP: usize = 20;

/// The connection state machine. `Link::next` is the whole transition table.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Disconnected {
        reason: Text,
        retry_in_ms: Option<u64>,
    },
    Connecting {
        attempt: u32,
    },
    Negotiating,
    /// Waiting for the first snapshot; events are dropped.
    Syncing,
    Live,
    /// A gap, an epoch change or `resync.required`: waiting for a fresh snapshot; events are dropped.
    Resyncing,
    /// The snapshot a sync or resync asked for never came: events stay dropped until `r` asks again.
    Stalled {
        reason: Text,
    },
    /// The server does not speak protocol v1. Terminal: the client stops reconnecting.
    Incompatible {
        supported: Vec<u32>,
        message: Text,
    },
    /// The endpoint can never work, such as a socket path longer than the kernel accepts. Terminal: no dialing.
    Unusable {
        message: Text,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkInput {
    Dialing { attempt: u32 },
    Opened,
    Welcomed { resuming: bool },
    Mismatch { supported: Vec<u32>, message: Text },
    Unusable { message: Text },
    SnapshotApplied,
    SnapshotTimedOut,
    StreamBroken,
    Lost { reason: Text, retry_in_ms: Option<u64> },
}

impl Link {
    /// `None` means the input does not apply in this state and is ignored.
    pub fn next(&self, input: LinkInput) -> Option<Link> {
        use Link::*;
        use LinkInput as I;
        match (self, input) {
            (Incompatible { .. } | Unusable { .. }, _) => None,
            (_, I::Unusable { message }) => Some(Unusable { message }),
            (_, I::Lost { reason, retry_in_ms }) => Some(Disconnected { reason, retry_in_ms }),
            (Disconnected { .. } | Connecting { .. }, I::Dialing { attempt }) => Some(Connecting { attempt }),
            (Disconnected { .. } | Connecting { .. }, I::Opened) => Some(Negotiating),
            (Negotiating, I::Mismatch { supported, message }) => Some(Incompatible { supported, message }),
            (Negotiating, I::Welcomed { resuming: true }) => Some(Live),
            (Negotiating, I::Welcomed { resuming: false }) => Some(Syncing),
            (Negotiating | Syncing | Live | Resyncing | Stalled { .. }, I::SnapshotApplied) => Some(Live),
            (Syncing | Resyncing, I::SnapshotTimedOut) => {
                Some(Stalled { reason: Text::new("the snapshot did not arrive; r asks again") })
            }
            (Live | Stalled { .. }, I::StreamBroken) => Some(Resyncing),
            _ => None,
        }
    }

    pub fn accepts_events(&self) -> bool {
        matches!(self, Link::Live)
    }

    /// The server has answered `hello` with `welcome`, so requests other than `hello` may go out.
    pub fn welcomed(&self) -> bool {
        matches!(self, Link::Syncing | Link::Live | Link::Resyncing | Link::Stalled { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamPos {
    pub epoch: Option<Epoch>,
    pub last_applied: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerInfo {
    pub protocol_version: u32,
    pub server_version: Text,
    pub session_id: Text,
    pub capabilities: Vec<Text>,
    /// `None` from a daemon that predates engine mode.
    pub mode: Option<DaemonMode>,
    pub read_only_reason: Option<Text>,
    /// `None` from a daemon that predates the checks.
    pub checks: Option<Vec<SetupCheck>>,
}

impl ServerInfo {
    /// Why a review command is not offered, for the `(not offered)` notes; `None` when the daemon gave no reason.
    pub fn commands_off(&self) -> Option<&'static str> {
        match (self.mode, &self.read_only_reason) {
            // With a check missing, --allow-commands alone only gets a read-only daemon whose engine failed to open.
            (Some(DaemonMode::ReadOnly), Some(r)) if r.as_str().contains("--allow-commands") => {
                Some(if self.missing().next().is_some() {
                    "commands are off (run pr-review setup, then --allow-commands)"
                } else {
                    "commands are off (start the daemon with --allow-commands)"
                })
            }
            (Some(DaemonMode::ReadOnly), _) => Some("daemon is read-only"),
            _ => None,
        }
    }

    /// The checks that keep reviews from running.
    pub fn missing(&self) -> impl Iterator<Item = &SetupCheck> {
        self.checks.iter().flatten().filter(|c| c.status == CheckStatus::Missing)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActivityKind {
    Tool { tool: Text, summary: Text },
    Phase { from: Phase, to: Phase },
    Finding { title: Text, status: FindingStatus, severity: Severity },
    Run { change: RunChange, status: RunStatus },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub sequence: u64,
    pub at: Text,
    pub pr_id: Option<PrId>,
    pub kind: ActivityKind,
    /// Client clock when it was applied: the live rate is about what the reviewer does now, not journal time.
    pub recv_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DomainCache {
    pub prs: BTreeMap<PrId, Pr>,
    pub runs: BTreeMap<RunId, Run>,
    pub findings: BTreeMap<FindingId, Finding>,
    /// In the daemon's order. `None` until a snapshot from a daemon that reports accounts.
    pub accounts: Option<Vec<Account>>,
    /// The latest `review.publication_changed` per run. Snapshots carry none, so they keep these.
    pub publications: BTreeMap<RunId, PublicationChanged>,
    pub activity: VecDeque<Activity>,
    pub tool_activity_count: u64,
    /// Each run's waterfall spans, in arrival order. Snapshots carry none, so they keep these; a run's next attempt
    /// starts its own.
    pub spans: BTreeMap<RunId, Vec<Span>>,
    /// The newest event's journal time and the client clock when it arrived: open spans grow from it.
    pub clock: Option<(i64, u64)>,
}

/// How a running review is doing, so it never reads as stuck: how long its phase has run, and how long its PR has
/// been quiet, in milliseconds. Either is `None` before this client has seen it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pulse {
    pub phase_ms: Option<i64>,
    pub quiet_ms: Option<u64>,
}

impl DomainCache {
    pub fn push_activity(&mut self, a: Activity) {
        if self.activity.len() == ACTIVITY_CAP {
            self.activity.pop_front();
        }
        self.activity.push_back(a);
    }

    /// The journal's time now: the newest event's, plus what the client clock says has passed since it arrived.
    pub fn journal_now(&self, now_ms: u64) -> Option<i64> {
        self.clock.map(|(at, recv)| at + now_ms.saturating_sub(recv) as i64)
    }

    /// The run's open phase span when the daemon sends spans; otherwise the phase change this client saw arrive.
    pub fn pulse(&self, run: &Run, now_ms: u64) -> Pulse {
        let mine = || self.activity.iter().rev().filter(|a| a.pr_id.as_ref() == Some(&run.pr_id));
        let open = self
            .spans
            .get(&run.id)
            .and_then(|v| v.iter().rev().find(|s| s.kind == SpanKind::Phase && s.ended_ms.is_none()));
        let phase_ms = match (open, self.journal_now(now_ms)) {
            (Some(s), Some(now)) => Some((now - s.started_ms).max(0)),
            _ => mine()
                .find(|a| {
                    matches!(a.kind, ActivityKind::Phase { .. } | ActivityKind::Run { change: RunChange::Started, .. })
                })
                .map(|a| now_ms.saturating_sub(a.recv_ms) as i64),
        };
        Pulse { phase_ms, quiet_ms: mine().next().map(|a| now_ms.saturating_sub(a.recv_ms)) }
    }

    pub fn latest_run(&self, pr: &PrId) -> Option<&Run> {
        self.runs.values().filter(|r| &r.pr_id == pr).max_by(|a, b| (&a.started_at, &a.id).cmp(&(&b.started_at, &b.id)))
    }

    /// Most severe first; stale findings after current ones.
    pub fn pr_findings(&self, pr: &PrId) -> Vec<&Finding> {
        let mut v: Vec<_> = self.findings.values().filter(|f| &f.pr_id == pr).collect();
        v.sort_by(|a, b| {
            (a.stale, a.severity, &a.file, a.line, &a.id).cmp(&(b.stale, b.severity, &b.file, b.line, &b.id))
        });
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Inbox,
    Review,
    Diff,
    Timeline,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 5] = [Screen::Inbox, Screen::Review, Screen::Diff, Screen::Timeline, Screen::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Screen::Inbox => "Inbox",
            Screen::Review => "Review",
            Screen::Diff => "Diff",
            Screen::Timeline => "Timeline",
            Screen::Settings => "Settings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Main,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPurpose {
    /// The PR list's fuzzy filter.
    Search,
    JumpTo,
    /// Timeline events whose text contains the buffer.
    TimelineFilter,
    /// A PR to add: its GitHub URL or `owner/repo#N`; Enter sends `review.add`.
    AddPr,
    /// Diff lines whose text contains the buffer; `n`/`N` walk them.
    DiffSearch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Help {
        scroll: u16,
    },
    Finding {
        id: FindingId,
        scroll: u16,
    },
    Input {
        purpose: InputPurpose,
        buffer: String,
        before: String,
    },
    /// The publication preview of a parked run, full page. `y` here, and only here and in `Confirm`, sends
    /// `review.publish`, naming the preview it shows.
    Publication {
        run_id: RunId,
        scroll: u16,
        preview: Preview,
        /// `Asking` until `y` sends the approval.
        publish: ConfirmState,
        /// `m`: the markdown exactly as it will be posted, instead of styled.
        raw: bool,
    },
    /// The only place `y` acts. Generic over the command it confirms.
    Confirm {
        prompt: String,
        command: Command,
        state: ConfirmState,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Loading(RequestId),
    Failed(Text),
    Ready(Box<PublicationPreview>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmState {
    Asking,
    Requested(RequestId),
    /// Accepted is not completed: a later event confirms the effect. `at_ms` starts `ACCEPTED_QUIET_MS`.
    Accepted {
        operation: Text,
        at_ms: u64,
    },
    /// Refused: `message` is the sentence, and `code` the daemon's error code beside the command name, or `None` when
    /// the client refused it before sending, knowing the daemon would.
    Refused {
        code: Option<ErrorCode>,
        message: Text,
    },
    /// No reply in time; `y` sends the same command again, under the same idempotency key.
    TimedOut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffKey {
    pub pr: PrId,
    pub base: Sha,
    pub head: Sha,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffView {
    pub key: DiffKey,
    pub files: Vec<DiffFile>,
    pub next_cursor: Option<Text>,
    pub truncated: bool,
    pub pending: Option<RequestId>,
    pub error: Option<Text>,
    pub selected_file: Option<Text>,
    /// Row index within the selected file's rows (hunk headers and lines).
    pub cursor: usize,
    /// Which of the findings stacked on the cursor's line `Enter` opens; `n` walks the stack before the next line.
    pub stack: usize,
    pub anchor: Option<(Text, Option<u32>)>,
    /// The text `/` searched for; while set, `n`/`N` walk its matches instead of the findings.
    pub search: Option<String>,
    /// `diff_matches` for `search` over these files, kept current by the reducer so a frame never rescans the diff.
    pub matches: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiffRow<'a> {
    Header(&'a Hunk),
    Line(&'a DiffLine),
}

impl DiffView {
    pub fn file(&self) -> Option<&DiffFile> {
        let sel = self.selected_file.as_ref()?;
        self.files.iter().find(|f| &f.path == sel)
    }
}

pub fn file_rows(file: &DiffFile) -> Vec<DiffRow<'_>> {
    file.hunks
        .iter()
        .flat_map(|h| std::iter::once(DiffRow::Header(h)).chain(h.lines.iter().map(DiffRow::Line)))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineScope {
    Pr,
    Run,
    Finding,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineQuery {
    pub pr: Option<PrId>,
    pub run: Option<RunId>,
    pub finding: Option<FindingId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCategory {
    Pr,
    Run,
    Phase,
    Finding,
    Tool,
    Account,
    Publication,
    Other,
}

impl EventCategory {
    /// The kinds `v` steps through. Phase changes are headings, not rows; `P` filters by phase.
    pub const CYCLE: [Option<EventCategory>; 8] = [
        None,
        Some(EventCategory::Tool),
        Some(EventCategory::Finding),
        Some(EventCategory::Run),
        Some(EventCategory::Pr),
        Some(EventCategory::Account),
        Some(EventCategory::Publication),
        Some(EventCategory::Other),
    ];
    pub fn of(body: &EventBody) -> EventCategory {
        match body {
            EventBody::PrUpdated(_) => EventCategory::Pr,
            EventBody::Run(..) => EventCategory::Run,
            EventBody::PhaseChanged(_) => EventCategory::Phase,
            EventBody::FindingUpdated(_) => EventCategory::Finding,
            EventBody::ToolActivity(_) => EventCategory::Tool,
            EventBody::AccountsUpdated(_) => EventCategory::Account,
            EventBody::PublicationChanged(_) => EventCategory::Publication,
            EventBody::Other(_) => EventCategory::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            EventCategory::Pr => "pr",
            EventCategory::Run => "run",
            EventCategory::Phase => "phase",
            EventCategory::Finding => "finding",
            EventCategory::Tool => "tool",
            EventCategory::Account => "account",
            EventCategory::Publication => "publication",
            EventCategory::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineView {
    pub scope: TimelineScope,
    pub query: TimelineQuery,
    /// Newest first, as the server pages them.
    pub events: Vec<SequencedEvent>,
    pub next_before: Option<u64>,
    pub pending: Option<RequestId>,
    pub error: Option<Text>,
    pub selected: Option<u64>,
    /// The event whose detail shows inline under its row.
    pub open: Option<u64>,
    pub category: Option<EventCategory>,
    pub phase: Option<Phase>,
    /// Shows only events whose text contains this, ignoring case.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub until_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    pub endpoint: String,
    pub resyncs: u32,
    pub reconnects: u32,
    pub last_heartbeat_ms: Option<u64>,
    pub last_snapshot_ms: Option<u64>,
    /// When each error last came, its text, and how many times in a row it came.
    pub errors: VecDeque<(u64, String, u32)>,
}

impl Diagnostics {
    /// A repeat of the last error, as a disconnect is on every retry, counts on its row instead of adding one.
    pub fn error(&mut self, at_ms: u64, text: impl Into<String>) {
        let text = crate::sanitize::sanitize(&text.into());
        if let Some(last) = self.errors.back_mut().filter(|e| e.1 == text) {
            last.0 = at_ms;
            last.2 += 1;
            return;
        }
        if self.errors.len() == ERROR_CAP {
            self.errors.pop_front();
        }
        self.errors.push_back((at_ms, text, 1));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub ascii: bool,
    pub color: bool,
    pub show_activity: bool,
    /// `[ui] background = "terminal"`: the terminal's own background shows where the theme would paint `base`.
    pub terminal_bg: bool,
    pub palette: crate::theme::Palette,
}

impl Default for Look {
    fn default() -> Self {
        Look { ascii: false, color: true, show_activity: true, terminal_bg: false, palette: Default::default() }
    }
}

/// A PR named on the command line. Daemon PR ids are hashes, so it matches on repository (any case) and number.
#[derive(Clone, Debug, PartialEq)]
pub struct PrRef {
    pub repository: String,
    pub number: u64,
}

impl PrRef {
    /// `https://github.com/owner/repo/pull/184` (a trailing `/files`, query or fragment is fine) or `owner/repo#184`,
    /// with owner and repo spelled as GitHub allows: letters, digits, `-`, `_` and `.`.
    pub fn parse(s: &str) -> Option<PrRef> {
        let s = s.trim();
        let (owner, repo, number) = match s.split_once('#') {
            Some((repo, n)) if !repo.contains("://") => {
                let (owner, repo) = repo.split_once('/')?;
                (owner, repo, n)
            }
            _ => {
                let path = s.strip_prefix("https://github.com/").or_else(|| s.strip_prefix("http://github.com/"))?;
                let path = path.split(['?', '#']).next()?;
                let mut parts = path.split('/');
                match (parts.next(), parts.next(), parts.next(), parts.next()) {
                    (Some(owner), Some(repo), Some("pull"), Some(n)) => (owner, repo, n),
                    _ => return None,
                }
            }
        };
        let name = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        let number = number.parse::<u64>().ok().filter(|n| *n > 0 && number.chars().all(|c| c.is_ascii_digit()))?;
        (name(owner) && name(repo)).then(|| PrRef { repository: format!("{owner}/{repo}"), number })
    }

    pub fn matches(&self, p: &Pr) -> bool {
        p.number == self.number && p.repository.as_str().eq_ignore_ascii_case(&self.repository)
    }
}

impl std::fmt::Display for PrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.repository, self.number)
    }
}

#[derive(Clone, Debug)]
pub struct Ui {
    pub screen: Screen,
    pub focus: Pane,
    pub modals: Vec<Modal>,
    pub selected_pr: Option<PrId>,
    pub selected_finding: Option<FindingId>,
    /// The parked run whose "Ready to publish" line holds the cursor, above the findings; see `Model::on_ready_line`.
    pub ready_line: Option<RunId>,
    /// Lines the reader scrolled the document past where the selection puts it; reset when the selection moves.
    pub doc_scroll: i32,
    pub pr_filter: String,
    /// From `--pr`: selected once it shows up in a snapshot.
    pub wanted_pr: Option<PrRef>,
    /// The Accounts cursor in Settings; an ID that vanished falls back to the first account.
    pub selected_account: Option<AccountId>,
    /// The PR list shows the archived PRs instead of the tracked ones (`Model::pr_list`).
    pub show_archived: bool,
}

/// Whose token the daemon uses for GitHub, in its precedence order (protocol/README.md "Accounts").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Identity<'a> {
    Env,
    Selected(&'a Account),
    GhActive(&'a Account),
    Nobody,
}

#[derive(Clone, Debug)]
pub struct Model {
    pub link: Link,
    pub stream: StreamPos,
    pub server: Option<ServerInfo>,
    pub domain: DomainCache,
    pub details: BTreeMap<FindingId, FindingDetail>,
    pub detail_requests: BTreeMap<RequestId, FindingId>,
    pub detail_errors: BTreeMap<FindingId, Text>,
    pub diff: Option<DiffView>,
    pub timeline: Option<TimelineView>,
    pub ui: Ui,
    pub look: Look,
    pub diag: Diagnostics,
    pub notice: Option<Notice>,
    pub now_ms: u64,
    pub size: (u16, u16),
    pub next_request: u64,
    /// When each request went out, until it times out; a reply does not remove it.
    pub sent_at: BTreeMap<RequestId, u64>,
    pub request_timeout_ms: u64,
    /// The unanswered snapshot.get, and the ones it superseded, whose replies are ignored.
    pub snapshot_request: Option<RequestId>,
    pub stale_snapshots: BTreeSet<RequestId>,
    /// Every command this client sent: a `command.result` for any other request is not this client's to show.
    /// Each command request in flight, with the verb its refusal reads with.
    pub commands_sent: BTreeMap<RequestId, &'static str>,
    /// The `review.add` in flight, until its PR is selected; a refusal stays until the next one.
    pub adding: Option<Adding>,
    /// The daemon's review model, for the desktop's Settings.
    pub agent: AgentSettings,
    /// Recorded model usage, for the desktop's Usage view.
    pub usage: UsageView,
    pub dirty: bool,
}

/// What the daemon said about its review model. `settings` is the last answer; `error` is why there is none (a
/// read-only daemon refuses settings.get), cleared by the next answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSettings {
    /// The client shows them (the desktop does; the TUI never asks).
    pub wanted: bool,
    pub request: Option<RequestId>,
    pub settings: Option<Settings>,
    pub error: Option<Text>,
    pub save: SaveState,
}

/// What the daemon said about recorded model usage. `by` is the grouping a client shows, `None` while none does (the
/// TUI never asks). `report` is the last answer, kept on screen until the next one replaces it; `stale` asks for a new
/// one, set on opening and whenever a review ends. `error` is why there is none, cleared by opening again. `group` is
/// the breakdown over the same range, kept while the page is closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageView {
    pub by: Option<UsageBy>,
    pub group: UsageGroupBy,
    pub request: Option<RequestId>,
    pub report: Option<UsageReport>,
    pub stale: bool,
    pub error: Option<Text>,
}

/// A `settings.set` from Save, until its result.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SaveState {
    #[default]
    Idle,
    Sending {
        request: RequestId,
        model: String,
    },
    Saved(String),
    Refused(Text),
    TimedOut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Adding {
    pub target: PrRef,
    pub state: AddState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AddState {
    Sending(RequestId),
    /// The daemon tracks `pr` (it just added it, or `already` had it); selected once the PR is in the cache.
    Landing {
        pr: PrId,
        already: bool,
    },
    /// `code` is `None` for an error reply that carried none of `command.result`'s shape.
    Refused {
        code: Option<ErrorCode>,
        message: Text,
    },
    TimedOut,
}

impl Model {
    pub fn new(endpoint: impl Into<String>, look: Look, start: Screen, wanted_pr: Option<PrRef>) -> Self {
        Model {
            link: Link::Connecting { attempt: 1 },
            stream: StreamPos::default(),
            server: None,
            domain: DomainCache::default(),
            details: BTreeMap::new(),
            detail_requests: BTreeMap::new(),
            detail_errors: BTreeMap::new(),
            diff: None,
            timeline: None,
            ui: Ui {
                screen: start,
                focus: Pane::Main,
                modals: Vec::new(),
                selected_pr: None,
                selected_finding: None,
                ready_line: None,
                doc_scroll: 0,
                pr_filter: String::new(),
                wanted_pr,
                selected_account: None,
                show_archived: false,
            },
            look,
            diag: Diagnostics { endpoint: endpoint.into(), ..Diagnostics::default() },
            notice: None,
            now_ms: 0,
            size: (80, 24),
            next_request: 1,
            sent_at: BTreeMap::new(),
            request_timeout_ms: 15_000,
            snapshot_request: None,
            stale_snapshots: BTreeSet::new(),
            commands_sent: BTreeMap::new(),
            adding: None,
            agent: AgentSettings::default(),
            usage: UsageView::default(),
            dirty: true,
        }
    }

    /// Tracked PRs that are not archived and match the search filter, in an order that never moves under the cursor:
    /// grouped by repository (alphabetically), newest number first. Activity shows in the row's glyph, not in its
    /// position. Every list of PRs a reader works through (Needs you, Done, the PR rail) starts here.
    pub fn inbox(&self) -> Vec<&Pr> {
        self.prs_where(false)
    }

    /// The archived PRs matching the search filter, in the inbox's order.
    pub fn archived(&self) -> Vec<&Pr> {
        self.prs_where(true)
    }

    /// The list the PR rail shows: the inbox, or the archived PRs while `show_archived` is on.
    pub fn pr_list(&self) -> Vec<&Pr> {
        self.prs_where(self.ui.show_archived)
    }

    /// How many PRs the rail's list holds before the search filter.
    pub fn pr_list_total(&self) -> usize {
        self.domain.prs.values().filter(|p| p.archived() == self.ui.show_archived).count()
    }

    fn prs_where(&self, archived: bool) -> Vec<&Pr> {
        let mut v: Vec<_> = self
            .domain
            .prs
            .values()
            .filter(|p| p.archived() == archived && fuzzy(&self.ui.pr_filter, &pr_haystack(p)))
            .collect();
        v.sort_by(|a, b| {
            (&a.repository, std::cmp::Reverse(a.number), &a.id).cmp(&(
                &b.repository,
                std::cmp::Reverse(b.number),
                &b.id,
            ))
        });
        v
    }

    pub fn selected_pr(&self) -> Option<&Pr> {
        self.ui.selected_pr.as_ref().and_then(|id| self.domain.prs.get(id))
    }

    /// The selected PR's latest run while it waits for approval.
    pub fn parked_run(&self) -> Option<&Run> {
        self.domain.latest_run(self.ui.selected_pr.as_ref()?).filter(|r| r.parked())
    }

    /// The cursor is on the ready line only while that run is still the selected PR's parked one.
    pub fn on_ready_line(&self) -> bool {
        self.ui.selected_finding.is_none()
            && self.ui.ready_line.is_some()
            && self.parked_run().map(|r| &r.id) == self.ui.ready_line.as_ref()
    }

    pub fn selected_finding(&self) -> Option<&Finding> {
        self.ui.selected_finding.as_ref().and_then(|id| self.domain.findings.get(id))
    }

    /// Writes need a live stream and a capability the server granted.
    pub fn may_write(&self, capability: &str) -> bool {
        self.link.accepts_events()
            && self.server.as_ref().is_some_and(|s| s.capabilities.iter().any(|c| c.as_str() == capability))
    }

    /// `None` while the daemon has not reported accounts.
    pub fn identity(&self) -> Option<Identity<'_>> {
        let accounts = self.domain.accounts.as_ref()?;
        let github = |a: &&Account| a.host.as_str() == "github.com";
        Some(if accounts.iter().any(|a| a.id.is_env()) {
            Identity::Env
        } else if let Some(a) = accounts.iter().find(|a| a.selected) {
            Identity::Selected(a)
        } else if let Some(a) = accounts.iter().filter(github).find(|a| a.gh_active) {
            Identity::GhActive(a)
        } else {
            Identity::Nobody
        })
    }

    pub fn cursor_account(&self) -> Option<&Account> {
        let accounts = self.domain.accounts.as_ref()?;
        let wanted = self.ui.selected_account.as_ref();
        accounts.iter().find(|a| Some(&a.id) == wanted).or(accounts.first())
    }

    /// Why `account.select` would not be sent for `a` right now, short enough for an 80-column row; `None` means it
    /// would be.
    pub fn account_block(&self, a: &Account) -> Option<&'static str> {
        let env = || self.domain.accounts.iter().flatten().any(|x| x.id.is_env());
        Some(if a.id.is_env() {
            "set by the daemon env"
        } else if a.host.as_str() != "github.com" {
            "github.com only"
        } else if a.state != AccountState::Ok {
            "run gh auth login"
        } else if env() {
            "env token wins"
        } else if a.selected {
            "already selected"
        } else if !self.link.accepts_events() {
            "not live"
        } else if !self.may_write("account.select") {
            "daemon has no select"
        } else {
            return None;
        })
    }

    pub fn top_modal(&self) -> Option<&Modal> {
        self.ui.modals.last()
    }
}

fn pr_haystack(p: &Pr) -> String {
    format!("#{} {} {} {}", p.number, p.title, p.repository, p.author.as_ref().map_or("", |a| a.as_str()))
}

/// Case-insensitive subsequence match: `sgn back` finds "Sign-in: send users back".
pub fn fuzzy(needle: &str, haystack: &str) -> bool {
    let hay = haystack.to_lowercase();
    let mut chars = hay.chars();
    needle.to_lowercase().chars().filter(|c| !c.is_whitespace()).all(|n| chars.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_matches_subsequences_and_rejects_out_of_order() {
        assert!(super::fuzzy("", "anything"));
        assert!(super::fuzzy("ordref", "#1287 Orders: self-serve refunds"));
        assert!(super::fuzzy("1287", "#1287 Orders"));
        assert!(!super::fuzzy("refo", "#1287 Orders: refunds"));
    }
}
