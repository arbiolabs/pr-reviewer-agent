//! Diff, editorial: a file rail on the left, the patch as a syntax-colored listing, and each finding written inline
//! under its anchor line like a review comment. Explicit truncation and staleness stay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::code::highlight;
use super::{Ctx, Width, window};
use crate::model::{DiffRow, DiffView, Pane, file_rows};
use crate::protocol::{DiffFile, FileStatus, Finding, LineKind};
use crate::text::{pad_left, width};
use crate::theme::{Bg, Role};
use crate::update::diff_findings;

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
    let (rail, body) = match c.width {
        Width::Narrow if tree_focus => (Some(area), None),
        Width::Narrow => (None, Some(area)),
        w => {
            let rw = if w == Width::Wide { 34 } else { 28 };
            (Some(Rect { width: rw, ..area }), Some(Rect { x: area.x + rw + 2, width: area.width - rw - 3, ..area }))
        }
    };
    if let Some(r) = rail {
        tree(c, f, r, d, &anchored, tree_focus);
    }
    if let Some(b) = body {
        let head = Rect { height: 3.min(b.height), ..b };
        f.render_widget(Paragraph::new(header(c, d, &anchored, b.width as usize)), head);
        let rest = Rect { y: b.y + 3, height: b.height.saturating_sub(3), ..b };
        hunks(c, f, rest, d, &anchored);
    }
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
        second.push(c.span("   loading…", Role::Pending));
    } else if let Some(e) = &d.error {
        second.push(c.span(format!("   diff unavailable: {e}"), Role::Fail));
    } else {
        let at = anchored.iter().position(|f| on_cursor(d, f));
        let pos = at.map_or(String::new(), |i| format!("finding {} of {}", i + 1, anchored.len()));
        second.push(c.span(format!("   {pos}"), Role::Accent));
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

fn status_letter(s: FileStatus) -> &'static str {
    match s {
        FileStatus::Added => "A",
        FileStatus::Modified => "M",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
        FileStatus::Binary => "B",
        FileStatus::Unknown => "?",
    }
}

