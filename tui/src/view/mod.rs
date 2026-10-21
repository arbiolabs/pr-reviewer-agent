//! `view(&Model, &mut Frame)`: a pure function of the model. Layout follows docs/TUI_SPEC.md section 4: wide at 110+
//! columns, medium at 80-109, narrow at 60-79, and a minimal mode below 60x16.

mod code;
mod diff;
mod doc;
mod live;
mod modals;
mod publish;
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
    /// A parked run's "Ready to publish" line.
    Ready,
    /// A file in the Diff screen's list, by path.
    DiffFile(crate::protocol::Text),
    /// A row of the selected file's patch: a hunk header, a line, or a finding written under it.
    DiffRow(usize),
    /// A timeline event's row, heading or open detail, by sequence.
    Event(u64),
    /// A view in the footer's list.
    View(Screen),
}

fn ctx(m: &Model, width: u16) -> Ctx<'_> {
    Ctx {
        m,
        t: Theme { color: m.look.color, ascii: m.look.ascii, terminal_bg: m.look.terminal_bg, palette: m.look.palette },
        width: Width::of(width),
    }
}

/// The title, spacer, banner, body and footer rows. Below 20 rows the spacer goes to the body.
fn regions(c: &Ctx, area: Rect) -> [Rect; 5] {
    let banner = banner(c, area.width).is_some();
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(area.height >= 20)),
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
    let [.., body, footer] = regions(&c, area);
    if y == footer.y {
        return view_at(&c, footer, x).map(Hit::View);
    }
    if y < body.y || y >= body.y + body.height {
        return None;
    }
    match m.ui.screen {
        Screen::Inbox | Screen::Review => doc::hit(&c, body, x, y),
        Screen::Diff => diff::hit(&c, body, x, y),
        Screen::Timeline => timeline::hit(&c, body, y).map(Hit::Event),
        Screen::Settings => None,
    }
}

/// The view whose number or name in the footer's list covers column `x`.
fn view_at(c: &Ctx, footer: Rect, x: u16) -> Option<Screen> {
    let vw = views_w(c) as u16;
    let mut at = (footer.x + footer.width.saturating_sub(vw)) as usize;
    for (i, pair) in views_line(c).spans.chunks(2).enumerate() {
        let w: usize = pair.iter().map(|s| crate::text::width(&s.content)).sum();
        if vw > 0 && (at..at + w).contains(&(x as usize)) {
            return Screen::ALL.get(i).copied();
        }
        at += w;
    }
    None
}

/// The largest useful scroll of the top page (help, finding reader, publication preview) at the current size.
pub fn page_scroll_limit(m: &Model) -> u16 {
    let area = Rect::new(0, 0, m.size.0, m.size.1);
    let c = ctx(m, area.width);
    let body = regions(&c, area)[3];
    let len = m.top_modal().and_then(|x| modals::page_lines(&c, body, x)).map_or(0, |l| l.len());
    modals::page_limit(len, body.height as usize) as u16
}

/// The largest useful `doc_scroll` at the current size, so J past the end does not pile up presses K must undo.
pub fn doc_scroll_limit(m: &Model) -> i32 {
    let area = Rect::new(0, 0, m.size.0, m.size.1);
    let c = ctx(m, area.width);
    doc::scroll_limit(&c, regions(&c, area)[3]).map_or(0, |n| n as i32)
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
    toast(&c, f, footer, body);
    if let Some(n) = m.notice.as_ref().filter(|_| m.top_modal().is_some()) {
        let row = if tabs.height > 0 { tabs } else { title };
        let line =
            c.span(c.pad(&c.fit(&format!(" {}", n.text), row.width as usize), row.width as usize), Role::Pending);
        f.render_widget(Paragraph::new(Line::from(line)), row);
    }
}

/// A notice too long for the footer beside the views takes the whole footer and wraps upward over the body's last
/// rows, instead of being cut.
fn toast(c: &Ctx, f: &mut Frame, footer: Rect, body: Rect) {
    let Some(n) = c.m.notice.as_ref().filter(|_| c.m.top_modal().is_none()) else {
        return;
    };
    let room = footer.width.saturating_sub(views_w(c) as u16 + 3) as usize;
    if crate::text::width(&n.text) <= room {
        return;
    }
    let w = footer.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = code::rich(c, &n.text, w, Role::Pending)
        .into_iter()
        .map(|mut l| {
            l.insert(0, Span::raw(" "));
            Line::from(l)
        })
        .collect();
    let h = (lines.len() as u16).min(body.height + 1);
    let area = Rect { y: footer.y + 1 - h, height: h, ..footer };
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(Paragraph::new(lines).style(c.t.bg(crate::theme::Bg::Base)), area);
}

