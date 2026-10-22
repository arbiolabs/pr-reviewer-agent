//! A review at work: its phase, a scrubbable track of everything it did, the log at the point the track is held,
//! and the findings as they land.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Mutation;
use pr_review_tui::model::{Activity, ActivityKind};
use pr_review_tui::protocol::{Finding, Pr, Run};

use crate::detail::*;
use crate::kit::{Gauge, OnClick, Tone, button, clock_s, overline, progress, step_of, tabular, tip};
use crate::queue::{self, phase_word};
use crate::tokens::{Colors, motion, radius, severity_mark, space, text};
use crate::ui::{Shell, live_dot};

/// How many log rows the page shows at once, newest last.
const SHOWN: usize = 14;

pub fn live(s: &Shell, t: &Colors, pr: &Pr, run: &Run, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let events: Vec<&Activity> = m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&pr.id)).collect();
    let total = events.len();
    let held = s.scrub.filter(|_| total > 0).map(|i| i.min(total - 1));
    let head = held.unwrap_or(total.saturating_sub(1));
    let kicker = div().flex().items_center().gap(space::S2).child(live_dot(t, s.still, "live-dot")).child(
        div().text_size(text::CAPTION).font_weight(FontWeight::BOLD).text_color(t.running).child("REVIEWING NOW"),
    );
    let landed: Vec<&Finding> = m.domain.findings.values().filter(|f| f.run_id == run.id).collect();
    let tools = events.iter().filter(|a| matches!(a.kind, ActivityKind::Tool { .. })).count();
    let stat = |value: String, label: String| {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(tabular(div()).text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child(value))
            .child(div().text_size(text::CAPTION).text_color(t.muted).child(label))
    };
    // Findings land in Verify: until then the count says where it stands, not only that it is zero.
    let noun = |n: usize, one: &str| queue::count(n, one).split_once(' ').map_or(String::new(), |(_, w)| w.into());
    let kept = if landed.is_empty() {
        if step_of(run.phase) < 3 { "findings kept · land in Verify".to_string() } else { "findings kept".into() }
    } else {
        format!("{} kept · {}", noun(landed.len(), "finding"), queue::severity_summary(&landed))
    };
    let stats = div()
        .flex()
        .gap(space::S6)
        .child(stat(tools.to_string(), noun(tools, "tool call")))
        .child(stat(landed.len().to_string(), kept))
        .child(stat(total.to_string(), noun(total, "event")));

    // Each tick is a 3-point mark in a slot at least 12 points wide, the whole slot a hit target; its height and
    // hue say its kind, and the legend under the track names them. While replaying, a handle stands on the
    // held tick.
    let track = div().id("track").flex().items_end().h(px(32.)).overflow_x_scroll().children(
        events.iter().enumerate().map(|(i, a)| {
            let (h, c) = tick_kind(t, a);
            let at = held == Some(i);
            let tick = div()
                .id(("tick", i))
                .relative()
                .flex_1()
                .min_w(px(12.))
                .h(px(32.))
                .flex()
                .items_end()
                .justify_center()
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|st| st.bg(t.hover))
                .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                    s.scrub = Some(i);
                    cx.notify();
                }))
                .child(div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(c).opacity(if i <= head { 1. } else { 0.3 }))
                .when(at, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .left(relative(0.5))
                            .ml(px(-5.))
                            .size(px(10.))
                            .rounded(radius::PILL)
                            .bg(t.text)
                            .border_2()
                            .border_color(t.bg),
                    )
                    .child(div().absolute().top(px(8.)).bottom_0().left(relative(0.5)).ml(px(-1.)).w(px(2.)).bg(t.text))
                });
            tip(tick, format!("{} · {}", clock_s(a.at.as_str()), event_words(t, a).2))
        }),
    );
    let legend = div().flex().items_center().gap(space::S4).text_size(text::CAPTION).text_color(t.muted).children(
        [("Phase", 14., t.running), ("Finding", 11., t.high), ("Run", 14., t.muted), ("Tool call", 7., t.faint)].map(
            |(word, h, c)| {
                div().flex().items_end().gap(px(5.)).child(div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(c)).child(word)
            },
        ),
    );
    // The row keeps one height whether it follows or replays, so entering replay moves nothing.
    let following = div()
        .flex()
        .items_center()
        .h(px(32.))
        .gap(space::S2)
        .child(overline(t, "Activity"))
        .child(div().flex_1())
        .child(match held {
            Some(i) => div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(
                    tabular(div())
                        .text_size(text::CAPTION)
                        .text_color(t.muted)
                        .child(format!("Replaying {} of {total}", i + 1)),
                )
                .child(button(
                    t,
                    "follow-live",
                    "Follow live",
                    None,
                    Tone::Secondary,
                    Some(Box::new(cx.listener(|s, _: &ClickEvent, _, cx| {
                        s.scrub = None;
                        cx.notify();
                    })) as OnClick),
                ))
                .into_any_element(),
            None => tabular(div())
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(text::CAPTION)
                .text_color(t.muted)
                .child(div().size(px(6.)).rounded(radius::PILL).bg(t.ok))
                .child(format!("Following · {} · click the track to replay", queue::count(total, "event")))
                .into_any_element(),
        });

    let lo = (head + 1).saturating_sub(SHOWN);
    let log = div().flex().flex_col().children(events.iter().enumerate().take(head + 1).skip(lo).map(|(i, a)| {
        let current = i == head;
        let fresh = held.is_none() && m.now_ms.saturating_sub(a.recv_ms) < motion::ARRIVAL_MS;
        let (tag, tag_hue, words) = event_words(t, a);
        let landed_hue = match &a.kind {
            ActivityKind::Finding { severity, .. } => Some(t.severity_wash(*severity)),
            _ => None,
        };
        let row = div()
            .id(("log", i))
            .flex()
            .items_center()
            .gap(space::S3)
            .h(px(30.))
            .px(space::S2)
            .rounded(radius::SM)
            .when(current, |d| d.bg(t.selected))
            .when_some(landed_hue, |d, h| d.bg(h))
            .when(matches!(a.kind, ActivityKind::Phase { .. }), |d| d.border_t_1().border_color(t.border))
            // The newest row wears the running hue on its left edge: this is where the reviewer is now.
            .when(current && held.is_none(), |d| d.border_l_2().border_color(t.running).bg(t.running.alpha(0.08)))
            .cursor_pointer()
            .hover(|st| st.bg(t.hover))
            .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.scrub = Some(i);
                cx.notify();
            }))
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .font_family(text::MONO)
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child(clock_s(a.at.as_str())),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .truncate()
                    .font_family(if matches!(a.kind, ActivityKind::Tool { .. }) { text::MONO } else { text::UI })
                    .text_size(if matches!(a.kind, ActivityKind::Tool { .. }) { px(11.) } else { text::SMALL })
                    .font_weight(if matches!(a.kind, ActivityKind::Tool { .. }) {
                        FontWeight::NORMAL
                    } else {
                        FontWeight::SEMIBOLD
                    })
                    .text_color(tag_hue)
                    .child(tag),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(text::BODY)
                    .font_weight(if landed_hue.is_some() { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(if current || landed_hue.is_some() { t.text } else { t.muted })
                    .child(words),
            );
        if current && fresh && !s.still {
            row.with_animation(("arrive", i), Animation::new(motion::FADE), |d, v| d.opacity(0.4 + 0.6 * v))
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }));

    let body = div()
        .flex()
        .flex_col()
        .gap(space::S6)
        .child(pr_header(t, pr, kicker, pr_sentence(m, pr, Some(run))))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S3)
                .p(space::S4)
                .rounded(radius::LG)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(progress(t, queue::marks(m, run), t.running, Gauge::Full))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
                    "{} · {} · run {}",
                    phase_word(run.phase),
                    run.model,
                    run.id.short()
                ))),
        )
        .child(stats)
        .child(div().flex().flex_col().gap(space::S2).child(following).child(track).child(legend).child(log))
        .child(section(
            t,
            &format!("Findings so far · {}", landed.len()),
            if landed.is_empty() {
                div()
                    .text_size(text::BODY)
                    .text_color(t.muted)
                    .child("None yet. Candidates appear here once the verifier keeps them.")
            } else {
                div().flex().flex_col().gap(space::S2).children(landed.into_iter().map(|f| finding_row(s, t, f, cx)))
            },
        ));
    let footer = Bar {
        lead: vec![],
        note: Some((
            gpui_kit::assets::IconName::Activity,
            format!("{} · findings land as the verifier keeps them.", phase_word(run.phase)),
        )),
        trail: vec![mutation_item(s, "l-cancel", "Cancel review", "C", Tone::Danger, Mutation::Cancel, cx)],
    };
    let footer = bar_view(s, t, footer, cx);
    page("detail-live", body, Some(footer), t, false)
}

/// A tick's height and hue, by its event's kind.
fn tick_kind(t: &Colors, a: &Activity) -> (f32, Hsla) {
    match &a.kind {
        ActivityKind::Phase { .. } => (26., t.running),
        ActivityKind::Finding { severity, .. } => (20., t.severity(*severity)),
        ActivityKind::Run { .. } => (26., t.muted),
        ActivityKind::Tool { .. } => (12., t.faint),
    }
}

/// An event as its tag, the tag's hue and its words.
fn event_words(t: &Colors, a: &Activity) -> (String, Hsla, String) {
    match &a.kind {
        ActivityKind::Tool { tool, summary } => (tool.to_string(), t.accent, crate::kit::middle(summary.as_str(), 64)),
        ActivityKind::Phase { to, .. } => ("phase".into(), t.running, format!("→ {}", phase_word(*to))),
        ActivityKind::Finding { title, severity, .. } => {
            let (mark, word) = severity_mark(*severity);
            (format!("{mark} {word}"), t.severity(*severity), format!("Finding landed: {title}"))
        }
        ActivityKind::Run { status, .. } => ("run".into(), t.muted, crate::queue::run_words(*status).into()),
    }
}
