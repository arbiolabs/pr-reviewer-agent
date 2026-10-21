//! The help page, the confirmation panel, and the Settings page. Like the finding reader they are pages in the reading
//! column, not boxes: section headings, muted keys, and a surface panel only where the user must answer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::Ctx;
use super::code::rich;
use super::doc::{column, filled};
use crate::keymap::{Context, help_rows};
use crate::model::{ConfirmState, Link, Modal};
use crate::protocol::{Account, AccountState};
use crate::theme::{Bg, Role};

pub fn modal(c: &Ctx, f: &mut Frame, body: Rect, modal: &Modal) {
    match modal {
        Modal::Input { .. } => {}
        Modal::Confirm { prompt, command, state } => confirm(c, f, body, prompt, command.name(), state),
        Modal::Help { scroll } | Modal::Finding { scroll, .. } | Modal::Publication { scroll, .. } => {
            scrolled(c, f, body, page_lines(c, body, modal).unwrap_or_default(), *scroll)
        }
    }
}

/// The lines of a scrolling page, the help, the finding reader or the publication preview, before scrolling.
pub fn page_lines(c: &Ctx, body: Rect, modal: &Modal) -> Option<Vec<Line<'static>>> {
    let w = column(body).width as usize;
    match modal {
        Modal::Help { .. } => Some(help_lines(c, body)),
        Modal::Finding { id, .. } => Some(super::doc::finding_lines(c, w, id)),
        Modal::Publication { run_id, preview, publish, raw, .. } => {
            Some(super::publish::lines(c, w, run_id, preview, publish, *raw))
        }
        Modal::Input { .. } | Modal::Confirm { .. } => None,
    }
}

/// A page over the body: cleared to the base background, content in the reading column.
fn page(c: &Ctx, f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new("").style(c.t.bg(Bg::Base)), area);
    f.render_widget(Paragraph::new(lines), column(area));
}

fn heading(c: &Ctx, text: &str) -> Line<'static> {
    Line::from(c.span(text.to_string(), Role::Focus))
}

/// The only place `y` acts: a page in the reading column, like the preview, asking the question in words, with the
/// daemon command in a dim detail line and the answer wrapped in full.
fn confirm(c: &Ctx, f: &mut Frame, body: Rect, prompt: &str, command: &str, state: &ConfirmState) {
    let (status, role) = match state {
        ConfirmState::Asking => (format!("y confirms {} n or Esc cancels", c.t.g().sep), Role::Title),
        ConfirmState::Requested(_) => (format!("requested{} waiting for the server", c.t.g().ellipsis), Role::Pending),
        ConfirmState::Accepted(_) => {
            ("accepted; waiting for the event that shows it happened".to_string(), Role::Active)
        }
        ConfirmState::Failed(e) => (format!("rejected: {e}"), Role::Fail),
        ConfirmState::TimedOut => ("request timed out; y sends it again, r refreshes".to_string(), Role::Fail),
    };
    let w = column(body).width as usize;
    let mut lines = vec![Line::raw("")];
    lines.extend(rich(c, prompt, w, Role::Title).into_iter().map(Line::from));
    lines.push(Line::from(c.span(c.fit(&format!("daemon command {command}"), w), Role::Muted)));
    lines.push(Line::raw(""));
    lines.extend(rich(c, &status, w, role).into_iter().map(Line::from));
    page(c, f, body, lines);
}

/// The largest scroll of a `len`-line page in `height` rows. An overflowing page keeps its last row for the cue.
pub fn page_limit(len: usize, height: usize) -> usize {
    if len <= height { 0 } else { len - height.saturating_sub(1) }
}

/// A page, scrolled, with a "more below" cue on its last row while it overflows.
fn scrolled(c: &Ctx, f: &mut Frame, body: Rect, mut lines: Vec<Line<'static>>, scroll: u16) {
    let (len, h) = (lines.len(), body.height as usize);
    if len > h {
        let start = (scroll as usize).min(page_limit(len, h));
        let end = (start + h - 1).min(len);
        lines = lines.drain(start..end).collect();
        let below = len - end;
        lines.push(if below > 0 {
            Line::from(c.span(format!("  {} {below} more below, j scrolls", c.t.g().ellipsis), Role::Muted))
        } else {
            Line::raw("")
        });
    }
    page(c, f, body, lines);
}

