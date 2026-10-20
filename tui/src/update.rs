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
            if m.ui.screen == Screen::Settings || !m.link.accepts_events() || running {
                m.dirty = true;
            }
            return fx;
        }
        Msg::Resize(w, h) => m.size = (w, h),
        Msg::Key(k) => {
            if let Some(a) = keymap::resolve(m.ui.screen, m.top_modal(), k) {
                act(m, a, &mut fx);
            }
        }
        Msg::Mouse(e) => mouse(m, e, &mut fx),
        Msg::Net(e) => net(m, e, &mut fx),
    }
    prefetch(m, &mut fx);
    m.dirty = true;
    fx
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
        if let Some(state) = waiting_confirm(m, &id) {
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

fn notice(m: &mut Model, text: impl Into<String>) {
    m.notice = Some(Notice { text: text.into(), until_ms: m.now_ms + NOTICE_MS });
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
            if let Some(state) = waiting_confirm(m, &r.request_id) {
                *state = match r.outcome {
                    CommandOutcome::Accepted { operation_id } => ConfirmState::Accepted(operation_id),
                    CommandOutcome::Rejected { code, message } => {
                        ConfirmState::Failed(Text::new(&format!("{code}: {message}")))
                    }
                };
                return;
            }
            let text = match r.outcome {
                CommandOutcome::Accepted { operation_id } => {
                    format!("request accepted ({operation_id}); waiting for events")
                }
                CommandOutcome::Rejected { code, message } => format!("rejected {code}: {message}"),
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
                notice(m, format!("server error {code}: {message}"));
                return;
            };
            if !fail_request(m, &id, message.clone()) {
                notice(m, format!("server error {code}: {message}"));
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
            notice(m, format!("{wanted} is not tracked by the reviewer"));
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
        Some(Modal::Publication { run_id, preview: Preview::Loading(_) | Preview::Failed(_), .. }) => {
            fetch_preview(m, run_id, fx)
        }
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
            matches!(state, ConfirmState::Requested(_) | ConfirmState::Accepted(_) | ConfirmState::TimedOut)
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
            !matches!(state, ConfirmState::Asking | ConfirmState::Failed(_)) && shows(command)
        }
        _ => false,
    };
    let Some(i) = m.ui.modals.iter().position(done) else { return };
    let Modal::Confirm { command, .. } = m.ui.modals.remove(i) else { return };
    let what = match &e.body {
        EventBody::Run(RunChange::Started, r) => format!("run {} started at {}", r.id.short(), r.head_sha.short()),
        EventBody::Run(RunChange::Failed, r) => {
            let why = r.failure_reason.as_ref().map_or(String::new(), |x| format!(": {x}"));
            format!("run {} failed{why}", r.id.short())
        }
        EventBody::Run(RunChange::Superseded, r) => format!("run {} superseded by a newer head", r.id.short()),
        EventBody::Run(RunChange::Completed, r) => format!("run {} completed", r.id.short()),
        EventBody::PhaseChanged(c) => {
            format!("run {} is now {}", c.run_id.short(), crate::theme::phase_name(c.to))
        }
        _ => return,
    };
    notice(m, format!("{} done: {what}", command.name()));
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
    let keep = m.ui.selected_finding.as_ref().is_some_and(|id| findings.iter().any(|f| &f.id == id));
    if !keep {
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
                notice(m, format!("{path} is not in this diff"));
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
    let (category, phase, text) =
        m.timeline.as_ref().map_or((None, None, String::new()), |t| (t.category, t.phase, t.text.clone()));
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

/// The phase each event happened in: the `from` of the next later phase change of its run, else the run's phase now.
pub fn event_phases(m: &Model, events: &[SequencedEvent]) -> Vec<Option<Phase>> {
    let mut later_from: std::collections::BTreeMap<&RunId, Phase> = Default::default();
    events
        .iter()
        .map(|e| {
            let run = e.run_id.as_ref()?;
            if let EventBody::PhaseChanged(p) = &e.body {
                later_from.insert(run, p.from);
                return Some(p.to);
            }
            later_from.get(run).copied().or_else(|| m.domain.runs.get(run).map(|r| r.phase))
        })
        .collect()
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
pub fn timeline_rows<'a>(m: &Model, t: &'a TimelineView) -> Vec<(&'a SequencedEvent, Option<Phase>)> {
    let phases = event_phases(m, &t.events);
    let text = t.text.to_lowercase();
    let mut rows: Vec<_> = t
        .events
        .iter()
        .zip(phases)
        .filter(|(e, p)| {
            t.category.is_none_or(|c| EventCategory::of(&e.body) == c)
                && t.phase.is_none_or(|want| *p == Some(want))
                && (text.is_empty() || event_text(e).to_lowercase().contains(&text))
        })
        .collect();
    rows.reverse();
    rows
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
        Action::Back => {
            if let Some(t) = m.timeline.as_mut().filter(|t| m.ui.screen == Screen::Timeline && t.open.is_some())
                && m.ui.modals.is_empty()
            {
                t.open = None;
            } else if let Some(d) = m.diff.as_mut().filter(|d| m.ui.screen == Screen::Diff && d.search.is_some())
                && m.ui.modals.is_empty()
            {
                d.search = None;
            } else if m.ui.modals.pop().is_none() {
                m.ui.screen = match m.ui.screen {
                    Screen::Diff | Screen::Timeline => Screen::Review,
                    _ => Screen::Inbox,
                };
            }
        }
        Action::Goto(s) => goto(m, s, fx),
        Action::Refresh => {
            if m.link == Link::Live {
                resync(m, "manual refresh", fx);
                notice(m, "refreshing snapshot");
            } else {
                notice(m, "not connected; the client reconnects on its own");
            }
        }
        Action::Up => move_by(m, -1, fx),
        Action::Down => move_by(m, 1, fx),
        Action::PageUp => move_by(m, -page(m), fx),
        Action::PageDown => move_by(m, page(m), fx),
        Action::Top => move_by(m, isize::MIN / 2, fx),
        Action::Bottom => move_by(m, isize::MAX / 2, fx),
        Action::Left => m.ui.focus = Pane::Sidebar,
        Action::Right => m.ui.focus = Pane::Main,
        // Two panes: either way round, Tab moves to the other one.
        Action::FocusNext | Action::FocusPrev => {
            m.ui.focus = if m.ui.focus == Pane::Main { Pane::Sidebar } else { Pane::Main }
        }
        Action::Open => open(m, fx),
        // `/` searches what the screen shows: the diff's text, the timeline's events, else the PR list.
        Action::Search => {
            let (purpose, before) = match (m.ui.screen, &m.timeline, &m.diff) {
                (Screen::Diff, _, Some(d)) => (InputPurpose::DiffSearch, d.search.clone().unwrap_or_default()),
                (Screen::Timeline, Some(t), _) => (InputPurpose::TimelineFilter, t.text.clone()),
                _ => (InputPurpose::Search, m.ui.pr_filter.clone()),
            };
            m.ui.modals.push(Modal::Input { purpose, buffer: before.clone(), before });
        }
        Action::JumpTo => m.ui.modals.push(Modal::Input {
            purpose: InputPurpose::JumpTo,
            buffer: String::new(),
            before: String::new(),
        }),
        Action::OpenDiff => open_diff(m, fx),
        Action::Watch => watch(m),
        Action::ScrollDown => m.ui.doc_scroll = (m.ui.doc_scroll + 1).min(crate::view::doc_scroll_limit(m)),
        Action::ScrollUp => m.ui.doc_scroll = (m.ui.doc_scroll - 1).max(0),
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
        Action::Accounts => {
            goto(m, Screen::Settings, fx);
            if m.domain.accounts.is_none() {
                notice(m, "this daemon reports no GitHub accounts");
            }
        }
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
        (Some(Modal::Finding { .. } | Modal::Help { .. }), _) => move_by(m, delta, fx),
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
    let cap = mu.capability();
    if let Some(why) = write_block(m, cap) {
        notice(m, format!("{cap} unavailable: {why}"));
        return;
    }
    let Some(p) = m.selected_pr().cloned() else {
        notice(m, format!("{cap} unavailable: no PR selected"));
        return;
    };
    let run = m.domain.latest_run(&p.id).cloned();
    let (prompt, command) = match (mu, run) {
        (Mutation::Start, _) => (
            format!("Start a review of #{} at {}?", p.number, p.head_sha.short()),
            Command::Start(StartParams { pr_id: p.id, expected_head_sha: p.head_sha }),
        ),
        (Mutation::Retry, Some(r)) if r.status == RunStatus::Failed => {
            (format!("Retry run {}?", r.id.short()), Command::Retry(RunParams { run_id: r.id }))
        }
        (Mutation::Cancel, Some(r)) if r.status == RunStatus::Running => {
            let what = if r.phase == Phase::AwaitingApproval { "parked" } else { "running" };
            (format!("Cancel the {what} review of #{}?", p.number), Command::Cancel(RunParams { run_id: r.id }))
        }
        (Mutation::Publish, Some(r)) if r.phase == Phase::AwaitingApproval => {
            open_publication(m, r, fx);
            return;
        }
        (mu, _) => {
            let why = match mu {
                Mutation::Retry => "its latest run has not failed",
                Mutation::Cancel => "no review is running on it",
                _ => "no review is awaiting approval on it",
            };
            notice(m, format!("{cap} unavailable for #{}: {why}", p.number));
            return;
        }
    };
    confirm(m, cap, prompt, command);
}

/// The publication page for a parked run: its stored payload, and the diff its comments sit on.
fn open_publication(m: &mut Model, r: Run, fx: &mut Vec<Effect>) {
    m.ui.modals.retain(|x| !matches!(x, Modal::Publication { .. }));
    m.ui.modals.push(Modal::Publication {
        run_id: r.id.clone(),
        scroll: 0,
        preview: Preview::Failed(Text::new(OFFLINE)),
        publish: ConfirmState::Asking,
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
            "stale: the run is now {}, no longer awaiting approval",
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
pub fn confirm(m: &mut Model, title: &str, prompt: String, command: Command) {
    m.ui.modals.push(Modal::Confirm { title: title.into(), prompt, command, state: ConfirmState::Asking });
}

/// The review's GitHub URL once it is published, and only an `https://github.com/` one.
pub fn published_url(m: &Model, run: &RunId) -> Option<String> {
    let p = m.domain.publications.get(run).filter(|p| p.status == PublicationStatus::Published)?;
    p.url.as_ref().map(|u| u.to_string()).filter(|u| u.starts_with("https://github.com/"))
}

fn send_confirmed(m: &mut Model, fx: &mut Vec<Effect>) {
    let command = match m.top_modal().cloned() {
        Some(Modal::Confirm { command, state: ConfirmState::Asking | ConfirmState::TimedOut, .. }) => command,
        Some(Modal::Publication {
            run_id,
            preview: Preview::Ready(p),
            publish: ConfirmState::Asking | ConfirmState::TimedOut,
            ..
        }) if publish_block(m, &p).is_none() => Command::Publish(PublishParams {
            run_id,
            preview_id: p.preview_id.clone(),
            expected_head_sha: p.head_sha.clone(),
        }),
        _ => return,
    };
    let idempotency_key = command.idempotency_key();
    let Some(request_id) =
        send(m, fx, |request_id| ClientMessage::Command(CommandRequest { request_id, idempotency_key, command }))
    else {
        return;
    };
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

fn move_by(m: &mut Model, delta: isize, fx: &mut Vec<Effect>) {
    let help_limit = crate::view::help_scroll_limit(m);
    if let Some(top) = m.ui.modals.last_mut() {
        match top {
            Modal::Finding { scroll, .. } => *scroll = (*scroll as isize + delta).clamp(0, u16::MAX as isize) as u16,
            Modal::Help { scroll } => *scroll = (*scroll as isize + delta).clamp(0, help_limit as isize) as u16,
            Modal::Publication { scroll, .. } => {
                *scroll = (*scroll as isize + delta).clamp(0, u16::MAX as isize) as u16
            }
            Modal::Input { .. } | Modal::Confirm { .. } => {}
        }
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
            let Some(pr) = m.ui.selected_pr.clone() else { return };
            let ids: Vec<FindingId> = m.domain.pr_findings(&pr).iter().map(|f| f.id.clone()).collect();
            let next = step(&ids, m.ui.selected_finding.as_ref(), delta);
            if next != m.ui.selected_finding {
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
            let seqs: Vec<u64> = timeline_rows(m, t).iter().map(|(e, _)| e.sequence.get()).collect();
            let next = step(&seqs, t.selected.as_ref(), delta);
            // Older events sit at the top: reaching it pages them in.
            let at_top = next.is_some() && next == seqs.first().copied();
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

fn open(m: &mut Model, fx: &mut Vec<Effect>) {
    if m.top_modal().is_some() {
        return;
    }
    match (m.ui.screen, m.ui.focus) {
        (Screen::Inbox, _) => {
            m.ui.screen = Screen::Review;
            m.ui.focus = Pane::Main;
        }
        (Screen::Review, Pane::Sidebar) => m.ui.focus = Pane::Main,
        (Screen::Review, _) => {
            if let Some(id) = m.ui.selected_finding.clone() {
                open_finding(m, id, fx);
            }
        }
        (Screen::Diff, Pane::Sidebar) => m.ui.focus = Pane::Main,
        (Screen::Diff, _) => {
            let Some(d) = &m.diff else { return };
            let Some(file) = d.file() else { return };
            let rows = file_rows(file);
            let hit = match rows.get(d.cursor) {
                Some(row @ DiffRow::Line(_)) => {
                    row_findings(&diff_findings(m, d), file, row).get(d.stack).map(|f| f.id.clone())
                }
                _ => diff_findings(m, d)
                    .into_iter()
                    .find(|f| f.file == file.path && f.line.is_none())
                    .map(|f| f.id.clone()),
            };
            if let Some(id) = hit {
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
        notice(m, "no GitHub accounts to select");
        return;
    };
    if let Some(why) = m.account_block(&a) {
        notice(m, format!("{} cannot be selected: {why}", a.id));
        return;
    }
    let prompt = format!("Use {} on {} for GitHub reads and publishing?", a.login, a.host);
    confirm(m, "account.select", prompt, Command::SelectAccount(AccountParams { account_id: a.id }));
}

/// `w`: the running review's document, one key from anywhere. Pressed again, it moves on to the next running PR.
fn watch(m: &mut Model) {
    // Running reviews first, then the parked ones waiting for approval.
    let mut running: Vec<(bool, u64, PrId)> = m
        .domain
        .runs
        .values()
        .filter(|r| r.status == RunStatus::Running)
        .filter_map(|r| Some((r.parked(), m.domain.prs.get(&r.pr_id)?.number, r.pr_id.clone())))
        .collect();
    running.sort();
    let active: Vec<PrId> = running.iter().filter(|r| !r.0).map(|r| r.2.clone()).collect();
    let running: Vec<(u64, PrId)> = running.into_iter().map(|(_, n, id)| (n, id)).collect();
    if running.is_empty() {
        notice(m, "no review is running");
        return;
    }
    // First the review the status line names: the running PR with the newest activity.
    let newest = m
        .domain
        .activity
        .iter()
        .rev()
        .find_map(|a| running.iter().find(|r| a.pr_id.as_ref() == Some(&r.1) && active.contains(&r.1)));
    let first = newest.unwrap_or(&running[0]).1.clone();
    let at = running.iter().position(|r| Some(&r.1) == m.ui.selected_pr.as_ref());
    let watching = m.ui.screen == Screen::Review && m.top_modal().is_none();
    let next = match at {
        Some(i) if watching => running[(i + 1) % running.len()].1.clone(),
        Some(i) => running[i].1.clone(),
        None => first,
    };
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
    let modal_finding = match m.top_modal() {
        Some(Modal::Finding { id, .. }) => Some(id.clone()),
        _ => None,
    };
    let finding = modal_finding
        .or_else(|| {
            (m.ui.screen == Screen::Review && m.ui.focus != Pane::Sidebar).then(|| m.ui.selected_finding.clone())?
        })
        .and_then(|id| m.domain.findings.get(&id).cloned());
    m.ui.modals.clear();
    match finding.and_then(|f| Some((finding_diff_key(m, &f.id)?, f))) {
        Some((key, f)) => {
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
        Screen::Review => {
            m.ui.focus = Pane::Main;
            move_by(m, dir, fx);
        }
        Screen::Diff if m.diff.as_ref().is_some_and(|d| d.search.is_some()) => walk_matches(m, dir, false),
        Screen::Diff => {
            let Some(d) = &m.diff else { return };
            let stops = finding_stops(m, d);
            if stops.is_empty() {
                notice(m, "no findings anchored to this revision");
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
    }
    walk_matches(m, 1, true);
}

/// Moves the diff cursor to the next (`dir` > 0) or previous match of its search, wrapping; `here` counts the line
/// under the cursor.
fn walk_matches(m: &mut Model, dir: isize, here: bool) {
    let Some(d) = &mut m.diff else { return };
    let Some(q) = d.search.clone() else { return };
    let stops = diff_matches(d, &q);
    let at = (d.selected_file.as_ref().and_then(|p| d.files.iter().position(|f| &f.path == p)).unwrap_or(0), d.cursor);
    let target = if dir > 0 {
        stops.iter().find(|s| **s > at || (here && **s == at)).or(stops.first())
    } else {
        stops.iter().rev().find(|s| **s < at).or(stops.last())
    };
    let Some(&(fi, row)) = target else {
        notice(m, format!("no line in this diff contains \"{q}\""));
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
