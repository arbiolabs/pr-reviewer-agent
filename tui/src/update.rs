//! The reducer: `update(&mut Model, Msg) -> Vec<Effect>`. All protocol recovery rules live here; the network task is a
//! dumb pipe and the view is a pure function of the model.

use std::collections::VecDeque;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::keymap::{self, Action, Mutation};
use crate::model::*;
use crate::protocol::*;

pub const NOTICE_MS: u64 = 4_000;
pub const TIMED_OUT: &str = "request timed out; r to retry";
pub const OFFLINE: &str = "not connected; r retries";

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    /// A click or a wheel turn; the input thread drops every other mouse event.
    Mouse(MouseEvent),
    Resize(u16, u16),
    /// Milliseconds since the client started.
    Tick(u64),
    Net(NetEvent),
}

#[derive(Debug)]
pub enum NetEvent {
    Dialing {
        attempt: u32,
    },
    Opened,
    Message(ServerMessage),
    /// Consecutive `tool.activity` events the network task merged because the app queue was full.
    Activities(ActivityBatch),
    /// The app queue overflowed on a domain event, which was dropped: state is no longer trustworthy.
    Overflow,
    LineTooLong,
    Garbled(String),
    /// The network task will never dial this endpoint.
    Unusable(String),
    Lost {
        reason: String,
        retry_in_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityBatch {
    pub stream_epoch: Epoch,
    pub first_sequence: u64,
    pub first_prev: u64,
    pub last_sequence: u64,
    pub count: u64,
    /// The newest `ACTIVITY_CAP` of the merged events.
    pub events: VecDeque<SequencedEvent>,
}

impl ActivityBatch {
    pub fn start(e: SequencedEvent) -> Self {
        ActivityBatch {
            stream_epoch: e.stream_epoch.clone(),
            first_sequence: e.sequence.get(),
            first_prev: e.prev_sequence,
            last_sequence: e.sequence.get(),
            count: 1,
            events: VecDeque::from([e]),
        }
    }

    /// Merges `e` when it continues this batch; hands it back otherwise.
    pub fn absorb(&mut self, e: SequencedEvent) -> Result<(), SequencedEvent> {
        let continues = matches!(e.body, EventBody::ToolActivity(_))
            && e.stream_epoch == self.stream_epoch
            && e.prev_sequence == self.last_sequence;
        if !continues {
            return Err(e);
        }
        self.last_sequence = e.sequence.get();
        self.count += 1;
        if self.events.len() == ACTIVITY_CAP {
            self.events.pop_front();
        }
        self.events.push_back(e);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(ClientMessage),
    /// Stop reconnecting: the server is incompatible.
    Close,
    /// Open a published review in the browser, on an explicit keypress. Always an `https://github.com/` URL.
    OpenUrl(String),
    Quit,
}

pub fn update(m: &mut Model, msg: Msg) -> Vec<Effect> {
    let mut fx = Vec::new();
    match msg {
        Msg::Tick(now) => {
            m.now_ms = now;
            if m.notice.as_ref().is_some_and(|n| n.until_ms <= now) {
                m.notice = None;
                m.dirty = true;
            }
            expire_requests(m);
            // Ages on the banner and the diagnostics screen tick.
            let running = m.domain.runs.values().any(|r| r.status == RunStatus::Running);
            let accepted = m.ui.modals.iter().any(|x| {
                matches!(
                    x,
                    Modal::Confirm { state: ConfirmState::Accepted { .. }, .. }
                        | Modal::Publication { publish: ConfirmState::Accepted { .. }, .. }
                )
            });
            if m.ui.screen == Screen::Settings || !m.link.accepts_events() || running || accepted {
                m.dirty = true;
            }
            return fx;
        }
        Msg::Resize(w, h) => m.size = (w, h),
        Msg::Key(k) => {
            if let Some(a) = keymap::resolve(m.ui.screen, m.top_modal(), k) {
                let before = (view_of(m), m.notice.clone());
                act(m, a, &mut fx);
                forget_old_toast(m, before);
            }
        }
        Msg::Mouse(e) => {
            let before = (view_of(m), m.notice.clone());
            mouse(m, e, &mut fx);
            forget_old_toast(m, before);
        }
        Msg::Net(e) => net(m, e, &mut fx),
    }
    prefetch(m, &mut fx);
    settle_timeline_cursor(m);
    if !page_reloading(m) {
        clamp_page(m);
    }
    // A resize or new content can leave less below the fold than the reader had scrolled past.
    if m.top_modal().is_none() && matches!(m.ui.screen, Screen::Inbox | Screen::Review) {
        m.ui.doc_scroll = m.ui.doc_scroll.min(crate::view::doc_scroll_limit(m));
    }
    m.dirty = true;
    fx
}

/// A selected row a filter or a newer event hid (a phase change is a heading, not a row) hands the cursor to the
/// nearest row still shown, so the cursor never vanishes or jumps to the top.
fn settle_timeline_cursor(m: &mut Model) {
    let Some(t) = &m.timeline else { return };
    let Some(sel) = t.selected else { return };
    let stops = timeline_stops(m, t);
    if stops.is_empty() || stops.contains(&sel) {
        return;
    }
    let at = t.events.iter().position(|e| e.sequence.get() == sel).unwrap_or(0);
    let nearest = t
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| stops.contains(&e.sequence.get()))
        .min_by_key(|(i, _)| i.abs_diff(at))
        .map(|(_, e)| e.sequence.get());
    if let Some(t) = &mut m.timeline {
        t.selected = nearest;
    }
}

/// The document shows every card's evidence, so the selected PR's details load as soon as it is selected.
fn prefetch(m: &mut Model, fx: &mut Vec<Effect>) {
    let Some(pr) = m.ui.selected_pr.clone() else { return };
    let ids: Vec<FindingId> = m
        .domain
        .pr_findings(&pr)
        .iter()
        .filter(|f| !m.details.contains_key(&f.id) && !m.detail_errors.contains_key(&f.id))
        .map(|f| f.id.clone())
        .collect();
    for id in ids {
        fetch_detail(m, &id, fx);
    }
}

fn request(m: &mut Model) -> RequestId {
    let id = RequestId::new(&format!("req-{}", m.next_request));
    m.next_request += 1;
    m.sent_at.insert(id.clone(), m.now_ms);
    id
}

/// Sends every request but `hello`. The server answers anything before its welcome with an error, so until then
/// this sends nothing and returns `None`; the caller leaves a mark the next snapshot retries.
fn send(m: &mut Model, fx: &mut Vec<Effect>, msg: impl FnOnce(RequestId) -> ClientMessage) -> Option<RequestId> {
    if !m.link.welcomed() {
        return None;
    }
    let request_id = request(m);
    fx.push(Effect::Send(msg(request_id.clone())));
    Some(request_id)
}

/// Ends every request older than the timeout that is still waited on, as if the server had answered with an error.
fn expire_requests(m: &mut Model) {
    let (now, timeout) = (m.now_ms, m.request_timeout_ms);
    let expired: Vec<RequestId> =
        m.sent_at.iter().filter(|(_, at)| now.saturating_sub(**at) >= timeout).map(|(id, _)| id.clone()).collect();
    for id in expired {
        m.sent_at.remove(&id);
        if m.snapshot_request.as_ref() == Some(&id) {
            m.snapshot_request = None;
            transition(m, LinkInput::SnapshotTimedOut);
            m.dirty = true;
        } else if let Some(state) = waiting_confirm(m, &id) {
            *state = ConfirmState::TimedOut;
            m.dirty = true;
        } else if fail_request(m, &id, Text::new(TIMED_OUT)) {
            m.dirty = true;
        }
    }
}

/// The confirmation, in a panel or the publication page, that waits on the reply to `id`.
fn waiting_confirm<'a>(m: &'a mut Model, id: &RequestId) -> Option<&'a mut ConfirmState> {
    m.ui.modals.iter_mut().find_map(|x| match x {
        Modal::Confirm { state, .. } | Modal::Publication { publish: state, .. }
            if *state == ConfirmState::Requested(id.clone()) =>
        {
            Some(state)
        }
        _ => None,
    })
}

/// A start the daemon answers with a run the client already knows sends no run.started. A running run took the
/// start in, so the panel gives way to that review; any other turns into the refusal its state means.
fn settle_replayed_start(m: &mut Model) {
    let known = m.ui.modals.iter().enumerate().find_map(|(i, modal)| match modal {
        Modal::Confirm { command: Command::Start(_), state: ConfirmState::Accepted { operation, .. }, .. } => {
            let run = m.domain.runs.get(&RunId::new(operation.as_str().strip_prefix("review.start:")?))?;
            Some((i, run.clone()))
        }
        _ => None,
    });
    let Some((i, run)) = known else { return };
    if run.status == RunStatus::Running && !run.parked() {
        m.ui.modals.remove(i);
        m.ui.screen = Screen::Review;
        m.ui.focus = Pane::Main;
        m.ui.selected_pr = Some(run.pr_id.clone());
        return notice(m, "Already running; showing that review.");
    }
    let message = Text::new(&start_refusal(m, &run));
    if let Modal::Confirm { state, .. } = &mut m.ui.modals[i] {
        *state = ConfirmState::Refused { code: Some(ErrorCode::Conflict), message };
    }
}

/// The run that makes the daemon refuse a start of `pr` for certain: a parked run on the same base and head, under a
/// daemon that grants publishing. The daemon keys a revision on its policy and reviewer too, which the client does
/// not know, so a start on any other head it has a run for goes to the daemon to decide; a running one it takes in.
/// Without review.publish (a dry_run restart) its policy differs from the parked run's, so it starts a new one.
fn parked_head<'a>(m: &'a Model, pr: &Pr) -> Option<&'a Run> {
    m.domain
        .runs
        .values()
        .filter(|_| m.may_write("review.publish"))
        .find(|r| r.pr_id == pr.id && r.base_sha == pr.base_sha && r.head_sha == pr.head_sha && r.parked())
}

/// The daemon's words for refusing a start at `run`'s head: `#184 at 4444444 is already being reviewed`.
fn start_refusal(m: &Model, run: &Run) -> String {
    let why = match run.status {
        RunStatus::Running if run.parked() => "is waiting for your approval",
        RunStatus::Running => "is already being reviewed",
        RunStatus::Failed => "failed its review; retry it instead",
        RunStatus::Superseded => "is superseded by a newer head",
        _ => "is already reviewed",
    };
    let number = m.domain.prs.get(&run.pr_id).map_or(run.pr_id.to_string(), |x| format!("#{}", x.number));
    format!("{number} at {} {why}", run.head_sha.short())
}

/// Clears the pending marker that waits on `id` and shows `message` there. False when nothing waits on it.
fn fail_request(m: &mut Model, id: &RequestId, message: Text) -> bool {
    let preview = m.ui.modals.iter_mut().find_map(|x| match x {
        Modal::Publication { preview, .. } if *preview == Preview::Loading(id.clone()) => Some(preview),
        _ => None,
    });
    if let Some(preview) = preview {
        *preview = Preview::Failed(message);
    } else if let Some(f) = m.detail_requests.remove(id) {
        m.detail_errors.insert(f, message);
    } else if let Some(d) = m.diff.as_mut().filter(|d| d.pending.as_ref() == Some(id)) {
        d.pending = None;
        d.error = Some(message);
    } else if let Some(t) = m.timeline.as_mut().filter(|t| t.pending.as_ref() == Some(id)) {
        t.pending = None;
        t.error = Some(message);
    } else {
        return false;
    }
    true
}