fn tree(c: &Ctx, f: &mut Frame, area: Rect, d: &DiffView, anchored: &[&Finding], focused: bool) {
    let bg = c.t.bg(Bg::Surface);
    let w = area.width as usize;
    let fill = |spans: Vec<Span<'static>>, st| {
        let used: usize = spans.iter().map(|s| width(&s.content)).sum();
        let mut spans = spans;
        spans.push(Span::styled(" ".repeat(w.saturating_sub(used)), st));
        Line::from(spans)
    };
    let mut lines = vec![
        fill(vec![], bg),
        fill(
            vec![Span::styled(
                format!("  FILES  {}", d.files.len()),
                c.s(if focused { Role::Accent } else { Role::Muted }).patch(bg),
            )],
            bg,
        ),
        fill(vec![], bg),
    ];
    let sel = d.selected_file.as_ref().and_then(|p| d.files.iter().position(|x| &x.path == p));
    let h = (area.height as usize).saturating_sub(4);
    for i in window(d.files.len(), sel, h / 2) {
        let file = &d.files[i];
        let selected = Some(i) == sel;
        let st = if selected { c.t.bg(Bg::Select) } else { bg };
        let on = |r: Role| c.s(r).patch(st);
        let marker = anchored.iter().filter(|x| x.file == file.path).map(|x| x.severity).min().map(|s| c.t.severity(s));
        let (name, dir) = match file.path.as_str().rsplit_once('/') {
            Some((dir, name)) => (name.to_string(), format!("{dir}/")),
            None => (file.path.to_string(), String::new()),
        };
        let bar = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
        let (glyph, grole) = marker.map_or((" ", Role::Muted), |(g, _, r)| (g, r));
        lines.push(fill(
            vec![
                Span::styled(bar.to_string(), on(Role::Accent)),
                Span::styled(format!(" {glyph} "), on(grole)),
                Span::styled(c.fit(&name, w.saturating_sub(5)), on(if selected { Role::Title } else { Role::Normal })),
            ],
            st,
        ));
        let counts = match file.status {
            FileStatus::Binary => "binary".to_string(),
            _ => format!("+{} -{}", file.additions, file.deletions),
        };
        lines.push(fill(
            vec![
                Span::styled(bar.to_string(), on(Role::Accent)),
                Span::styled(format!("   {} ", status_letter(file.status)), on(Role::Muted)),
                Span::styled(c.fit(&dir, w.saturating_sub(7 + counts.len())), on(Role::Muted)),
                Span::styled(format!(" {counts}"), on(Role::Muted)),
            ],
            st,
        ));
    }
    if d.next_cursor.is_some() || d.truncated {
        lines.push(fill(vec![Span::styled("  more files below", c.s(Role::Pending).patch(bg))], bg));
    }
    while lines.len() < area.height as usize {
        lines.push(fill(vec![], bg));
    }
    f.render_widget(Paragraph::new(lines), area);
}

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
    let rows = file_rows(file);
    let h = (area.height as usize).saturating_sub(usize::from(file.truncated));
    let range = window(rows.len(), Some(d.cursor), h.saturating_sub(3));
    let mut lines: Vec<Line> = Vec::new();
    for i in range {
        let cursor = i == d.cursor && c.focused(Pane::Main);
        lines.push(row_line(c, file, &rows[i], anchored, w, cursor));
        // A finding reads like a review comment under the line it is about.
        if let DiffRow::Line(l) = &rows[i]
            && let Some(x) =
                anchored.iter().find(|x| x.file == file.path && l.new.is_some() && x.line.map(|n| n.get()) == l.new)
        {
            let (glyph, _, role) = c.t.severity(x.severity);
            let bar = if c.t.ascii { "|" } else { "┃" };
            let pad = " ".repeat(14);
            lines.push(Line::from(vec![
                c.span(format!("{pad}{bar} "), role),
                c.span(format!("{glyph} {} ", super::doc::sev_word(x.severity)), role),
                c.span(c.fit(x.title.as_str(), w.saturating_sub(30)), Role::Title),
            ]));
            lines.push(Line::from(vec![
                c.span(format!("{pad}{bar} "), role),
                c.span("Enter reads the finding   n next finding", Role::Muted),
            ]));
        }
    }
    lines.truncate(h);
    if file.truncated {
        let shown: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        let total = (file.additions + file.deletions) as usize;
        lines.push(Line::from(
            c.span(format!("TRUNCATED by the server: {shown} of about {total} changed lines shown"), Role::Pending),
        ));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn row_line(c: &Ctx, file: &DiffFile, row: &DiffRow, anchored: &[&Finding], w: usize, cursor: bool) -> Line<'static> {
    let g = c.t.g();
    match row {
        DiffRow::Header(h) => {
            let text = h.header.as_str();
            let ctx = text.rsplit_once("@@").map_or("", |x| x.1).trim();
            let at = text.split("@@").nth(1).unwrap_or("").trim();
            Line::from(vec![
                c.span(format!("{:>11} {} ", "", g.ellipsis), Role::Muted),
                c.span(format!("{at}  "), Role::Muted),
                c.span(c.fit(ctx, w.saturating_sub(width(at) + 16)), Role::Type),
            ])
        }
        DiffRow::Line(l) => {
            let hit =
                anchored.iter().find(|x| x.file == file.path && l.new.is_some() && x.line.map(|n| n.get()) == l.new);
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
            let mut spans = vec![
                marker,
                Span::styled(format!(" {} {} ", num(l.old), num(l.new)), on(Role::Muted)),
                Span::styled(format!("{sign} "), on(role)),
            ];
            let used: usize = spans.iter().map(|s| width(&s.content)).sum();
            spans.extend(highlight(c, l.text.as_str(), w.saturating_sub(used), bg));
            let used: usize = spans.iter().map(|s| width(&s.content)).sum();
            spans.push(Span::styled(" ".repeat(w.saturating_sub(used)), st));
            Line::from(spans)
        }
    }
}
