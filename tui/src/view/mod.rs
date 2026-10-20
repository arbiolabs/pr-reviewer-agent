//! `view(&Model, &mut Frame)`: a pure function of the model. Layout follows docs/TUI_SPEC.md section 4: wide at 110+
//! columns, medium at 80-109, narrow at 60-79, and a minimal mode below 60x16.

mod code;
mod diff;
mod doc;
mod live;
mod modals;
mod rail;
mod timeline;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::{InputPurpose, Link, Modal, Model, Pane, Screen};
use crate::text::{fit, pad};
use crate::theme::{Role, Theme};

pub const MIN_W: u16 = 60;
pub const MIN_H: u16 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Narrow,
    Medium,
    Wide,
}

impl Width {
    fn of(w: u16) -> Width {
        match w {
            110.. => Width::Wide,
            80..=109 => Width::Medium,
            _ => Width::Narrow,
        }
    }
}

pub(crate) struct Ctx<'a> {
    pub m: &'a Model,
    pub t: Theme,
    pub width: Width,
}

impl Ctx<'_> {
    pub fn s(&self, role: Role) -> ratatui::style::Style {
        self.t.style(role)
    }
    pub fn span<'s>(&self, text: impl Into<String>, role: Role) -> Span<'s> {
        Span::styled(text.into(), self.s(role))
    }
    pub fn fit(&self, text: &str, cols: usize) -> String {
        fit(text, cols, self.t.g().ellipsis)
    }
    pub fn pad(&self, text: &str, cols: usize) -> String {
        pad(text, cols, self.t.g().ellipsis)
    }
    pub fn focused(&self, pane: Pane) -> bool {
        self.m.ui.focus == pane && self.m.top_modal().is_none()
    }
}

/// Lines `[start, end)` of a `len`-line list that keep `selected` in view, centered when possible.
pub(crate) fn window(len: usize, selected: Option<usize>, height: usize) -> std::ops::Range<usize> {
    if len <= height {
        return 0..len;
    }
    let start = selected.map_or(0, |s| s.saturating_sub(height / 2)).min(len - height);
    start..start + height
}

/// What a click lands on.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Pr(crate::protocol::PrId),
    Finding(crate::protocol::FindingId),
}

fn ctx(m: &Model, width: u16) -> Ctx<'_> {
    Ctx {
        m,
        t: Theme { color: m.look.color, ascii: m.look.ascii, terminal_bg: m.look.terminal_bg },
        width: Width::of(width),
    }
}

/// The title, spacer, banner, body and footer rows.
fn regions(c: &Ctx, area: Rect) -> [Rect; 5] {
    let banner = banner(c, area.width).is_some();
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(banner)),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area)
}

/// The view's own layout, recomputed for the reducer: what sits at screen cell (`x`, `y`). Modals take every click.
pub fn hit(m: &Model, x: u16, y: u16) -> Option<Hit> {
    let area = Rect::new(0, 0, m.size.0, m.size.1);
    let c = ctx(m, area.width);
    if area.width < MIN_W || area.height < MIN_H || m.top_modal().is_some() {
        return None;
    }
    let body = regions(&c, area)[3];
    if !matches!(m.ui.screen, Screen::Inbox | Screen::Review) || y < body.y || y >= body.y + body.height {
        return None;
    }
    doc::hit(&c, body, x, y)
}

pub fn view(m: &Model, f: &mut Frame) {
    let area = f.area();
    let c = ctx(m, area.width);
    let t = c.t;
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < MIN_W || area.height < MIN_H {
        minimal(&c, f, area);
        return;
    }
    f.render_widget(Paragraph::new("").style(t.bg(crate::theme::Bg::Base)), area);
    let banner = banner(&c, area.width);
    let [title, tabs, banner_area, body, footer] = regions(&c, area);
    f.render_widget(Paragraph::new(status_line(&c, title.width)), title);
    let _ = tabs;
    if let Some(b) = banner {
        f.render_widget(Paragraph::new(b), banner_area);
    }
    match m.ui.screen {
        Screen::Inbox | Screen::Review => doc::screen(&c, f, body),
        Screen::Diff => diff::diff(&c, f, body),
        Screen::Timeline => timeline::timeline(&c, f, body),
        Screen::Settings => modals::settings(&c, f, body),
    }
    f.render_widget(Paragraph::new(footer_line(&c, footer.width.saturating_sub(views_w(&c) as u16 + 2))), footer);
    let vw = views_w(&c) as u16;
    let vr = Rect { x: footer.x + footer.width.saturating_sub(vw), width: vw.min(footer.width), ..footer };
    f.render_widget(Paragraph::new(views_line(&c)), vr);
    for modal in &m.ui.modals {
        modals::modal(&c, f, body, modal);
    }
}

