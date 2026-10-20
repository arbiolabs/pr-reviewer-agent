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

/// The detail under an open row: where the event sits in the stream, then its whole text, as an inset block.
fn detail_block(c: &Ctx, e: &SequencedEvent, phase: Option<Phase>, w: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let surface = c.t.bg(Bg::Surface);
    let sep = format!("  {}  ", g.sep);
    let row = |spans: Vec<Span<'static>>| Line::from(filled(spans, w, surface));
    let field = |k: &str, v: String| {
        vec![
            Span::styled(format!("{k} "), c.s(Role::Muted).patch(surface)),
            Span::styled(v, c.s(Role::Normal).patch(surface)),
        ]
    };
    let join = |parts: Vec<Vec<Span<'static>>>| {
        let mut out = vec![Span::styled("  ", surface)];
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push(Span::styled(sep.clone(), c.s(Role::Muted).patch(surface)));
            }
            out.extend(p);
        }
        out
    };
    let mut lines = vec![row(vec![])];
    lines.push(row(join(vec![
        field("sequence", e.sequence.get().to_string()),
        field("after", e.prev_sequence.to_string()),
        field("epoch", e.stream_epoch.to_string()),
    ])));
    let mut second = vec![field("at", e.occurred_at.to_string()), field("by", actor(e).into())];
    if let Some(p) = phase {
        second.push(field("phase", phase_name(p).into()));
    }
    if let Some(r) = &e.run_id {
        second.push(field("run", r.short().to_string()));
    }
    lines.push(row(join(second)));
    lines.push(row(vec![]));
    let (text, role) = summary(c, e);
    for l in super::code::rich(c, &text, w.saturating_sub(4), role) {
        let mut s = vec![Span::styled("  ", surface)];
        s.extend(l.into_iter().map(|x| x.patch_style(surface)));
        lines.push(row(s));
    }
    if let EventBody::FindingUpdated(_) = e.body {
        lines.push(row(vec![]));
        lines.push(row(vec![Span::styled("  Enter reads the finding", c.s(Role::Accent).patch(surface))]));
    }
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
    let w = col.width as usize;
    let Some(t) = &c.m.timeline else {
        let text = "No timeline loaded. Select a PR and press t.";
        f.render_widget(Paragraph::new(vec![Line::raw(""), Line::from(c.span(text, Role::Muted))]), col);
        return;
    };
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
                "{} of {} events, oldest first {} {scope} (f) {} {} (v) {} phase {} (P)",
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
    )));
    head.push(Line::raw(""));
    if let Some(e) = &t.error {
        head.push(Line::from(c.span(c.fit(&format!("timeline unavailable: {e}"), w), Role::Fail)));
    } else if rows.is_empty() {
        head.push(Line::from(c.span(if t.pending.is_some() { "loading…" } else { "No events match." }, Role::Muted)));
    }
    let mut body: Vec<Line<'static>> = Vec::new();
    if t.next_before.is_some() {
        body.push(Line::from(c.span("older events load as you reach the top", Role::Muted)));
    }
    let mut at = 0;
    let mut last: Option<Option<Phase>> = None;
    let mut after_heading = false;
    for (e, phase) in &rows {
        let selected = t.selected == Some(e.sequence.get());
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
        body.push(if change {
            heading(c, *phase, e.occurred_at.as_str(), w, selected)
        } else {
            event_line(c, e, w, selected)
        });
        if t.open == Some(e.sequence.get()) {
            body.extend(detail_block(c, e, *phase, w));
        }
    }
    let h = (col.height as usize).saturating_sub(head.len());
    // Keep the open detail in view under its row when it fits.
    let focus = if t.open.is_some() && t.open == t.selected { at + h / 3 } else { at };
    let range = window(body.len(), Some(focus.min(body.len().saturating_sub(1))), h);
    head.extend(body.drain(range));
    f.render_widget(Paragraph::new(head), col);
}