/// The sentence for a diff the daemon would not serve; its own words go to Settings' errors.
fn diff_error(m: &Model, key: &DiffKey, code: ErrorCode) -> String {
    let failed = m.domain.runs.values().any(|r| {
        r.pr_id == key.pr && r.base_sha == key.base && r.head_sha == key.head && r.status == RunStatus::Failed
    });
    match code {
        ErrorCode::NotFound | ErrorCode::TemporarilyUnavailable if failed => {
            "No diff for this run. The review failed before it read the diff.".into()
        }
        ErrorCode::NotFound => "The daemon has no diff for this revision.".into(),
        ErrorCode::TemporarilyUnavailable => "The daemon can't read this diff now. Settings lists why.".into(),
        _ => "The diff did not load. Settings lists the error.".into(),
    }
}

/// What the reader is looking at: the screen and the panels over it.
fn view_of(m: &Model) -> (Screen, usize) {
    (m.ui.screen, m.ui.modals.len())
}

/// A toast speaks about the view it was raised in, so a key that changes the view without raising one clears it.
fn forget_old_toast(m: &mut Model, (view, notice): ((Screen, usize), Option<Notice>)) {
    if view_of(m) != view && m.notice == notice {
        m.notice = None;
    }
}

/// Every toast goes through here and reads as a sentence: a debug build, and so every test that reaches one, fails on
/// a toast that starts with a lowercase letter.
fn notice(m: &mut Model, text: impl Into<String>) {
    let text = text.into();
    debug_assert!(!text.starts_with(|c: char| c.is_lowercase()), "a toast starts with a capital: {text:?}");
    m.notice = Some(Notice { text, until_ms: m.now_ms + NOTICE_MS });
}

fn transition(m: &mut Model, input: LinkInput) -> bool {
    match m.link.next(input) {
        Some(next) => {
            m.link = next;
            true
        }
        None => false,
    }
}

fn net(m: &mut Model, e: NetEvent, fx: &mut Vec<Effect>) {
    match e {
        NetEvent::Dialing { attempt } => {
            transition(m, LinkInput::Dialing { attempt });
        }
        NetEvent::Opened => {
            if transition(m, LinkInput::Opened) {
                if m.diag.last_snapshot_ms.is_some() {
                    m.diag.reconnects += 1;
                }
                let request_id = request(m);
                fx.push(Effect::Send(ClientMessage::Hello {
                    request_id,
                    protocol_version: PROTOCOL_VERSION,
                    client: "ratatui".into(),
                    capabilities: vec!["snapshots".into(), "events".into()],
                }));
            }
        }
        NetEvent::Message(msg) => server_message(m, msg, fx),
        NetEvent::Activities(batch) => apply_batch(m, batch, fx),
        NetEvent::Overflow => resync(m, "client queue overflowed", fx),
        NetEvent::LineTooLong => {
            m.diag.error(m.now_ms, "server sent a line over 1 MiB");
            resync(m, "oversized line", fx);
        }
        NetEvent::Unusable(reason) => {
            m.diag.error(m.now_ms, &reason);
            transition(m, LinkInput::Unusable { message: Text::new(&reason) });
        }
        NetEvent::Garbled(err) => m.diag.error(m.now_ms, format!("unreadable server message: {err}")),
        NetEvent::Lost { reason, retry_in_ms } => {
            m.diag.error(m.now_ms, format!("disconnected: {reason}"));
            transition(m, LinkInput::Lost { reason: Text::new(&reason), retry_in_ms });
            let lost = Text::new("connection lost before the reply arrived");
            if let Some(t) = &mut m.timeline
                && t.pending.take().is_some()
            {
                t.error = Some(lost.clone());
            }
            if let Some(d) = &mut m.diff
                && d.pending.take().is_some()
            {
                d.error = Some(lost);
            }
            for x in &mut m.ui.modals {
                if let Modal::Publication { preview: preview @ Preview::Loading(_), .. } = x {
                    *preview = Preview::Failed(Text::new("connection lost before the preview arrived"));
                }
            }
            m.detail_requests.clear();
            m.snapshot_request = None;
            m.stale_snapshots.clear();
        }
    }
}

fn resync(m: &mut Model, reason: &str, fx: &mut Vec<Effect>) {
    if transition(m, LinkInput::StreamBroken) {
        tracing::info!(reason, "resync");
        snapshot_get(m, fx);
    }
}

/// Asks for a full snapshot. The reply to any earlier, unanswered snapshot.get is stale from now on.
fn snapshot_get(m: &mut Model, fx: &mut Vec<Effect>) {
    m.diag.resyncs += 1;
    if let Some(old) = m.snapshot_request.take() {
        m.stale_snapshots.insert(old);
    }
    m.snapshot_request = send(m, fx, |request_id| ClientMessage::SnapshotGet { request_id });
}

fn server_message(m: &mut Model, msg: ServerMessage, fx: &mut Vec<Effect>) {
    match msg {
        ServerMessage::Welcome {
            protocol_version,
            server_version,
            session_id,
            capabilities,
            stream_epoch,
            mode,
            read_only_reason,
            ..
        } => {
            if protocol_version != PROTOCOL_VERSION {
                let message = Text::new(&format!("server speaks protocol {protocol_version}"));
                if transition(m, LinkInput::Mismatch { supported: vec![protocol_version], message }) {
                    fx.push(Effect::Close);
                }
                return;
            }
            m.server =
                Some(ServerInfo { protocol_version, server_version, session_id, capabilities, mode, read_only_reason });
            let resuming = m.stream.epoch.as_ref() == Some(&stream_epoch);
            if !transition(m, LinkInput::Welcomed { resuming }) {
                return;
            }
            let (epoch, after_sequence) = (m.stream.epoch.clone(), m.stream.last_applied);
            send(m, fx, |request_id| match (resuming, epoch) {
                (true, Some(stream_epoch)) => ClientMessage::Resume { request_id, stream_epoch, after_sequence },
                _ => ClientMessage::SnapshotGet { request_id },
            });
        }
        ServerMessage::Snapshot(s) => apply_snapshot(m, s, fx),
        ServerMessage::Event(Event::Heartbeat { .. }) => m.diag.last_heartbeat_ms = Some(m.now_ms),
        ServerMessage::Event(Event::Sequenced(e)) => apply_event(m, e, fx),
        ServerMessage::ResyncRequired { reason, .. } => {
            m.diag.error(m.now_ms, format!("resync required: {reason}"));
            if m.link == Link::Live {
                resync(m, reason.as_str(), fx);
            } else if m.link == Link::Resyncing {
                // The server dropped what it had queued, maybe the snapshot reply too: ask again.
                snapshot_get(m, fx);
            } else if m.link == Link::Syncing || m.link == Link::Negotiating {
                // A rejected resume: fall back to a full snapshot.
                m.link = Link::Syncing;
                snapshot_get(m, fx);
            }
        }
        ServerMessage::DiffPage(p) => apply_diff_page(m, p, fx),
        ServerMessage::FindingDetail { request_id, finding } => {
            if let Some(id) = request_id.as_ref() {
                m.detail_requests.remove(id);
            }
            m.detail_errors.remove(&finding.finding.id);
            m.details.insert(finding.finding.id.clone(), finding);
        }
        ServerMessage::TimelinePage { request_id, events, next_before_sequence } => {
            if let Some(t) = &mut m.timeline
                && t.pending.is_some()
                && t.pending == request_id
            {
                t.pending = None;
                t.events.extend(events);
                t.next_before = next_before_sequence;
                if t.selected.is_none() {
                    t.selected = t.events.first().map(|e| e.sequence.get());
                }
            }
        }
        ServerMessage::PublicationPreview(p) => {
            // Snapshots carry no publication status, so after a resync the preview is where it comes back from.
            if m.domain
                .publications
                .get(&p.run_id)
                .is_none_or(|x| x.status != p.status || x.review_id.is_none() && p.review_id.is_some())
            {
                let (review_id, url) = (p.review_id, p.url.clone());
                let seeded = PublicationChanged { run_id: p.run_id.clone(), status: p.status, review_id, url };
                m.domain.publications.insert(p.run_id.clone(), seeded);
            }
            for x in &mut m.ui.modals {
                if let Modal::Publication { run_id, preview, .. } = x
                    && *run_id == p.run_id
                    && matches!(&*preview, Preview::Loading(id) if Some(id) == p.request_id.as_ref())
                {
                    *preview = Preview::Ready(Box::new(p.clone()));
                }
            }
        }
        ServerMessage::CommandResult(r) => {
            let Some(&verb) = m.commands_sent.get(&r.request_id) else { return };
            let now = m.now_ms;
            if let Some(state) = waiting_confirm(m, &r.request_id) {
                *state = match r.outcome {
                    CommandOutcome::Accepted { operation_id } => {
                        ConfirmState::Accepted { operation: operation_id, at_ms: now }
                    }
                    CommandOutcome::Rejected { code, message } => ConfirmState::Refused { code: Some(code), message },
                };
                settle_replayed_start(m);
                return;
            }
            let text = match r.outcome {
                CommandOutcome::Accepted { .. } => "Sent; waiting for the daemon to confirm it".to_string(),
                CommandOutcome::Rejected { code, message } => {
                    m.diag.error(now, format!("{code}: {message}"));
                    format!("Can't {verb}: {message}")
                }
            };
            notice(m, text);
        }
        ServerMessage::Error { request_id, code, message, supported } => {
            if code == ErrorCode::ProtocolMismatch {
                let supported = supported.unwrap_or_default();
                if transition(m, LinkInput::Mismatch { supported, message }) {
                    fx.push(Effect::Close);
                }
                return;
            }
            m.diag.error(m.now_ms, format!("{code}: {message}"));
            let Some(id) = request_id else {
                notice(m, format!("Server error {code}: {message}"));
                return;
            };
            let shown = match &m.diff {
                Some(d) if d.pending.as_ref() == Some(&id) => Text::new(&diff_error(m, &d.key, code)),
                _ => message.clone(),
            };
            if !fail_request(m, &id, shown) {
                notice(m, format!("Server error {code}: {message}"));
            }
        }
    }
}

fn apply_snapshot(m: &mut Model, s: Snapshot, fx: &mut Vec<Effect>) {
    if s.request_id.as_ref().is_some_and(|id| m.stale_snapshots.remove(id))
        || !transition(m, LinkInput::SnapshotApplied)
    {
        return;
    }
    if s.request_id.is_some() && s.request_id == m.snapshot_request {
        m.snapshot_request = None;
    }
    m.stream = StreamPos { epoch: Some(s.stream_epoch), last_applied: s.sequence };
    m.diag.last_snapshot_ms = Some(m.now_ms);
    m.domain.prs = s.prs.into_iter().map(|p| (p.id.clone(), p)).collect();
    m.domain.runs = s.runs.into_iter().map(|r| (r.id.clone(), r)).collect();
    m.domain.findings = s.findings.into_iter().map(|f| (f.id.clone(), f)).collect();
    m.domain.accounts = s.accounts;
    if let Some(accounts) = m.domain.accounts.clone() {
        settle_account_select(m, &accounts);
    }
    m.details.clear();
    m.detail_errors.clear();
    // A reply lost to an overflow never comes: ask again for what the screen waits on or failed to get.
    m.detail_requests.clear();
    let retry_diff = m
        .diff
        .as_ref()
        .filter(|d| d.pending.is_some() || d.error.is_some())
        .and_then(|d| if d.files.is_empty() { Some(None) } else { d.next_cursor.clone().map(Some) });
    if let Some(cursor) = retry_diff {
        diff_get(m, cursor, fx);
    }
    if let Some(wanted) = m.ui.wanted_pr.take() {
        if let Some(p) = m.domain.prs.values().find(|p| wanted.matches(p)) {
            m.ui.selected_pr = Some(p.id.clone());
            m.ui.screen = Screen::Review;
        } else {
            notice(m, format!("The reviewer does not track {wanted}"));
        }
    }
    // Land on the story that matters most: the PR with the most severe current finding.
    if m.ui.selected_pr.is_none() {
        let d = &m.domain;
        m.ui.selected_pr = d
            .prs
            .values()
            .filter_map(|p| d.pr_findings(&p.id).iter().filter(|f| !f.stale).map(|f| f.severity).min().map(|s| (s, p)))
            .min_by_key(|(s, p)| (*s, std::cmp::Reverse(p.number)))
            .map(|(_, p)| p.id.clone());
    }
    reselect(m);
    if let Some(t) = &m.timeline {
        let (scope, query) = (t.scope, t.query.clone());
        load_timeline(m, scope, query, fx);
    }
    match m.top_modal().cloned() {
        Some(Modal::Finding { id, .. }) => fetch_detail(m, &id, fx),
        // The events that moved the publication may be the ones the resync lost: ask again, whatever it showed.
        Some(Modal::Publication { run_id, .. }) => fetch_preview(m, run_id, fx),
        _ => {}
    }
}

