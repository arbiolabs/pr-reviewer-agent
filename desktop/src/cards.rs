//! The home's lists: the triage queue as cards, sectioned by tier, and the done list.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::kit::{Tone, button, icon, overline};
use crate::queue::{self, Card, Tier};
use crate::tokens::{Colors, layout, radius, severity_mark, space, text};
use crate::ui::*;

fn column(t: &Colors) -> Stateful<Div> {
    div()
        .id("column")
        .flex()
        .flex_col()
        .flex_none()
        .w(layout::QUEUE)
        .h_full()
        .overflow_y_scroll()
        .border_r_1()
        .border_color(t.border)
        .px(space::S4)
        .py(space::S4)
        .gap(space::S2)
}

pub fn queue_column(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let cards = queue::cards(m);
    let selected = queue::card_of(&cards, m.ui.selected_pr.as_ref());
    let mut col = column(t);
    if cards.is_empty() {
        let (title, body) = if !m.link.accepts_events() && m.domain.prs.is_empty() {
            ("Not connected", "Nothing to show until the reviewer answers. The app retries on its own; ⌘R asks now.")
        } else if m.domain.prs.is_empty() {
            (
                "No PRs yet",
                "Tracked PRs show up here as soon as the reviewer picks one up. Start a review with pr-review review <url>.",
            )
        } else {
            (
                "Nothing needs you",
                "Reviews land here when they need a decision: a post to approve, a finding to judge, a failure to retry.",
            )
        };
        return col.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S3)
                .pt(px(120.))
                .px(space::S6)
                .child(
                    div()
                        .size(px(48.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(t.ok.alpha(0.12))
                        .child(icon(IconName::Check, px(22.), t.ok)),
                )
                .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(
                    div()
                        .text_center()
                        .text_size(text::BODY)
                        .line_height(text::LINE_BODY)
                        .text_color(t.muted)
                        .child(body),
                ),
        );
    }
    let mut tier: Option<Tier> = None;
    for (i, c) in cards.iter().enumerate() {
        if tier != Some(c.tier()) {
            tier = Some(c.tier());
            let n = cards.iter().filter(|x| x.tier() == c.tier()).count();
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .when(i > 0, |d| d.pt(space::S3))
                    .child(overline(t, c.tier().label()))
                    .child(div().text_size(text::CAPTION).text_color(t.faint).child(n.to_string())),
            );
        }
        col = col.child(card(s, t, c, i, selected == Some(i), cx));
    }
    col
}

struct Face {
    glyph: Option<IconName>,
    mark: Option<&'static str>,
    kicker: String,
    headline: String,
    sub: String,
    meta: String,
    action: &'static str,
}

fn face(c: &Card) -> Face {
    match c {
        Card::Setup { missing, commands_off } => Face {
            glyph: Some(IconName::TriangleAlert),
            mark: None,
            kicker: "Setup".into(),
            headline: match missing.first() {
                Some(c) => format!("Reviews can't run: {}", c.detail),
                None => "Review commands are off".into(),
            },
            sub: missing
                .first()
                .and_then(|c| c.fix.as_ref().map(|f| f.to_string()))
                .or(commands_off.map(String::from))
                .unwrap_or_default(),
            meta: match (missing.len(), commands_off) {
                (0, _) => "Read-only daemon".into(),
                (n, Some(_)) => format!("{} missing · commands off", n),
                (n, None) => format!("{} missing", n),
            },
            action: "Show setup",
        },
        Card::Approve { pr, findings, .. } => Face {
            glyph: Some(IconName::Send),
            mark: None,
            kicker: "Awaiting your approval".into(),
            headline: format!("Approve and post #{}", pr.number),
            sub: pr.title.to_string(),
            meta: if findings.is_empty() {
                "No findings · posts a clean review".into()
            } else {
                format!("{} · {}", queue::count(findings.len(), "finding"), queue::severity_summary(findings))
            },
            action: "Review post",
        },
        Card::Posting { pr, status, .. } => Face {
            glyph: Some(IconName::CircleAlert),
            mark: None,
            kicker: "Posting needs a look".into(),
            headline: format!(
                "Posting to #{} {}",
                pr.number,
                if *status == pr_review_tui::protocol::PublicationStatus::Failed { "failed" } else { "is unconfirmed" }
            ),
            sub: pr.title.to_string(),
            meta: "The daemon is checking GitHub".into(),
            action: "Open",
        },
        Card::Findings { pr, findings } => {
            let top = findings[0];
            let (mark, word) = severity_mark(top.severity);
            let more = &findings[1..];
            let file = top.file.as_str().rsplit('/').next().unwrap_or("");
            Face {
                glyph: None,
                mark: Some(mark),
                kicker: format!("{word} · {}", top.category.label()),
                headline: format!("{word} in #{}: {}", pr.number, top.title),
                sub: format!("{}{}", file, top.line.map_or(String::new(), |l| format!(":{l}"))),
                meta: if more.is_empty() {
                    format!("{:.0}% sure", top.confidence * 100.)
                } else {
                    format!(
                        "{:.0}% sure · +{} more: {}",
                        top.confidence * 100.,
                        more.len(),
                        queue::severity_summary(more)
                    )
                },
                action: "Open finding",
            }
        }
        Card::Failed { pr, run } => Face {
            glyph: Some(IconName::CircleX),
            mark: None,
            kicker: "Review failed".into(),
            headline: format!("Review failed on #{}: retry?", pr.number),
            sub: run.failure_reason.as_ref().map_or("No reason given".into(), |r| r.to_string()),
            meta: pr.title.to_string(),
            action: "Retry",
        },
        Card::Start { pr } => Face {
            glyph: Some(IconName::Play),
            mark: None,
            kicker: "Not reviewed yet".into(),
            headline: format!("Start a review of #{}", pr.number),
            sub: pr.title.to_string(),
            meta: format!("{} · by {}", pr.repository, pr.author.as_ref().map_or("unknown", |a| a.as_str())),
            action: "Start review",
        },
    }
}

