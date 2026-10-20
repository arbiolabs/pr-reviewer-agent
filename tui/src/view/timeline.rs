//! Timeline: the journal for a PR, run or finding, newest first, filterable by category and phase.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::lists::time_of;
use super::{Ctx, Width, window};
use crate::model::{EventCategory, Pane, TimelineScope};
use crate::protocol::{EventBody, Phase, PrId, RunChange, SequencedEvent};
use crate::text::{fit_len, pad_left, width};
use crate::theme::{Role, finding_status, phase_name};
use crate::update::timeline_rows;

pub fn actor(e: &SequencedEvent) -> &'static str {
    match e.body {
        EventBody::ToolActivity(_) => "agent",
        EventBody::PrUpdated(_) => "github",
        _ => "system",
    }
}

pub fn summary(c: &Ctx, e: &SequencedEvent) -> (String, Role) {
    let g = c.t.g();
    match &e.body {
        EventBody::PrUpdated(p) => (format!("PR #{} updated: head {}", p.number, p.head_sha.short()), Role::Normal),
        EventBody::Run(change, r) => {
            let what = match change {
                RunChange::Started => "run started",
                RunChange::Completed => "review completed",
                RunChange::Superseded => "run superseded",
                RunChange::Failed => "run failed",
            };
            let (glyph, word, role) = c.t.run_status(r.status);
            let reason = r.failure_reason.as_ref().map_or(String::new(), |x| format!(": {x}"));
            (format!("{what} {}: {glyph} {word}{reason}", r.head_sha.short()), role)
        }
        EventBody::PhaseChanged(p) => {
            let detail = p.detail.as_ref().map_or(String::new(), |d| format!(" ({d})"));
            (format!("{} {} {} on {}{detail}", phase_name(p.from), g.arrow, phase_name(p.to), p.event), Role::Active)
        }
        EventBody::FindingUpdated(f) => {
            let (glyph, sev, role) = c.t.severity(f.severity);
            (format!("{glyph} {sev} {}: {}", f.title, finding_status(f.status).0), role)
        }
        EventBody::ToolActivity(a) => (format!("{} {}", a.tool, a.summary), Role::Normal),
        EventBody::AccountsUpdated(accounts) => {
            let selected = accounts.iter().find(|a| a.selected).map_or("none".into(), |a| a.id.to_string());
            (format!("GitHub accounts updated: {} listed, selected {selected}", accounts.len()), Role::Normal)
        }
        EventBody::Other(kind) => (format!("{kind} (not shown by this client version)"), Role::Muted),
    }
}

// IDs are engine hashes; people know a PR by number and repository.
fn pr_label(c: &Ctx, id: &PrId) -> String {
    match c.m.domain.prs.get(id) {
        Some(p) => format!("#{} {}", p.number, p.repository),
        None => id.to_string(),
    }
}

/// The detail lines for one event, used by the wide side pane and the event modal.
pub fn detail_lines(c: &Ctx, e: &SequencedEvent, phase: Option<Phase>, w: usize) -> Vec<Line<'static>> {
    let field = |k: &str, v: String| {
        Line::from(vec![c.span(format!("{k:<10}"), Role::Muted), c.span(c.fit(&v, w.saturating_sub(10)), Role::Normal)])
    };
    let none = || "-".to_string();
    let (text, role) = summary(c, e);
    let mut lines = vec![
        field("sequence", format!("{} (after {})", e.sequence, e.prev_sequence)),
        field("epoch", e.stream_epoch.to_string()),
        field("at", e.occurred_at.to_string()),
        field("actor", actor(e).into()),
        field("category", EventCategory::of(&e.body).label().into()),
        field("phase", phase.map_or_else(none, |p| phase_name(p).into())),
        field("pr", e.pr_id.as_ref().map_or_else(none, |p| pr_label(c, p))),
        field("run", e.run_id.as_ref().map_or_else(none, |r| r.to_string())),
        Line::raw(""),
    ];
    let mut rest = text.as_str();
    while !rest.is_empty() && w > 0 {
        let cut = fit_len(rest, w);
        if cut == 0 {
            break;
        }
        lines.push(Line::from(c.span(rest[..cut].to_string(), role)));
        rest = &rest[cut..];
    }
    if let EventBody::FindingUpdated(_) = e.body {
        lines.push(Line::raw(""));
        lines.push(Line::from(c.span("Enter opens the finding", Role::Muted)));
    }
    lines
}