/// Asks for `run`'s stored payload; the top publication page shows it when it arrives.
fn fetch_preview(m: &mut Model, run: RunId, fx: &mut Vec<Effect>) {
    let request = send(m, fx, |request_id| ClientMessage::PublicationGet { request_id, run_id: run.clone() });
    if let Some(Modal::Publication { preview, .. }) = m.ui.modals.last_mut() {
        *preview = match request {
            Some(id) => Preview::Loading(id),
            None => Preview::Failed(Text::new(OFFLINE)),
        };
    }
}

enum Admit {
    Apply,
    Drop,
    Resync,
}

/// protocol/README.md "Recovery rules", in order.
fn admit(m: &Model, epoch: &Epoch, first_sequence: u64, prev: u64) -> Admit {
    if !m.link.accepts_events() {
        Admit::Drop
    } else if m.stream.epoch.as_ref() != Some(epoch) {
        Admit::Resync
    } else if first_sequence <= m.stream.last_applied {
        Admit::Drop
    } else if prev != m.stream.last_applied {
        Admit::Resync
    } else {
        Admit::Apply
    }
}

fn apply_event(m: &mut Model, e: SequencedEvent, fx: &mut Vec<Effect>) {
    match admit(m, &e.stream_epoch, e.sequence.get(), e.prev_sequence) {
        Admit::Drop => {}
        Admit::Resync => resync(m, "sequence gap or epoch change", fx),
        Admit::Apply => {
            m.stream.last_applied = e.sequence.get();
            apply_body(m, &e, fx);
            if let Some(t) = &mut m.timeline
                && timeline_matches(&t.query, &e)
            {
                t.events.insert(0, e);
            }
        }
    }
}

fn apply_batch(m: &mut Model, b: ActivityBatch, fx: &mut Vec<Effect>) {
    if b.last_sequence <= m.stream.last_applied && m.stream.epoch.as_ref() == Some(&b.stream_epoch) {
        return;
    }
    match admit(m, &b.stream_epoch, b.first_sequence, b.first_prev) {
        Admit::Drop => {}
        Admit::Resync => resync(m, "sequence gap or epoch change", fx),
        Admit::Apply => {
            m.stream.last_applied = b.last_sequence;
            let unseen = b.count - b.events.len() as u64;
            m.domain.tool_activity_count += unseen;
            for e in b.events {
                apply_body(m, &e, fx);
            }
        }
    }
}

fn apply_body(m: &mut Model, e: &SequencedEvent, fx: &mut Vec<Effect>) {
    let seq = e.sequence.get();
    let d = &mut m.domain;
    let kind = match &e.body {
        EventBody::PrUpdated(pr) => {
            d.prs.insert(pr.id.clone(), pr.clone());
            None
        }
        EventBody::Run(change, run) => {
            d.runs.insert(run.id.clone(), run.clone());
            Some(ActivityKind::Run { change: *change, status: run.status })
        }
        EventBody::PhaseChanged(p) => {
            if let Some(r) = d.runs.get_mut(&p.run_id) {
                r.phase = p.to;
            }
            Some(ActivityKind::Phase { from: p.from, to: p.to })
        }
        EventBody::FindingUpdated(f) => {
            d.findings.insert(f.id.clone(), f.clone());
            m.details.remove(&f.id);
            Some(ActivityKind::Finding { title: f.title.clone(), status: f.status, severity: f.severity })
        }
        EventBody::ToolActivity(a) => {
            d.tool_activity_count += 1;
            Some(ActivityKind::Tool { tool: a.tool.clone(), summary: a.summary.clone() })
        }
        EventBody::AccountsUpdated(accounts) => {
            d.accounts = Some(accounts.clone());
            settle_account_select(m, accounts);
            return;
        }
        EventBody::PublicationChanged(p) => {
            d.publications.insert(p.run_id.clone(), p.clone());
            None
        }
        EventBody::Other(_) => None,
    };
    let pr = match &e.body {
        EventBody::PrUpdated(p) => Some(p.id.clone()),
        EventBody::Run(_, r) => Some(r.pr_id.clone()),
        EventBody::FindingUpdated(f) => Some(f.pr_id.clone()),
        _ => e.pr_id.clone(),
    };
    if let Some(kind) = kind {
        d.push_activity(Activity { sequence: seq, at: e.occurred_at.clone(), pr_id: pr, kind, recv_ms: m.now_ms });
    }
    settle_command(m, e);
    if let EventBody::FindingUpdated(f) = &e.body
        && matches!(m.top_modal(), Some(Modal::Finding { id, .. }) if id == &f.id)
    {
        fetch_detail(m, &f.id.clone(), fx);
    }
    reselect(m);
}

/// The selection shows only once accounts.updated or a snapshot reports it: that is what closes a pending
/// account.select modal, even one whose command.result was dropped.
fn settle_account_select(m: &mut Model, accounts: &[Account]) {
    let done = |modal: &Modal| match modal {
        Modal::Confirm { command: Command::SelectAccount(p), state, .. } => {
            matches!(state, ConfirmState::Requested(_) | ConfirmState::Accepted { .. } | ConfirmState::TimedOut)
                && accounts.iter().any(|a| a.selected && a.id == p.account_id)
        }
        _ => false,
    };
    let Some(i) = m.ui.modals.iter().position(done) else { return };
    m.ui.modals.remove(i);
    if let Some(a) = accounts.iter().find(|a| a.selected) {
        notice(m, format!("GitHub reads and publishing now use {} on {}", a.login, a.host));
    }
}

/// A start, retry or cancel is done when an event shows its effect, never on `accepted`: the run it started, the
/// retried run moving again, the cancelled run ending. That event closes the panel, even one whose reply was dropped.
fn settle_command(m: &mut Model, e: &SequencedEvent) {
    let shows = |command: &Command| match (command, &e.body) {
        (Command::Start(p), EventBody::Run(RunChange::Started | RunChange::Failed, r)) => {
            r.pr_id == p.pr_id && r.head_sha == p.expected_head_sha
        }
        (Command::Retry(p), EventBody::Run(_, r)) => r.id == p.run_id,
        (Command::Retry(p), EventBody::PhaseChanged(c)) => c.run_id == p.run_id,
        (Command::Cancel(p), EventBody::Run(RunChange::Failed | RunChange::Superseded, r)) => r.id == p.run_id,
        (Command::Cancel(p), EventBody::PhaseChanged(c)) => {
            c.run_id == p.run_id && matches!(c.to, Phase::Failed | Phase::Superseded)
        }
        _ => false,
    };
    let done = |modal: &Modal| match modal {
        Modal::Confirm { command, state, .. } => {
            !matches!(state, ConfirmState::Asking | ConfirmState::Refused { .. }) && shows(command)
        }
        _ => false,
    };
    let Some(i) = m.ui.modals.iter().position(done) else { return };
    let Modal::Confirm { command, .. } = m.ui.modals.remove(i) else { return };
    let number = |run: &RunId| {
        let pr = m.domain.runs.get(run).and_then(|r| m.domain.prs.get(&r.pr_id));
        pr.map_or(format!("run {}", run.short()), |p| format!("#{}", p.number))
    };
    let what = match (&command, &e.body) {
        (Command::Cancel(_), EventBody::Run(_, r)) => format!("Cancelled the review of {}", number(&r.id)),
        (Command::Cancel(p), _) => format!("Cancelled the review of {}", number(&p.run_id)),
        (Command::Retry(p), _) => {
            let attempt = m.domain.runs.get(&p.run_id).and_then(|r| r.attempt);
            format!("Retrying {}{}", number(&p.run_id), attempt.map_or(String::new(), |n| format!(" (attempt {n})")))
        }
        (_, EventBody::Run(RunChange::Started, r)) => {
            format!("Started reviewing {} at {}", number(&r.id), r.head_sha.short())
        }
        (_, EventBody::Run(RunChange::Failed, r)) => {
            let why = r.failure_reason.as_ref().map_or(String::new(), |x| format!(": {x}"));
            format!("The review of {} failed{why}", number(&r.id))
        }
        _ => return,
    };
    notice(m, what);
}

/// Re-resolves selections by ID after the lists changed; an ID that vanished falls back to the first entry.
fn reselect(m: &mut Model) {
    let inbox = m.inbox();
    let keep = m.ui.selected_pr.as_ref().is_some_and(|id| inbox.iter().any(|p| &p.id == id));
    if !keep {
        m.ui.selected_pr = inbox.first().map(|p| p.id.clone());
    }
    let Some(pr) = m.ui.selected_pr.clone() else {
        m.ui.selected_finding = None;
        return;
    };
    let findings = m.domain.pr_findings(&pr);
    let keep =
        m.on_ready_line() || m.ui.selected_finding.as_ref().is_some_and(|id| findings.iter().any(|f| &f.id == id));
    if !keep {
        // With no finding to hold it, the cursor rests on a parked run's ready line, so Enter opens the preview.
        m.ui.ready_line = if findings.is_empty() { m.parked_run().map(|r| r.id.clone()) } else { None };
        m.ui.selected_finding = findings.first().map(|f| f.id.clone());
    }
}

fn timeline_matches(q: &TimelineQuery, e: &SequencedEvent) -> bool {
    let pr_ok = q.pr.is_none() || q.pr == e.pr_id;
    let run_ok = q.run.is_none() || q.run == e.run_id;
    let finding_ok = match (&q.finding, &e.body) {
        (None, _) => true,
        (Some(id), EventBody::FindingUpdated(f)) => &f.id == id,
        _ => false,
    };
    pr_ok && run_ok && finding_ok
}

fn fetch_detail(m: &mut Model, id: &FindingId, fx: &mut Vec<Effect>) {
    if m.detail_requests.values().any(|f| f == id) {
        return;
    }
    let Some(request_id) = send(m, fx, |request_id| ClientMessage::FindingGet { request_id, finding_id: id.clone() })
    else {
        return;
    };
    m.detail_errors.remove(id);
    m.detail_requests.insert(request_id, id.clone());
}

// ---- diff ----

/// The finding reader shows the diff at the anchor, so opening a finding loads its revision's diff in the background.
fn prefetch_diff(m: &mut Model, id: &FindingId, fx: &mut Vec<Effect>) {
    let Some(key) = finding_diff_key(m, id) else { return };
    if m.diff.as_ref().is_some_and(|d| d.key == key && d.error.is_none()) {
        return;
    }
    let anchor = m.domain.findings.get(id).map(|f| (f.file.clone(), f.line.map(|l| l.get())));
    load_diff(m, key, anchor, fx);
}

