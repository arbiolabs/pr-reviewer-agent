//! A review at work: its phase, a quiet stream of what it does, and the findings as they land.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::ActivityKind;
use pr_review_tui::protocol::{Finding, Pr, Run};
use pr_review_tui::update::can_mutate;

use crate::detail::*;
use crate::kit::{OnClick, Tone, button, clock, clock_s, icon};
use crate::tokens::{Colors, motion, radius, severity_mark, space, text};
use crate::ui::{STEPS, Shell, live_dot, phase_word, step_of};

pub fn live(s: &Shell, t: &Colors, pr: &Pr, run: &Run, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let kicker = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .child(live_dot(t, s.still, "live-dot"))
        .child(
            div().text_size(text::CAPTION).font_weight(FontWeight::BOLD).text_color(t.running).child("REVIEWING NOW"),
        )
        .child(div().text_size(text::CAPTION).text_color(t.faint).child(format!(
            "{} · started {}",
            run.model,
            clock(run.started_at.as_str())
        )));
    let at = step_of(run.phase);
    let steps = div().flex().items_start().children(STEPS.iter().enumerate().map(|(i, (label, _))| {
        let (fill, ring, fg) = if i < at {
            (t.running, t.running, t.muted)
        } else if i == at {
            (t.running.alpha(0.18), t.running, t.text)
        } else {
            (t.bg, t.border_strong, t.faint)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(space::S2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().h(px(2.)).bg(if i == 0 {
                        t.bg.alpha(0.)
                    } else if i <= at {
                        t.running
                    } else {
                        t.border
                    }))
                    .child(
                        div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius::PILL)
                            .border_2()
                            .border_color(ring)
                            .bg(fill)
                            .when(i < at, |d| d.child(icon(IconName::Check, px(11.), t.on_accent)))
                            .when(i == at, |d| d.child(div().size(px(6.)).rounded(radius::PILL).bg(t.running))),
                    )
                    .child(div().flex_1().h(px(2.)).bg(if i == STEPS.len() - 1 {
                        t.bg.alpha(0.)
                    } else if i < at {
                        t.running
                    } else {
                        t.border
                    })),
            )
            .child(
                div()
                    .text_center()
                    .text_size(text::SMALL)
                    .font_weight(if i == at { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(fg)
                    .child(label.to_string()),
            )
    }));
    let recent: Vec<_> = m.domain.activity.iter().rev().filter(|a| a.pr_id.as_ref() == Some(&pr.id)).take(16).collect();
    let window_ms = 10_000;
    let rate = m
        .domain
        .activity
        .iter()
        .filter(|a| a.pr_id.as_ref() == Some(&pr.id) && m.now_ms.saturating_sub(a.recv_ms) < window_ms)
        .count();
    let stream = div().flex().flex_col().children(recent.into_iter().enumerate().map(|(i, a)| {
        let fresh = m.now_ms.saturating_sub(a.recv_ms) < motion::ARRIVAL_MS;
        let row = div()
            .flex()
            .items_center()
            .gap(space::S3)
            .h(px(30.))
            .px(space::S2)
            .rounded(radius::SM)
            .when(i == 0 && fresh, |d| d.bg(t.running.alpha(0.08)))
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .font_family(text::MONO)
                    .text_size(px(11.))
                    .text_color(t.faint)
                    .child(clock_s(a.at.as_str())),
            );
        match &a.kind {
            ActivityKind::Tool { tool, summary } => row
                .child(
                    div()
                        .w(px(96.))
                        .flex_none()
                        .font_family(text::MONO)
                        .text_size(px(11.))
                        .text_color(t.accent)
                        .child(tool.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(text::BODY)
                        .text_color(t.muted)
                        .child(summary.to_string()),
                ),
            ActivityKind::Phase { to, .. } => row.child(icon(IconName::ArrowRight, px(13.), t.running)).child(
                div()
                    .text_size(text::BODY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.running)
                    .child(phase_word(*to)),
            ),
            ActivityKind::Finding { title, severity, .. } => {
                let (mark, word) = severity_mark(*severity);
                row.bg(t.severity_wash(*severity))
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .text_size(text::SMALL)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.severity(*severity))
                            .child(format!("{mark} {word}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(text::BODY)
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Finding landed: {title}")),
                    )
            }
            ActivityKind::Run { status, .. } => row.child(
                div()
                    .text_size(text::BODY)
                    .text_color(t.muted)
                    .child(format!("Run {}", format!("{status:?}").to_lowercase())),
            ),
        }
    }));
    let landed: Vec<&Finding> = m.domain.findings.values().filter(|f| f.run_id == run.id).collect();
    let body = div()
        .flex()
        .flex_col()
        .gap(space::S6)
        .child(pr_header(t, pr, kicker))
        .child(steps)
        .child(section(t, &format!("Activity · {rate} steps in the last 10 seconds"), stream))
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
    page("detail-live", body, Some(footer), t)
}