fn hue_of(t: &Colors, c: &Card) -> Hsla {
    match c {
        Card::Setup { .. } => t.warn,
        Card::Approve { .. } => t.accent,
        Card::Posting { .. } | Card::Failed { .. } => t.danger,
        Card::Findings { findings, .. } => t.severity(findings[0].severity),
        Card::Start { .. } => t.muted,
    }
}

fn card(_: &Shell, t: &Colors, c: &Card, i: usize, selected: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let f = face(c);
    let hue = hue_of(t, c);
    let repo = c.pr().map(|p| p.repository.to_string()).unwrap_or_default();
    let ring = BoxShadow {
        color: t.accent.alpha(0.30),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
        inset: false,
    };
    let lift = BoxShadow {
        color: t.shadow.alpha(0.18),
        offset: point(px(0.), px(1.)),
        blur_radius: px(2.),
        spread_radius: px(0.),
        inset: false,
    };
    let key = if selected { Some("⏎") } else { None };
    div()
        .id(("card", i))
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(6.))
        .pl(space::S5)
        .pr(space::S4)
        .py(space::S3)
        .rounded(radius::MD)
        .border_1()
        .cursor_pointer()
        .bg(if selected { t.selected } else { t.surface })
        .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
        .shadow(if selected { vec![ring, lift] } else { vec![lift] })
        .hover(|d| {
            d.bg(if selected { t.selected } else { t.surface_hover }).border_color(if selected {
                t.accent
            } else {
                t.border_strong
            })
        })
        .active(|d| d.bg(t.surface_press))
        .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
            let cards = queue::cards(&s.model);
            let again = queue::card_of(&cards, s.model.ui.selected_pr.as_ref()) == Some(i);
            drop(cards);
            if again { s.primary(i, cx) } else { s.select_card(i, cx) }
        }))
        .child(div().absolute().left(px(7.)).top(space::S3).bottom(space::S3).w(px(3.)).rounded(px(2.)).bg(hue))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(f.glyph.map(|g| icon(g, px(12.), hue)))
                .children(f.mark.map(|mk| div().text_size(px(10.)).text_color(hue).child(mk)))
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .font_weight(FontWeight::BOLD)
                        .text_color(hue)
                        .child(f.kicker.to_uppercase()),
                )
                .child(div().flex_1())
                .child(div().text_size(text::CAPTION).text_color(t.faint).child(repo)),
        )
        .child(
            div()
                .text_size(text::LEAD)
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .line_clamp(2)
                .child(f.headline),
        )
        .child(
            div()
                .text_size(text::SMALL)
                .line_height(text::LINE_TIGHT)
                .text_color(t.muted)
                .line_clamp(2)
                .child(crate::detail::md_line(t, &f.sub)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .pt(space::S1)
                .child(div().flex_1().min_w_0().truncate().text_size(text::SMALL).text_color(t.faint).child(f.meta))
                .child(button(
                    t,
                    ("card-go", i),
                    f.action,
                    key,
                    if selected { Tone::Primary } else { Tone::Secondary },
                    on(cx, move |s, cx| s.primary(i, cx)),
                )),
        )
}

pub fn done_column(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let done = queue::done(m);
    let mut col = column(t);
    if done.is_empty() {
        return col.child(
            div()
                .pt(px(120.))
                .text_center()
                .text_size(text::BODY)
                .text_color(t.muted)
                .child("Finished reviews collect here."),
        );
    }
    col = col.child(overline(t, "Nothing left for you"));
    for (p, r) in done {
        let selected = m.ui.selected_pr.as_ref() == Some(&p.id);
        let id = p.id.clone();
        col = col.child(
            div()
                .id(SharedString::from(format!("done-{}", p.number)))
                .flex()
                .gap(space::S3)
                .p(space::S3)
                .rounded(radius::MD)
                .border_1()
                .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
                .bg(if selected { t.selected } else { t.surface })
                .cursor_pointer()
                .hover(|d| d.bg(t.surface_hover))
                .active(|d| d.bg(t.surface_press))
                .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.select_pr(id.clone(), cx)))
                .child(div().pt(px(2.)).child(icon(IconName::CircleCheck, px(15.), t.ok)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(text::BODY)
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(format!("#{} {}", p.number, p.title)),
                        )
                        .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
                            "{} · {}",
                            p.repository,
                            queue::outcome(m, p, r)
                        ))),
                ),
        );
    }
    col
}