fn link_status(c: &Ctx) -> (String, Role) {
    let g = c.t.g();
    match &c.m.link {
        Link::Live => (format!("{} live", g.active), Role::Ok),
        Link::Syncing => (format!("{} loading snapshot", g.pending), Role::Pending),
        Link::Resyncing => (format!("{} resyncing", g.pending), Role::Pending),
        Link::Negotiating => (format!("{} negotiating", g.pending), Role::Pending),
        Link::Connecting { attempt } => (format!("{} connecting (attempt {attempt})", g.pending), Role::Pending),
        Link::Disconnected { .. } => (format!("{} disconnected", g.fail), Role::Fail),
        Link::Incompatible { .. } => (format!("{} incompatible", g.fail), Role::Fail),
        Link::Unusable { .. } => (format!("{} cannot connect", g.fail), Role::Fail),
    }
}

/// The quiet live line at the top: what the reviewer is doing right now, across every PR.
fn status_line(c: &Ctx, width: u16) -> Line<'static> {
    let w = width as usize;
    let g = c.t.g();
    let (link, link_role) = link_status(c);
    let right = vec![c.span(format!("{link}  "), link_role)];
    let mut left = vec![c.span("  pr-review", Role::Title), c.span(format!(" {} ", g.sep), Role::Muted)];
    let running: Vec<_> =
        c.m.domain.runs.values().filter(|r| r.status == crate::protocol::RunStatus::Running).collect();
    let newest = c.m.domain.activity.iter().rev().find(|a| running.iter().any(|r| a.pr_id.as_ref() == Some(&r.pr_id)));
    match (running.first(), newest) {
        (Some(_), Some(a)) => {
            let pr = a.pr_id.as_ref().and_then(|id| c.m.domain.prs.get(id));
            let run = pr.and_then(|p| c.m.domain.latest_run(&p.id));
            left.push(c.span(format!("{} ", doc::spinner(c)), Role::Accent));
            if let (Some(p), Some(r)) = (pr, run) {
                left.push(c.span(format!("reviewing #{} ", p.number), Role::Normal));
                left.push(c.span(format!("{} ", doc::phase_label(r.phase)), Role::Accent));
            }
            let used: usize = left.iter().chain(&right).map(|s| crate::text::width(&s.content)).sum();
            let more = match running.len() {
                1 => String::new(),
                n if w >= 100 => format!("  +{} more running", n - 1),
                n => format!("  +{}", n - 1),
            };
            let room = w.saturating_sub(used + crate::text::width(&more) + 2);
            left.push(c.span(whole(activity_words(&a.kind), room), Role::Muted));
            left.push(c.span(more, Role::Muted));
        }
        _ => left.push(c.span(format!("{} PRs tracked {} nothing running", c.m.domain.prs.len(), g.sep), Role::Muted)),
    }
    let used: usize = left.iter().chain(&right).map(|s| crate::text::width(&s.content)).sum();
    left.push(Span::raw(" ".repeat(w.saturating_sub(used))));
    left.extend(right);
    Line::from(left)
}

/// The longest of `options` (longest first) that fits in `room` columns, never cut mid-word.
fn whole(options: Vec<String>, room: usize) -> String {
    options.into_iter().find(|o| crate::text::width(o) <= room).unwrap_or_default()
}

/// What one activity says in the status line, from the most to the least detailed.
fn activity_words(kind: &crate::model::ActivityKind) -> Vec<String> {
    use crate::model::ActivityKind as K;
    match kind {
        K::Tool { tool, summary } => {
            let target = summary.as_str().split(": ").next().unwrap_or("");
            vec![format!("{tool} {summary}"), format!("{tool} {target}"), tool.to_string()]
        }
        K::Phase { to, .. } => vec![format!("entered {}", crate::theme::phase_name(*to))],
        K::Finding { title, .. } => vec![format!("found: {title}"), "new finding".into()],
        K::Run { .. } => vec!["started".into()],
    }
}

fn views_line(c: &Ctx) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, s) in Screen::ALL.iter().take(4).enumerate() {
        let on = *s == c.m.ui.screen || (*s == Screen::Review && c.m.ui.screen == Screen::Inbox && false);
        spans.push(c.span(format!("{}", i + 1), Role::Muted));
        spans.push(c.span(format!(" {}  ", s.label().to_lowercase()), if on { Role::Focus } else { Role::Muted }));
    }
    Line::from(spans)
}

fn views_w(c: &Ctx) -> usize {
    if c.m.size.0 < 100 { 0 } else { views_line(c).spans.iter().map(|s| crate::text::width(&s.content)).sum() }
}

