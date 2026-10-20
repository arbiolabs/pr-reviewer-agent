//! Diff: file tree with counts, unified hunks with line numbers and finding markers, explicit truncation and staleness.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Ctx, Width, window};
use crate::model::{DiffRow, DiffView, Pane, file_rows};
use crate::protocol::{DiffFile, FileStatus, Finding, LineKind};
use crate::text::{code, pad_left, width};
use crate::theme::Role;
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
    let [head, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    f.render_widget(Paragraph::new(header(c, d, &anchored, head.width as usize)), head);

    let tree_focus = c.focused(Pane::Sidebar);
    match c.width {
        Width::Narrow if tree_focus => tree(c, f, body, d, &anchored),
        Width::Narrow => hunks(c, f, body, d, &anchored),
        w => {
            let tree_w = if w == Width::Wide { 36 } else { 30 };
            let [left, right] = Layout::horizontal([Constraint::Length(tree_w), Constraint::Min(20)]).areas(body);
            tree(c, f, left, d, &anchored);
            hunks(c, f, right, d, &anchored);
        }
    }
}

fn header(c: &Ctx, d: &DiffView, anchored: &[&Finding], w: usize) -> Vec<Line<'static>> {
    let g = c.t.g();
    let pr = c.m.domain.prs.get(&d.key.pr);
    let number = pr.map_or(d.key.pr.to_string(), |p| format!("#{}", p.number));
    let pinned = format!("{number} {} base {} {} head {}", g.sep, d.key.base.short(), g.arrow, d.key.head.short());
    let mut spans = vec![c.span(pinned, Role::Title)];
    if let Some(p) = pr.filter(|p| p.head_sha != d.key.head || p.base_sha != d.key.base) {
        let text = format!("  STALE: historical revision; the PR head is now {}", p.head_sha.short());
        spans.push(c.span(text, Role::Pending));
    }
    if d.truncated || d.files.iter().any(|f| f.truncated) {
        spans.push(c.span("  TRUNCATED", Role::Pending));
    }
    let mut second = Vec::new();
    if d.pending.is_some() {
        second.push(c.span("loading diff page...", Role::Pending));
    } else if let Some(e) = &d.error {
        second.push(c.span(format!("diff unavailable: {e}"), Role::Fail));
    } else {
        let others = c.m.domain.pr_findings(&d.key.pr).len() - anchored.len();
        let at = anchored.iter().position(|f| on_cursor(d, f));
        let pos = at.map_or(String::new(), |i| format!("finding {}/{}  ", i + 1, anchored.len()));
        let plural = if anchored.len() == 1 { "" } else { "s" };
        second.push(c.span(format!("{pos}{} finding{plural} on this revision", anchored.len()), Role::Muted));
        if others > 0 {
            second
                .push(c.span(format!(" {} {others} anchored to other revisions (not remapped)", g.sep), Role::Pending));
        }
    }
    let fit = |spans: Vec<Span<'static>>| {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        if width(&text) <= w { Line::from(spans) } else { Line::from(c.span(c.fit(&text, w), Role::Pending)) }
    };
    vec![fit(spans), fit(second)]
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

