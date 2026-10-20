//! Diff, editorial: a file list on the left, the patch as a syntax-colored listing, and each finding written inline
//! under its anchor line like a review comment. Long lines soft-wrap (up to `WRAP_ROWS` rows) instead of scrolling
//! sideways, so no key hides code. Explicit truncation and staleness stay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::code::{highlight, wrap_spans};
use super::doc::{filled, spans_w, split_row};
use super::{Ctx, Width, window};
use crate::model::{DiffRow, DiffView, Pane, file_rows};
use crate::protocol::{FileStatus, Finding, LineKind};
use crate::text::{pad_left, width};
use crate::theme::{Bg, Role};
use crate::update::{diff_findings, row_findings};

pub fn diff(c: &Ctx, f: &mut Frame, area: Rect) {
    let Some(d) = &c.m.diff else {
        let text = if c.m.selected_pr().is_some() { "Press 3 to load the diff." } else { "No PR selected." };
        f.render_widget(
            Paragraph::new(vec![Line::raw(""), Line::from(c.span(format!("  {text}"), Role::Muted))]),
            area,
        );
        return;
    };
    let anchored = diff_findings(c.m, d);
    let tree_focus = c.focused(Pane::Sidebar);
    let (rail, body) = layout(c, area);
    if let Some(r) = rail {
        tree(c, f, r, d, &anchored, tree_focus);
    }
    if let Some(b) = body {
        let head = Rect { height: 3.min(b.height), ..b };
        f.render_widget(Paragraph::new(header(c, d, &anchored, b.width as usize)), head);
        hunks(c, f, listing(b), d, &anchored);
    }
}

/// The file list and the patch areas. Below the wide layout the code needs every column; h shows the file list in
/// its place.
fn layout(c: &Ctx, area: Rect) -> (Option<Rect>, Option<Rect>) {
    match c.width {
        Width::Narrow | Width::Medium if c.focused(Pane::Sidebar) => (Some(area), None),
        Width::Narrow | Width::Medium => {
            (None, Some(Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area }))
        }
        Width::Wide => {
            let rw = super::rail::rail_w(area.width);
            (Some(Rect { width: rw, ..area }), Some(Rect { x: area.x + rw + 2, width: area.width - rw - 3, ..area }))
        }
    }
}

/// The patch listing under the three header rows.
fn listing(b: Rect) -> Rect {
    Rect { y: b.y + 3, height: b.height.saturating_sub(3), ..b }
}

/// What a click at (`x`, `y`) on the Diff screen in `area` lands on: a file in the list or a row of the patch.
pub fn hit(c: &Ctx, area: Rect, x: u16, y: u16) -> Option<super::Hit> {
    let d = c.m.diff.as_ref()?;
    let (rail, body) = layout(c, area);
    if let Some(r) = rail.filter(|r| x < r.x + r.width) {
        let (range, rh) = tree_window(d, r);
        let i = range.start + (y.checked_sub(r.y + TREE_HEAD)? as usize) / rh;
        return (i < range.end).then(|| super::Hit::DiffFile(d.files[i].path.clone()));
    }
    let rest = listing(body?);
    let (_, owners) = listing_lines(c, rest, d, &diff_findings(c.m, d))?;
    owners.get(y.checked_sub(rest.y)? as usize).copied().flatten().map(super::Hit::DiffRow)
}

