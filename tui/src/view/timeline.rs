//! Timeline: the journal for a PR, run or finding, oldest first and grouped under the phase each event happened in,
//! filterable by category and phase. Enter opens an event's detail inline under its row.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::doc::{filled, spans_w, time_of};
use super::{Ctx, window};
use crate::model::TimelineScope;
use crate::protocol::{EventBody, Phase, RunChange, SequencedEvent};
use crate::text::width;
use crate::theme::{Bg, Role, finding_status, phase_name};
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

/// The detail under an open row, as an inset block: what the event carries in full (the whole tool summary; for a
/// finding its title, severity, place and the start of its explanation), then where it sits in the stream.
fn detail_block(c: &Ctx, e: &SequencedEvent, phase: Option<Phase>, w: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let surface = c.t.bg(Bg::Surface);
    let sep = format!("  {}  ", g.sep);
    let row = |spans: Vec<Span<'static>>| Line::from(filled(spans, w, surface));
    let text = |t: &str, role: Role| -> Vec<Line<'static>> {
        super::code::rich(c, t, w.saturating_sub(4), role)
            .into_iter()
            .map(|l| {
                let mut s = vec![Span::styled("  ", surface)];
                s.extend(l.into_iter().map(|x| x.patch_style(surface)));
                row(s)
            })
            .collect()
    };
    let mut lines = vec![row(vec![])];
    match &e.body {
        EventBody::FindingUpdated(f) => {
            let (glyph, sev, role) = c.t.severity(f.severity);
            let (status, _) = finding_status(f.status);
            let cat = format!("{:?}", f.category).to_lowercase();
            lines.extend(text(f.title.as_str(), Role::Title));
            let head = format!("{glyph} {sev}  {cat}  {status} {} {:.0}% confident", g.sep, f.confidence * 100.0);
            lines.extend(text(&head, role));
            let place = match f.line {
                Some(l) => format!("{}:{l}", f.file),
                None => format!("{} (whole file)", f.file),
            };
            lines.extend(text(&place, Role::Accent));
            lines.push(row(vec![]));
            match c.m.details.get(&f.id) {
                Some(d) => {
                    let prose = text(d.explanation.as_str(), Role::Normal);
                    let more = prose.len() > 4;
                    lines.extend(prose.into_iter().take(4));
                    if more {
                        lines.push(row(vec![Span::styled(
                            format!("  {}", g.ellipsis),
                            c.s(Role::Muted).patch(surface),
                        )]));
                    }
                }
                None => lines.extend(text(&format!("loading the explanation{}", g.ellipsis), Role::Muted)),
            }
            lines.push(row(vec![]));
            lines.push(row(vec![Span::styled("  Enter reads the finding", c.s(Role::Accent).patch(surface))]));
        }
        EventBody::ToolActivity(a) => {
            lines.extend(text(a.tool.as_str(), Role::Accent));
            lines.extend(text(a.summary.as_str(), Role::Normal));
        }
        _ => {
            let (t, role) = summary(c, e);
            lines.extend(text(&t, role));
        }
    }
    lines.push(row(vec![]));
    let mut place = vec![format!("at {}", e.occurred_at), format!("by {}", actor(e))];
    if let Some(p) = phase {
        place.push(format!("phase {}", phase_name(p)));
    }
    if let Some(r) = &e.run_id {
        place.push(format!("run {}", r.short()));
    }
    place.push(format!("sequence {} after {} in {}", e.sequence.get(), e.prev_sequence, e.stream_epoch));
    lines.extend(text(&place.join(&sep), Role::Muted));
    lines.push(row(vec![]));
    lines
}