/// The diff a finding was found against: its run's revision, else the PR's.
pub fn finding_diff_key(m: &Model, id: &FindingId) -> Option<DiffKey> {
    let f = m.domain.findings.get(id)?;
    let (base, head) = match (m.domain.runs.get(&f.run_id), m.domain.prs.get(&f.pr_id)) {
        (Some(r), _) => (r.base_sha.clone(), r.head_sha.clone()),
        (None, Some(p)) => (p.base_sha.clone(), p.head_sha.clone()),
        (None, None) => return None,
    };
    Some(DiffKey { pr: f.pr_id.clone(), base, head })
}

fn load_diff(m: &mut Model, key: DiffKey, anchor: Option<(Text, Option<u32>)>, fx: &mut Vec<Effect>) {
    if let Some(d) = m.diff.as_mut().filter(|d| d.key == key && d.error.is_none()) {
        if anchor.is_some() {
            d.anchor = anchor;
        }
        resolve_anchor(m, fx);
        return;
    }
    m.diff = Some(DiffView {
        key,
        files: Vec::new(),
        next_cursor: None,
        truncated: false,
        pending: None,
        error: None,
        selected_file: None,
        cursor: 0,
        stack: 0,
        anchor,
        search: None,
        matches: Vec::new(),
    });
    diff_get(m, None, fx);
}

fn load_more_diff(m: &mut Model, fx: &mut Vec<Effect>) {
    let Some(d) = &m.diff else { return };
    let (Some(cursor), None) = (d.next_cursor.clone(), &d.pending) else { return };
    diff_get(m, Some(cursor), fx);
}

/// Requests one page of the open diff: the first with no `cursor`.
fn diff_get(m: &mut Model, cursor: Option<Text>, fx: &mut Vec<Effect>) {
    let Some(key) = m.diff.as_ref().map(|d| d.key.clone()) else { return };
    let pending = send(m, fx, |request_id| ClientMessage::DiffGet {
        request_id,
        pr_id: key.pr,
        base_sha: key.base,
        head_sha: key.head,
        file: None,
        cursor,
        limit: Some(Limit::DEFAULT),
    });
    if let Some(d) = &mut m.diff {
        d.error = if pending.is_none() { Some(Text::new(OFFLINE)) } else { None };
        d.pending = pending;
    }
}

fn apply_diff_page(m: &mut Model, p: DiffPage, fx: &mut Vec<Effect>) {
    let Some(d) = &mut m.diff else { return };
    let key = DiffKey { pr: p.pr_id, base: p.base_sha, head: p.head_sha };
    if d.key != key || d.pending.is_none() || (p.request_id.is_some() && p.request_id != d.pending) {
        return;
    }
    d.pending = None;
    for file in p.files {
        match d.files.iter_mut().find(|f| f.path == file.path) {
            Some(existing) => {
                existing.hunks.extend(file.hunks);
                existing.truncated = file.truncated;
            }
            None => d.files.push(file),
        }
    }
    d.next_cursor = p.next_cursor;
    d.truncated = p.truncated;
    d.matches = d.search.as_ref().map_or(vec![], |q| diff_matches(d, q));
    if d.selected_file.is_none() {
        d.selected_file = d.files.first().map(|f| f.path.clone());
    }
    resolve_anchor(m, fx);
}

/// Moves the diff cursor to the anchor once its file has loaded. Only on the Diff screen does it take focus or say
/// the file is missing: a prefetch behind the finding reader stays silent.
fn resolve_anchor(m: &mut Model, fx: &mut Vec<Effect>) {
    let shown = m.ui.screen == Screen::Diff;
    let Some(d) = &mut m.diff else { return };
    let Some((path, line)) = d.anchor.clone() else { return };
    let p = path.as_str();
    let found = d
        .files
        .iter()
        .position(|f| f.path == path)
        .or_else(|| d.files.iter().position(|f| !p.is_empty() && f.path.as_str().ends_with(p)));
    match found {
        Some(i) => {
            d.cursor = line.map_or(0, |l| row_for_line(&d.files[i], l));
            d.stack = 0;
            d.selected_file = Some(d.files[i].path.clone());
            d.anchor = None;
            if shown {
                m.ui.focus = Pane::Main;
            }
        }
        None if d.next_cursor.is_some() => load_more_diff(m, fx),
        None if d.pending.is_some() => {}
        None => {
            d.anchor = None;
            if shown {
                notice(m, format!("No file in this diff matches \"{path}\""));
            }
        }
    }
}

/// The row showing head-revision `line`, or the first row after it.
fn row_for_line(file: &DiffFile, line: u32) -> usize {
    let rows = file_rows(file);
    rows.iter()
        .position(|r| matches!(r, DiffRow::Line(l) if l.new.is_some_and(|n| n >= line)))
        .or_else(|| rows.iter().rposition(|r| matches!(r, DiffRow::Line(_))))
        .unwrap_or(0)
}

/// Findings anchored to exactly this diff's head revision. Anchors are never remapped onto another revision.
pub fn diff_findings<'a>(m: &'a Model, d: &DiffView) -> Vec<&'a Finding> {
    m.domain
        .pr_findings(&d.key.pr)
        .into_iter()
        .filter(|f| m.domain.runs.get(&f.run_id).is_some_and(|r| r.head_sha == d.key.head))
        .collect()
}

/// The findings anchored to `row` of `file`, most severe first: they stack under that line in the diff.
pub fn row_findings<'a>(anchored: &[&'a Finding], file: &DiffFile, row: &DiffRow) -> Vec<&'a Finding> {
    let DiffRow::Line(l) = row else { return vec![] };
    anchored
        .iter()
        .filter(|f| f.file == file.path && l.new.is_some() && f.line.map(|n| n.get()) == l.new)
        .copied()
        .collect()
}

/// (file index, row, place in its line's stack) of each anchored finding, in diff order.
fn finding_stops(m: &Model, d: &DiffView) -> Vec<(usize, usize, usize)> {
    let mut stops: Vec<_> = diff_findings(m, d)
        .into_iter()
        .filter_map(|f| {
            let fi = d.files.iter().position(|file| file.path == f.file)?;
            Some((fi, f.line.map_or(0, |l| row_for_line(&d.files[fi], l.get()))))
        })
        .collect();
    stops.sort();
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    for (fi, row) in stops {
        let k = out.iter().filter(|s| (s.0, s.1) == (fi, row)).count();
        out.push((fi, row, k));
    }
    out
}

// ---- timeline ----

fn load_timeline(m: &mut Model, scope: TimelineScope, query: TimelineQuery, fx: &mut Vec<Effect>) {
    // Filters belong to the PR they were set on; another PR starts unfiltered, so its timeline is never empty
    // for a reason set elsewhere.
    let (category, phase, text) = (m.timeline.as_ref().filter(|t| t.query.pr == query.pr))
        .map_or((None, None, String::new()), |t| (t.category, t.phase, t.text.clone()));
    let pending = send(m, fx, |request_id| ClientMessage::TimelineGet {
        request_id,
        pr_id: query.pr.clone(),
        run_id: query.run.clone(),
        finding_id: query.finding.clone(),
        before_sequence: None,
        limit: Limit::DEFAULT,
    });
    m.timeline = Some(TimelineView {
        scope,
        query,
        events: Vec::new(),
        next_before: None,
        error: pending.is_none().then(|| Text::new(OFFLINE)),
        pending,
        selected: None,
        open: None,
        category,
        phase,
        text,
    });
}

fn timeline_query(m: &Model, scope: TimelineScope) -> TimelineQuery {
    let pr = m.ui.selected_pr.clone();
    let run = pr.as_ref().and_then(|p| m.domain.latest_run(p)).map(|r| r.id.clone());
    match scope {
        TimelineScope::Pr => TimelineQuery { pr, run: None, finding: None },
        TimelineScope::Run => TimelineQuery { pr, run, finding: None },
        TimelineScope::Finding => TimelineQuery { pr, run: None, finding: m.ui.selected_finding.clone() },
    }
}

/// The phase each event happened in (`events` newest first): the run's phase as the events before it left it, from
/// phase changes and the runs that run events carry. An event older than every loaded phase change takes the `from`
/// of the next later one, else the run's phase now.
pub fn event_phases(m: &Model, events: &[SequencedEvent]) -> Vec<Option<Phase>> {
    let mut later_from: std::collections::BTreeMap<&RunId, Phase> = Default::default();
    let fallback: Vec<Option<Phase>> = events
        .iter()
        .map(|e| {
            let run = e.run_id.as_ref()?;
            let phase = later_from.get(run).copied().or_else(|| m.domain.runs.get(run).map(|r| r.phase));
            if let EventBody::PhaseChanged(p) = &e.body {
                later_from.insert(run, p.from);
            }
            phase
        })
        .collect();
    let mut now: std::collections::BTreeMap<&RunId, Phase> = Default::default();
    let mut out: Vec<Option<Phase>> = events
        .iter()
        .zip(fallback)
        .rev()
        .map(|(e, fallback)| {
            let run = e.run_id.as_ref()?;
            match &e.body {
                EventBody::PhaseChanged(p) => {
                    now.insert(run, p.to);
                }
                EventBody::Run(_, r) => {
                    now.insert(run, r.phase);
                }
                // The publisher's own statuses belong to PUBLISH, even when the journal wrote the run's next phase first.
                EventBody::PublicationChanged(p)
                    if matches!(
                        p.status,
                        PublicationStatus::Publishing | PublicationStatus::Published | PublicationStatus::Uncertain
                    ) =>
                {
                    return Some(Phase::Publish);
                }
                _ => {}
            }
            now.get(run).copied().or(fallback)
        })
        .collect();
    out.reverse();
    out
}

/// The words `/` matches an event by: what it names, the way its row reads.
pub fn event_text(e: &SequencedEvent) -> String {
    match &e.body {
        EventBody::PrUpdated(p) => format!("#{} {} {}", p.number, p.title, p.head_sha.short()),
        EventBody::Run(change, r) => {
            let reason = r.failure_reason.as_ref().map_or("", |x| x.as_str());
            format!("{change:?} {:?} {} {reason}", r.status, r.head_sha.short())
        }
        EventBody::PhaseChanged(p) => format!("{:?} {:?} {}", p.from, p.to, p.event),
        EventBody::FindingUpdated(f) => format!("{} {} {:?} {:?}", f.title, f.file, f.severity, f.status),
        EventBody::ToolActivity(a) => format!("{} {}", a.tool, a.summary),
        EventBody::PublicationChanged(p) => {
            format!("publication {:?} {}", p.status, p.url.as_ref().map_or("", |u| u.as_str()))
        }
        EventBody::AccountsUpdated(a) => a.iter().map(|x| x.login.to_string()).collect::<Vec<_>>().join(" "),
        EventBody::Other(kind) => kind.to_string(),
    }
}

/// The events the timeline shows after its category, phase and text filters, oldest first, with their inferred phases.
/// The rows the timeline shows with no filter, oldest first. A phase the run passed straight through (a retry
/// resuming from a checkpoint) has no rows, so its change is no row either.
pub fn timeline_shown<'a>(m: &Model, t: &'a TimelineView) -> Vec<(&'a SequencedEvent, Option<Phase>)> {
    let mut rows: Vec<_> = t.events.iter().zip(event_phases(m, &t.events)).collect();
    rows.reverse();
    let passed: Vec<bool> = rows
        .windows(2)
        .map(|w| is_heading(w[0].0) && is_heading(w[1].0) && w[0].0.run_id == w[1].0.run_id)
        .chain([false])
        .collect();
    rows.into_iter().zip(passed).filter(|(_, skip)| !skip).map(|(row, _)| row).collect()
}

