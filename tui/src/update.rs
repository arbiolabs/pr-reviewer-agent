//! The reducer: `update(&mut Model, Msg) -> Vec<Effect>`. All protocol recovery rules live here; the network task is a
//! dumb pipe and the view is a pure function of the model.

use std::collections::VecDeque;

use crossterm::event::KeyEvent;

use crate::keymap::{self, Action, Mutation};
use crate::model::*;
use crate::protocol::*;

pub const NOTICE_MS: u64 = 4_000;

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
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
            // Ages on the banner and the diagnostics screen tick.
            if m.ui.screen == Screen::Settings || !m.link.accepts_events() {
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
        Msg::Net(e) => net(m, e, &mut fx),
    }
    m.dirty = true;
    fx
}

fn request(m: &mut Model) -> RequestId {
    let id = RequestId::new(&format!("req-{}", m.next_request));
    m.next_request += 1;
    id
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
        NetEvent::Garbled(err) => m.diag.error(m.now_ms, format!("unreadable server message: {err}")),
        NetEvent::Lost { reason, retry_in_ms } => {
            m.diag.error(m.now_ms, format!("connection lost: {reason}"));
            transition(m, LinkInput::Lost { reason: Text::new(&reason), retry_in_ms });
            for t in m.timeline.iter_mut() {
                t.pending = None;
            }
            if let Some(d) = &mut m.diff {
                d.pending = None;
            }
        }
    }
}

fn resync(m: &mut Model, reason: &str, fx: &mut Vec<Effect>) {
    if transition(m, LinkInput::StreamBroken) {
        m.diag.resyncs += 1;
        tracing::info!(reason, "resync");
        let request_id = request(m);
        fx.push(Effect::Send(ClientMessage::SnapshotGet { request_id }));
    }
}

