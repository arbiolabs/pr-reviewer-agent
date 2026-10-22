//! The timeline: everything the reviewer did on a PR, in order, sectioned by the phase it happened in.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::EventCategory;
use pr_review_tui::protocol::{EventBody, Pr, RunChange, SequencedEvent};
use pr_review_tui::update::{Msg, event_text, is_heading, timeline_rows};
use pr_review_tui::view::Hit;

use crate::detail::{Bar, Item, bar_view, page, pr_header, pr_sentence, tabs};
use crate::kit::{Tone, callout, overline};
use crate::queue::phase_word;
use crate::tokens::{Colors, radius, severity_mark, space, text};
use crate::ui::{Shell, on};

/// The characters a tool summary keeps on its line at 1280 points; a longer one loses its middle.
const SUMMARY: usize = 72;

pub fn timeline(s: &Shell, t: &Colors, pr: &Pr, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let run = m.domain.latest_run(&pr.id);
    let findings = m.domain.pr_findings(&pr.id).into_iter().filter(|f| !f.stale).count();
    let mut body = div()
        .flex()
        .flex_col()
        .gap(space::S6)
        .child(pr_header(t, pr, overline(t, "Pull request"), pr_sentence(m, pr, run)))
        .child(tabs(t, findings, true, cx));
    let Some(tl) = &m.timeline else {
        return page(
            "detail-tl",
            body.child(callout(t, t.muted, gpui_kit::assets::IconName::Clock, "Loading the timeline…", None)),
            None,
            t,
            false,
        );
    };
    if let Some(e) = &tl.error {
        return page(
            "detail-tl",
            body.child(callout(
                t,
                t.danger,
                gpui_kit::assets::IconName::CircleX,
                "Couldn't load the timeline",
                Some(e.to_string()),
            )),
            None,
            t,
            false,
        );
    }
    let rows = timeline_rows(m, tl);
    let mut list = div().flex().flex_col();
    let mut last_phase = None;
    for (e, phase) in rows {
        if phase != last_phase {
            if let Some(p) = phase {
                list = list.child(
                    div()
                        .pt(space::S5)
                        .pb(space::S2)
                        .border_b_1()
                        .border_color(t.border)
                        .child(overline(t, phase_word(p))),
                );
            }
            last_phase = phase;
        }
        if is_heading(e) {
            continue;
        }
        let seq = e.sequence.get();
        let on_it = tl.selected == Some(seq);
        let row = div()
            .id(("ev-face", seq as usize))
            .relative()
            .flex()
            .items_start()
            .gap(space::S3)
            .px(space::S2)
            .py(px(6.))
            .rounded(radius::SM)
            .when(on_it, |d| d.bg(t.selected).child(crate::kit::rail(t)))
            .hover(move |d| d.bg(if on_it { crate::kit::lift(t, t.selected) } else { t.hover }))
            .active(|d| d.bg(t.press))
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .font_family(text::MONO)
                    .text_size(px(11.))
                    .pt(px(2.))
                    .text_color(t.muted)
                    .child(e.occurred_at.as_str().get(11..19).unwrap_or("").to_string()),
            )
            // The kind column holds a tool's name in place of the word "tool", so every row's words start at one x.
            .child(match &e.body {
                EventBody::ToolActivity(a) => div()
                    .w(px(104.))
                    .flex_none()
                    .truncate()
                    .pt(px(2.))
                    .font_family(text::MONO)
                    .text_size(px(11.))
                    .text_color(t.accent)
                    .child(a.tool.to_string()),
                _ => div()
                    .w(px(104.))
                    .flex_none()
                    .text_size(text::SMALL)
                    .text_color(t.muted)
                    .child(EventCategory::of(&e.body).label()),
            })
            .child(event_line(t, e));
        list = list.child(crate::kit::focusable(
            ("ev", seq as usize),
            row,
            t,
            radius::SM,
            Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| s.send(Msg::Pick(Hit::Event(seq)), cx))),
        ));
    }
    if tl.pending.is_some() {
        list = list.child(div().py(space::S3).text_size(text::SMALL).text_color(t.faint).child("Loading…"));
    }
    if tl.events.is_empty() {
        list = list
            .child(div().py(space::S3).text_size(text::BODY).text_color(t.muted).child("No events for this PR yet."));
    }
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(space::S1)
            .child(
                div()
                    .text_size(text::BODY)
                    .text_color(t.muted)
                    .child("Everything the reviewer did on this PR, in the order it happened."),
            )
            .child(list),
    );
    let footer = Bar {
        lead: vec![
            Item::new("tl-back", "Findings", "T", Tone::Secondary, on(cx, |s, cx| s.act(Action::Back, cx))),
            Item::new("tl-filter", "Filter", "F", Tone::Ghost, on(cx, |s, cx| s.act(Action::Search, cx))),
        ],
        note: None,
        trail: vec![Item::new("tl-gh", "Open on GitHub", "⌘⇧O", Tone::Ghost, on(cx, |s, cx| s.open_github(cx)))],
    };
    let footer = bar_view(s, t, footer, cx);
    page("detail-tl", body, Some(footer), t, false)
}

/// An event in words, its severity or tool set the way the rest of the window sets them.
fn event_line(t: &Colors, e: &SequencedEvent) -> Div {
    // Every row starts with the same 16-point glyph slot, so the words line up whether or not a row has a glyph.
    let glyph = |mark: Option<(&'static str, Hsla)>| {
        div()
            .w(px(16.))
            .flex_none()
            .text_center()
            .text_size(px(10.))
            .line_height(text::LINE_BODY)
            .children(mark.map(|(m, hue)| div().text_color(hue).child(m)))
    };
    let row = || div().flex_1().min_w_0().flex().items_start().gap(space::S2);
    let plain = row().child(glyph(None));
    let words = |s: String| {
        div()
            .flex_1()
            .min_w_0()
            .text_size(text::BODY)
            .line_height(text::LINE_BODY)
            .text_color(t.text)
            .text_ellipsis()
            .line_clamp(2)
            .child(s)
    };
    match &e.body {
        EventBody::Run(change, r) => plain.child(words(match change {
            RunChange::Started => format!("Review started at {}", r.head_sha.short()),
            RunChange::Completed => "Review completed".into(),
            RunChange::Superseded => "Superseded by a newer head".into(),
            RunChange::Failed => {
                format!("Review failed{}", r.failure_reason.as_ref().map_or(String::new(), |w| format!(": {w}")))
            }
        })),
        EventBody::PhaseChanged(p) => {
            plain.child(words(format!("{} → {}", phase_word(p.from), phase_word(p.to).to_lowercase())))
        }
        // The glyph already says the severity; a status other than new follows the title.
        EventBody::FindingUpdated(f) => {
            let (mark, _) = severity_mark(f.severity);
            let status = crate::detail::status_sentence(f, &[]).0;
            row().child(glyph(Some((mark, t.severity(f.severity))))).child(words(if status == "New" {
                f.title.to_string()
            } else {
                format!("{} · {}", f.title, status.to_lowercase())
            }))
        }
        EventBody::ToolActivity(a) => plain.child(words(crate::kit::middle(a.summary.as_str(), SUMMARY))),
        EventBody::PublicationChanged(p) => {
            plain.child(words(format!("Publication {}", format!("{:?}", p.status).to_lowercase())))
        }
        EventBody::PrUpdated(p) => plain.child(words(format!("PR updated, head now {}", p.head_sha.short()))),
        _ => plain.child(words(event_text(e))),
    }
}