/// The shown rows the filters keep.
pub fn timeline_rows<'a>(m: &Model, t: &'a TimelineView) -> Vec<(&'a SequencedEvent, Option<Phase>)> {
    let text = t.text.to_lowercase();
    timeline_shown(m, t)
        .into_iter()
        .filter(|(e, p)| {
            t.category.is_none_or(|c| EventCategory::of(&e.body) == c)
                && t.phase.is_none_or(|want| *p == Some(want))
                && (text.is_empty() || event_text(e).to_lowercase().contains(&text))
        })
        .collect()
}

/// The sequences the cursor stops on: the rows the filters keep, without the phase changes drawn as headings.
pub fn timeline_stops(m: &Model, t: &TimelineView) -> Vec<u64> {
    timeline_rows(m, t).iter().filter(|(e, _)| !is_heading(e)).map(|(e, _)| e.sequence.get()).collect()
}

pub fn is_heading(e: &SequencedEvent) -> bool {
    matches!(e.body, EventBody::PhaseChanged(_))
}

const PHASE_CYCLE: [Option<Phase>; 9] = [
    None,
    Some(Phase::CollectContext),
    Some(Phase::Triage),
    Some(Phase::Analyze),
    Some(Phase::Verify),
    Some(Phase::PreparePublication),
    Some(Phase::Publish),
    Some(Phase::Reconcile),
    Some(Phase::Completed),
];

fn cycle<T: PartialEq + Copy>(all: &[T], current: T) -> T {
    let i = all.iter().position(|x| *x == current).unwrap_or(0);
    all[(i + 1) % all.len()]
}

// ---- keys ----

fn page(m: &Model) -> isize {
    (m.size.1 as isize - 8).max(1)
}

fn step<T: PartialEq + Clone>(items: &[T], current: Option<&T>, delta: isize) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let i = current.and_then(|c| items.iter().position(|x| x == c));
    let next = match i {
        None => 0,
        Some(i) => (i as isize + delta).clamp(0, items.len() as isize - 1) as usize,
    };
    Some(items[next].clone())
}

fn act(m: &mut Model, a: Action, fx: &mut Vec<Effect>) {
    match a {
        Action::Quit => fx.push(Effect::Quit),
        Action::Help => {
            if matches!(m.top_modal(), Some(Modal::Help { .. })) {
                m.ui.modals.pop();
            } else {
                m.ui.modals.push(Modal::Help { scroll: 0 });
            }
        }
        Action::Back if !can_back(m) => {}
        Action::Back => {
            if let Some(t) = m.timeline.as_mut().filter(|t| m.ui.screen == Screen::Timeline && t.open.is_some())
                && m.ui.modals.is_empty()
            {
                t.open = None;
            } else if let Some(d) = m.diff.as_mut().filter(|d| m.ui.screen == Screen::Diff && d.search.is_some())
                && m.ui.modals.is_empty()
            {
                d.search = None;
                d.matches.clear();
            } else if m.ui.modals.pop().is_none() {
                m.ui.screen = if m.ui.screen == Screen::Settings { Screen::Inbox } else { Screen::Review };
            }
        }
        Action::Goto(s) => goto(m, s, fx),
        Action::Refresh => {
            if can_refresh(m) {
                resync(m, "manual refresh", fx);
                notice(m, format!("Refreshing the snapshot{}", crate::theme::glyphs(m.look.ascii).ellipsis));
            } else {
                notice(m, "Not connected; the client reconnects on its own");
            }
        }
        Action::Up => move_by(m, -1, fx),
        Action::Down => move_by(m, 1, fx),
        Action::PageUp => move_by(m, -page(m), fx),
        Action::PageDown => move_by(m, page(m), fx),
        Action::Top => move_by(m, isize::MIN / 2, fx),
        Action::Bottom => move_by(m, isize::MAX / 2, fx),
        Action::Left => focus(m, Pane::Sidebar),
        Action::Right => focus(m, Pane::Main),
        // Two panes: either way round, Tab moves to the other one.
        Action::FocusNext | Action::FocusPrev => {
            focus(m, if m.ui.focus == Pane::Main { Pane::Sidebar } else { Pane::Main })
        }
        Action::Open => open(m, fx),
        // `/` searches what the screen shows: the diff's text, the timeline's events, else the PR list.
        Action::Search if !can_search(m) => {}
        Action::Search => {
            let (purpose, before) = match (m.ui.screen, &m.timeline, &m.diff) {
                (Screen::Diff, _, Some(d)) => (InputPurpose::DiffSearch, d.search.clone().unwrap_or_default()),
                (Screen::Timeline, Some(t), _) => (InputPurpose::TimelineFilter, t.text.clone()),
                _ => (InputPurpose::Search, m.ui.pr_filter.clone()),
            };
            m.ui.modals.push(Modal::Input { purpose, buffer: before.clone(), before });
        }
        Action::JumpTo if !can_jump(m) => {}
        Action::JumpTo => m.ui.modals.push(Modal::Input {
            purpose: InputPurpose::JumpTo,
            buffer: String::new(),
            before: String::new(),
        }),
        Action::OpenDiff => open_diff(m, fx),
        Action::Watch => watch(m),
        Action::ScrollDown => m.ui.doc_scroll = (m.ui.doc_scroll + 1).min(crate::view::doc_scroll_limit(m)),
        Action::ScrollUp => m.ui.doc_scroll = (m.ui.doc_scroll - 1).max(0),
        Action::OpenTimeline if !can_timeline(m) => {
            notice(
                m,
                if m.selected_pr().is_none() {
                    "No PR selected"
                } else {
                    "Can't load the timeline: the client is not live"
                },
            );
        }
        Action::OpenTimeline => {
            m.ui.modals.retain(|x| !matches!(x, Modal::Finding { .. }));
            // The document is the PR, so its timeline is the PR's; f narrows to the run or the finding.
            let scope = TimelineScope::Pr;
            m.ui.screen = Screen::Timeline;
            let q = timeline_query(m, scope);
            load_timeline(m, scope, q, fx);
        }
        Action::NextFinding => next_finding(m, 1, fx),
        Action::PrevFinding => next_finding(m, -1, fx),
        Action::Mutation(mu) => mutation(m, mu, fx),
        Action::OpenUrl => {
            if let Some(Modal::Publication { run_id, .. }) = m.top_modal()
                && let Some(url) = published_url(m, run_id)
            {
                fx.push(Effect::OpenUrl(url));
            }
        }
        Action::ToggleRaw => {
            if let Some(Modal::Publication { raw, .. }) = m.ui.modals.last_mut() {
                *raw = !*raw;
            }
        }
        Action::Accounts => {
            goto(m, Screen::Settings, fx);
            if m.domain.accounts.is_none() {
                notice(m, "This daemon reports no GitHub accounts");
            }
        }
        Action::CycleScope if !can_filter_timeline(m) => {}
        Action::CycleCategory if !can_cycle_kind(m) => {}
        Action::CyclePhase if !can_cycle_phase(m) => {}
        Action::CycleScope => {
            if let Some(t) = &m.timeline {
                let scope = cycle(&[TimelineScope::Pr, TimelineScope::Run, TimelineScope::Finding], t.scope);
                let q = timeline_query(m, scope);
                load_timeline(m, scope, q, fx);
            }
        }
        Action::CycleCategory => {
            if let Some(t) = &mut m.timeline {
                t.category = cycle(&EventCategory::CYCLE, t.category);
            }
        }
        Action::CyclePhase => {
            if let Some(t) = &mut m.timeline {
                t.phase = cycle(&PHASE_CYCLE, t.phase);
            }
        }
        Action::InputChar(c) => edit_input(m, |b| b.push(c)),
        Action::InputBackspace => edit_input(m, |b| {
            b.pop();
        }),
        Action::InputCancel => match m.ui.modals.pop() {
            Some(Modal::Input { purpose: InputPurpose::Search, before, .. }) => {
                m.ui.pr_filter = before;
                reselect(m);
            }
            Some(Modal::Input { purpose: InputPurpose::TimelineFilter, before, .. }) => {
                if let Some(t) = &mut m.timeline {
                    t.text = before;
                }
            }
            _ => {}
        },
        Action::Confirm => send_confirmed(m, fx),
        Action::InputSubmit => match m.ui.modals.pop() {
            Some(Modal::Input { purpose: InputPurpose::JumpTo, buffer, .. }) => jump_to(m, &buffer, fx),
            Some(Modal::Input { purpose: InputPurpose::DiffSearch, buffer, .. }) => diff_search(m, buffer),
            _ => {}
        },
    }
}

/// Lines one wheel notch moves.
const WHEEL: isize = 3;

/// The wheel scrolls the reading column: the open reader, the document, the diff or the timeline. A click selects the
/// PR or the finding under it; a click on the selected finding reads it.
fn mouse(m: &mut Model, e: MouseEvent, fx: &mut Vec<Effect>) {
    let delta = match e.kind {
        MouseEventKind::ScrollDown => WHEEL,
        MouseEventKind::ScrollUp => -WHEEL,
        MouseEventKind::Down(MouseButton::Left) => {
            click(m, e.column, e.row, fx);
            return;
        }
        _ => return,
    };
    match (m.top_modal(), m.ui.screen) {
        (Some(Modal::Finding { .. } | Modal::Help { .. } | Modal::Publication { .. }), _) => move_by(m, delta, fx),
        (Some(_), _) => {}
        (None, Screen::Inbox | Screen::Review) => {
            m.ui.doc_scroll = (m.ui.doc_scroll + delta as i32).clamp(0, crate::view::doc_scroll_limit(m));
        }
        (None, Screen::Diff) => {
            m.ui.focus = Pane::Main;
            move_by(m, delta, fx);
        }
        (None, Screen::Timeline) => move_by(m, delta, fx),
        (None, Screen::Settings) => {}
    }
}

fn click(m: &mut Model, x: u16, y: u16, fx: &mut Vec<Effect>) {
    match crate::view::hit(m, x, y) {
        Some(crate::view::Hit::Pr(id)) => {
            if m.ui.selected_pr.as_ref() != Some(&id) {
                m.ui.selected_pr = Some(id);
                m.ui.selected_finding = None;
                m.ui.doc_scroll = 0;
                reselect(m);
            }
        }
        Some(crate::view::Hit::Ready) => {
            if let Some(r) = m.parked_run().cloned() {
                m.ui.screen = Screen::Review;
                m.ui.focus = Pane::Main;
                m.ui.ready_line = Some(r.id.clone());
                m.ui.selected_finding = None;
                open_publication(m, r, fx);
            }
        }
        Some(crate::view::Hit::Finding(id)) => {
            m.ui.screen = Screen::Review;
            m.ui.focus = Pane::Main;
            if m.ui.selected_finding.as_ref() == Some(&id) {
                open_finding(m, id, fx);
            } else {
                m.ui.selected_finding = Some(id);
                m.ui.doc_scroll = 0;
            }
        }
        Some(crate::view::Hit::DiffFile(path)) => {
            if let Some(d) = &mut m.diff
                && d.selected_file.as_ref() != Some(&path)
            {
                d.selected_file = Some(path);
                d.cursor = 0;
                d.stack = 0;
            }
        }
        // A click puts the cursor on the row; a click on the cursor's row opens what Enter would.
        Some(crate::view::Hit::DiffRow(row)) => {
            m.ui.focus = Pane::Main;
            if let Some(d) = &mut m.diff {
                if d.cursor == row {
                    open(m, fx);
                } else {
                    d.cursor = row;
                    d.stack = 0;
                }
            }
        }
        Some(crate::view::Hit::Event(seq)) => {
            if let Some(t) = &mut m.timeline {
                if t.selected == Some(seq) {
                    open(m, fx);
                } else {
                    t.selected = Some(seq);
                }
            }
        }
        Some(crate::view::Hit::View(s)) => goto(m, s, fx),
        None => {}
    }
}

