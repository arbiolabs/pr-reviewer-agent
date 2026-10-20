//! Inbox and Review.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Ctx, Width, window};
use crate::model::{Activity, ActivityKind, Link, Pane};
use crate::protocol::{Finding, Lifecycle, Phase, Pr, Run, RunChange, RunStatus, Severity};
use crate::text::width;
use crate::theme::{Role, finding_status, phase_name};

pub fn time_of(at: &str) -> &str {
    at.get(11..19).unwrap_or(at)
}

fn empty_inbox(c: &Ctx) -> (String, Role) {
    let m = c.m;
    if m.domain.prs.is_empty() {
        match &m.link {
            Link::Disconnected { reason, .. } if m.diag.last_snapshot_ms.is_none() => {
                (format!("Reviewer service unavailable: {reason}\nIs `pr-review daemon` running?"), Role::Fail)
            }
            Link::Incompatible { .. } => ("Incompatible reviewer service; see the banner.".into(), Role::Fail),
            Link::Unusable { .. } => ("Cannot connect to the reviewer service; see the banner.".into(), Role::Fail),
            _ if m.diag.last_snapshot_ms.is_none() => ("Loading snapshot...".into(), Role::Pending),
            _ => ("No tracked PRs yet. The reviewer lists PRs here once it tracks them.".into(), Role::Muted),
        }
    } else {
        (format!("No PRs match \"{}\". Press / to edit the search, Esc in it to clear.", m.ui.pr_filter), Role::Muted)
    }
}

fn message(c: &Ctx, f: &mut Frame, area: Rect, text: &str, role: Role) {
    let w = area.width as usize;
    let mut lines = vec![Line::raw("")];
    lines.extend(text.split('\n').map(|t| Line::from(c.span(c.fit(&format!("  {t}"), w), role))));
    f.render_widget(Paragraph::new(lines), area);
}

/// The PR's latest run, as a glyph, a word and a role.
fn run_state(c: &Ctx, pr: &Pr) -> (String, Role) {
    let g = c.t.g();
    let base = match c.m.domain.latest_run(&pr.id) {
        None => (format!("{} no run", g.idle), Role::Muted),
        Some(r) if r.phase == Phase::Reconcile => (format!("{} PUBLISH?", g.pending), Role::Pending),
        Some(r) if r.status == RunStatus::Running => (format!("{} {}", g.active, phase_name(r.phase)), Role::Active),
        Some(r) => {
            let (glyph, word, role) = c.t.run_status(r.status);
            (format!("{glyph} {word}"), role)
        }
    };
    match pr.lifecycle {
        Lifecycle::Active => base,
        Lifecycle::Quiescing => (format!("{} (quiescing)", base.0), base.1),
        Lifecycle::Closed => (format!("{} (closed)", base.0), Role::Muted),
        Lifecycle::Merged => (format!("{} (merged)", base.0), Role::Muted),
        Lifecycle::Unknown => base,
    }
}

fn severity_counts(c: &Ctx, pr: &Pr) -> String {
    let current: Vec<&Finding> = c.m.domain.pr_findings(&pr.id).into_iter().filter(|f| !f.stale).collect();
    if current.is_empty() {
        return "none".into();
    }
    let g = c.t.g();
    [
        (Severity::Critical, g.sev_crit),
        (Severity::High, g.sev_high),
        (Severity::Medium, g.sev_med),
        (Severity::Low, g.sev_low),
    ]
    .iter()
    .filter_map(|(s, glyph)| {
        let n = current.iter().filter(|f| f.severity == *s).count();
        (n > 0).then(|| format!("{glyph}{n}"))
    })
    .collect::<Vec<_>>()
    .join(" ")
}

fn is_stale(c: &Ctx, pr: &Pr) -> bool {
    c.m.domain.latest_run(&pr.id).is_some_and(|r| r.head_sha != pr.head_sha)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Inbox at 80+ columns: every column.
    Table,
    /// Inbox below 80 columns: title and review state.
    Compact,
    /// Review sidebar: title and a status glyph.
    Sidebar,
}

