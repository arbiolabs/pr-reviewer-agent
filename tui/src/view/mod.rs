//! `view(&Model, &mut Frame)`: a pure function of the model. Layout follows TUI_SPEC.md section 4: wide at 110+
//! columns, medium at 80-109, narrow at 60-79, and a minimal mode below 60x16.

mod diff;
mod lists;
mod modals;
mod timeline;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

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
    /// A bordered pane; the focused one gets a bold border and a marker in its title.
    pub fn pane<'b>(&self, title: &str, focused: bool) -> Block<'b> {
        let marker = if focused { format!("{} ", self.t.g().cursor) } else { String::new() };
        Block::default()
            .borders(Borders::ALL)
            .border_set(self.t.border())
            .border_style(self.s(if focused { Role::Focus } else { Role::Border }))
            .title(Span::styled(format!(" {marker}{title} "), self.s(Role::Title)))
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

pub fn view(m: &Model, f: &mut Frame) {
    let area = f.area();
    let t = Theme { color: m.look.color, ascii: m.look.ascii };
    let c = Ctx { m, t, width: Width::of(area.width) };
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < MIN_W || area.height < MIN_H {
        minimal(&c, f, area);
        return;
    }
    let banner = banner(&c, area.width);
    let [title, tabs, banner_area, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(banner.is_some())),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(title_line(&c, title.width)), title);
    f.render_widget(Paragraph::new(tab_line(&c)), tabs);
    if let Some(b) = banner {
        f.render_widget(Paragraph::new(b), banner_area);
    }
    match m.ui.screen {
        Screen::Inbox => lists::inbox(&c, f, body),
        Screen::Review => lists::review(&c, f, body),
        Screen::Diff => diff::diff(&c, f, body),
        Screen::Timeline => timeline::timeline(&c, f, body),
        Screen::Settings => modals::settings(&c, f, body),
    }
    f.render_widget(Paragraph::new(footer_line(&c, footer.width)), footer);
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
    }
}

fn title_line(c: &Ctx, width: u16) -> Line<'static> {
    let (status, role) = link_status(c);
    let left = " PR REVIEWER ";
    let right = " ? help ";
    let endpoint = format!(" {} {} ", c.t.g().sep, c.m.diag.endpoint);
    let fixed = crate::text::width(left) + crate::text::width(&status) + crate::text::width(right);
    let room = (width as usize).saturating_sub(fixed);
    let endpoint = c.fit(&endpoint, room);
    let gap = room.saturating_sub(crate::text::width(&endpoint));
    Line::from(vec![
        c.span(left, Role::Title),
        c.span(status, role),
        c.span(endpoint, Role::Muted),
        Span::raw(" ".repeat(gap)),
        c.span(right, Role::Muted),
    ])
}

fn tab_line(c: &Ctx) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, s) in Screen::ALL.iter().enumerate() {
        let label = format!("{} {}", i + 1, s.label());
        let role = if *s == c.m.ui.screen { Role::Selected } else { Role::Muted };
        spans.push(c.span(format!(" {label} "), role));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
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
        (Some(Modal::Event { .. }), _) => "Esc close",
        (Some(Modal::Confirm { .. }), _) => "y confirm  n/Esc cancel",
        (_, Screen::Inbox) => "j/k move  Enter open  / search  d diff  t timeline  r refresh  ? help  q quit",
        (_, Screen::Review) => "j/k move  Tab pane  Enter inspect  d diff  t timeline  n/N finding  ? help  q quit",
        (_, Screen::Diff) => "j/k line  h/l pane  n/N finding  : jump  Enter finding  Esc back  ? help  q quit",
        (_, Screen::Timeline) => "j/k move  Enter detail  f scope  v category  P phase  Esc back  ? help  q quit",
        (_, Screen::Settings) => "r refresh  1-4 views  ? help  q quit",
    };
    Line::from(c.span(c.fit(&format!(" {hints}"), w), Role::Muted))
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
