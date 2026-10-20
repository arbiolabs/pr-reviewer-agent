//! The PR rail: two rows per PR in a fixed order. Row one names it, with the worst severity (or a pulse while a review
//! runs). Row two is its state: severity counts and the head SHA when reviewed, the dot stepper and a calls/s cell
//! while running.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::doc::{current, filled, spinner, split_row};
use super::live::{per_sec, rate_cell, stepper};
use super::{Ctx, window};
use crate::model::{InputPurpose, Modal};
use crate::protocol::{Finding, FindingStatus, Pr, RunStatus, Severity};
use crate::text::width;
use crate::theme::{Bg, Role};

pub const RAIL_W: u16 = 34;
/// Rows above the first PR: blank, heading, filter or blank.
pub const RAIL_HEAD: u16 = 3;

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
        let text = if total == 0 { "  nothing tracked yet" } else { "  no PR matches" };
        lines.push(Line::from(filled(vec![Span::styled(text, c.s(Role::Muted).patch(bg))], w, bg)));
    }
    let rh = row_h(c, area.height) as usize;
    let sel = c.m.ui.selected_pr.as_ref().and_then(|id| prs.iter().position(|p| &p.id == id));
    let fits = (area.height as usize).saturating_sub(RAIL_HEAD as usize) / rh;
    let range = window(prs.len(), sel, fits);
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

fn pr_rows(c: &Ctx, pr: &Pr, w: usize, selected: bool, rh: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let bg = c.t.bg(Bg::Surface);
    let row_bg = if selected { c.t.bg(Bg::Select) } else { bg };
    let on = |role: Role| c.s(role).patch(row_bg);
    let marker = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
    let run = c.m.domain.latest_run(&pr.id);
    let running = run.is_some_and(|r| r.status == RunStatus::Running);
    let findings = current(c, pr);
    let (glyph, grole) = match findings.iter().map(|f| f.severity).min() {
        _ if running => (spinner(c), Role::Accent),
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
    let repo = pr.repository.as_str().rsplit('/').next().unwrap_or("").to_string();
    let left =
        vec![Span::styled(marker.to_string(), on(Role::Accent)), Span::styled(format!("   {repo} "), on(Role::Muted))];
    let right: Vec<Span<'static>> = match run {
        None => vec![Span::styled("not reviewed ", on(Role::Muted))],
        Some(r) if r.status == RunStatus::Running => {
            let rate = per_sec(c, Some(&pr.id));
            let mut s: Vec<Span<'static>> = stepper(c, r).into_iter().map(|s| s.patch_style(row_bg)).collect();
            s.push(Span::styled(format!(" {}", rate_cell(c, rate)), on(Role::Accent)));
            s.push(Span::styled(format!("{rate:>4.1}/s "), on(Role::Muted)));
            s
        }
        Some(r) if r.status == RunStatus::Failed => vec![Span::styled(format!("{} failed ", g.fail), on(Role::Fail))],
        Some(_) if findings.is_empty() => vec![Span::styled(format!("{} clean ", g.ok), on(Role::Ok))],
        Some(_) => counts(c, &findings, row_bg),
    };
    let sha_room = w.saturating_sub(width(&repo) + 4 + right.iter().map(|s| width(&s.content)).sum::<usize>());
    let mut left = left;
    if sha_room >= 9 && run.is_none_or(|r| r.status != RunStatus::Running) {
        left.push(Span::styled(pr.head_sha.short().to_string(), on(Role::Muted)));
    }
    let mut out = vec![Line::from(filled(first, w, row_bg)), Line::from(filled(split_row(left, right, w), w, row_bg))];
    if rh == 3 {
        out.push(Line::from(filled(vec![], w, bg)));
    }
    out
}

/// `◆1 ▲1 ●1 ○1 →2`: findings per severity, then how many are on GitHub.
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
    if published > 0 {
        s.push(Span::styled(format!("{}{published} ", c.t.g().arrow), c.s(Role::Ok).patch(bg)));
    }
    s
}