fn pr_row(c: &Ctx, pr: &Pr, w: usize, selected: bool, kind: RowKind) -> Line<'static> {
    let g = c.t.g();
    let cursor = if selected { g.cursor } else { " " };
    let num = format!("#{}", pr.number);
    let (state, state_role) = run_state(c, pr);
    let stale = if is_stale(c, pr) { " stale" } else { "" };
    let mut spans = vec![Span::raw(format!("{cursor} ")), c.span(c.pad(&num, 6), Role::Title)];
    let used = match kind {
        RowKind::Table => {
            let counts = severity_counts(c, pr);
            let head = pr.head_sha.short().to_string();
            let title_w = w.saturating_sub(8 + TABLE_RIGHT).max(4);
            spans.push(c.span(c.pad(pr.title.as_str(), title_w), Role::Normal));
            spans.push(c.span(c.pad(&format!(" {state}"), 22), state_role));
            spans.push(c.span(c.pad(&format!(" {head}"), 9), Role::Muted));
            spans.push(c.span(c.pad(&format!(" {counts}"), 12), Role::Normal));
            spans.push(c.span(c.pad(stale, 6), Role::Pending));
            8 + title_w + TABLE_RIGHT
        }
        RowKind::Compact => {
            let state_w = width(&state).min(22) + 1;
            let title_w = w.saturating_sub(8 + state_w + stale.len()).max(1);
            spans.push(c.span(c.pad(pr.title.as_str(), title_w), Role::Normal));
            spans.push(c.span(format!(" {}", c.fit(&state, state_w - 1)), state_role));
            spans.push(c.span(stale, Role::Pending));
            8 + title_w + state_w + stale.len()
        }
        RowKind::Sidebar => {
            let glyph = state.split(' ').next().unwrap_or("").to_string();
            let title_w = w.saturating_sub(8 + 2).max(1);
            spans.push(c.span(c.pad(pr.title.as_str(), title_w), Role::Normal));
            spans.push(c.span(format!(" {glyph}"), state_role));
            8 + title_w + 1 + width(&glyph)
        }
    };
    if used < w {
        spans.push(Span::raw(" ".repeat(w - used)));
    }
    let line = Line::from(spans);
    if selected { line.style(c.s(Role::Selected)) } else { line }
}

const TABLE_RIGHT: usize = 22 + 9 + 12 + 6;

fn pr_list(c: &Ctx, f: &mut Frame, area: Rect, title: &str, focused: bool, kind: RowKind) {
    let block = c.pane(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let prs = c.m.inbox();
    if prs.is_empty() {
        let (text, role) = empty_inbox(c);
        message(c, f, inner, &text, role);
        return;
    }
    let w = inner.width as usize;
    let mut lines = Vec::new();
    let mut h = inner.height as usize;
    if kind == RowKind::Table && h > 1 {
        let head = format!("  {}{}", c.pad("PR", 6), c.pad("TITLE", w.saturating_sub(8 + TABLE_RIGHT).max(4)));
        let head = format!("{head} {}{}{}", c.pad("REVIEW", 21), c.pad(" HEAD", 9), c.pad(" FINDINGS", 12));
        lines.push(Line::from(c.span(c.fit(&head, w), Role::Muted)));
        h -= 1;
    }
    let sel = c.m.ui.selected_pr.as_ref().and_then(|id| prs.iter().position(|p| &p.id == id));
    for i in window(prs.len(), sel, h) {
        lines.push(pr_row(c, prs[i], w, Some(i) == sel, kind));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn inbox(c: &Ctx, f: &mut Frame, area: Rect) {
    let filter = if c.m.ui.pr_filter.is_empty() { String::new() } else { format!(" / \"{}\"", c.m.ui.pr_filter) };
    let title = format!("TRACKED PRS ({}){filter}", c.m.inbox().len());
    match c.width {
        Width::Wide => {
            let [list, preview] = Layout::horizontal([Constraint::Min(40), Constraint::Length(40)]).areas(area);
            pr_list(c, f, list, &title, true, RowKind::Table);
            let block = c.pane("SELECTED", false);
            let inner = block.inner(preview);
            f.render_widget(block, preview);
            if let Some(pr) = c.m.selected_pr() {
                let mut lines = header_lines(c, pr, inner.width as usize);
                lines.push(Line::raw(""));
                lines.push(Line::from(c.span(format!("findings: {}", severity_counts(c, pr)), Role::Normal)));
                lines.push(Line::from(c.span("Enter opens the review", Role::Muted)));
                f.render_widget(Paragraph::new(lines), inner);
            }
        }
        Width::Medium => pr_list(c, f, area, &title, true, RowKind::Table),
        Width::Narrow => pr_list(c, f, area, &title, true, RowKind::Compact),
    }
}

pub fn stepper(c: &Ctx, run: Option<&Run>, w: usize) -> Line<'static> {
    let g = c.t.g();
    const LONG: [&str; 6] = ["Collect", "Triage", "Analyze", "Verify", "Prepare", "Publish"];
    const SHORT: [&str; 6] = ["Col", "Tri", "Ana", "Ver", "Prep", "Pub"];
    let Some(run) = run else { return Line::from(c.span("no review run yet", Role::Muted)) };
    let current = match run.phase {
        Phase::CollectContext => Some(0),
        Phase::Triage => Some(1),
        Phase::Analyze => Some(2),
        Phase::Verify => Some(3),
        Phase::PreparePublication => Some(4),
        Phase::Publish | Phase::Reconcile => Some(5),
        Phase::Completed => Some(6),
        Phase::Failed | Phase::Superseded | Phase::Unknown => None,
    };
    let end = match run.phase {
        Phase::Completed => Some((format!("{} done", g.ok), Role::Ok)),
        Phase::Reconcile => Some(("publish uncertain: reconciling".to_string(), Role::Pending)),
        Phase::Failed => Some((format!("{} FAILED", g.fail), Role::Fail)),
        Phase::Superseded => Some((format!("{} SUPERSEDED", g.idle), Role::Muted)),
        Phase::Unknown => Some(("? unknown phase".to_string(), Role::Muted)),
        _ => None,
    };
    let build = |labels: [&str; 6]| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            let (glyph, role) = match current {
                None => (g.idle, Role::Muted),
                Some(cur) if i < cur => (g.ok, Role::Ok),
                Some(cur) if i == cur && run.phase == Phase::Reconcile => ("?", Role::Pending),
                Some(cur) if i == cur => (g.pending, Role::Active),
                _ => (g.idle, Role::Muted),
            };
            spans.push(c.span(format!("{label} {glyph}"), role));
            spans.push(Span::raw("  "));
        }
        if let Some((text, role)) = &end {
            spans.push(c.span(text.clone(), *role));
        }
        spans
    };
    let long = build(LONG);
    let len: usize = long.iter().map(|s| width(&s.content)).sum();
    Line::from(if len <= w { long } else { build(SHORT) })
}

