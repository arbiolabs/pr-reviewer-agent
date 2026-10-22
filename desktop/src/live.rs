//! A review at work: its phase, a scrubbable track of everything it did, the log at the point the track is held,
//! and the findings as they land.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{Activity, ActivityKind};
use pr_review_tui::protocol::{Finding, Pr, Run};
use pr_review_tui::update::can_mutate;

use crate::detail::*;
use crate::kit::{Gauge, OnClick, Tone, button, clock_s, overline, progress, step_of, tip};
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
    let stat = |value: String, label: &str| {
        div()
            .flex()
            .flex_col()
            .child(div().font_family(text::MONO).text_size(text::TITLE).font_weight(FontWeight::MEDIUM).child(value))
            .child(div().text_size(text::CAPTION).text_color(t.faint).child(label.to_string()))
    };
    let stats = div()
        .flex()
        .gap(space::S8)
        .child(stat(tools.to_string(), "tool calls"))
        .child(stat(
            if landed.is_empty() { "none yet".into() } else { queue::severity_summary(&landed) },
            "findings kept",
        ))
        .child(stat(total.to_string(), "events"));

    // The track: one tick per event, colored by kind, dim past the held point. Click one to hold the log there.
    let track = div().flex().items_end().gap(px(1.)).h(px(24.)).children(events.iter().enumerate().map(|(i, a)| {
        let (h, c) = match &a.kind {
            ActivityKind::Phase { .. } => (22., t.running),
            ActivityKind::Finding { severity, .. } => (18., t.severity(*severity)),
            ActivityKind::Run { .. } => (22., t.muted),
            ActivityKind::Tool { .. } => (10., t.faint),
        };
        let tick = div()
            .id(("tick", i))
            .flex_1()
            .h(px(24.))
            .flex()
            .items_end()
            .justify_center()
            .cursor_pointer()
            .hover(|st| st.bg(t.hover))
            .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.scrub = Some(i);
                cx.notify();
            }))
            .child(div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(c).opacity(if i <= head { 1. } else { 0.3 }))
            .when(held == Some(i), |d| d.border_b_2().border_color(t.text));
        tip(tick, format!("{} · {}", clock_s(a.at.as_str()), event_words(t, a).2))
    }));
    let following =
        div().flex().items_center().gap(space::S2).child(overline(t, "Activity")).child(div().flex_1()).child(
            match held {
                Some(i) => div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .font_family(text::MONO)
                            .text_size(text::CAPTION)
                            .text_color(t.muted)
                            .child(format!("replaying {} of {total}", i + 1)),
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
                None => div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(text::CAPTION)
                    .text_color(t.faint)
                    .child(div().size(px(6.)).rounded(radius::PILL).bg(t.ok))
                    .child(format!("following · {total} events · click the track to replay"))
                    .into_any_element(),
            },
        );

    // The log up to the held point, newest last, with a landed finding in its severity's wash.
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
            .when(current && fresh, |d| d.bg(t.running.alpha(0.08)))
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
                    .text_color(t.faint)
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
                .child(progress(t, step_of(run.phase), t.running, false, Gauge::Full))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
                    "{} · {} · run {}",
                    phase_word(run.phase),
                    run.model,
                    run.id.short()
                ))),
        )
        .child(stats)
        .child(div().flex().flex_col().gap(space::S2).child(following).child(track).child(log))
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
    let footer = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .flex_1()
        .child(
            div()
                .flex_1()
                .text_size(text::SMALL)
                .text_color(t.muted)
                .child(format!("{} · findings land here as the verifier keeps them.", phase_word(run.phase))),
        )
        .child(button(
            t,
            "l-cancel",
            "Cancel review",
            Some("C"),
            Tone::Danger,
            can_mutate(m, Mutation::Cancel).then(|| -> OnClick {
                Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Mutation(Mutation::Cancel), cx)))
            }),
        ));
    page("detail-live", body, Some(footer), t, false)
}

/// An event as its tag, the tag's hue and its words.
fn event_words(t: &Colors, a: &Activity) -> (String, Hsla, String) {
    match &a.kind {
        ActivityKind::Tool { tool, summary } => (tool.to_string(), t.accent, summary.to_string()),
        ActivityKind::Phase { to, .. } => ("phase".into(), t.running, format!("→ {}", phase_word(*to))),
        ActivityKind::Finding { title, severity, .. } => {
            let (mark, word) = severity_mark(*severity);
            (format!("{mark} {word}"), t.severity(*severity), format!("Finding landed: {title}"))
        }
        ActivityKind::Run { status, .. } => {
            ("run".into(), t.muted, format!("Run {}", format!("{status:?}").to_lowercase()))
        }
    }
}