/// Every key by where it works, then what the glyphs mean.
pub fn help_lines(c: &Ctx, body: Rect) -> Vec<Line<'static>> {
    let g = c.t.g();
    let col = column(body);
    let rows: Vec<_> = help_rows().into_iter().filter(|r| r.0 != Context::ConfirmModal).collect();
    // Two columns from 100 columns up; below that one column, its key column as narrow as the longest keys.
    let two = body.width >= 100;
    let half = if two { col.width as usize / 2 } else { col.width as usize };
    let kw = if two { 11 } else { rows.iter().map(|r| crate::text::width(&r.1)).max().unwrap_or(0) + 2 };
    let section = |ctx: Context, name: &str| -> Vec<Line<'static>> {
        let mut out = vec![heading(c, name)];
        for (_, keys, what, capability) in rows.iter().filter(|r| r.0 == ctx) {
            // A write key the daemon does not grant shows dimmed and says so, even without color.
            let offered = capability
                .is_none_or(|cap| c.m.server.as_ref().is_none_or(|s| s.capabilities.iter().any(|x| x.as_str() == cap)));
            let (what, key_role, role) = match offered {
                true => (what.to_string(), Role::Accent, Role::Normal),
                false => {
                    // The short form fits a help column; the last lines carry the whole reason.
                    let why = c.m.server.as_ref().and_then(|s| s.commands_off()).unwrap_or("not offered");
                    let why = why.split(" (").next().unwrap_or(why);
                    (format!("{what} ({why})"), Role::Muted, Role::Muted)
                }
            };
            out.push(Line::from(vec![
                c.span(format!("  {}", c.pad(keys, kw)), key_role),
                c.span(c.fit(&what, half.saturating_sub(kw + 3)), role),
            ]));
        }
        out.push(Line::raw(""));
        out
    };
    let sections: Vec<Vec<Line<'static>>> = [
        (Context::Global, "Everywhere"),
        (Context::Review, "Review"),
        (Context::Diff, "Diff"),
        (Context::Timeline, "Timeline"),
        (Context::FindingModal, "Finding reader"),
        (Context::PublicationModal, "Publication preview"),
    ]
    .into_iter()
    .map(|(ctx, name)| section(ctx, name))
    .collect();
    // Two columns split between sections, at the boundary that ends the page highest.
    let total: usize = sections.iter().map(Vec::len).sum();
    let split = (0..=sections.len())
        .min_by_key(|&k| {
            let left: usize = sections[..k].iter().map(Vec::len).sum();
            left.max(total - left)
        })
        .unwrap_or(0);
    let (left, right): (Vec<_>, Vec<_>) =
        if two { (sections[..split].concat(), sections[split..].concat()) } else { (sections.concat(), Vec::new()) };
    let mut lines = vec![Line::raw(""), Line::from(c.span("Keys", Role::Title)), Line::raw("")];
    // Below 70 columns the subtitle would be cut; the page is the same without it.
    if body.width >= 70 {
        lines.insert(
            2,
            Line::from(c.span("Read from the keymap, so this page cannot drift from what the keys do.", Role::Muted)),
        );
    }
    if two {
        for i in 0..left.len().max(right.len()) {
            let mut spans: Vec<Span<'static>> = left.get(i).map_or(vec![], |l| l.spans.clone());
            spans = filled(spans, half, ratatui::style::Style::default());
            spans.extend(right.get(i).map_or(vec![], |l| l.spans.clone()));
            lines.push(Line::from(spans));
        }
    } else {
        lines.extend(left);
    }
    lines.push(heading(c, "Glyphs"));
    let mut sev = vec![Span::raw("  ")];
    for s in [
        crate::protocol::Severity::Critical,
        crate::protocol::Severity::High,
        crate::protocol::Severity::Medium,
        crate::protocol::Severity::Low,
    ] {
        let (gl, _, role) = c.t.severity(s);
        sev.push(c.span(format!("{gl} "), role));
        sev.push(c.span(format!("{}   ", super::doc::sev_word(s).to_lowercase()), Role::Normal));
    }
    lines.push(Line::from(sev));
    lines.push(Line::from(vec![
        // One still frame: a legend that animates reads as something happening.
        c.span(if c.t.ascii { "  - " } else { "  ▘ " }, Role::Accent),
        c.span("reviewing   ", Role::Normal),
        c.span(format!("{} ", g.pending), Role::Pending),
        c.span("waiting for you   ", Role::Normal),
        c.span(format!("{} ", g.ok), Role::Ok),
        c.span("clean   ", Role::Normal),
        c.span(format!("{} ", g.fail), Role::Fail),
        c.span("failed", Role::Normal),
    ]));
    lines.push(Line::from(vec![
        c.span(format!("  {}2 ", g.arrow), Role::Ok),
        c.span("findings published on GitHub", Role::Normal),
    ]));
    lines.push(Line::raw(""));
    lines.push(heading(c, "Mouse"));
    lines.push(Line::from(c.span("  Click a PR, finding, file, line or event to select it.", Role::Normal)));
    lines.push(Line::from(c.span("  Click it again to open it. Click a view in the footer.", Role::Normal)));
    lines.push(Line::from(c.span("  The wheel scrolls the page under it.", Role::Normal)));
    let off = c.m.server.as_ref().and_then(|s| s.commands_off());
    lines.push(Line::from(c.span(
        match off {
            Some(why) => format!("  Write keys are off: {why}."),
            None => "  Keys marked (not offered) are off on this daemon.".into(),
        },
        Role::Muted,
    )));
    lines
}