// ---- what a key would do ----
//
// Each key's guard in the reducer and its hint in the footer (`crate::offer`) ask the same question here, so the footer
// offers a key exactly when pressing it acts.

/// A cursor over `items` moves when there is somewhere else to go.
fn walks<T: PartialEq>(items: &[T], current: Option<&T>) -> bool {
    items.len() > 1 || items.first().is_some_and(|only| current != Some(only))
}

/// The Review cursor's stops, a parked run's ready line (`None`) above the findings, and where it rests now.
fn finding_cursor(m: &Model) -> (Vec<Option<FindingId>>, Option<FindingId>) {
    let Some(pr) = &m.ui.selected_pr else { return (vec![], None) };
    let ready = m.parked_run().map(|_| None);
    let stops = ready.into_iter().chain(m.domain.pr_findings(pr).iter().map(|f| Some(f.id.clone()))).collect();
    (stops, if m.on_ready_line() { None } else { m.ui.selected_finding.clone() })
}

/// j/k and the paging keys: a cursor with somewhere to go, a page with more to scroll, or a page to load.
pub fn can_move(m: &Model) -> bool {
    match m.top_modal() {
        Some(Modal::Finding { .. } | Modal::Help { .. } | Modal::Publication { .. }) => {
            crate::view::page_scroll_limit(m) > 0
        }
        Some(_) => false,
        None => match (m.ui.screen, m.ui.focus) {
            (Screen::Inbox, _) | (Screen::Review, Pane::Sidebar) => {
                let ids: Vec<PrId> = m.inbox().iter().map(|p| p.id.clone()).collect();
                walks(&ids, m.ui.selected_pr.as_ref())
            }
            (Screen::Review, Pane::Main) => {
                let (stops, at) = finding_cursor(m);
                walks(&stops, Some(&at))
            }
            // A next page is somewhere to go too: j on the last loaded file asks for it.
            (Screen::Diff, Pane::Sidebar) => m.diff.as_ref().is_some_and(|d| {
                let paths: Vec<&Text> = d.files.iter().map(|f| &f.path).collect();
                walks(&paths, d.selected_file.as_ref().as_ref()) || d.next_cursor.is_some() && d.pending.is_none()
            }),
            (Screen::Diff, Pane::Main) => {
                m.diff.as_ref().and_then(|d| d.file()).is_some_and(|f| file_rows(f).len() > 1)
            }
            (Screen::Timeline, _) => m.timeline.as_ref().is_some_and(|t| {
                walks(&timeline_stops(m, t), t.selected.as_ref()) || t.next_before.is_some() && t.pending.is_none()
            }),
            (Screen::Settings, _) => m.domain.accounts.as_ref().is_some_and(|a| a.len() > 1),
        },
    }
}

/// The finding under the Diff cursor: the one stacked at its line, else one anchored to the file as a whole.
fn diff_cursor_finding(m: &Model) -> Option<FindingId> {
    let d = m.diff.as_ref()?;
    let file = d.file()?;
    match file_rows(file).get(d.cursor) {
        Some(row @ DiffRow::Line(_)) => {
            row_findings(&diff_findings(m, d), file, row).get(d.stack).map(|f| f.id.clone())
        }
        _ => diff_findings(m, d).into_iter().find(|f| f.file == file.path && f.line.is_none()).map(|f| f.id.clone()),
    }
}

/// Enter: read the selection, open the code pane, fold a timeline row or select an account. On the Inbox and in a
/// side pane it moves into the page, so it needs something to show there.
pub fn can_open(m: &Model) -> bool {
    if m.top_modal().is_some() {
        return false;
    }
    match (m.ui.screen, m.ui.focus) {
        (Screen::Inbox, _) | (Screen::Review, Pane::Sidebar) => m.selected_pr().is_some(),
        (Screen::Review, Pane::Main) => m.on_ready_line() || m.ui.selected_finding.is_some(),
        (Screen::Diff, Pane::Sidebar) => m.diff.as_ref().and_then(|d| d.file()).is_some(),
        (Screen::Diff, Pane::Main) => diff_cursor_finding(m).is_some(),
        (Screen::Timeline, _) => {
            m.timeline.as_ref().is_some_and(|t| t.selected.is_some_and(|s| timeline_stops(m, t).contains(&s)))
        }
        (Screen::Settings, _) => m.cursor_account().is_some_and(|a| m.account_block(a).is_none()),
    }
}

/// Esc: closes the top panel, folds the open timeline row, clears the diff search, or goes back to the Review page.
/// The Review page and the Inbox are where Esc ends.
pub fn can_back(m: &Model) -> bool {
    m.top_modal().is_some() || matches!(m.ui.screen, Screen::Diff | Screen::Timeline | Screen::Settings)
}

/// h, l and Tab: the Review page's PR list and findings, the Diff's files and code. Another screen has one pane.
pub fn can_focus(m: &Model, pane: Pane) -> bool {
    if m.top_modal().is_some() || m.ui.focus == pane {
        return false;
    }
    match (m.ui.screen, pane) {
        (Screen::Review, Pane::Sidebar) => !m.domain.prs.is_empty(),
        (Screen::Review, Pane::Main) => m.selected_pr().is_some(),
        (Screen::Diff, Pane::Sidebar) => m.diff.as_ref().is_some_and(|d| !d.files.is_empty()),
        (Screen::Diff, Pane::Main) => m.diff.as_ref().and_then(|d| d.file()).is_some(),
        _ => false,
    }
}

/// `/`: searches what the screen lists, so there must be something listed.
pub fn can_search(m: &Model) -> bool {
    m.top_modal().is_none()
        && match m.ui.screen {
            Screen::Inbox | Screen::Review => !m.domain.prs.is_empty(),
            Screen::Diff => can_jump(m),
            Screen::Timeline => can_filter_timeline(m),
            Screen::Settings => false,
        }
}

/// `:` jumps to a path in the diff on screen.
pub fn can_jump(m: &Model) -> bool {
    m.diff.as_ref().is_some_and(|d| !d.files.is_empty())
}

/// f, v, P and `/` on the Timeline: something to narrow. Its other scopes are parts of the PR's, so a PR with no
/// events has nothing in any scope.
pub fn can_filter_timeline(m: &Model) -> bool {
    m.timeline.as_ref().is_some_and(|t| !timeline_shown(m, t).is_empty())
}

/// v: some kind in the cycle keeps rows that the others do not, so there are rows to narrow with any kind.
pub fn can_cycle_kind(m: &Model) -> bool {
    m.timeline.as_ref().is_some_and(|t| !timeline_rows(m, &TimelineView { category: None, ..t.clone() }).is_empty())
}

/// P: as v, for the phase.
pub fn can_cycle_phase(m: &Model) -> bool {
    m.timeline.as_ref().is_some_and(|t| !timeline_rows(m, &TimelineView { phase: None, ..t.clone() }).is_empty())
}

/// J/K: the open card is longer than the page.
pub fn can_scroll_doc(m: &Model) -> bool {
    m.top_modal().is_none() && crate::view::doc_scroll_limit(m) > 0
}

/// n/N: the next finding in the reader; on the Diff, the next match of its search or the next anchored finding;
/// on the Review page, the next finding as j does.
pub fn can_turn(m: &Model) -> bool {
    match (m.top_modal(), m.ui.screen) {
        (Some(Modal::Finding { id, .. }), _) => {
            m.domain.findings.get(id).is_some_and(|f| m.domain.pr_findings(&f.pr_id).len() > 1)
        }
        (Some(_), _) => false,
        (None, Screen::Review) => {
            let (stops, at) = finding_cursor(m);
            stops.iter().any(Option::is_some) && (m.ui.focus == Pane::Sidebar || walks(&stops, Some(&at)))
        }
        // A jump also moves into the code pane.
        (None, Screen::Diff) => m.diff.as_ref().is_some_and(|d| {
            let here = diff_position(d);
            let side = m.ui.focus == Pane::Sidebar;
            match &d.search {
                Some(_) => walks(&d.matches, Some(&(here.0, here.1))) || side && !d.matches.is_empty(),
                None => {
                    let stops = finding_stops(m, d);
                    walks(&stops, Some(&here)) || side && !stops.is_empty()
                }
            }
        }),
        (None, _) => false,
    }
}

/// (file index, row, place in the line's stack) of the Diff cursor.
fn diff_position(d: &DiffView) -> (usize, usize, usize) {
    (d.selected_file.as_ref().and_then(|p| d.files.iter().position(|f| &f.path == p)).unwrap_or(0), d.cursor, d.stack)
}

/// The revision `d` opens: the finding in the reader or under the Review cursor, at its run's revision; else the
/// selected PR at its head.
fn diff_target(m: &Model) -> Option<(DiffKey, Option<Finding>)> {
    let modal = match m.top_modal() {
        Some(Modal::Finding { id, .. }) => Some(id.clone()),
        _ => None,
    };
    let review = (m.ui.screen == Screen::Review && m.ui.focus != Pane::Sidebar && m.top_modal().is_none())
        .then(|| m.ui.selected_finding.clone());
    let finding = modal.or(review.flatten()).and_then(|id| m.domain.findings.get(&id).cloned());
    if let Some(f) = finding
        && let Some(key) = finding_diff_key(m, &f.id)
    {
        return Some((key, Some(f)));
    }
    let pr = m.selected_pr()?;
    Some((DiffKey { pr: pr.id.clone(), base: pr.base_sha.clone(), head: pr.head_sha.clone() }, None))
}

/// d: a diff already loaded, or one to ask the daemon for. Whether it has one for a head it never reviewed is its to
/// say, and the Diff says what it answered.
pub fn can_diff(m: &Model) -> bool {
    let Some((key, _)) = diff_target(m) else { return false };
    let loaded = m.diff.as_ref().is_some_and(|d| d.key == key && d.error.is_none() && !d.files.is_empty());
    loaded || m.link.welcomed()
}

/// t: the selected PR's events, which the daemon sends.
pub fn can_timeline(m: &Model) -> bool {
    m.selected_pr().is_some() && m.link.welcomed()
}

/// r: asks the daemon for a fresh snapshot, so the client must be talking to one.
pub fn can_refresh(m: &Model) -> bool {
    matches!(m.link, Link::Live | Link::Stalled { .. })
}

/// The review `w` moves to: parked ones first, because the human is the bottleneck, then the running ones; pressed
/// again on the one it shows, the next. With nothing parked it starts at the review the status line names.
fn watch_target(m: &Model) -> Option<PrId> {
    let mut running: Vec<(bool, u64, PrId)> = m
        .domain
        .runs
        .values()
        .filter(|r| r.status == RunStatus::Running)
        .filter_map(|r| Some((!r.parked(), m.domain.prs.get(&r.pr_id)?.number, r.pr_id.clone())))
        .collect();
    running.sort();
    running.dedup();
    let parked = running.first().is_some_and(|r| !r.0);
    let active: Vec<&PrId> = running.iter().filter(|r| r.0).map(|r| &r.2).collect();
    let newest = m
        .domain
        .activity
        .iter()
        .rev()
        .filter(|_| !parked)
        .find_map(|a| active.iter().find(|id| a.pr_id.as_ref() == Some(**id)).copied());
    let ids: Vec<&PrId> = running.iter().map(|r| &r.2).collect();
    let at = ids.iter().position(|id| Some(*id) == m.ui.selected_pr.as_ref());
    match at {
        Some(i) if watching(m) => Some(ids[(i + 1) % ids.len()].clone()),
        Some(i) => Some(ids[i].clone()),
        None => newest.or(ids.first().copied()).cloned(),
    }
}

