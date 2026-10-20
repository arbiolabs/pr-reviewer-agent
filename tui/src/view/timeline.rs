//! Timeline: the journal for a PR, run or finding, newest first, filterable by category and phase.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::lists::time_of;
use super::{Ctx, Width, window};
use crate::model::{EventCategory, Pane, TimelineScope};
use crate::protocol::{EventBody, Phase, RunChange, SequencedEvent};
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
            (format!("{} {glyph} {sev} {}: {}", f.id, f.title, finding_status(f.status).0), role)
        }
        EventBody::ToolActivity(a) => (format!("{} {}", a.tool, a.summary), Role::Normal),
        EventBody::AccountsUpdated(accounts) => {
            let selected = accounts.iter().find(|a| a.selected).map_or("none".into(), |a| a.id.to_string());
            (format!("GitHub accounts updated: {} listed, selected {selected}", accounts.len()), Role::Normal)
        }
        EventBody::Other(kind) => (format!("{kind} (not shown by this client version)"), Role::Muted),
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
        field("pr", e.pr_id.as_ref().map_or_else(none, |p| p.to_string())),
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
    let Some(t) = &c.m.timeline else {
        let text = "  No timeline loaded. Select a PR and press 4 or t.";
        f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(c.span(text, Role::Muted))]), area);
        return;
    };
    let g = c.t.g();
    // IDs are engine hashes; people know a PR by number and repository, and a finding by its title.
    let pr = t.query.pr.as_ref().map_or("-".into(), |id| match c.m.domain.prs.get(id) {
        Some(p) => format!("#{} {}", p.number, p.repository),
        None => id.to_string(),
    });
    let scope = match t.scope {
        TimelineScope::Pr => format!("PR {pr}"),
        TimelineScope::Run => format!("run {} of {pr}", t.query.run.as_ref().map_or("-", |r| r.short())),
        TimelineScope::Finding => {
            let finding = t.query.finding.as_ref().map_or("-".into(), |id| match c.m.domain.findings.get(id) {
                Some(f) => format!("\"{}\"", c.fit(f.title.as_str(), 32)),
                None => id.to_string(),
            });
            format!("finding {finding} on {pr}")
        }
    };
    let category = t.category.map_or("all", |x| x.label());
    let phase = t.phase.map_or("all", phase_name);
    let rows = timeline_rows(c.m, t);
    let state = if t.pending.is_some() {
        format!(" {} loading...", g.sep)
    } else if t.next_before.is_some() {
        format!(" {} older events: move to the end to load", g.sep)
    } else {
        String::new()
    };
    let head = format!("scope {scope} (f) {} category {category} (v) {} phase {phase} (P){state}", g.sep, g.sep);
    let [head_area, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(c.span(c.fit(&head, head_area.width as usize), Role::Muted))), head_area);

    let (list_area, detail_area) = match c.width {
        Width::Wide => {
            let [l, r] = Layout::horizontal([Constraint::Min(50), Constraint::Length(48)]).areas(body);
            (l, Some(r))
        }
        _ => (body, None),
    };
    let block = c.pane(
        &format!("TIMELINE ({} of {})", rows.len(), t.events.len()),
        c.focused(Pane::Main) || c.focused(Pane::Sidebar),
    );
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    let w = inner.width as usize;
    if let Some(e) = &t.error {
        let line = Line::from(c.span(c.fit(&format!(" timeline unavailable: {e}"), w), Role::Fail));
        f.render_widget(Paragraph::new(vec![line]), inner);
    } else if rows.is_empty() {
        let text = if t.pending.is_some() { " loading..." } else { " no events match" };
        f.render_widget(Paragraph::new(vec![Line::from(c.span(text, Role::Muted))]), inner);
    } else {
        let sel = t.selected.and_then(|s| rows.iter().position(|(e, _)| e.sequence.get() == s));
        let narrow = c.width == Width::Narrow;
        let lines: Vec<Line> = window(rows.len(), sel, inner.height as usize)
            .map(|i| {
                let (e, _) = rows[i];
                let selected = Some(i) == sel;
                let (text, role) = summary(c, e);
                let cursor = if selected { g.cursor } else { " " };
                let mut spans = vec![
                    Span::raw(cursor.to_string()),
                    c.span(pad_left(&e.sequence.to_string(), 5), Role::Muted),
                    c.span(format!(" {} ", time_of(e.occurred_at.as_str())), Role::Muted),
                ];
                if !narrow {
                    spans.push(c.span(format!("{:<7}", actor(e)), Role::Muted));
                    spans.push(c.span(format!("{:<8}", EventCategory::of(&e.body).label()), Role::Muted));
                }
                let used: usize = spans.iter().map(|s| width(&s.content)).sum();
                spans.push(c.span(c.pad(&text, w.saturating_sub(used)), role));
                let line = Line::from(spans);
                if selected { line.style(c.s(Role::Selected)) } else { line }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    if let Some(area) = detail_area {
        let block = c.pane("EVENT", false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let selected = t.selected.and_then(|s| rows.iter().find(|(e, _)| e.sequence.get() == s));
        if let Some((e, phase)) = selected {
            f.render_widget(Paragraph::new(detail_lines(c, e, *phase, inner.width as usize)), inner);
        }
    }
}