/// The GitHub identity the daemon uses.
fn identity(c: &Ctx) -> (String, Role) {
    use crate::model::Identity;
    match c.m.identity() {
        None => ("not reported by this daemon".into(), Role::Muted),
        Some(Identity::Env) => ("the daemon's GITHUB_TOKEN".into(), Role::Pending),
        Some(Identity::Selected(a)) if a.state != AccountState::Ok => (format!("{} (error)", a.login), Role::Fail),
        Some(Identity::Selected(a) | Identity::GhActive(a)) => (format!("{} on {}", a.login, a.host), Role::Normal),
        Some(Identity::Nobody) => ("nobody: run gh auth login".into(), Role::Pending),
    }
}

fn account_row(c: &Ctx, a: &Account, w: usize, cursor: bool) -> Line<'static> {
    let g = c.t.g();
    let st = c.t.bg(if cursor { Bg::Select } else { Bg::Base });
    let on = |r: Role| c.s(r).patch(st);
    let (state, state_role) = match a.state {
        AccountState::Ok => (format!("{} ok", g.ok), Role::Ok),
        AccountState::Error => (format!("{} error", g.fail), Role::Fail),
        AccountState::Unknown => ("? unknown".to_string(), Role::Muted),
    };
    let marks = match (a.selected, a.gh_active) {
        (true, true) => "selected, gh active",
        (true, false) => "selected",
        (false, true) => "gh active",
        (false, false) => "",
    };
    let block = c.m.account_block(a);
    let bar = if cursor { if c.t.ascii { ">" } else { "▌" } } else { " " };
    let mut spans = vec![
        Span::styled(format!("{bar} "), on(Role::Accent)),
        Span::styled(c.pad(a.login.as_str(), 16), on(if block.is_none() { Role::Title } else { Role::Normal })),
        Span::styled(c.pad(&format!(" {}", a.host), 12), on(Role::Muted)),
        Span::styled(c.pad(&format!(" {state}"), 10), on(state_role)),
        Span::styled(c.pad(&format!(" {marks}"), 20), on(Role::Active)),
    ];
    let used = 2 + 16 + 12 + 10 + 20;
    let (why, role) = match block {
        None => ("Enter selects".to_string(), Role::Accent),
        Some(why) => (why.to_string(), Role::Pending),
    };
    spans.push(Span::styled(c.fit(&format!(" {why}"), w.saturating_sub(used)), on(role)));
    Line::from(filled(spans, w, st))
}