fn header(c: &Ctx, d: &DiffView, anchored: &[&Finding], w: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let pr = c.m.domain.prs.get(&d.key.pr);
    let mut first = vec![];
    if let Some(file) = d.file() {
        first.push(c.span(c.fit(file.path.as_str(), w.saturating_sub(24)), Role::Title));
        first.push(c.span(format!("   +{}", file.additions), Role::Add));
        first.push(c.span(format!(" -{}", file.deletions), Role::Del));
    }
    let number = pr.map_or(String::new(), |p| format!("{} #{}", p.repository, p.number));
    let mut second =
        vec![c.span(
            format!("{number} {} {} {} {}", g.sep, d.key.base.short(), g.arrow, d.key.head.short()),
            Role::Muted,
        )];
    if let Some(p) = pr.filter(|p| p.head_sha != d.key.head || p.base_sha != d.key.base) {
        second.push(c.span(format!("   historical revision; head is now {}", p.head_sha.short()), Role::Pending));
    }
    if d.truncated || d.files.iter().any(|f| f.truncated) {
        second.push(c.span("   TRUNCATED", Role::Pending));
    }
    if d.pending.is_some() {
        second.push(c.span(format!("   loading{}", g.ellipsis), Role::Pending));
    } else if let Some(e) = &d.error {
        second.push(c.span(format!("   diff unavailable: {e}"), Role::Fail));
    } else if let Some(q) = &d.search {
        let matches = crate::update::diff_matches(d, q);
        let here = d.selected_file.as_ref().and_then(|p| d.files.iter().position(|f| &f.path == p));
        let pos = match matches.iter().position(|m| Some(m.0) == here && m.1 == d.cursor) {
            Some(i) => format!("match {} of {} for \"{q}\"", i + 1, matches.len()),
            None => format!("{} matches for \"{q}\"", matches.len()),
        };
        second.push(c.span(format!("   {pos}"), Role::Accent));
    } else {
        let at = anchored.iter().position(|f| on_cursor(d, f));
        let pos = at.map_or(String::new(), |i| format!("finding {} of {}", i + 1, anchored.len()));
        second.push(c.span(format!("   {pos}"), Role::Accent));
    }
    // The least important notes go first rather than being cut mid-word.
    while second.len() > 1 && super::doc::spans_w(&second) > w {
        second.pop();
    }
    vec![Line::from(first), Line::from(second), Line::raw("")]
}

fn on_cursor(d: &DiffView, f: &Finding) -> bool {
    let Some(file) = d.file().filter(|file| file.path == f.file) else { return false };
    match (file_rows(file).get(d.cursor), f.line) {
        (Some(DiffRow::Line(l)), Some(line)) => l.new == Some(line.get()),
        (_, None) => d.cursor == 0,
        _ => false,
    }
}

/// What the file list says about a file's status; a plain modification says nothing.
fn status_word(s: FileStatus) -> &'static str {
    match s {
        FileStatus::Added => "new  ",
        FileStatus::Deleted => "deleted  ",
        FileStatus::Renamed => "renamed  ",
        FileStatus::Modified | FileStatus::Binary | FileStatus::Unknown => "",
    }
}

