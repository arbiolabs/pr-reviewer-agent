//! The PR rail: two rows per PR in a fixed order. Row one names it, with the worst severity (or a pulse while a review
//! runs). Row two is its state: severity counts and the head SHA when reviewed, the dot stepper and a calls/s cell
//! while running.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::doc::{current, filled, spans_w, spinner, split_row};
use super::live::{per_sec, rate_cell, stepper};
use super::{Ctx, window};
use crate::model::{InputPurpose, Link, Modal};
use crate::protocol::{Finding, FindingStatus, Pr, RunStatus, Severity};
use crate::text::width;
use crate::theme::{Bg, Role};

/// The rail's width on a `width`-column terminal: 34 below 140 columns, then a column more for every four or so, up
/// to 48 at 200.
pub fn rail_w(width: u16) -> u16 {
    (34 + width.saturating_sub(140) * 14 / 60).min(48)
}
/// Rows above the first PR: blank, heading, filter or blank.
pub const RAIL_HEAD: u16 = 3;

/// What an empty rail says: no snapshot yet, no reviewer to ask, nothing tracked, or nothing matching the filter.
pub fn empty_rail(c: &Ctx, total: usize) -> Vec<(String, Role)> {
    if total > 0 {
        return vec![("  no PR matches".into(), Role::Muted)];
    }
    let reason = match &c.m.link {
        Link::Disconnected { reason, .. } => reason,
        Link::Unusable { message } | Link::Incompatible { message, .. } | Link::Stalled { reason: message } => {
            return vec![(format!("  unavailable: {message}"), Role::Fail)];
        }
        Link::Connecting { .. } | Link::Negotiating | Link::Syncing => {
            return vec![(format!("  loading the snapshot{}", c.t.g().ellipsis), Role::Muted)];
        }
        Link::Live | Link::Resyncing => return vec![("  nothing tracked yet".into(), Role::Muted)],
    };
    let path = c.m.diag.endpoint.strip_prefix("unix:").unwrap_or(&c.m.diag.endpoint);
    vec![
        (format!("  No daemon at {path}."), Role::Fail),
        ("  Start one with `pr-review daemon`.".into(), Role::Normal),
        (format!("  {reason}"), Role::Muted),
    ]
}

/// Rows each PR takes; the spacer row goes when the list would not fit.
pub fn row_h(c: &Ctx, height: u16) -> u16 {
    if c.m.inbox().len() as u16 * 3 + RAIL_HEAD <= height { 3 } else { 2 }
}

pub fn rail(c: &Ctx, f: &mut Frame, area: Rect, focused: bool) {
    let bg = c.t.bg(Bg::Surface);
    f.render_widget(Paragraph::new("").style(bg), area);
    let w = area.width as usize;
    let prs = c.m.inbox();
    let total = c.m.domain.prs.len();
    let head_role = if focused { Role::Accent } else { Role::Muted };
    let typing = matches!(c.m.top_modal(), Some(Modal::Input { purpose: InputPurpose::Search, .. }));
    let count = if prs.len() == total { format!("{total}  ") } else { format!("{} of {total}  ", prs.len()) };
    let mut lines = vec![
        Line::from(filled(vec![], w, bg)),
        Line::from(filled(
            split_row(
                vec![Span::raw("  "), Span::styled("PULL REQUESTS", c.s(head_role).patch(bg))],
                vec![Span::styled(count, c.s(Role::Muted).patch(bg))],
                w,
            ),
            w,
            bg,
        )),
    ];
    let filter = &c.m.ui.pr_filter;
    lines.push(Line::from(if filter.is_empty() && !typing {
        filled(vec![], w, bg)
    } else {
        let cursor = if typing { "_" } else { "" };
        filled(
            vec![
                Span::styled("  / ", c.s(Role::Accent).patch(bg)),
                Span::styled(c.fit(&format!("{filter}{cursor}"), w.saturating_sub(5)), c.s(Role::Title).patch(bg)),
            ],
            w,
            bg,
        )
    }));
    if prs.is_empty() {
        let mut empty = empty_rail(c, total);
        if area.width < c.m.size.0 && empty[0].1 == Role::Fail {
            // Beside the reading column, which carries the reason in full.
            empty = vec![("  no daemon".into(), Role::Fail)];
        }
        for (text, role) in empty {
            lines.push(Line::from(filled(vec![Span::styled(c.fit(&text, w), c.s(role).patch(bg))], w, bg)));
        }
    }
    let (range, rh) = visible(c, &prs, area.height);
    let sel = c.m.ui.selected_pr.as_ref().and_then(|id| prs.iter().position(|p| &p.id == id));
    if range.start > 0 {
        lines.pop();
        let more = format!("  {} {} more above", c.t.g().arrow, range.start);
        lines.push(Line::from(filled(vec![Span::styled(more, c.s(Role::Muted).patch(bg))], w, bg)));
    }
    let below = prs.len() - range.end;
    for pr in &prs[range] {
        let selected = sel.is_some_and(|i| prs[i].id == pr.id);
        lines.extend(pr_rows(c, pr, w, selected, rh));
    }
    if below > 0 {
        let more = format!("  {} more below", below);
        let last = lines.len().min(area.height as usize) - 1;
        lines.truncate(last);
        lines.push(Line::from(filled(vec![Span::styled(more, c.s(Role::Muted).patch(bg))], w, bg)));
    }
    f.render_widget(Paragraph::new(lines).style(bg), area);
}