fn server_message(m: &mut Model, msg: ServerMessage, fx: &mut Vec<Effect>) {
    match msg {
        ServerMessage::Welcome { protocol_version, server_version, session_id, capabilities, stream_epoch, .. } => {
            if protocol_version != PROTOCOL_VERSION {
                let message = Text::new(&format!("server speaks protocol {protocol_version}"));
                if transition(m, LinkInput::Mismatch { supported: vec![protocol_version], message }) {
                    fx.push(Effect::Close);
                }
                return;
            }
            m.server = Some(ServerInfo { protocol_version, server_version, session_id, capabilities });
            let resuming = m.stream.epoch.as_ref() == Some(&stream_epoch);
            if !transition(m, LinkInput::Welcomed { resuming }) {
                return;
            }
            let request_id = request(m);
            fx.push(Effect::Send(match (resuming, m.stream.epoch.clone()) {
                (true, Some(stream_epoch)) => {
                    ClientMessage::Resume { request_id, stream_epoch, after_sequence: m.stream.last_applied }
                }
                _ => ClientMessage::SnapshotGet { request_id },
            }));
        }
        ServerMessage::Snapshot(s) => apply_snapshot(m, s, fx),
        ServerMessage::Event(Event::Heartbeat { .. }) => m.diag.last_heartbeat_ms = Some(m.now_ms),
        ServerMessage::Event(Event::Sequenced(e)) => apply_event(m, e, fx),
        ServerMessage::ResyncRequired { reason, .. } => {
            m.diag.error(m.now_ms, format!("resync required: {reason}"));
            if m.link == Link::Live {
                resync(m, reason.as_str(), fx);
            } else if m.link == Link::Syncing || m.link == Link::Negotiating {
                // A rejected resume: fall back to a full snapshot.
                let request_id = request(m);
                m.link = Link::Syncing;
                m.diag.resyncs += 1;
                fx.push(Effect::Send(ClientMessage::SnapshotGet { request_id }));
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
        ServerMessage::CommandResult(r) => {
            let text = match r.outcome {
                CommandOutcome::Accepted { operation_id } => {
                    format!("request accepted ({operation_id}); waiting for events")
                }
                CommandOutcome::Rejected { code, message } => format!("rejected {code:?}: {message}"),
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
            m.diag.error(m.now_ms, format!("{code:?}: {message}"));
            let Some(id) = request_id else {
                notice(m, format!("server error {code:?}: {message}"));
                return;
            };
            if let Some(f) = m.detail_requests.remove(&id) {
                m.detail_errors.insert(f, message);
            } else if let Some(d) = m.diff.as_mut().filter(|d| d.pending.as_ref() == Some(&id)) {
                d.pending = None;
                d.error = Some(message);
            } else if let Some(t) = m.timeline.as_mut().filter(|t| t.pending.as_ref() == Some(&id)) {
                t.pending = None;
                t.error = Some(message);
            } else {
                notice(m, format!("server error {code:?}: {message}"));
            }
        }
    }
}

fn apply_snapshot(m: &mut Model, s: Snapshot, fx: &mut Vec<Effect>) {
    if !transition(m, LinkInput::SnapshotApplied) {
        return;
    }
    m.stream = StreamPos { epoch: Some(s.stream_epoch), last_applied: s.sequence };
    m.diag.last_snapshot_ms = Some(m.now_ms);
    m.domain.prs = s.prs.into_iter().map(|p| (p.id.clone(), p)).collect();
    m.domain.runs = s.runs.into_iter().map(|r| (r.id.clone(), r)).collect();
    m.domain.findings = s.findings.into_iter().map(|f| (f.id.clone(), f)).collect();
    m.domain.touched.retain(|id, _| m.domain.prs.contains_key(id));
    m.details.clear();
    if let Some(wanted) = m.ui.wanted_pr.take() {
        if m.domain.prs.contains_key(&wanted) {
            m.ui.selected_pr = Some(wanted);
            m.ui.screen = Screen::Review;
        } else {
            notice(m, format!("{wanted} is not tracked by the reviewer"));
        }
    }
    reselect(m);
    if let Some(t) = &m.timeline {
        let (scope, query) = (t.scope, t.query.clone());
        load_timeline(m, scope, query, fx);
    }
    if let Some(Modal::Finding { id, .. }) = m.top_modal().cloned() {
        fetch_detail(m, &id, fx);
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
        EventBody::Other(_) => None,
    };
    let pr = match &e.body {
        EventBody::PrUpdated(p) => Some(p.id.clone()),
        EventBody::Run(_, r) => Some(r.pr_id.clone()),
        EventBody::FindingUpdated(f) => Some(f.pr_id.clone()),
        _ => e.pr_id.clone(),
    };
    if let Some(pr) = &pr {
        d.touched.insert(pr.clone(), seq);
    }
    if let Some(kind) = kind {
        d.push_activity(Activity { sequence: seq, at: e.occurred_at.clone(), pr_id: pr, kind });
    }
    if let EventBody::FindingUpdated(f) = &e.body
        && matches!(m.top_modal(), Some(Modal::Finding { id, .. }) if id == &f.id)
    {
        fetch_detail(m, &f.id.clone(), fx);
    }
    reselect(m);
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
    if m.detail_requests.values().any(|f| f == id) || !m.link.connected() {
        return;
    }
    let request_id = request(m);
    m.detail_requests.insert(request_id.clone(), id.clone());
    fx.push(Effect::Send(ClientMessage::FindingGet { request_id, finding_id: id.clone() }));
}

// ---- diff ----

fn load_diff(m: &mut Model, key: DiffKey, anchor: Option<(Text, Option<u32>)>, fx: &mut Vec<Effect>) {
    m.ui.screen = Screen::Diff;
    if let Some(d) = m.diff.as_mut().filter(|d| d.key == key && d.error.is_none()) {
        if anchor.is_some() {
            d.anchor = anchor;
        }
        resolve_anchor(m, fx);
        return;
    }
    let request_id = request(m);
    fx.push(Effect::Send(ClientMessage::DiffGet {
        request_id: request_id.clone(),
        pr_id: key.pr.clone(),
        base_sha: key.base.clone(),
        head_sha: key.head.clone(),
        file: None,
        cursor: None,
        limit: Some(Limit::DEFAULT),
    }));
    m.diff = Some(DiffView {
        key,
        files: Vec::new(),
        next_cursor: None,
        truncated: false,
        pending: Some(request_id),
        error: None,
        selected_file: None,
        cursor: 0,
        anchor,
    });
}

fn load_more_diff(m: &mut Model, fx: &mut Vec<Effect>) {
    let Some(d) = &m.diff else { return };
    let (Some(cursor), None) = (d.next_cursor.clone(), &d.pending) else { return };
    let key = d.key.clone();
    let request_id = request(m);
    fx.push(Effect::Send(ClientMessage::DiffGet {
        request_id: request_id.clone(),
        pr_id: key.pr,
        base_sha: key.base,
        head_sha: key.head,
        file: None,
        cursor: Some(cursor),
        limit: Some(Limit::DEFAULT),
    }));
    if let Some(d) = &mut m.diff {
        d.pending = Some(request_id);
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

fn resolve_anchor(m: &mut Model, fx: &mut Vec<Effect>) {
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
            d.selected_file = Some(d.files[i].path.clone());
            d.anchor = None;
            m.ui.focus = Pane::Main;
        }
        None if d.next_cursor.is_some() => load_more_diff(m, fx),
        None if d.pending.is_some() => {}
        None => {
            d.anchor = None;
            notice(m, format!("{path} is not in this diff"));
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

/// (file index, row) of each anchored finding in diff order.
fn finding_stops(m: &Model, d: &DiffView) -> Vec<(usize, usize)> {
    let mut stops: Vec<_> = diff_findings(m, d)
        .into_iter()
        .filter_map(|f| {
            let fi = d.files.iter().position(|file| file.path == f.file)?;
            Some((fi, f.line.map_or(0, |l| row_for_line(&d.files[fi], l.get()))))
        })
        .collect();
    stops.sort();
    stops.dedup();
    stops
}

// ---- timeline ----

fn load_timeline(m: &mut Model, scope: TimelineScope, query: TimelineQuery, fx: &mut Vec<Effect>) {
    let (category, phase) = m.timeline.as_ref().map_or((None, None), |t| (t.category, t.phase));
    let request_id = request(m);
    fx.push(Effect::Send(ClientMessage::TimelineGet {
        request_id: request_id.clone(),
        pr_id: query.pr.clone(),
        run_id: query.run.clone(),
        finding_id: query.finding.clone(),
        before_sequence: None,
        limit: Limit::DEFAULT,
    }));
    m.timeline = Some(TimelineView {
        scope,
        query,
        events: Vec::new(),
        next_before: None,
        pending: Some(request_id),
        error: None,
        selected: None,
        category,
        phase,
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

/// The events the timeline shows after its category and phase filters, with their inferred phases.
pub fn timeline_rows<'a>(m: &Model, t: &'a TimelineView) -> Vec<(&'a SequencedEvent, Option<Phase>)> {
    let phases = event_phases(m, &t.events);
    t.events
        .iter()
        .zip(phases)
        .filter(|(e, p)| {
            t.category.is_none_or(|c| EventCategory::of(&e.body) == c) && t.phase.is_none_or(|want| *p == Some(want))
        })
        .collect()
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
            if m.top_modal() == Some(&Modal::Help) {
                m.ui.modals.pop();
            } else {
                m.ui.modals.push(Modal::Help);
            }
        }
        Action::Back => {
            if m.ui.modals.pop().is_none() {
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
        Action::Left => m.ui.focus = left_of(m.ui.focus),
        Action::Right => m.ui.focus = right_of(m.ui.focus, m.ui.screen),
        Action::FocusNext => {
            m.ui.focus = match right_of(m.ui.focus, m.ui.screen) {
                f if f == m.ui.focus => Pane::Sidebar,
                f => f,
            }
        }
        Action::FocusPrev => {
            m.ui.focus = match left_of(m.ui.focus) {
                f if f == m.ui.focus => right_of(right_of(f, m.ui.screen), m.ui.screen),
                f => f,
            }
        }
        Action::Open => open(m, fx),
        Action::Search => {
            if m.ui.screen == Screen::Diff {
                act(m, Action::JumpTo, fx);
            } else {
                let before = m.ui.pr_filter.clone();
                m.ui.modals.push(Modal::Input { purpose: InputPurpose::Search, buffer: before.clone(), before });
            }
        }
        Action::JumpTo => m.ui.modals.push(Modal::Input {
            purpose: InputPurpose::JumpTo,
            buffer: String::new(),
            before: String::new(),
        }),
        Action::OpenDiff => open_diff(m, fx),
        Action::OpenTimeline => {
            m.ui.modals.retain(|x| !matches!(x, Modal::Finding { .. }));
            let scope = if m.ui.screen == Screen::Review && m.ui.focus == Pane::Main && m.ui.selected_finding.is_some()
            {
                TimelineScope::Finding
            } else {
                TimelineScope::Pr
            };
            m.ui.screen = Screen::Timeline;
            let q = timeline_query(m, scope);
            load_timeline(m, scope, q, fx);
        }
        Action::NextFinding => next_finding(m, 1, fx),
        Action::PrevFinding => next_finding(m, -1, fx),
        Action::Mutation(mu) => mutation(m, mu),
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
        Action::InputCancel => {
            if let Some(Modal::Input { purpose: InputPurpose::Search, before, .. }) = m.ui.modals.pop() {
                m.ui.pr_filter = before;
                reselect(m);
            }
        }
        Action::InputSubmit => {
            if let Some(Modal::Input { purpose, buffer, .. }) = m.ui.modals.pop()
                && purpose == InputPurpose::JumpTo
            {
                jump_to(m, &buffer, fx);
            }
        }
    }
}

fn mutation(m: &mut Model, mu: Mutation) {
    let cap = mu.capability();
    let why = if !m.link.accepts_events() {
        "the client is not live"
    } else if !m.may_write(cap) {
        "the server grants no write capability (read-only)"
    } else {
        "this client version has no write commands yet"
    };
    notice(m, format!("{cap} unavailable: {why}"));
}

fn edit_input(m: &mut Model, f: impl FnOnce(&mut String)) {
    if let Some(Modal::Input { purpose, buffer, .. }) = m.ui.modals.last_mut() {
        f(buffer);
        if *purpose == InputPurpose::Search {
            m.ui.pr_filter = buffer.clone();
            reselect(m);
        }
    }
}

fn left_of(p: Pane) -> Pane {
    match p {
        Pane::Feed => Pane::Main,
        _ => Pane::Sidebar,
    }
}

fn right_of(p: Pane, screen: Screen) -> Pane {
    match (p, screen) {
        (Pane::Sidebar, _) => Pane::Main,
        (Pane::Main, Screen::Review) => Pane::Feed,
        (other, _) => other,
    }
}

fn goto(m: &mut Model, s: Screen, fx: &mut Vec<Effect>) {
    m.ui.modals.clear();
    m.ui.screen = s;
    match s {
        Screen::Diff => {
            let have = m.diff.as_ref().is_some_and(|d| Some(&d.key.pr) == m.ui.selected_pr.as_ref());
            if !have && let Some(pr) = m.selected_pr() {
                let key = DiffKey { pr: pr.id.clone(), base: pr.base_sha.clone(), head: pr.head_sha.clone() };
                load_diff(m, key, None, fx);
            }
        }
        Screen::Timeline => {
            let have = m.timeline.as_ref().is_some_and(|t| t.query.pr == m.ui.selected_pr);
            if !have && m.ui.selected_pr.is_some() {
                let q = timeline_query(m, TimelineScope::Pr);
                load_timeline(m, TimelineScope::Pr, q, fx);
            }
        }
        Screen::Inbox | Screen::Review | Screen::Settings => {}
    }
}

fn move_by(m: &mut Model, delta: isize, fx: &mut Vec<Effect>) {
    if let Some(top) = m.ui.modals.last_mut() {
        if let Modal::Finding { scroll, .. } = top {
            *scroll = (*scroll as isize + delta).clamp(0, u16::MAX as isize) as u16;
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
                m.ui.feed_scroll = 0;
                reselect(m);
            }
        }
        (Screen::Review, Pane::Main) => {
            let Some(pr) = m.ui.selected_pr.clone() else { return };
            let ids: Vec<FindingId> = m.domain.pr_findings(&pr).iter().map(|f| f.id.clone()).collect();
            m.ui.selected_finding = step(&ids, m.ui.selected_finding.as_ref(), delta);
        }
        (Screen::Review, Pane::Feed) => {
            m.ui.feed_scroll = (m.ui.feed_scroll as isize - delta).clamp(0, ACTIVITY_CAP as isize) as usize;
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
            let at_end = next.is_some() && next == seqs.last().copied();
            let more = t.next_before.filter(|_| at_end && t.pending.is_none());
            let (scope, query) = (t.scope, t.query.clone());
            if let Some(t) = &mut m.timeline {
                t.selected = next;
            }
            if let Some(before) = more {
                let request_id = request(m);
                fx.push(Effect::Send(ClientMessage::TimelineGet {
                    request_id: request_id.clone(),
                    pr_id: query.pr,
                    run_id: query.run,
                    finding_id: query.finding,
                    before_sequence: Some(before),
                    limit: Limit::DEFAULT,
                }));
                if let Some(t) = &mut m.timeline {
                    t.pending = Some(request_id);
                    t.scope = scope;
                }
            }
        }
        (Screen::Settings, _) => {}
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
            let new = match rows.get(d.cursor) {
                Some(DiffRow::Line(l)) => l.new,
                _ => None,
            };
            let hit = diff_findings(m, d)
                .into_iter()
                .find(|f| f.file == file.path && (f.line.map(|l| l.get()) == new || f.line.is_none() && d.cursor == 0))
                .map(|f| f.id.clone());
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
            match finding {
                Some(id) => open_finding(m, id, fx),
                None => m.ui.modals.push(Modal::Event { sequence: seq }),
            }
        }
        (Screen::Settings, _) => {}
    }
}

fn open_finding(m: &mut Model, id: FindingId, fx: &mut Vec<Effect>) {
    if !m.details.contains_key(&id) {
        fetch_detail(m, &id, fx);
    }
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
    match finding {
        Some(f) => {
            let run = m.domain.runs.get(&f.run_id);
            let pr = m.domain.prs.get(&f.pr_id);
            let (base, head) = match (run, pr) {
                (Some(r), _) => (r.base_sha.clone(), r.head_sha.clone()),
                (None, Some(p)) => (p.base_sha.clone(), p.head_sha.clone()),
                (None, None) => return,
            };
            m.ui.selected_pr = Some(f.pr_id.clone());
            m.ui.selected_finding = Some(f.id.clone());
            load_diff(m, DiffKey { pr: f.pr_id, base, head }, Some((f.file, f.line.map(|l| l.get()))), fx);
        }
        None => goto(m, Screen::Diff, fx),
    }
}

fn next_finding(m: &mut Model, dir: isize, fx: &mut Vec<Effect>) {
    match m.ui.screen {
        Screen::Review => {
            m.ui.focus = Pane::Main;
            move_by(m, dir, fx);
        }
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
            );
            let target = if dir > 0 {
                stops.iter().find(|s| **s > here).or(stops.first())
            } else {
                stops.iter().rev().find(|s| **s < here).or(stops.last())
            };
            let &(fi, row) = target.unwrap_or(&stops[0]);
            if let Some(d) = &mut m.diff {
                d.selected_file = Some(d.files[fi].path.clone());
                d.cursor = row;
            }
            m.ui.focus = Pane::Main;
        }
        _ => {}
    }
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