fn header_lines(c: &Ctx, pr: &Pr, w: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let sep = format!(" {} ", g.sep);
    let run = c.m.domain.latest_run(&pr.id);
    let mut lines = vec![Line::from(c.span(c.fit(&format!("#{} {}", pr.number, pr.title), w), Role::Title))];
    let by = pr.author.as_ref().map_or(String::new(), |a| format!("{sep}by {a}"));
    let shas = format!("{sep}base {} {} head {}", pr.base_sha.short(), g.arrow, pr.head_sha.short());
    lines.push(Line::from(c.span(c.fit(&format!("{}{by}{shas}", pr.repository), w), Role::Muted)));
    match run {
        Some(r) => {
            let (glyph, word, role) = c.t.run_status(r.status);
            let status = format!("{glyph} {word}");
            let info = format!("{sep}run {}{sep}{}{sep}started {}", r.id, r.model, time_of(r.started_at.as_str()));
            let info = c.fit(&info, w.saturating_sub(width(&status)));
            lines.push(Line::from(vec![c.span(status, role), c.span(info, Role::Normal)]));
        }
        None => lines.push(Line::from(c.span("no review run yet", Role::Muted))),
    }
    if run.is_some() {
        lines.push(stepper(c, run, w));
    }
    if let Some(r) = run {
        if r.head_sha != pr.head_sha {
            let text =
                format!("STALE: this run reviewed {}; the PR head is now {}", r.head_sha.short(), pr.head_sha.short());
            lines.push(Line::from(c.span(c.fit(&text, w), Role::Pending)));
        }
        if let Some(reason) = &r.failure_reason {
            lines.push(Line::from(c.span(c.fit(&format!("failure: {reason}"), w), Role::Fail)));
        }
    }
    lines
}

fn finding_row(c: &Ctx, f: &Finding, w: usize, selected: bool) -> Line<'static> {
    let g = c.t.g();
    let (glyph, sev, sev_role) = c.t.severity(f.severity);
    let (status, status_role) = finding_status(f.status);
    let cursor = if selected { g.cursor } else { " " };
    let loc = match f.line {
        Some(l) => format!("{}:{l}", f.file),
        None => format!("{} (file)", f.file),
    };
    let stale = if f.stale { "stale" } else { "" };
    let mut spans = vec![Span::raw(format!("{cursor} ")), c.span(format!("{glyph} {}", c.pad(sev, 5)), sev_role)];
    let used = if w < 64 {
        let title_w = w.saturating_sub(2 + 7 + 15 + 6).max(1);
        spans.push(c.span(c.pad(f.title.as_str(), title_w), Role::Normal));
        spans.push(c.span(c.pad(&format!(" {status}"), 15), status_role));
        spans.push(c.span(c.pad(stale, 6), Role::Pending));
        2 + 7 + title_w + 15 + 6
    } else {
        let full = w >= 96;
        let loc_w = (w / 4).clamp(12, 32);
        let extra = if full { 10 } else { 0 };
        let title_w = w.saturating_sub(2 + 7 + loc_w + 15 + extra + 6).max(1);
        spans.push(c.span(c.pad(f.title.as_str(), title_w), Role::Normal));
        spans.push(c.span(c.pad(&format!(" {loc}"), loc_w), Role::Muted));
        spans.push(c.span(c.pad(&format!(" {status}"), 15), status_role));
        if full {
            spans.push(c.span(c.pad(&format!(" {:.0}%", f.confidence * 100.0), 5), Role::Muted));
            spans.push(c.span(c.pad(&format!(" ev{}", f.evidence_count), 5), Role::Muted));
        }
        spans.push(c.span(c.pad(&format!(" {stale}"), 6), Role::Pending));
        2 + 7 + title_w + loc_w + 15 + extra + 6
    };
    if used < w {
        spans.push(Span::raw(" ".repeat(w - used)));
    }
    let line = Line::from(spans);
    if selected { line.style(c.s(Role::Selected)) } else { line }
}