fn banner(c: &Ctx, width: u16) -> Option<Line<'static>> {
    let g = c.t.g();
    let text = match &c.m.link {
        Link::Disconnected { reason, retry_in_ms } => {
            let retry =
                retry_in_ms.map_or(String::new(), |ms| format!(" {} retrying in {:.1}s", g.sep, ms as f64 / 1000.0));
            format!(
                " {} disconnected: {reason}{retry} {} showing last known state {} writes disabled",
                g.fail, g.sep, g.sep
            )
        }
        Link::Incompatible { supported, message } => format!(
            " {} protocol mismatch: {message}; server supports {supported:?}, this client speaks [1]. Upgrade the client or daemon.",
            g.fail
        ),
        Link::Unusable { message } => format!(" {} {message}", g.fail),
        Link::Resyncing => format!(" {} resyncing: fetching an authoritative snapshot; live events paused", g.pending),
        _ => return None,
    };
    let role = if c.m.link == Link::Resyncing { Role::Pending } else { Role::Fail };
    Some(Line::from(c.span(c.fit(&text, width as usize), role)))
}

fn footer_line(c: &Ctx, width: u16) -> Line<'static> {
    let w = width as usize;
    if let Some(Modal::Input { purpose, buffer, .. }) = c.m.top_modal() {
        let prompt = match purpose {
            InputPurpose::Search => " / search PRs: ",
            InputPurpose::JumpTo => " : jump to path[:line]: ",
        };
        let field = c.fit(&format!("{buffer}_"), w.saturating_sub(crate::text::width(prompt) + 22));
        return Line::from(vec![
            c.span(prompt, Role::Title),
            c.span(field, Role::Selected),
            c.span("  Enter apply  Esc cancel", Role::Muted),
        ]);
    }
    if let Some(n) = &c.m.notice {
        return Line::from(c.span(c.pad(&format!(" {}", n.text), w), Role::Pending));
    }
    let hints = match (c.m.top_modal(), c.m.ui.screen) {
        (Some(Modal::Help), _) => "Esc close  q quit",
        (Some(Modal::Finding { .. }), _) => "j/k scroll  d diff at anchor  t timeline  Esc close",
        (Some(Modal::Confirm { .. }), _) => "y confirm  n/Esc cancel",
        (_, Screen::Inbox) => "j/k pull request  Enter read  / search  ? help  q quit",
        (_, Screen::Review) if c.m.ui.focus == Pane::Sidebar => {
            "j/k pull request  Enter read  / search  l findings  ? help  q quit"
        }
        (_, Screen::Review) => {
            "j/k finding  Enter read  d diff  J/K scroll  h pull requests  t timeline  ? help  q quit"
        }
        (_, Screen::Diff) => "j/k line  n/N finding  Enter read  h files  : jump  Esc back  ? help",
        (_, Screen::Timeline) if c.m.timeline.as_ref().is_some_and(|t| t.open.is_some()) => {
            "j/k move  Enter fold or read  f scope  v kind  P phase  Esc fold  ? help"
        }
        (_, Screen::Timeline) => "j/k move  Enter detail  f scope  v kind  P phase  Esc back  ? help",
        (_, Screen::Settings) => "j/k account  Enter select account  r refresh  1-4 views  ? help  q quit",
    };
    let running = c.m.domain.runs.values().any(|r| r.status == crate::protocol::RunStatus::Running);
    let watch = running && c.m.top_modal().is_none();
    let mut hints: Vec<&str> = hints.split("  ").collect();
    if watch {
        hints.insert(2.min(hints.len()), "w watch live");
    }
    let mut out = String::from(" ");
    for hint in hints {
        if crate::text::width(&out) + crate::text::width(hint) + 2 <= w {
            out.push_str("  ");
            out.push_str(hint);
        }
    }
    Line::from(c.span(out, Role::Muted))
}

fn minimal(c: &Ctx, f: &mut Frame, area: Rect) {
    let (status, role) = link_status(c);
    let lines = vec![
        Line::from(vec![c.span("PR REVIEWER ", Role::Title), c.span(status, role)]),
        Line::from(c.span(format!("Terminal is {}x{}.", area.width, area.height), Role::Normal)),
        Line::from(c.span(format!("Resize to at least {MIN_W}x{MIN_H}."), Role::Pending)),
        Line::from(c.span("q quit  ? help", Role::Muted)),
    ];
    let lines: Vec<Line> = lines
        .into_iter()
        .map(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            let style = l.spans.last().map(|s| s.style).unwrap_or_default();
            Line::from(Span::styled(c.fit(&text, area.width as usize), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::{activity_words, whole};
    use crate::model::ActivityKind;
    use crate::protocol::Text;

    #[test]
    fn the_status_line_drops_whole_parts_instead_of_cutting_text() {
        let kind = ActivityKind::Tool {
            tool: Text::new("search_code"),
            summary: Text::new("\"returnTo\": 6 matches in 3 files"),
        };
        assert_eq!(whole(activity_words(&kind), 80), "search_code \"returnTo\": 6 matches in 3 files");
        assert_eq!(whole(activity_words(&kind), 30), "search_code \"returnTo\"");
        assert_eq!(whole(activity_words(&kind), 12), "search_code");
        assert_eq!(whole(activity_words(&kind), 5), "");
    }
}