/// `ANALYZE ───── 14:30:04`: where a phase starts. A phase change is its own heading, so it is never a row too.
fn heading(c: &Ctx, phase: Option<Phase>, at: &str, w: usize, selected: bool) -> Line<'static> {
    let st = c.t.bg(if selected { Bg::Select } else { Bg::Base });
    let name = phase.map_or("BEFORE THE RUN", phase_name).replace('_', " ");
    let time = format!(" {}", time_of(at));
    let rule = (if c.t.ascii { "-" } else { "─" }).repeat(w.saturating_sub(width(&name) + width(&time) + 2));
    Line::from(filled(
        vec![
            Span::styled(name, c.s(Role::Focus).patch(st)),
            Span::styled(format!(" {rule}"), c.s(Role::Muted).patch(st)),
            Span::styled(time, c.s(Role::Muted).patch(st)),
        ],
        w,
        st,
    ))
}

/// One event: its time, a glyph for what kind it is, the tool when there is one, and what happened.
fn event_line(c: &Ctx, e: &SequencedEvent, w: usize, selected: bool) -> Line<'static> {
    let g = c.t.g();
    let st = c.t.bg(if selected { Bg::Select } else { Bg::Base });
    let on = |r: Role| c.s(r).patch(st);
    let bar = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
    let (text, role) = summary(c, e);
    let (glyph, grole, label, body, trole) = match &e.body {
        EventBody::ToolActivity(a) => {
            (" ", Role::Muted, c.pad(a.tool.as_str(), 13), a.summary.to_string(), Role::Normal)
        }
        EventBody::FindingUpdated(f) => {
            let (gl, sev, r) = c.t.severity(f.severity);
            (gl, r, c.pad(sev, 13), format!("{}  {}", f.title, finding_status(f.status).0), r)
        }
        EventBody::Run(change, r) => {
            let (gl, word, role) = c.t.run_status(r.status);
            let what = match change {
                RunChange::Started => "review started",
                RunChange::Completed => "review finished",
                RunChange::Superseded => "review superseded by a newer head",
                RunChange::Failed => "review failed",
            };
            let reason = r.failure_reason.as_ref().map_or(String::new(), |x| format!(": {x}"));
            (gl, role, c.pad(word, 13), format!("{what} at {}{reason}", r.head_sha.short()), Role::Title)
        }
        _ => (g.idle, Role::Muted, c.pad("", 13), text, role),
    };
    let lead = vec![
        Span::styled(bar.to_string(), on(Role::Accent)),
        Span::styled(format!(" {}  ", time_of(e.occurred_at.as_str())), on(Role::Muted)),
        Span::styled(format!("{glyph} "), on(grole)),
        Span::styled(
            format!("{label} "),
            on(if matches!(e.body, EventBody::ToolActivity(_)) { Role::Accent } else { grole }),
        ),
    ];
    let room = w.saturating_sub(spans_w(&lead));
    let mut spans = lead;
    spans.push(Span::styled(c.fit(&body, room), on(trole)));
    Line::from(filled(spans, w, st))
}

pub fn timeline(c: &Ctx, f: &mut Frame, area: Rect) {
    let col = super::doc::column(area);
    let Some(t) = &c.m.timeline else {
        let text = "No timeline loaded. Select a PR and press t.";
        f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(c.span(text, Role::Muted))]), col);
        return;
    };
    f.render_widget(Paragraph::new(page(c, t, col).0), col);
}

/// The event whose row, heading or open detail is at screen row `y` of a timeline drawn in `area`.
pub fn hit(c: &Ctx, area: Rect, y: u16) -> Option<u64> {
    let col = super::doc::column(area);
    let (_, owners) = page(c, c.m.timeline.as_ref()?, col);
    *owners.get(y.checked_sub(col.y)? as usize)?
}