pub fn settings(c: &Ctx, f: &mut Frame, area: Rect) {
    let m = c.m;
    let col = column(area);
    let w = col.width as usize;
    let tight = area.height < 30;
    let row = |k: &str, v: String, role: Role| {
        Line::from(vec![c.span(format!("  {k:<22}"), Role::Muted), c.span(c.fit(&v, w.saturating_sub(24)), role)])
    };
    let gap = |lines: &mut Vec<Line<'static>>| {
        if !tight {
            lines.push(Line::raw(""));
        }
    };
    let age = |at: Option<u64>| {
        at.map_or("never".into(), |t| format!("{:.1}s ago", m.now_ms.saturating_sub(t) as f64 / 1000.0))
    };
    let (mut status, role) = super::link_status(c);
    // The banner stays short, so the reason a connection failed lives here.
    if let Link::Disconnected { reason, .. } = &m.link {
        status = format!("{status}: {reason}");
    }
    let server = m.server.as_ref();
    let caps = server.map_or("-".into(), |s| {
        if s.capabilities.is_empty() {
            "none (read-only)".to_string()
        } else {
            s.capabilities.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        }
    });
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(c.span("Settings", Role::Title)));
    let state = if m.link.welcomed() { "connected" } else { "not connected" };
    let socket = format!("socket {} {} {state}", m.diag.endpoint, c.t.g().sep);
    lines.push(Line::from(c.span(c.fit(&socket, w), Role::Muted)));
    gap(&mut lines);
    lines.push(heading(c, "GitHub accounts"));
    let (who, who_role) = identity(c);
    lines.push(row("publishes as", who, who_role));
    let cursor = m.cursor_account().map(|a| &a.id);
    match &m.domain.accounts {
        None if m.diag.last_snapshot_ms.is_none() => {
            lines.push(row("accounts", "waiting for a snapshot".into(), Role::Muted))
        }
        None => lines.push(row("accounts", "not reported by this daemon version".into(), Role::Muted)),
        Some(accounts) if accounts.is_empty() => {
            lines.push(row("accounts", "none: run gh auth login, or set GITHUB_TOKEN".into(), Role::Pending))
        }
        Some(accounts) => lines.extend(accounts.iter().map(|a| account_row(c, a, w, Some(&a.id) == cursor))),
    }
    gap(&mut lines);
    lines.push(heading(c, "Connection"));
    lines.extend([
        row("state", status, role),
        row(
            "protocol",
            format!("client 1, server {}", server.map_or("-".into(), |s| s.protocol_version.to_string())),
            Role::Normal,
        ),
        row("server version", server.map_or("-".into(), |s| s.server_version.to_string()), Role::Normal),
        row("session", server.map_or("-".into(), |s| s.session_id.to_string()), Role::Normal),
        row("capabilities", caps, Role::Normal),
    ]);
    if let Some(s) = server {
        use crate::protocol::DaemonMode;
        let (mode, role) = match (s.mode, &s.read_only_reason) {
            (Some(DaemonMode::Engine), _) => ("engine: runs reviews and takes commands".to_string(), Role::Ok),
            (Some(DaemonMode::ReadOnly), Some(why)) => (format!("read-only: {why}"), Role::Pending),
            (Some(DaemonMode::ReadOnly), None) => ("read-only".to_string(), Role::Pending),
            (Some(DaemonMode::Unknown), _) => ("unknown to this client".to_string(), Role::Muted),
            (None, _) => ("not reported by this daemon version".to_string(), Role::Muted),
        };
        lines.push(row("mode", mode, role));
    }
    gap(&mut lines);
    lines.push(heading(c, "Stream"));
    lines.extend([
        row(
            "epoch, last sequence",
            format!("{}, {}", m.stream.epoch.as_ref().map_or("-".into(), |e| e.to_string()), m.stream.last_applied),
            Role::Normal,
        ),
        row(
            "snapshot, heartbeat",
            format!("{}, {}", age(m.diag.last_snapshot_ms), age(m.diag.last_heartbeat_ms)),
            Role::Normal,
        ),
        row("resyncs / reconnects", format!("{} / {}", m.diag.resyncs, m.diag.reconnects), Role::Normal),
        row(
            "activity",
            format!(
                "{} feed entries kept (cap 2000); {} tool events applied",
                m.domain.activity.len(),
                m.domain.tool_activity_count
            ),
            Role::Normal,
        ),
    ]);
    gap(&mut lines);
    lines.push(heading(c, "Display"));
    let look = m.look;
    lines.extend([
        row("glyphs", if look.ascii { "ascii" } else { "unicode" }.into(), Role::Normal),
        row("color", if look.color { "on" } else { "off" }.into(), Role::Normal),
        row("background", if look.terminal_bg { "terminal" } else { "painted (theme)" }.into(), Role::Normal),
    ]);
    gap(&mut lines);
    lines.push(heading(c, "Recent errors"));
    if let Link::Incompatible { message, .. } = &m.link {
        lines.push(row("incompatible", message.to_string(), Role::Fail));
    }
    let room = (area.height as usize).saturating_sub(lines.len());
    let errors: Vec<_> = m.diag.errors.iter().rev().take(room.max(1)).collect();
    if errors.is_empty() {
        lines.push(Line::from(c.span("  none", Role::Muted)));
    }
    for (at, e) in errors {
        lines.push(row(&age(Some(*at)), e.clone(), Role::Fail));
    }
    f.render_widget(Paragraph::new(lines), col);
}