/// Which of `prs` the rail shows in `height` rows, and the rows each takes.
fn visible(c: &Ctx, prs: &[&Pr], height: u16) -> (std::ops::Range<usize>, usize) {
    let rh = row_h(c, height) as usize;
    let sel = c.m.ui.selected_pr.as_ref().and_then(|id| prs.iter().position(|p| &p.id == id));
    let fits = (height as usize).saturating_sub(RAIL_HEAD as usize) / rh;
    (window(prs.len(), sel, fits), rh)
}

/// The PR whose rows cover screen row `y` of a rail drawn in `area`.
pub fn pr_at(c: &Ctx, area: Rect, y: u16) -> Option<crate::protocol::PrId> {
    let prs = c.m.inbox();
    let (range, rh) = visible(c, &prs, area.height);
    let k = (y.checked_sub(area.y + RAIL_HEAD)? as usize) / rh;
    let i = range.start + k;
    (i < range.end).then(|| prs[i].id.clone())
}

fn pr_rows(c: &Ctx, pr: &Pr, w: usize, selected: bool, rh: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let bg = c.t.bg(Bg::Surface);
    let row_bg = if selected { c.t.bg(Bg::Select) } else { bg };
    let on = |role: Role| c.s(role).patch(row_bg);
    let marker = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
    let run = c.m.domain.latest_run(&pr.id);
    let running = run.is_some_and(|r| r.active());
    let parked = run.is_some_and(|r| r.parked());
    let findings = current(c, pr);
    let (glyph, grole) = match findings.iter().map(|f| f.severity).min() {
        _ if running => (spinner(c), Role::Accent),
        _ if parked => (g.pending, Role::Pending),
        Some(s) => {
            let (gl, _, r) = c.t.severity(s);
            (gl, r)
        }
        None => (g.idle, Role::Muted),
    };
    let num = format!("#{} ", pr.number);
    let title_w = w.saturating_sub(width(&num) + 5);
    let first = vec![
        Span::styled(marker.to_string(), on(Role::Accent)),
        Span::styled(format!(" {glyph} "), on(grole)),
        Span::styled(num, on(Role::Accent)),
        Span::styled(c.fit(pr.title.as_str(), title_w), on(if selected { Role::Title } else { Role::Normal })),
    ];
    // The full-width inbox has room for the whole repository, the author and the phase a run is in.
    let repo = if w >= 60 {
        let by = pr.author.as_ref().map_or(String::new(), |a| format!(" {} by {a}", g.sep));
        let phase = run
            .filter(|_| running)
            .map_or(String::new(), |r| format!(" {} {}", g.sep, super::doc::phase_label(r.phase)));
        format!("{}{by}{phase}", pr.repository)
    } else {
        pr.repository.as_str().rsplit('/').next().unwrap_or("").to_string()
    };
    let left =
        vec![Span::styled(marker.to_string(), on(Role::Accent)), Span::styled(format!("   {repo} "), on(Role::Muted))];
    let right: Vec<Span<'static>> = match run {
        None => vec![Span::styled("not reviewed ", on(Role::Muted))],
        Some(r) if r.parked() => vec![Span::styled(format!("{} waiting for you ", g.pending), on(Role::Pending))],
        Some(r) if r.status == RunStatus::Running => {
            let rate = per_sec(c, Some(&pr.id));
            let mut s: Vec<Span<'static>> = stepper(c, r).into_iter().map(|s| s.patch_style(row_bg)).collect();
            if c.m.look.show_activity {
                s.push(Span::styled(format!(" {}", rate_cell(c, rate)), on(Role::Accent)));
                s.push(Span::styled(format!("{:>4}/s ", super::live::rate_label(rate)), on(Role::Muted)));
            } else {
                s.push(Span::styled(" ", row_bg));
            }
            s
        }
        Some(r) if r.status == RunStatus::Failed => vec![Span::styled(format!("{} failed ", g.fail), on(Role::Fail))],
        Some(_) if findings.is_empty() => vec![Span::styled(format!("{} clean ", g.ok), on(Role::Ok))],
        Some(_) => counts(c, &findings, row_bg),
    };
    let mut left = left;
    let sep = if w >= 60 { format!("{} ", g.sep) } else { String::new() };
    let sha = format!("{sep}{}", pr.head_sha.short());
    // The cells the row's glyphs really take, so `->0` in ASCII counts one wider than `→0`; two spaces before the counts.
    let fits = spans_w(&left) + width(&sha) + 2 + spans_w(&right) <= w;
    if fits && run.is_none_or(|r| r.status != RunStatus::Running) {
        left.push(Span::styled(sha, on(Role::Muted)));
    }
    let mut out = vec![
        Line::from(filled(first, w, row_bg)).style(row_bg),
        Line::from(filled(split_row(left, right, w), w, row_bg)).style(row_bg),
    ];
    if rh == 3 {
        out.push(Line::from(filled(vec![], w, bg)));
    }
    out
}

/// `◆1 ▲1 ●1 ○1 →2`: findings per severity, then how many are on GitHub (`→0`, muted, when none is).
pub fn counts(c: &Ctx, findings: &[Finding], bg: Style) -> Vec<Span<'static>> {
    let mut s = Vec::new();
    for sev in [Severity::Critical, Severity::High, Severity::Medium, Severity::Low] {
        let n = findings.iter().filter(|f| f.severity == sev).count();
        if n > 0 {
            let (gl, _, role) = c.t.severity(sev);
            s.push(Span::styled(format!("{gl}{n} "), c.s(role).patch(bg)));
        }
    }
    let published = findings.iter().filter(|f| f.status == FindingStatus::Published).count();
    let role = if published == 0 { Role::Muted } else { Role::Ok };
    s.push(Span::styled(format!("{}{published} ", c.t.g().arrow), c.s(role).patch(bg)));
    s
}