/// The lines the timeline shows in `col`, and the event each one belongs to.
fn page(c: &Ctx, t: &crate::model::TimelineView, col: Rect) -> (Vec<Line<'static>>, Vec<Option<u64>>) {
    let w = col.width as usize;
    let g = c.t.g();
    let pr = t.query.pr.as_ref().and_then(|id| c.m.domain.prs.get(id));
    let scope = match t.scope {
        TimelineScope::Pr => "the whole PR".to_string(),
        TimelineScope::Run => format!("run {}", t.query.run.as_ref().map_or("-", |r| r.short())),
        TimelineScope::Finding => match t.query.finding.as_ref().and_then(|id| c.m.domain.findings.get(id)) {
            Some(f) => format!("\"{}\"", c.fit(f.title.as_str(), 40)),
            None => "a finding".into(),
        },
    };
    let rows = timeline_rows(c.m, t);
    let mut head = vec![Line::raw("")];
    head.push(Line::from(match pr {
        Some(p) => vec![
            c.span(p.repository.to_string(), Role::Muted),
            c.span(format!("  #{}", p.number), Role::Accent),
            c.span("  timeline", Role::Muted),
        ],
        None => vec![c.span("Timeline", Role::Muted)],
    }));
    if let Some(p) = pr {
        head.push(Line::from(c.span(c.fit(p.title.as_str(), w), Role::Title)));
    }
    head.push(Line::from(c.span(
        c.fit(
            &format!(
                "{} of {} {}, oldest first {} {scope} (f) {} {} (v) {} phase {} (P){}",
                rows.len(),
                t.events.len(),
                if t.events.len() == 1 { "event" } else { "events" },
                g.sep,
                g.sep,
                t.category.map_or("all kinds", |x| x.label()),
                g.sep,
                t.phase.map_or("any", phase_name),
                if t.text.is_empty() { String::new() } else { format!(" {} \"{}\" (/)", g.sep, t.text) },
            ),
            w,
        ),
        Role::Muted,
    )));
    head.push(Line::raw(""));
    if let Some(e) = &t.error {
        head.push(Line::from(c.span(c.fit(&format!("timeline unavailable: {e}"), w), Role::Fail)));
    } else if rows.is_empty() {
        let filtered = t.scope != TimelineScope::Pr || t.category.is_some() || t.phase.is_some() || !t.text.is_empty();
        let text = match (t.pending.is_some(), filtered) {
            (true, _) => format!("loading{}", g.ellipsis),
            (false, true) => "no events match the filters (f v P /)".into(),
            (false, false) => "no events recorded yet".into(),
        };
        head.push(Line::from(c.span(text, Role::Muted)));
    }
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<u64>> = Vec::new();
    if t.next_before.is_some() {
        body.push(Line::from(c.span("older events load as you reach the top", Role::Muted)));
    }
    let mut at = 0;
    let mut last: Option<Option<Phase>> = None;
    let mut after_heading = false;
    for (e, phase) in &rows {
        let seq = e.sequence.get();
        let selected = t.selected == Some(seq);
        owners.resize(body.len(), None);
        if selected {
            at = body.len();
        }
        let change = matches!(e.body, EventBody::PhaseChanged(_));
        if change || last.is_some_and(|p| p != *phase) || (last.is_none() && phase.is_some()) {
            // An empty phase's heading sits right under the one before it.
            if !body.is_empty() && !after_heading {
                body.push(Line::raw(""));
            }
            if !change {
                body.push(heading(c, *phase, e.occurred_at.as_str(), w, false));
            }
        }
        last = Some(*phase);
        after_heading = change && t.open != Some(e.sequence.get());
        owners.resize(body.len(), None);
        body.push(if change {
            heading(c, *phase, e.occurred_at.as_str(), w, selected)
        } else {
            event_line(c, e, w, selected)
        });
        if t.open == Some(e.sequence.get()) {
            body.extend(detail_block(c, e, *phase, w));
        }
        owners.resize(body.len(), Some(seq));
    }
    owners.resize(body.len(), None);
    let h = (col.height as usize).saturating_sub(head.len());
    // Keep the open detail in view under its row when it fits.
    let focus = if t.open.is_some() && t.open == t.selected { at + h / 3 } else { at };
    let range = window(body.len(), Some(focus.min(body.len().saturating_sub(1))), h);
    let mut owned = vec![None; head.len()];
    owned.extend(owners.drain(range.clone()));
    head.extend(body.drain(range));
    (head, owned)
}