/// The file list: no panel, two rows a file like the PR rail, the selection a band with the accent bar.
fn tree(c: &Ctx, f: &mut Frame, area: Rect, d: &DiffView, anchored: &[&Finding], focused: bool) {
    let base = c.t.bg(Bg::Base);
    let w = area.width as usize;
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            c.span("  FILES", if focused { Role::Accent } else { Role::Muted }),
            c.span(format!("  {}", d.files.len()), Role::Muted),
        ]),
        Line::raw(""),
    ];
    let sel = d.selected_file.as_ref().and_then(|p| d.files.iter().position(|x| &x.path == p));
    let (range, rh) = tree_window(d, area);
    for i in range {
        let file = &d.files[i];
        let selected = Some(i) == sel;
        let st = if selected { c.t.bg(Bg::Select) } else { base };
        let on = |r: Role| c.s(r).patch(st);
        let marker = anchored.iter().filter(|x| x.file == file.path).map(|x| x.severity).min().map(|s| c.t.severity(s));
        let (name, dir) = match file.path.as_str().rsplit_once('/') {
            Some((dir, name)) => (name.to_string(), dir.to_string()),
            None => (file.path.to_string(), String::new()),
        };
        let bar = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
        let (glyph, grole) = marker.map_or((" ", Role::Muted), |(g, _, r)| (g, r));
        lines.push(Line::from(filled(
            vec![
                Span::styled(bar.to_string(), on(Role::Accent)),
                Span::styled(format!(" {glyph} "), on(grole)),
                Span::styled(c.fit(&name, w.saturating_sub(5)), on(if selected { Role::Title } else { Role::Normal })),
            ],
            w,
            st,
        )));
        let counts = match file.status {
            FileStatus::Binary => vec![Span::styled("binary ", on(Role::Muted))],
            _ => vec![
                Span::styled(format!("+{}", file.additions), on(Role::Add)),
                Span::styled(format!(" -{} ", file.deletions), on(Role::Del)),
            ],
        };
        let word = status_word(file.status);
        let room = w.saturating_sub(5 + width(word) + spans_w(&counts));
        let left = vec![
            Span::styled(bar.to_string(), on(Role::Accent)),
            Span::styled(format!("   {word}"), on(Role::Pending)),
            Span::styled(c.fit(&dir, room), on(Role::Muted)),
        ];
        lines.push(Line::from(filled(split_row(left, counts, w), w, st)).style(st));
        if rh == 3 {
            lines.push(Line::raw(""));
        }
    }
    if d.next_cursor.is_some() || d.truncated {
        lines.push(Line::from(c.span("  more files below", Role::Pending)));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Rows above the first file: blank, heading, blank.
const TREE_HEAD: u16 = 3;

/// Which files the list shows in `area`, and the rows each takes.
fn tree_window(d: &DiffView, area: Rect) -> (std::ops::Range<usize>, usize) {
    let sel = d.selected_file.as_ref().and_then(|p| d.files.iter().position(|x| &x.path == p));
    let rh = if d.files.len() * 3 + 4 <= area.height as usize { 3 } else { 2 };
    let h = (area.height as usize).saturating_sub(4);
    (window(d.files.len(), sel, h / rh), rh)
}

/// A code line soft-wraps onto at most this many rows, then ends in an ellipsis.
const WRAP_ROWS: usize = 4;
/// Columns before the code: severity marker, old and new line numbers, the sign.
const GUTTER: usize = 14;

fn hunks(c: &Ctx, f: &mut Frame, area: Rect, d: &DiffView, anchored: &[&Finding]) {
    let Some(file) = d.file() else { return };
    let w = area.width as usize;
    let note = |text: &str| Paragraph::new(vec![Line::from(c.span(c.fit(text, w), Role::Muted))]);
    if file.status == FileStatus::Binary {
        f.render_widget(note("Binary file: metadata only, not decoded or rendered."), area);
        return;
    }
    if file.hunks.is_empty() {
        let text = match file.status {
            FileStatus::Renamed => "Renamed with no content changes.",
            _ => "No patch available for this file: metadata only.",
        };
        f.render_widget(note(text), area);
        return;
    }
    let Some((mut lines, _)) = listing_lines(c, area, d, anchored) else { return };
    if file.truncated {
        let shown: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        let total = (file.additions + file.deletions) as usize;
        lines.push(Line::from(
            c.span(format!("TRUNCATED by the server: {shown} of about {total} changed lines shown"), Role::Pending),
        ));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The patch lines the listing shows in `area` around the cursor, and the file row each belongs to.
fn listing_lines(
    c: &Ctx,
    area: Rect,
    d: &DiffView,
    anchored: &[&Finding],
) -> Option<(Vec<Line<'static>>, Vec<Option<usize>>)> {
    let file = d.file().filter(|f| f.status != FileStatus::Binary && !f.hunks.is_empty())?;
    let w = area.width as usize;
    let rows = file_rows(file);
    let h = (area.height as usize).saturating_sub(usize::from(file.truncated));
    // Every row draws at least one line, so the rows within `h` of the cursor hold every line the window can show.
    let near = d.cursor.saturating_sub(h)..(d.cursor + h + 1).min(rows.len());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<usize>> = Vec::new();
    let mut at = 0;
    for (i, row) in rows.iter().enumerate().take(near.end).skip(near.start) {
        let cursor = i == d.cursor;
        if cursor {
            at = lines.len();
        }
        if matches!(row, DiffRow::Header(_)) && i > 0 {
            lines.push(Line::raw(""));
            owners.push(None);
        }
        let stack = row_findings(anchored, file, row);
        lines.extend(row_lines(c, row, stack.first().copied(), w, cursor && c.focused(Pane::Main)));
        lines.extend(annotations(c, &stack, w, cursor.then_some(d.stack)));
        owners.resize(lines.len(), Some(i));
    }
    let range = window(lines.len(), Some(at), h);
    let owners = owners.drain(range.clone()).collect();
    Some((lines.drain(range).collect(), owners))
}

/// The findings on one line, written under it like review comments: one row each, most severe first, in its own
/// severity color. At the cursor the one `Enter` opens is bright and a hint row follows.
fn annotations(c: &Ctx, stack: &[&Finding], w: usize, focus: Option<usize>) -> Vec<Line<'static>> {
    let bar = if c.t.ascii { "|" } else { "┃" };
    let pad = " ".repeat(GUTTER - 1);
    let mut out = Vec::new();
    for (k, x) in stack.iter().enumerate() {
        let (glyph, _, role) = c.t.severity(x.severity);
        let lit = focus.is_none_or(|f| f == k);
        let word = format!("{glyph} {:<9}", super::doc::sev_word(x.severity));
        out.push(Line::from(vec![
            c.span(format!("{pad}{bar} "), role),
            c.span(word, role),
            c.span(c.fit(x.title.as_str(), w.saturating_sub(GUTTER + 12)), if lit { Role::Title } else { Role::Muted }),
        ]));
    }
    if let (Some(k), Some(x)) = (focus, focus.and_then(|k| stack.get(k))) {
        let role = c.t.severity(x.severity).2;
        let hint = match stack.len() {
            1 => "Enter reads the finding   n next finding",
            _ if k + 1 < stack.len() => "Enter reads the bright one   n the next one here",
            _ => "Enter reads the bright one   n next finding",
        };
        out.push(Line::from(vec![c.span(format!("{pad}{bar} "), role), c.span(hint, Role::Muted)]));
    }
    out
}

fn row_lines(c: &Ctx, row: &DiffRow, hit: Option<&Finding>, w: usize, cursor: bool) -> Vec<Line<'static>> {
    match row {
        DiffRow::Header(h) => {
            let st = c.t.bg(if cursor { Bg::Select } else { Bg::Surface });
            let text = h.header.as_str();
            let (at, ctx) = match text.strip_prefix("@@").and_then(|t| t.split_once("@@")) {
                Some((at, ctx)) => (format!("@@ {} @@", at.trim()), ctx.trim().to_string()),
                None => (text.to_string(), String::new()),
            };
            // The ranges, then the function name; a long name wraps under itself the way code does.
            let room = w.saturating_sub(GUTTER - 1).max(1);
            let head = vec![
                Span::styled(format!("{at}  "), c.s(Role::Muted).patch(st)),
                Span::styled(c.fit(&ctx, (room * WRAP_ROWS).saturating_sub(width(&at) + 2)), c.s(Role::Hunk).patch(st)),
            ];
            wrap_spans(head, room)
                .into_iter()
                .map(|piece| {
                    let mut spans = vec![Span::styled(" ".repeat(GUTTER - 1), st)];
                    spans.extend(piece);
                    Line::from(filled(spans, w, st))
                })
                .collect()
        }
        DiffRow::Line(l) => {
            let bg = match (cursor, hit.is_some(), l.kind) {
                (true, _, _) => Bg::Select,
                (_, true, _) => Bg::Anchor,
                (_, _, LineKind::Add) => Bg::Add,
                (_, _, LineKind::Del) => Bg::Del,
                _ => Bg::Base,
            };
            let st = c.t.bg(bg);
            let on = |r: Role| c.s(r).patch(st);
            let marker = match hit {
                Some(x) => {
                    let (glyph, _, role) = c.t.severity(x.severity);
                    Span::styled(glyph.to_string(), on(role))
                }
                None => Span::styled(" ", st),
            };
            let num = |n: Option<u32>| pad_left(&n.map_or(String::new(), |n| n.to_string()), 4);
            let (sign, role) = match l.kind {
                LineKind::Add => ("+", Role::Add),
                LineKind::Del => ("-", Role::Del),
                LineKind::Context => (" ", Role::Muted),
                LineKind::Unknown => ("?", Role::Muted),
            };
            let room = w.saturating_sub(GUTTER);
            // Wrapped rows hang under the line's own indent, two deeper.
            let text = super::code::expand(l.text.as_str(), room * WRAP_ROWS);
            let indent = (text.len() - text.trim_start().len() + 2).min(room / 3);
            let code = highlight(c, l.text.as_str(), room * WRAP_ROWS, bg);
            let wrap = c.t.g().ellipsis;
            wrap_spans(code, room.saturating_sub(indent).max(1))
                .into_iter()
                .enumerate()
                .map(|(k, piece)| {
                    let mut spans = if k == 0 {
                        vec![
                            marker.clone(),
                            Span::styled(format!(" {} {} ", num(l.old), num(l.new)), on(Role::Muted)),
                            Span::styled(format!("{sign} "), on(role)),
                        ]
                    } else {
                        vec![Span::styled(format!("{:>w$} {:indent$}", wrap, "", w = GUTTER - 1), on(Role::Muted))]
                    };
                    spans.extend(piece);
                    Line::from(filled(spans, w, st))
                })
                .collect()
        }
    }
}