/// The Review page shows the selected PR's card with nothing over it.
fn watching(m: &Model) -> bool {
    m.ui.screen == Screen::Review && m.top_modal().is_none()
}

/// w: a review to move to that is not the one already in front of the reader.
pub fn can_watch(m: &Model) -> bool {
    watch_target(m).is_some_and(|t| {
        !(watching(m) && m.ui.focus == Pane::Main && m.ui.pr_filter.is_empty() && m.ui.selected_pr == Some(t))
    })
}

/// s, R, c and p on the selected PR: the daemon grants it, and the PR's latest run is in the state the command acts on.
pub fn can_mutate(m: &Model, mu: Mutation) -> bool {
    let Some(p) = m.selected_pr() else { return false };
    let run = m.domain.latest_run(&p.id);
    write_block(m, mu.capability()).is_none()
        && match mu {
            Mutation::Start => parked_head(m, p).is_none(),
            Mutation::Retry => run.is_some_and(|r| r.status == RunStatus::Failed),
            Mutation::Cancel => run.is_some_and(|r| r.status == RunStatus::Running),
            Mutation::Publish => run.is_some_and(|r| r.phase == Phase::AwaitingApproval),
        }
}

/// y: the panel on top asks, and for a preview, it is still the one to publish.
pub fn can_confirm(m: &Model) -> bool {
    match m.top_modal() {
        Some(Modal::Confirm { state, .. }) => matches!(state, ConfirmState::Asking | ConfirmState::TimedOut),
        Some(Modal::Publication { preview: Preview::Ready(p), publish, .. }) => {
            matches!(publish, ConfirmState::Asking | ConfirmState::TimedOut) && publish_block(m, p).is_none()
        }
        _ => false,
    }
}

/// Why `capability` cannot be sent now, in the words the help page and the notices use; `None` when it can.
pub fn write_block(m: &Model, capability: &str) -> Option<&'static str> {
    if !m.link.accepts_events() {
        Some("the client is not live")
    } else if m.may_write(capability) {
        None
    } else {
        Some(m.server.as_ref().and_then(|s| s.commands_off()).unwrap_or("the daemon does not grant it"))
    }
}

/// `s`, `R`, `c` and `p` on the selected PR: a confirmation panel naming the exact target, or a notice saying why not.
fn mutation(m: &mut Model, mu: Mutation, fx: &mut Vec<Effect>) {
    if !can_mutate(m, mu) {
        refuse(m, mu);
        return;
    }
    let Some(p) = m.selected_pr().cloned() else { return };
    let Some(r) = m.domain.latest_run(&p.id).cloned().filter(|_| mu != Mutation::Start) else {
        let prompt = format!("Start a review of #{} at {}?", p.number, p.head_sha.short());
        confirm(m, prompt, Command::Start(StartParams { pr_id: p.id, expected_head_sha: p.head_sha }));
        return;
    };
    let (prompt, command) = match mu {
        Mutation::Retry => {
            let next = r.attempt.map_or(String::new(), |n| format!(" (attempt {})", n + 1));
            (format!("Retry the failed review of #{}{next}?", p.number), Command::Retry(RunParams { run_id: r.id }))
        }
        Mutation::Cancel => {
            let what = if r.phase == Phase::AwaitingApproval { "parked" } else { "running" };
            (format!("Cancel the {what} review of #{}?", p.number), Command::Cancel(RunParams { run_id: r.id }))
        }
        _ => return open_publication(m, r, fx),
    };
    confirm(m, prompt, command);
}

/// Why `mu` does nothing now, in one sentence: in a notice, or for a start the client knows the daemon refuses, in
/// the panel the start would have opened, marked as never sent.
fn refuse(m: &mut Model, mu: Mutation) {
    let verb = match mu {
        Mutation::Start => "start",
        Mutation::Retry => "retry",
        Mutation::Cancel => "cancel",
        Mutation::Publish => "publish",
    };
    if let Some(why) = write_block(m, mu.capability()) {
        return notice(m, format!("Can't {verb}: {why}"));
    }
    let Some(p) = m.selected_pr().cloned() else {
        return notice(m, format!("Can't {verb}: no PR is selected"));
    };
    if let Some(run) = parked_head(m, &p).filter(|_| mu == Mutation::Start) {
        let message = Text::new(&start_refusal(m, run));
        m.ui.modals.push(Modal::Confirm {
            prompt: format!("Start a review of #{} at {}?", p.number, p.head_sha.short()),
            command: Command::Start(StartParams { pr_id: p.id, expected_head_sha: p.head_sha }),
            state: ConfirmState::Refused { code: None, message },
        });
        return;
    }
    notice(
        m,
        match mu {
            Mutation::Retry => format!("Can't retry #{}: its latest run has not failed", p.number),
            Mutation::Cancel => format!("Can't cancel #{}: no review is running on it", p.number),
            _ => format!("Nothing to publish on #{}", p.number),
        },
    );
}

/// The publication page for a parked run: its stored payload, and the diff its comments sit on.
fn open_publication(m: &mut Model, r: Run, fx: &mut Vec<Effect>) {
    m.ui.modals.retain(|x| !matches!(x, Modal::Publication { .. }));
    m.ui.modals.push(Modal::Publication {
        run_id: r.id.clone(),
        scroll: 0,
        preview: Preview::Failed(Text::new(OFFLINE)),
        publish: ConfirmState::Asking,
        raw: false,
    });
    fetch_preview(m, r.id, fx);
    load_diff(m, DiffKey { pr: r.pr_id, base: r.base_sha, head: r.head_sha }, None, fx);
}

/// Why `y` on this preview would not publish it, or `None` when it would. A preview is stale once its run left
/// AWAITING_APPROVAL or the PR has a newer head than the one it was written for.
pub fn publish_block(m: &Model, p: &PublicationPreview) -> Option<String> {
    if let Some(why) = write_block(m, "review.publish") {
        return Some(why.to_string());
    }
    let Some(run) = m.domain.runs.get(&p.run_id) else { return Some("the run is no longer tracked".into()) };
    if run.phase != Phase::AwaitingApproval {
        return Some(format!(
            "stale: the run is now {}, no longer waiting for approval",
            crate::theme::phase_name(run.phase)
        ));
    }
    let head = m.domain.prs.get(&run.pr_id).map_or(&run.head_sha, |pr| &pr.head_sha);
    if *head != p.head_sha || run.head_sha != p.head_sha {
        return Some(format!("stale: the PR head is now {}; this preview is for {}", head.short(), p.head_sha.short()));
    }
    None
}

/// Opens the generic confirmation modal; `y` inside it sends `command`.
pub fn confirm(m: &mut Model, prompt: String, command: Command) {
    m.ui.modals.push(Modal::Confirm { prompt, command, state: ConfirmState::Asking });
}

/// The review's GitHub URL once it is published, and only an `https://github.com/` one.
pub fn published_url(m: &Model, run: &RunId) -> Option<String> {
    let p = m.domain.publications.get(run).filter(|p| p.status == PublicationStatus::Published)?;
    p.url.as_ref().map(|u| u.to_string()).filter(|u| u.starts_with("https://github.com/"))
}

fn send_confirmed(m: &mut Model, fx: &mut Vec<Effect>) {
    if !can_confirm(m) {
        return;
    }
    let command = match m.top_modal().cloned() {
        Some(Modal::Confirm { command, .. }) => command,
        Some(Modal::Publication { run_id, preview: Preview::Ready(p), .. }) => Command::Publish(PublishParams {
            run_id,
            preview_id: p.preview_id.clone(),
            expected_head_sha: p.head_sha.clone(),
        }),
        _ => return,
    };
    let idempotency_key = command.idempotency_key();
    let verb = command.verb();
    let Some(request_id) =
        send(m, fx, |request_id| ClientMessage::Command(CommandRequest { request_id, idempotency_key, command }))
    else {
        return;
    };
    m.commands_sent.insert(request_id.clone(), verb);
    if let Some(Modal::Confirm { state, .. } | Modal::Publication { publish: state, .. }) = m.ui.modals.last_mut() {
        *state = ConfirmState::Requested(request_id);
    }
}

fn edit_input(m: &mut Model, f: impl FnOnce(&mut String)) {
    if let Some(Modal::Input { purpose, buffer, .. }) = m.ui.modals.last_mut() {
        f(buffer);
        match purpose {
            InputPurpose::Search => {
                m.ui.pr_filter = buffer.clone();
                reselect(m);
            }
            InputPurpose::TimelineFilter => {
                if let Some(t) = &mut m.timeline {
                    t.text = buffer.clone();
                }
            }
            InputPurpose::JumpTo | InputPurpose::DiffSearch => {}
        }
    }
}

fn goto(m: &mut Model, s: Screen, fx: &mut Vec<Effect>) {
    m.ui.modals.clear();
    m.ui.screen = s;
    match s {
        Screen::Diff => {
            let have =
                m.diff.as_ref().is_some_and(|d| Some(&d.key.pr) == m.ui.selected_pr.as_ref() && d.error.is_none());
            if !have && let Some(pr) = m.selected_pr() {
                let key = DiffKey { pr: pr.id.clone(), base: pr.base_sha.clone(), head: pr.head_sha.clone() };
                load_diff(m, key, None, fx);
            }
        }
        Screen::Timeline => {
            let have = m.timeline.as_ref().is_some_and(|t| t.query.pr == m.ui.selected_pr && t.error.is_none());
            if !have && m.ui.selected_pr.is_some() {
                let q = timeline_query(m, TimelineScope::Pr);
                load_timeline(m, TimelineScope::Pr, q, fx);
            }
        }
        Screen::Inbox | Screen::Review | Screen::Settings => {}
    }
}

fn focus(m: &mut Model, pane: Pane) {
    if can_focus(m, pane) {
        m.ui.focus = pane;
    }
}