fn link_status(c: &Ctx) -> (String, Role) {
    let g = c.t.g();
    match &c.m.link {
        Link::Live => (format!("{} live", g.active), Role::Ok),
        Link::Syncing => (format!("{} loading snapshot{}", g.pending, g.ellipsis), Role::Pending),
        Link::Resyncing => (format!("{} resyncing{}", g.pending, g.ellipsis), Role::Pending),
        Link::Stalled { .. } => (format!("{} snapshot timed out", g.fail), Role::Fail),
        Link::Negotiating => (format!("{} negotiating{}", g.pending, g.ellipsis), Role::Pending),
        Link::Connecting { attempt } => {
            (format!("{} connecting{} (attempt {attempt})", g.pending, g.ellipsis), Role::Pending)
        }
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
    let mut active: Vec<_> = c.m.domain.runs.values().filter(|r| r.active()).collect();
    active.sort_by_key(|r| c.m.domain.prs.get(&r.pr_id).map(|p| p.number));
    let parked: Vec<_> = c.m.domain.runs.values().filter(|r| r.parked()).collect();
    let newest = c.m.domain.activity.iter().rev().find(|a| active.iter().any(|r| a.pr_id.as_ref() == Some(&r.pr_id)));
    // The review the line names: the one with the newest activity, else the first running one.
    let named = newest.and_then(|a| active.iter().find(|r| a.pr_id.as_ref() == Some(&r.pr_id))).or(active.first());
    match named {
        // Off the stream, the cache is a photo: say how old it is rather than what it claims is running.
        _ if c.m.link != Link::Live => {
            let text = match (c.m.diag.last_snapshot_ms, &c.m.link) {
                (Some(at), _) => format!("last known state, {}s old", c.m.now_ms.saturating_sub(at) / 1000),
                (None, Link::Negotiating | Link::Syncing) => format!("waiting for the first snapshot{}", g.ellipsis),
                (None, _) => "not connected".into(),
            };
            left.push(c.span(text, Role::Muted));
        }
        Some(r) => {
            let pr = c.m.domain.prs.get(&r.pr_id);
            left.push(c.span(format!("{} ", doc::spinner(c)), Role::Accent));
            if let Some(p) = pr {
                left.push(c.span(format!("reviewing #{} ", p.number), Role::Normal));
            }
            left.push(c.span(format!("{} ", doc::phase_label(r.phase)), Role::Accent));
            // Each count keeps a noun or goes: the longest pair of notes that fits, then shorter, then none.
            let (others, n_parked) = (active.len() - 1, parked.len());
            let notes = |level: usize| {
                let more = match (others, level) {
                    (0, _) | (_, 2..) => String::new(),
                    (n, 0) => format!("  +{n} more running"),
                    (n, _) => format!("  +{n} more"),
                };
                let parked_note = match (n_parked, level) {
                    (0, _) | (_, 2..) => String::new(),
                    (n, 0) => format!("  {} {n} waiting for you", g.pending),
                    (n, _) => format!("  {} {n} waiting", g.pending),
                };
                (more, parked_note)
            };
            let used: usize = left.iter().chain(&right).map(|s| crate::text::width(&s.content)).sum();
            let width_of = |(a, b): &(String, String)| crate::text::width(a) + crate::text::width(b);
            // Below 100 columns the short notes leave the activity words room.
            let first = if w >= 100 { 0 } else { 1 };
            let fits = |n: &(String, String)| used + width_of(n) + 2 <= w;
            let chosen = (first..3).map(notes).find(fits).unwrap_or_else(|| notes(2));
            let room = w.saturating_sub(used + width_of(&chosen) + 2);
            let (more, parked_note) = chosen;
            let words = newest
                .filter(|a| a.pr_id.as_ref() == Some(&r.pr_id))
                .map_or(vec![], |a| activity_words(&a.kind, Some(r.phase)));
            left.push(c.span(whole(words, room), Role::Muted));
            left.push(c.span(more, Role::Muted));
            left.push(c.span(parked_note, Role::Pending));
        }
        // Nothing runs, but a review waits for the reader: amber, the one state that asks for them.
        None if !parked.is_empty() => {
            let mut numbers: Vec<u64> =
                parked.iter().filter_map(|r| c.m.domain.prs.get(&r.pr_id).map(|p| p.number)).collect();
            numbers.sort();
            let list = numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(" ");
            // The key that gets the reader there from here, if one does.
            let offered = crate::offer::offered(c.m);
            let key = ["p previews", "w shows it"].into_iter().find(|k| offered.iter().any(|h| k.starts_with(h.keys)));
            let key = key.map_or(String::new(), |k| format!(" {} {k}", g.sep));
            left.push(c.span(format!("{} {list} waiting for you{key}", g.pending), Role::Pending));
        }
        None => {
            let n = c.m.domain.prs.len();
            let prs = if n == 1 { "PR" } else { "PRs" };
            left.push(c.span(format!("{n} {prs} tracked {} nothing running", g.sep), Role::Muted))
        }
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

/// What one activity says in the status line, from the most to the least detailed. The line already names the run's
/// phase, so entering that phase says nothing more.
fn activity_words(kind: &crate::model::ActivityKind, phase: Option<crate::protocol::Phase>) -> Vec<String> {
    use crate::model::ActivityKind as K;
    match kind {
        K::Tool { tool, summary } => {
            let target = summary.as_str().split(": ").next().unwrap_or("");
            vec![format!("{tool} {summary}"), format!("{tool} {target}"), tool.to_string()]
        }
        K::Phase { to, .. } if Some(*to) == phase => vec![],
        K::Phase { to, .. } => vec![format!("entered {}", crate::theme::phase_name(*to))],
        K::Finding { title, .. } => vec![format!("found: {title}"), "new finding".into()],
        K::Run { .. } => vec!["started".into()],
    }
}

fn views_line(c: &Ctx) -> Line<'static> {
    let mut spans = Vec::new();
    // Settings joins the list once the footer has room for it beside the hints.
    let shown = if c.m.size.0 >= 120 { 5 } else { 4 };
    for (i, s) in Screen::ALL.iter().take(shown).enumerate() {
        let on = *s == c.m.ui.screen;
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
        // The reason is in the column and in Settings; the banner says only what the reader must know.
        Link::Disconnected { retry_in_ms, .. } => {
            let mut parts = vec![format!(" {} disconnected", g.fail)];
            parts.extend(retry_in_ms.map(|ms| format!("retry in {}s", ms.div_ceil(1000))));
            if c.m.diag.last_snapshot_ms.is_some() {
                parts.push("last known state".into());
            }
            parts.join(&format!(" {} ", g.sep))
        }
        Link::Incompatible { supported, message } => format!(
            " {} protocol mismatch: {message}; server supports {supported:?}, this client speaks [1]. Upgrade the client or daemon.",
            g.fail
        ),
        Link::Unusable { message } | Link::Stalled { reason: message } => format!(" {} {message}", g.fail),
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
            InputPurpose::TimelineFilter => " / filter events: ",
            InputPurpose::DiffSearch => " / search the diff: ",
        };
        let field = c.fit(&format!("{buffer}_"), w.saturating_sub(crate::text::width(prompt) + 22));
        return Line::from(vec![
            c.span(prompt, Role::Title),
            c.span(field, Role::Selected),
            c.span("  Enter apply  Esc cancel", Role::Muted),
        ]);
    }
    // Under a panel the footer is the panel's key line, and the toast shows below the title.
    if let Some(n) = c.m.notice.as_ref().filter(|_| c.m.top_modal().is_none()) {
        return Line::from(c.span(c.pad(&format!(" {}", n.text), w), Role::Pending));
    }
    let hints = crate::offer::fit(crate::offer::offered(c.m), w);
    Line::from(c.span(c.fit(&crate::offer::line(&hints), w), Role::Muted))
}

fn minimal(c: &Ctx, f: &mut Frame, area: Rect) {
    let (status, role) = link_status(c);
    let lines = vec![
        Line::from(vec![c.span("pr-review ", Role::Title), c.span(status, role)]),
        Line::from(c.span(format!("Terminal is {}x{}.", area.width, area.height), Role::Normal)),
        Line::from(c.span(format!("Resize to at least {MIN_W}x{MIN_H}."), Role::Pending)),
        Line::from(c.span("q quit", Role::Muted)),
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
        assert_eq!(whole(activity_words(&kind, None), 80), "search_code \"returnTo\": 6 matches in 3 files");
        assert_eq!(whole(activity_words(&kind, None), 30), "search_code \"returnTo\"");
        assert_eq!(whole(activity_words(&kind, None), 12), "search_code");
        assert_eq!(whole(activity_words(&kind, None), 5), "");
    }
}