fn tree(c: &Ctx, f: &mut Frame, area: Rect, d: &DiffView, anchored: &[&Finding]) {
    let block = c.pane(&format!("FILES ({})", d.files.len()), c.focused(Pane::Sidebar));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let sel = d.selected_file.as_ref().and_then(|p| d.files.iter().position(|x| &x.path == p));
    let more = d.next_cursor.is_some() || d.truncated;
    let h = (inner.height as usize).saturating_sub(usize::from(more));
    let mut lines: Vec<Line> = window(d.files.len(), sel, h)
        .map(|i| {
            let file = &d.files[i];
            let marker =
                anchored.iter().filter(|x| x.file == file.path).map(|x| x.severity).min().map(|s| c.t.severity(s));
            let counts = match file.status {
                FileStatus::Binary => "bin".to_string(),
                _ => format!("+{} -{}", file.additions, file.deletions),
            };
            let cursor = if Some(i) == sel { c.t.g().cursor } else { " " };
            let tail = format!(" {counts}{}", if file.truncated { " trunc" } else { "" });
            let path_w = w.saturating_sub(4 + 2 + width(&tail));
            let mut spans = vec![
                Span::raw(format!("{cursor} ")),
                c.span(format!("{} ", status_letter(file.status)), Role::Muted),
                c.span(c.pad(file.path.as_str(), path_w), Role::Normal),
            ];
            match marker {
                Some((glyph, _, role)) => spans.push(c.span(format!("{glyph} "), role)),
                None => spans.push(Span::raw("  ")),
            }
            spans.push(c.span(tail, if file.truncated { Role::Pending } else { Role::Muted }));
            let line = Line::from(spans);
            if Some(i) == sel { line.style(c.s(Role::Selected)) } else { line }
        })
        .collect();
    if d.files.is_empty() && d.pending.is_none() && d.error.is_none() {
        lines.push(Line::from(c.span(" no changed files", Role::Muted)));
    }
    if more {
        let text = if d.pending.is_some() { " loading more files..." } else { " more files: scroll down to load" };
        lines.push(Line::from(c.span(c.fit(text, w), Role::Pending)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn hunks(c: &Ctx, f: &mut Frame, area: Rect, d: &DiffView, anchored: &[&Finding]) {
    let Some(file) = d.file() else {
        let block = c.pane("DIFF", c.focused(Pane::Main));
        f.render_widget(block, area);
        return;
    };
    let title = match &file.old_path {
        Some(old) => format!("{} (renamed from {old})", file.path),
        None => file.path.to_string(),
    };
    let block = c.pane(&c.fit(&title, area.width.saturating_sub(6) as usize), c.focused(Pane::Main));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let placeholder = |text: &str| Paragraph::new(vec![Line::raw(""), Line::from(c.span(c.fit(text, w), Role::Muted))]);
    if file.status == FileStatus::Binary {
        let text = "  Binary file: metadata only, not decoded or rendered.";
        f.render_widget(placeholder(text), inner);
        return;
    }
    if file.hunks.is_empty() {
        let text = match file.status {
            FileStatus::Renamed => "  Renamed with no content changes.",
            _ => "  No patch available for this file: metadata only.",
        };
        f.render_widget(placeholder(text), inner);
        return;
    }
    let rows = file_rows(file);
    let truncated_note = usize::from(file.truncated);
    let h = (inner.height as usize).saturating_sub(truncated_note);
    let mut lines: Vec<Line> = window(rows.len(), Some(d.cursor), h)
        .map(|i| row_line(c, file, &rows[i], anchored, w, i == d.cursor && c.focused(Pane::Main)))
        .collect();
    if file.truncated {
        let shown: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        let total = (file.additions + file.deletions) as usize;
        let text = format!(" TRUNCATED by the server: {shown} of about {total} changed lines shown");
        lines.push(Line::from(c.span(c.fit(&text, w), Role::Pending)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn row_line(c: &Ctx, file: &DiffFile, row: &DiffRow, anchored: &[&Finding], w: usize, cursor: bool) -> Line<'static> {
    let line = match row {
        DiffRow::Header(h) => Line::from(c.span(c.pad(&code(h.header.as_str()), w), Role::Hunk)),
        DiffRow::Line(l) => {
            let hit =
                anchored.iter().find(|x| x.file == file.path && x.line.map(|n| n.get()) == l.new && l.new.is_some());
            let marker = match hit {
                Some(x) => {
                    let (glyph, _, role) = c.t.severity(x.severity);
                    c.span(glyph, role)
                }
                None => Span::raw(" "),
            };
            let num = |n: Option<u32>| pad_left(&n.map_or(String::new(), |n| n.to_string()), 5);
            let (sign, role) = match l.kind {
                LineKind::Add => ("+", Role::Add),
                LineKind::Del => ("-", Role::Del),
                LineKind::Context => (" ", Role::Normal),
                LineKind::Unknown => ("?", Role::Muted),
            };
            let gutter = format!("{} {} {} ", num(l.old), num(l.new), c.t.g().vbar);
            let text_w = w.saturating_sub(1 + width(&gutter) + 1);
            Line::from(vec![
                marker,
                c.span(gutter, Role::Muted),
                c.span(sign, role),
                c.span(c.pad(&code(l.text.as_str()), text_w), role),
            ])
        }
    };
    if cursor { line.style(c.s(Role::Selected)) } else { line }
}
