//! Help, finding detail and event detail modals, and the Settings / diagnostics screen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::Ctx;
use crate::keymap::help_rows;
use crate::model::{ConfirmState, Link, Modal};
use crate::protocol::{EvidenceKind, FindingId};
use crate::theme::{Role, finding_status};
use crate::update::event_phases;

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

pub fn modal(c: &Ctx, f: &mut Frame, body: Rect, modal: &Modal) {
    let big = if body.width < 80 { body } else { centered(body, body.width * 9 / 10, (body.height * 9 / 10).max(12)) };
    match modal {
        Modal::Input { .. } => {}
        Modal::Confirm { title, prompt, command, state } => {
            let (status, role) = match state {
                ConfirmState::Asking => ("y confirm   n / Esc cancel".to_string(), Role::Title),
                ConfirmState::Requested(_) => ("requested... waiting for the server".to_string(), Role::Pending),
                ConfirmState::Accepted(op) => {
                    (format!("accepted ({op}); waiting for events to confirm it"), Role::Active)
                }
                ConfirmState::Failed(e) => (format!("rejected: {e}"), Role::Fail),
            };
            let lines = vec![
                Line::from(c.span(prompt.clone(), Role::Normal)),
                Line::from(c.span(format!("command {}", command.name()), Role::Muted)),
                Line::raw(""),
                Line::from(c.span(status, role)),
            ];
            draw(c, f, centered(body, 64, 6), &title.to_uppercase(), lines, 0);
        }
        Modal::Help => {
            let rows = help_rows();
            let two = body.width >= 80;
            let per_col = if two { rows.len().div_ceil(2) } else { rows.len() };
            let col = |i: usize| -> Vec<Span<'static>> {
                rows.get(i).map_or(vec![], |(keys, what)| {
                    vec![c.span(format!(" {}", c.pad(keys, 10)), Role::Title), c.span(c.pad(what, 28), Role::Normal)]
                })
            };
            let mut lines: Vec<Line> = (0..per_col)
                .map(|i| {
                    let mut spans = col(i);
                    if two {
                        spans.extend(col(i + per_col));
                    }
                    Line::from(spans)
                })
                .collect();
            lines.push(Line::from(c.span(" Write keys need a capability the server grants.", Role::Muted)));
            let area = centered(body, if two { 80 } else { 44 }, lines.len() as u16 + 2);
            draw(c, f, area, "HELP", lines, 0);
        }
        Modal::Finding { id, scroll } => draw(c, f, big, "FINDING", finding_lines(c, id), *scroll),
        Modal::Event { sequence } => {
            let Some(t) = &c.m.timeline else { return };
            let phases = event_phases(c.m, &t.events);
            let Some((e, phase)) = t.events.iter().zip(phases).find(|(e, _)| e.sequence.get() == *sequence) else {
                return;
            };
            let lines = super::timeline::detail_lines(c, e, phase, big.width.saturating_sub(2) as usize);
            draw(c, f, big, "EVENT", lines, 0);
        }
    }
}

fn draw(c: &Ctx, f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, scroll: u16) {
    let block = c.pane(title, true);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }).scroll((scroll, 0)), area);
}

