//! The diff: the PR's files with their finding counts, the selected file's hunks with the cursor, and each finding
//! anchored under its line. A long line wraps with a continuation mark; it never clips.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::{DiffRow, file_rows};
use pr_review_tui::protocol::{FileStatus, LineKind};
use pr_review_tui::update::{Msg, diff_findings, row_findings};
use pr_review_tui::view::Hit;

use crate::code::Lexer;
use crate::detail::{action_bar, code_row, runs};
use crate::kit::{Tone, button, callout, focusable, icon, overline, severity_count};
use crate::tokens::{Colors, layout, radius, severity_mark, space, text};
use crate::ui::{Shell, on};

/// Rows drawn above the cursor, so the anchored line sits in the upper third.
const LEAD: usize = 8;
/// Rows drawn in all; the pane clips what doesn't fit.
const SPAN: usize = 64;

pub fn view(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let shell = div().flex().flex_1().min_h_0();
    let Some(d) = m.diff.as_ref() else {
        return shell.child(div().p(space::S8).child(callout(
            t,
            t.muted,
            IconName::LoaderCircle,
            "No diff loaded",
            None,
        )));
    };
    if let Some(e) = &d.error {
        return shell.child(div().p(space::S8).child(callout(
            t,
            t.danger,
            IconName::CircleX,
            "The diff didn't load",
            Some(e.to_string()),
        )));
    }
    if d.files.is_empty() {
        return shell.child(div().p(space::S8).child(callout(
            t,
            t.muted,
            IconName::LoaderCircle,
            "Loading the diff…",
            None,
        )));
    }
    let anchored = diff_findings(m, d);
    let files = div()
        .id("files")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(264.))
        .h_full()
        .overflow_y_scroll()
        .border_r_1()
        .border_color(t.border)
        .p(space::S3)
        .pb(space::S8)
        .gap(px(2.))
        .child(div().px(space::S2).pb(space::S2).child(overline(t, format!("{} files", d.files.len()))))
        .children(d.files.iter().enumerate().map(|(i, f)| {
            let selected = d.selected_file.as_ref() == Some(&f.path);
            let here: Vec<_> = anchored.iter().filter(|x| x.file == f.path).collect();
            let worst = here.iter().map(|x| x.severity).min();
            let path = f.path.clone();
            let (dir, name) = f.path.as_str().rsplit_once('/').unwrap_or(("", f.path.as_str()));
            let letter = match f.status {
                FileStatus::Added => "A",
                FileStatus::Deleted => "D",
                FileStatus::Renamed => "R",
                FileStatus::Binary => "B",
                _ => "M",
            };
            let line = || div().h(px(18.)).flex().items_center().flex_none();
            let row = div()
                .id(("file", i))
                .flex()
                .items_start()
                .gap(space::S2)
                .h(px(44.))
                .px(space::S2)
                .py(px(5.))
                .rounded(radius::SM)
                .cursor_pointer()
                .when(selected, |d| d.bg(t.selected))
                .hover(|d| d.bg(if selected { t.selected } else { t.hover }))
                .active(|d| d.bg(t.press))
                .on_click(
                    cx.listener(move |s, _: &ClickEvent, _, cx| s.send(Msg::Pick(Hit::DiffFile(path.clone())), cx)),
                )
                .child(
                    line()
                        .w(px(14.))
                        .text_size(text::CAPTION)
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.muted)
                        .child(letter),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(text::SMALL)
                                .line_height(px(18.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text)
                                .child(name.to_string()),
                        )
                        .child(div().truncate().text_size(text::CAPTION).text_color(t.muted).child(dir.to_string())),
                )
                .children(worst.map(|w| line().child(severity_count(t, w, here.len()))))
                .child(line().text_size(text::CAPTION).text_color(t.add_fg).child(format!("+{}", f.additions)))
                .child(line().text_size(text::CAPTION).text_color(t.del_fg).child(format!("−{}", f.deletions)));
            focusable(row, t)
        }));
    let Some(file) = d.file() else { return shell.child(files) };
    let rows = file_rows(file);
    let mut lx = Lexer::for_path(file.path.as_str());
    let from = d.cursor.saturating_sub(LEAD).min(rows.len().saturating_sub(1));
    // Lex the rows above the window too, so a comment or string opened there colors the rows shown.
    for row in rows.iter().take(from) {
        match row {
            DiffRow::Header(_) => lx = Lexer::for_path(file.path.as_str()),
            DiffRow::Line(l) => drop(lx.line(l.text.as_str())),
        }
    }
    let in_file: Vec<_> = anchored.iter().filter(|f| f.file == file.path).collect();
    let hunk_header = |h: &str, first: bool| {
        div()
            .flex()
            .items_center()
            .h(px(26.))
            .mt(if first { px(0.) } else { space::S2 })
            .px(space::S4)
            .bg(t.accent.alpha(0.06))
            .text_size(px(11.))
            .text_color(t.muted)
            .child(h.to_string())
    };
    let mut code =
        div().flex().flex_col().w_full().font_family(text::MONO).text_size(text::CODE).py(space::S2).pb(space::S8);
    // The window opens inside a hunk: its header still leads, so the reader knows where these rows sit.
    if !matches!(rows.get(from), Some(DiffRow::Header(_)))
        && let Some(DiffRow::Header(h)) = rows[..from].iter().rev().find(|r| matches!(r, DiffRow::Header(_)))
    {
        code = code.child(hunk_header(h.header.as_str(), true));
    }
    for (i, row) in rows.iter().enumerate().skip(from).take(SPAN) {
        let cursor = i == d.cursor;
        let here = row_findings(&anchored, file, row);
        let line: Div = match row {
            DiffRow::Header(h) => {
                lx = Lexer::for_path(file.path.as_str());
                hunk_header(h.header.as_str(), i == from)
            }
            DiffRow::Line(l) => {
                let (wash, sign, sign_fg) = match l.kind {
                    LineKind::Add => (Some(t.add_bg), "+", t.add_fg),
                    LineKind::Del => (Some(t.del_bg), "−", t.del_fg),
                    _ => (None, " ", t.faint),
                };
                let worst = here.first().map(|f| f.severity);
                let wash = worst.map(|s| t.severity_wash(s)).or(wash);
                let mark = worst.map(|s| t.severity(s)).or(cursor.then_some(t.accent));
                let mut block = div().flex().flex_col();
                for (k, (run, cont)) in runs(t, &mut lx, l.text.as_str(), layout::COLS_DIFF).into_iter().enumerate() {
                    block = block.child(
                        div()
                            .flex()
                            .child(div().w(px(40.)).flex_none().text_right().pr(space::S2).text_color(t.faint).child(
                                if k == 0 { l.old.map_or(String::new(), |n| n.to_string()) } else { String::new() },
                            ))
                            .child(div().w(px(14.)).flex_none().text_color(sign_fg).child(if k == 0 {
                                sign
                            } else {
                                " "
                            }))
                            .child(div().flex_1().min_w_0().child(code_row(t, l.new, run, cont, wash, mark))),
                    );
                }
                block
            }
        };
        let id = i;
        code = code.child(
            div()
                .id(("row", i))
                .relative()
                .cursor_pointer()
                .hover(|d| d.bg(t.hover))
                .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.send(Msg::Pick(Hit::DiffRow(id)), cx)))
                .child(line),
        );
        for f in here {
            let hue = t.severity(f.severity);
            let (mark, word) = severity_mark(f.severity);
            let fid = f.id.clone();
            code = code.child(
                div()
                    .id(SharedString::from(format!("anchor-{}", f.id.as_str())))
                    .flex()
                    .gap(space::S3)
                    .ml(px(57.))
                    .mr(space::S4)
                    .my(space::S2)
                    .p(space::S3)
                    .rounded(radius::LG)
                    .border_1()
                    .border_color(hue.alpha(0.7))
                    .bg(t.surface)
                    .shadow(vec![BoxShadow {
                        color: t.shadow.alpha(0.18),
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .font_family(text::UI)
                    .min_w_0()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover))
                    .active(|d| d.bg(t.press))
                    .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                        if s.model.ui.selected_finding.as_ref() != Some(&fid) {
                            s.send(Msg::Pick(Hit::Finding(fid.clone())), cx);
                        }
                        s.send(Msg::Pick(Hit::Finding(fid.clone())), cx);
                    }))
                    .child(div().w(px(3.)).rounded(px(2.)).bg(hue))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(text::SMALL)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hue)
                                    .child(div().text_size(px(10.)).child(mark))
                                    .child(format!("{word} · {}", f.category.label()))
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_color(t.faint)
                                            .font_weight(FontWeight::NORMAL)
                                            .child(format!("{:.0}% sure · ⏎ opens", f.confidence * 100.)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(text::BODY)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child(f.title.to_string()),
                            )
                            .children(s.model.details.get(&f.id).map(|x| {
                                div()
                                    .text_size(text::BODY)
                                    .line_height(text::LINE_BODY)
                                    .text_color(t.muted)
                                    .child(crate::detail::md_line(t, &crate::kit::lead(x.explanation.as_str(), 2)))
                            })),
                    ),
            );
        }
    }
    let stop = in_file.iter().position(|f| s.model.ui.selected_finding.as_ref() == Some(&f.id));
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S3)
        .h(px(44.))
        .px(space::S4)
        .border_b_1()
        .border_color(t.border)
        .child(icon(IconName::FileText, px(14.), t.faint))
        .child(div().font_family(text::MONO).text_size(text::SMALL).text_color(t.text).child(file.path.to_string()))
        .child(div().text_size(text::SMALL).text_color(t.add_fg).child(format!("+{}", file.additions)))
        .child(div().text_size(text::SMALL).text_color(t.del_fg).child(format!("−{}", file.deletions)))
        .child(div().flex_1())
        .child(div().text_size(text::SMALL).text_color(t.muted).child(match (stop, in_file.len()) {
            (_, 0) => "No findings in this file".to_string(),
            (Some(i), n) => format!("Finding {} of {n} here", i + 1),
            (None, n) => format!("{n} findings here"),
        }));
    let pane = div().flex().flex_col().flex_1().min_w_0().child(header).child(
        div()
            .id("diff-rows")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|s, e: &ScrollWheelEvent, w, cx| {
                let dy = e.delta.pixel_delta(w.line_height()).y;
                if dy == px(0.) {
                    return;
                }
                let a = if dy < px(0.) { Action::Down } else { Action::Up };
                for _ in 0..3 {
                    s.act(a, cx);
                }
            }))
            .child(code)
            .children(file.truncated.then(|| {
                div()
                    .px(space::S4)
                    .py(space::S2)
                    .text_size(text::SMALL)
                    .text_color(t.warn)
                    .child("This file is truncated: the daemon sent part of it.")
            })),
    );
    let bar = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .flex_1()
        .child(button(t, "d-back", "Back", Some("esc"), Tone::Secondary, on(cx, |s, cx| s.act(Action::Back, cx))))
        .child(button(
            t,
            "d-next",
            "Next finding",
            Some("N"),
            Tone::Secondary,
            (!in_file.is_empty()).then(|| crate::ui::click(cx, |s, cx| s.act(Action::NextFinding, cx))),
        ))
        .child(div().flex_1())
        .child(button(t, "d-gh", "Open on GitHub", Some("⌘⇧O"), Tone::Ghost, on(cx, |s, cx| s.open_github(cx))));
    div().flex().flex_col().flex_1().min_h_0().min_w_0().child(shell.child(files).child(pane)).child(action_bar(t, bar))
}