pub fn timeline(c: &Ctx, f: &mut Frame, area: Rect) {
    let col = super::doc::column(area);
    let w = col.width as usize;
    let Some(t) = &c.m.timeline else {
        let text = "No timeline loaded. Select a PR and press t.";
        f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(c.span(text, Role::Muted))]), col);
        return;
    };
    let g = c.t.g();
    let pr = t.query.pr.as_ref().map_or("-".into(), |id| pr_label(c, id));
    let scope = match t.scope {
        TimelineScope::Pr => "the whole PR".to_string(),
        TimelineScope::Run => format!("run {}", t.query.run.as_ref().map_or("-", |r| r.short())),
        TimelineScope::Finding => match t.query.finding.as_ref().and_then(|id| c.m.domain.findings.get(id)) {
            Some(f) => format!("\"{}\"", c.fit(f.title.as_str(), 40)),
            None => "a finding".into(),
        },
    };
    let rows = timeline_rows(c.m, t);
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![c.span(format!("{pr}  "), Role::Muted), c.span("Timeline", Role::Title)]),
        Line::from(c.span(
            c.fit(
                &format!(
                    "{} of {} events {} {scope} (f) {} {} (v) {} phase {} (P)",
                    rows.len(),
                    t.events.len(),
                    g.sep,
                    g.sep,
                    t.category.map_or("all kinds", |x| x.label()),
                    g.sep,
                    t.phase.map_or("any", phase_name)
                ),
                w,
            ),
            Role::Muted,
        )),
        Line::raw(""),
    ];
    if let Some(e) = &t.error {
        lines.push(Line::from(c.span(c.fit(&format!("timeline unavailable: {e}"), w), Role::Fail)));
    } else if rows.is_empty() {
        lines.push(Line::from(c.span(if t.pending.is_some() { "loading…" } else { "No events match." }, Role::Muted)));
    }
    let sel = t.selected.and_then(|s| rows.iter().position(|(e, _)| e.sequence.get() == s));
    let h = (col.height as usize).saturating_sub(lines.len() + 1);
    let stem = if c.t.ascii { "|" } else { "│" };
    for i in window(rows.len(), sel, h) {
        let (e, _) = rows[i];
        let selected = Some(i) == sel;
        let (text, role) = summary(c, e);
        let cat = EventCategory::of(&e.body);
        let (node, nrole, trole) = match &e.body {
            EventBody::PhaseChanged(_) => ("●", Role::Accent, Role::Focus),
            EventBody::Run(..) => ("◆", role, Role::Title),
            EventBody::FindingUpdated(_) => ("▲", role, role),
            EventBody::ToolActivity(_) => (stem, Role::Muted, Role::Normal),
            _ => ("·", Role::Muted, Role::Normal),
        };
        let node = if c.t.ascii && node != stem { "*" } else { node };
        let st = if selected { c.t.bg(crate::theme::Bg::Select) } else { c.t.bg(crate::theme::Bg::Base) };
        let on = |r: Role| c.s(r).patch(st);
        let label = match &e.body {
            EventBody::ToolActivity(a) => format!("{:<12} ", c.fit(a.tool.as_str(), 12)),
            _ => String::new(),
        };
        let body = match &e.body {
            EventBody::ToolActivity(a) => a.summary.to_string(),
            EventBody::PhaseChanged(p) => format!("{} {} {}", phase_name(p.from), g.arrow, phase_name(p.to)),
            _ => text,
        };
        let mut spans = vec![
            Span::styled(format!("{}  ", time_of(e.occurred_at.as_str())), on(Role::Muted)),
            Span::styled(format!("{node}  "), on(nrole)),
            Span::styled(label, on(Role::Accent)),
        ];
        let used: usize = spans.iter().map(|s| width(&s.content)).sum();
        spans.push(Span::styled(c.pad(&body, w.saturating_sub(used + 8)), on(trole)));
        spans.push(Span::styled(format!("{:>8}", cat.label()), on(Role::Muted)));
        lines.push(Line::from(spans));
    }
    if t.next_before.is_some() {
        lines.push(Line::from(c.span("older events load as you reach the end", Role::Muted)));
    }
    f.render_widget(Paragraph::new(lines), col);
    let _ = (Layout::vertical([Constraint::Min(1)]), Width::Wide, pad_left("", 0), fit_len("", 0), Pane::Main);
}