fn finding_lines(c: &Ctx, id: &FindingId) -> Vec<Line<'static>> {
    let g = c.t.g();
    let sep = format!(" {} ", g.sep);
    let Some(f) = c.m.domain.findings.get(id) else {
        return vec![Line::from(c.span(format!("{id} is no longer in the snapshot."), Role::Muted))];
    };
    let (glyph, sev, sev_role) = c.t.severity(f.severity);
    let (status, status_role) = finding_status(f.status);
    let run = c.m.domain.runs.get(&f.run_id);
    let head = run.map_or("?".to_string(), |r| r.head_sha.short().to_string());
    let anchor = match f.line {
        Some(l) => format!("{}:{l}", f.file),
        None => format!("{} (whole file)", f.file),
    };
    let mut lines = vec![
        Line::from(vec![c.span(format!("{glyph} {sev} "), sev_role), c.span(f.title.to_string(), Role::Title)]),
        Line::from(vec![
            c.span(format!("{:?}", f.category).to_lowercase(), Role::Muted),
            c.span(sep.clone(), Role::Muted),
            c.span(status, status_role),
            c.span(
                format!("{sep}confidence {:.0}%{sep}{} evidence", f.confidence * 100.0, f.evidence_count),
                Role::Muted,
            ),
        ]),
        Line::from(c.span(
            format!("anchor {anchor} at head {head} (run {}){sep}fingerprint {}", f.run_id, f.fingerprint),
            Role::Muted,
        )),
    ];
    if f.stale {
        lines.push(Line::from(
            c.span("STALE: the PR head moved past this anchor; it is shown as found, not remapped.", Role::Pending),
        ));
    }
    lines.push(Line::raw(""));
    let Some(d) = c.m.details.get(id) else {
        let text = match c.m.detail_errors.get(id) {
            Some(e) => (format!("detail unavailable: {e}"), Role::Fail),
            None if c.m.link.connected() => ("loading detail...".to_string(), Role::Pending),
            None => ("detail not loaded; reconnect to fetch it".to_string(), Role::Muted),
        };
        lines.push(Line::from(c.span(text.0, text.1)));
        return lines;
    };
    let section = |lines: &mut Vec<Line<'static>>, title: &str, body: &str| {
        lines.push(Line::from(c.span(title.to_string(), Role::Title)));
        lines.extend(body.split('\n').map(|l| Line::from(c.span(format!("  {l}"), Role::Normal))));
        lines.push(Line::raw(""));
    };
    section(&mut lines, "Explanation", d.explanation.as_str());
    section(&mut lines, "Reasoning summary (model-written, not a confirmation)", d.reasoning_summary.as_str());
    if let Some(fix) = &d.suggested_fix {
        section(&mut lines, "Suggested fix", fix.as_str());
    }
    lines.push(Line::from(c.span(format!("Evidence ({})", d.evidence.len()), Role::Title)));
    for e in &d.evidence {
        let kind = match e.kind {
            EvidenceKind::CodeRef => "code",
            EvidenceKind::ToolSummary => "tool",
            EvidenceKind::TestResult => "test",
            EvidenceKind::Scenario => "scenario",
            EvidenceKind::Link => "link",
            EvidenceKind::Unknown => "other",
        };
        let mut at = e.path.as_ref().map_or(String::new(), |p| format!(" {p}"));
        match (e.start, e.end) {
            (Some(s), Some(end)) if end != s => at.push_str(&format!(":{s}-{end}")),
            (Some(s), _) => at.push_str(&format!(":{s}")),
            _ => {}
        }
        if let Some(sha) = &e.ref_sha {
            at.push_str(&format!(" @{}", sha.short()));
        }
        lines.push(Line::from(vec![c.span(format!("  [{kind}]"), Role::Active), c.span(at, Role::Muted)]));
        lines.extend(e.text.as_str().split('\n').map(|l| Line::from(c.span(format!("    {l}"), Role::Normal))));
    }
    if d.evidence.is_empty() {
        lines.push(Line::from(c.span("  none attached", Role::Muted)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(c.span("History", Role::Title)));
    for h in &d.history {
        let (word, role) = finding_status(h.disposition);
        lines.push(Line::from(vec![c.span(format!("  {} {} ", h.at, h.run_id), Role::Muted), c.span(word, role)]));
    }
    if d.history.is_empty() {
        lines.push(Line::from(c.span("  first seen in this run", Role::Muted)));
    }
    lines
}

pub fn settings(c: &Ctx, f: &mut Frame, area: Rect) {
    let m = c.m;
    let row = |k: &str, v: String, role: Role| {
        Line::from(vec![
            c.span(format!("  {k:<22}"), Role::Muted),
            c.span(c.fit(&v, (area.width as usize).saturating_sub(26)), role),
        ])
    };
    let age = |at: Option<u64>| {
        at.map_or("never".into(), |t| format!("{:.1}s ago", m.now_ms.saturating_sub(t) as f64 / 1000.0))
    };
    let (status, role) = super::link_status(c);
    let server = m.server.as_ref();
    let caps = server.map_or("-".into(), |s| {
        if s.capabilities.is_empty() {
            "none (read-only)".to_string()
        } else {
            s.capabilities.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        }
    });
    let mut lines = vec![
        Line::from(c.span(" CONNECTION", Role::Title)),
        row("endpoint", m.diag.endpoint.clone(), Role::Normal),
        row("state", status, role),
        row(
            "protocol",
            format!("client 1, server {}", server.map_or("-".into(), |s| s.protocol_version.to_string())),
            Role::Normal,
        ),
        row("server version", server.map_or("-".into(), |s| s.server_version.to_string()), Role::Normal),
        row("session", server.map_or("-".into(), |s| s.session_id.to_string()), Role::Normal),
        row("capabilities", caps, Role::Normal),
        Line::raw(""),
        Line::from(c.span(" DIAGNOSTICS", Role::Title)),
        row("stream epoch", m.stream.epoch.as_ref().map_or("-".into(), |e| e.to_string()), Role::Normal),
        row("last applied sequence", m.stream.last_applied.to_string(), Role::Normal),
        row("last snapshot", age(m.diag.last_snapshot_ms), Role::Normal),
        row("last heartbeat", age(m.diag.last_heartbeat_ms), Role::Normal),
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
        Line::raw(""),
        Line::from(c.span(" DISPLAY", Role::Title)),
        row(
            "glyphs / color",
            format!(
                "{} / {}",
                if m.look.ascii { "ascii" } else { "unicode" },
                if m.look.color { "color" } else { "monochrome" }
            ),
            Role::Normal,
        ),
        Line::raw(""),
        Line::from(c.span(" RECENT ERRORS", Role::Title)),
    ];
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
    f.render_widget(Paragraph::new(lines), area);
}