pub fn activity_line(c: &Ctx, a: &Activity, w: usize) -> Line<'static> {
    let g = c.t.g();
    let (text, role) = match &a.kind {
        ActivityKind::Tool { tool, summary } => (format!("{tool} {summary}"), Role::Normal),
        ActivityKind::Phase { from, to } => {
            (format!("phase {} {} {}", phase_name(*from), g.arrow, phase_name(*to)), Role::Active)
        }
        ActivityKind::Finding { title, status, severity } => {
            let (glyph, sev, role) = c.t.severity(*severity);
            (format!("finding {glyph} {sev} {title}: {}", finding_status(*status).0), role)
        }
        ActivityKind::Run { change, status } => {
            let what = match change {
                RunChange::Started => "run started",
                RunChange::Completed => "review completed",
                RunChange::Superseded => "run superseded",
                RunChange::Failed => "run failed",
            };
            let (glyph, word, role) = c.t.run_status(*status);
            (format!("{what}: {glyph} {word}"), role)
        }
    };
    let at = time_of(a.at.as_str()).to_string();
    let rest = w.saturating_sub(width(&at) + 1);
    Line::from(vec![c.span(format!("{at} "), Role::Muted), c.span(c.fit(&text, rest), role)])
}

pub fn review(c: &Ctx, f: &mut Frame, area: Rect) {
    let main = match c.width {
        Width::Narrow => area,
        w => {
            let side_w = if w == Width::Wide { 34 } else { 28 };
            let [side, main] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(30)]).areas(area);
            pr_list(c, f, side, "TRACKED PRS", c.focused(Pane::Sidebar), RowKind::Sidebar);
            main
        }
    };
    let Some(pr) = c.m.selected_pr() else {
        message(c, f, main, "No PR selected. Pick one in the Inbox (1).", Role::Muted);
        return;
    };
    let inner_w = main.width.saturating_sub(2) as usize;
    let header = header_lines(c, pr, inner_w);
    let show_feed = c.m.look.show_activity;
    let feed_h = if show_feed { (main.height / 3).max(3) } else { 0 };
    let [head, findings, feed] =
        Layout::vertical([Constraint::Length(header.len() as u16 + 2), Constraint::Min(3), Constraint::Length(feed_h)])
            .areas(main);
    let block = c.pane("REVIEW", false);
    let inner = block.inner(head);
    f.render_widget(block, head);
    f.render_widget(Paragraph::new(header), inner);

    let list = c.m.domain.pr_findings(&pr.id);
    let block = c.pane(&format!("FINDINGS ({})", list.len()), c.focused(Pane::Main));
    let inner = block.inner(findings);
    f.render_widget(block, findings);
    if list.is_empty() {
        // SPEC section 12's wording: a finished review with nothing to report is a result, not a pending state.
        let text = match c.m.domain.latest_run(&pr.id).map(|r| r.status) {
            Some(RunStatus::Completed) => "No actionable issues found in the reviewed scope",
            Some(RunStatus::Failed) => "No findings: the latest run failed.",
            _ => "No findings for this PR yet.",
        };
        message(c, f, inner, text, Role::Muted);
    } else {
        let sel = c.m.ui.selected_finding.as_ref().and_then(|id| list.iter().position(|x| &x.id == id));
        let rows: Vec<Line> = window(list.len(), sel, inner.height as usize)
            .map(|i| finding_row(c, list[i], inner.width as usize, Some(i) == sel))
            .collect();
        f.render_widget(Paragraph::new(rows), inner);
    }

    if show_feed {
        let block = c.pane("RECENT ACTIVITY", c.focused(Pane::Feed));
        let inner = block.inner(feed);
        f.render_widget(block, feed);
        let items: Vec<&Activity> = c.m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&pr.id)).collect();
        if items.is_empty() {
            message(c, f, inner, "No activity since the client attached.", Role::Muted);
            return;
        }
        let h = inner.height as usize;
        let end = items.len().saturating_sub(c.m.ui.feed_scroll.min(items.len().saturating_sub(1)));
        let start = end.saturating_sub(h);
        let rows: Vec<Line> = items[start..end].iter().map(|a| activity_line(c, a, inner.width as usize)).collect();
        f.render_widget(Paragraph::new(rows), inner);
    }
}