fn move_by(m: &mut Model, delta: isize, fx: &mut Vec<Effect>) {
    if !can_move(m) {
        return;
    }
    if let Some(top) = m.ui.modals.last_mut() {
        if let Modal::Finding { scroll, .. } | Modal::Help { scroll } | Modal::Publication { scroll, .. } = top {
            *scroll = (*scroll as isize + delta).max(0).min(u16::MAX as isize) as u16;
        }
        clamp_page(m);
        return;
    }
    match (m.ui.screen, m.ui.focus) {
        (Screen::Inbox, _) | (Screen::Review, Pane::Sidebar) => {
            let ids: Vec<PrId> = m.inbox().iter().map(|p| p.id.clone()).collect();
            let next = step(&ids, m.ui.selected_pr.as_ref(), delta);
            if next != m.ui.selected_pr {
                m.ui.selected_pr = next;
                m.ui.selected_finding = None;
                m.ui.doc_scroll = 0;
                reselect(m);
            }
        }
        (Screen::Review, Pane::Main) => {
            let parked = m.parked_run().map(|r| r.id.clone());
            let (ids, current) = finding_cursor(m);
            if let Some(next) = step(&ids, Some(&current), delta)
                && next != current
            {
                m.ui.ready_line = if next.is_none() { parked } else { None };
                m.ui.selected_finding = next;
                m.ui.doc_scroll = 0;
            }
        }
        (Screen::Diff, focus) => {
            let Some(d) = &mut m.diff else { return };
            if focus == Pane::Sidebar {
                let paths: Vec<Text> = d.files.iter().map(|f| f.path.clone()).collect();
                let next = step(&paths, d.selected_file.as_ref(), delta);
                if next != d.selected_file {
                    d.selected_file = next;
                    d.cursor = 0;
                }
                let last = d.files.last().map(|f| &f.path) == d.selected_file.as_ref();
                if last {
                    load_more_diff(m, fx);
                }
            } else if let Some(rows) = d.file().map(|f| file_rows(f).len()) {
                d.stack = 0;
                d.cursor = (d.cursor as isize + delta).clamp(0, rows.saturating_sub(1) as isize) as usize;
                let at_end = d.cursor + 1 >= rows && d.files.last().map(|f| &f.path) == d.selected_file.as_ref();
                if at_end {
                    load_more_diff(m, fx);
                }
            }
        }
        (Screen::Timeline, _) => {
            let Some(t) = &m.timeline else { return };
            let seqs = timeline_stops(m, t);
            let next = step(&seqs, t.selected.as_ref(), delta);
            // Older events sit at the top: reaching it, or a page with no row to stop on, pages them in.
            let at_top = next == seqs.first().copied();
            let more = t.next_before.filter(|_| at_top && t.pending.is_none());
            let (scope, query) = (t.scope, t.query.clone());
            if let Some(t) = &mut m.timeline {
                t.selected = next;
            }
            if let Some(before) = more {
                let pending = send(m, fx, |request_id| ClientMessage::TimelineGet {
                    request_id,
                    pr_id: query.pr,
                    run_id: query.run,
                    finding_id: query.finding,
                    before_sequence: Some(before),
                    limit: Limit::DEFAULT,
                });
                if let Some(t) = &mut m.timeline
                    && pending.is_some()
                {
                    t.pending = pending;
                    t.scope = scope;
                }
            }
        }
        (Screen::Settings, _) => {
            let ids: Vec<AccountId> = m.domain.accounts.iter().flatten().map(|a| a.id.clone()).collect();
            let current = m.cursor_account().map(|a| a.id.clone());
            m.ui.selected_account = step(&ids, current.as_ref(), delta);
        }
    }
}

/// The top page's content is on its way, after a snapshot or on first open: its placeholder is shorter than what
/// comes back, so only the reader's own moves clamp to it.
fn page_reloading(m: &Model) -> bool {
    match m.top_modal() {
        Some(Modal::Publication { preview, .. }) => matches!(preview, Preview::Loading(_)),
        Some(Modal::Finding { id, .. }) => !m.details.contains_key(id) && !m.detail_errors.contains_key(id),
        _ => false,
    }
}

/// Keeps the top page's scroll within its content, which changes as details, previews and sizes arrive.
fn clamp_page(m: &mut Model) {
    let limit = crate::view::page_scroll_limit(m);
    if let Some(Modal::Finding { scroll, .. } | Modal::Help { scroll } | Modal::Publication { scroll, .. }) =
        m.ui.modals.last_mut()
    {
        *scroll = (*scroll).min(limit);
    }
}

fn open(m: &mut Model, fx: &mut Vec<Effect>) {
    // On Settings an account that cannot be selected says why.
    if m.ui.screen == Screen::Settings && m.top_modal().is_none() {
        return select_account(m);
    }
    if !can_open(m) {
        return;
    }
    match (m.ui.screen, m.ui.focus) {
        (Screen::Inbox, _) => {
            m.ui.screen = Screen::Review;
            m.ui.focus = Pane::Main;
        }
        (Screen::Review, Pane::Sidebar) => m.ui.focus = Pane::Main,
        (Screen::Review, _) => {
            if let Some(r) = m.parked_run().filter(|_| m.on_ready_line()).cloned() {
                open_publication(m, r, fx);
            } else if let Some(id) = m.ui.selected_finding.clone() {
                open_finding(m, id, fx);
            }
        }
        (Screen::Diff, Pane::Sidebar) => m.ui.focus = Pane::Main,
        (Screen::Diff, _) => {
            if let Some(id) = diff_cursor_finding(m) {
                open_finding(m, id, fx);
            }
        }
        (Screen::Timeline, _) => {
            let Some(t) = &m.timeline else { return };
            let Some(seq) = t.selected else { return };
            let finding = t.events.iter().find(|e| e.sequence.get() == seq).and_then(|e| match &e.body {
                EventBody::FindingUpdated(f) => Some(f.id.clone()),
                _ => None,
            });
            // Enter shows the detail under the row; on an open finding event it reads the finding, else it folds.
            match (t.open == Some(seq), finding) {
                (true, Some(id)) => open_finding(m, id, fx),
                (open, finding) => {
                    if let Some(t) = &mut m.timeline {
                        t.open = (!open).then_some(seq);
                    }
                    // The open detail quotes the finding's explanation.
                    if let Some(id) = finding.filter(|id| !open && !m.details.contains_key(id)) {
                        fetch_detail(m, &id, fx);
                    }
                }
            }
        }
        (Screen::Settings, _) => select_account(m),
    }
}

fn select_account(m: &mut Model) {
    let Some(a) = m.cursor_account().cloned() else {
        notice(m, "No GitHub accounts to select");
        return;
    };
    if let Some(why) = m.account_block(&a) {
        notice(m, format!("Can't select {}: {why}", a.id));
        return;
    }
    let prompt = format!("Use {} on {} for GitHub reads and publishing?", a.login, a.host);
    confirm(m, prompt, Command::SelectAccount(AccountParams { account_id: a.id }));
}

/// `w`: the review that needs the reader, one key from anywhere; see `watch_target`.
fn watch(m: &mut Model) {
    if !can_watch(m) {
        if watch_target(m).is_none() {
            notice(m, "No review is running");
        }
        return;
    }
    let Some(next) = watch_target(m) else { return };
    m.ui.modals.clear();
    m.ui.pr_filter.clear();
    m.ui.screen = Screen::Review;
    m.ui.focus = Pane::Main;
    if m.ui.selected_pr.as_ref() != Some(&next) {
        m.ui.selected_pr = Some(next);
        m.ui.selected_finding = None;
        m.ui.doc_scroll = 0;
    }
    reselect(m);
}

fn open_finding(m: &mut Model, id: FindingId, fx: &mut Vec<Effect>) {
    if !m.details.contains_key(&id) {
        fetch_detail(m, &id, fx);
    }
    prefetch_diff(m, &id, fx);
    m.ui.modals.push(Modal::Finding { id, scroll: 0 });
}

/// The diff for the finding in focus (modal, Review list), pinned to that finding's own revision; else the PR's head.
fn open_diff(m: &mut Model, fx: &mut Vec<Effect>) {
    if !can_diff(m) {
        let why =
            if m.selected_pr().is_none() { "No PR selected" } else { "Can't open the diff: the client is not live" };
        return notice(m, why);
    }
    let Some((key, finding)) = diff_target(m) else { return };
    m.ui.modals.clear();
    match finding {
        Some(f) => {
            m.ui.selected_pr = Some(f.pr_id.clone());
            m.ui.selected_finding = Some(f.id.clone());
            m.ui.screen = Screen::Diff;
            load_diff(m, key, Some((f.file, f.line.map(|l| l.get()))), fx);
        }
        None => goto(m, Screen::Diff, fx),
    }
}

fn next_finding(m: &mut Model, dir: isize, fx: &mut Vec<Effect>) {
    // In the reader, n and N turn the page to the PR's next or previous finding, and the document follows.
    if let Some(Modal::Finding { id, .. }) = m.top_modal() {
        let id = id.clone();
        let Some(pr) = m.domain.findings.get(&id).map(|f| f.pr_id.clone()) else { return };
        let ids: Vec<FindingId> = m.domain.pr_findings(&pr).iter().map(|f| f.id.clone()).collect();
        if let Some(next) = step(&ids, Some(&id), dir).filter(|n| *n != id) {
            m.ui.modals.pop();
            m.ui.selected_finding = Some(next.clone());
            m.ui.doc_scroll = 0;
            open_finding(m, next, fx);
        }
        return;
    }
    match m.ui.screen {
        Screen::Review if can_turn(m) => {
            m.ui.focus = Pane::Main;
            move_by(m, dir, fx);
        }
        Screen::Diff if m.diff.as_ref().is_some_and(|d| d.search.is_some()) => walk_matches(m, dir, false),
        Screen::Diff => {
            let Some(d) = &m.diff else { return };
            let stops = finding_stops(m, d);
            if stops.is_empty() {
                notice(m, "No findings anchored to this revision");
                return;
            }
            let here = (
                d.selected_file.as_ref().and_then(|p| d.files.iter().position(|f| &f.path == p)).unwrap_or(0),
                d.cursor,
                d.stack,
            );
            let target = if dir > 0 {
                stops.iter().find(|s| **s > here).or(stops.first())
            } else {
                stops.iter().rev().find(|s| **s < here).or(stops.last())
            };
            let &(fi, row, k) = target.unwrap_or(&stops[0]);
            if let Some(d) = &mut m.diff {
                d.selected_file = Some(d.files[fi].path.clone());
                d.cursor = row;
                d.stack = k;
            }
            m.ui.focus = Pane::Main;
        }
        _ => {}
    }
}

/// (file index, row) of every diff line whose text contains `query`, ignoring case, in diff order.
pub fn diff_matches(d: &DiffView, query: &str) -> Vec<(usize, usize)> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for (fi, f) in d.files.iter().enumerate() {
        for (ri, row) in file_rows(f).iter().enumerate() {
            if matches!(row, DiffRow::Line(l) if l.text.as_str().to_lowercase().contains(&q)) {
                out.push((fi, ri));
            }
        }
    }
    out
}

fn diff_search(m: &mut Model, query: String) {
    if let Some(d) = &mut m.diff {
        d.search = Some(query).filter(|q| !q.is_empty());
        d.matches = d.search.as_ref().map_or(vec![], |q| diff_matches(d, q));
    }
    walk_matches(m, 1, true);
}

/// Moves the diff cursor to the next (`dir` > 0) or previous match of its search, wrapping; `here` counts the line
/// under the cursor.
fn walk_matches(m: &mut Model, dir: isize, here: bool) {
    let Some(d) = &mut m.diff else { return };
    let Some(q) = d.search.clone() else { return };
    let stops = d.matches.clone();
    let at = (d.selected_file.as_ref().and_then(|p| d.files.iter().position(|f| &f.path == p)).unwrap_or(0), d.cursor);
    let target = if dir > 0 {
        stops.iter().find(|s| **s > at || (here && **s == at)).or(stops.first())
    } else {
        stops.iter().rev().find(|s| **s < at).or(stops.last())
    };
    let Some(&(fi, row)) = target else {
        notice(m, format!("No line in this diff contains \"{q}\""));
        return;
    };
    d.selected_file = Some(d.files[fi].path.clone());
    d.cursor = row;
    d.stack = 0;
    m.ui.focus = Pane::Main;
}

fn jump_to(m: &mut Model, spec: &str, fx: &mut Vec<Effect>) {
    let spec = spec.trim();
    let (path, line) = match spec.rsplit_once(':') {
        Some((p, l)) if l.parse::<u32>().is_ok() => (p, l.parse().ok()),
        _ => (spec, None),
    };
    if let Some(d) = &mut m.diff {
        let path = match (path, &d.selected_file) {
            ("", Some(current)) => current.clone(),
            _ => Text::new(path),
        };
        d.anchor = Some((path, line));
    }
    resolve_anchor(m, fx);
}
