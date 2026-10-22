//! The home's lists: the triage queue as cards, sectioned by tier, the done list, and the skeleton that holds their
//! place while the first snapshot loads.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::PublicationStatus;

use crate::kit::{Tone, button, icon, overline};
use crate::queue::{self, Card, Ending, Tier};
use crate::tokens::{Colors, layout, radius, severity_mark, space, text};
use crate::ui::*;

/// The column, with a bottom inset so the last card clears the edge, and a fade where more cards scroll under.
fn column(t: &Colors, id: &'static str, inner: Div) -> Div {
    div()
        .relative()
        .flex_none()
        .w(layout::QUEUE)
        .h_full()
        .border_r_1()
        .border_color(t.border)
        .child(
            div()
                .id(id)
                .size_full()
                .overflow_y_scroll()
                // The first label sits on the detail page's first line: both columns start 24 points down.
                .child(inner.flex().flex_col().gap(space::S4).px(space::S4).pt(space::S6).pb(space::S8 + space::S6)),
        )
        .child(div().absolute().bottom_0().left_0().right(px(1.)).h(space::S8).bg(linear_gradient(
            180.,
            linear_color_stop(t.bg.alpha(0.), 0.),
            linear_color_stop(t.bg, 1.),
        )))
}

/// Grey shapes where cards will be, while the first snapshot loads.
pub fn skeleton_column(t: &Colors) -> Div {
    let bone = |w: Pixels, h: Pixels| div().w(w).h(h).rounded(px(4.)).bg(t.border.alpha(0.7));
    let card = || {
        div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .p(space::S4)
            .rounded(radius::LG)
            .border_1()
            .border_color(t.border.alpha(0.6))
            .child(bone(px(120.), px(10.)))
            .child(bone(px(280.), px(14.)))
            .child(bone(px(220.), px(10.)))
    };
    column(
        t,
        "skeleton",
        div()
            .child(div().flex().items_center().gap(space::S2).child(overline(t, "Loading the queue")))
            .child(card())
            .child(card())
            .child(card()),
    )
}

pub fn queue_column(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let cards = queue::cards(m);
    let selected = queue::card_of(&cards, m.ui.selected_pr.as_ref());
    let mut col = div();
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
                    .when(i > 0, |d| d.pt(space::S2))
                    .child(overline(t, c.tier().label()))
                    .child(div().text_size(text::CAPTION).text_color(t.faint).child(n.to_string())),
            );
        }
        col = col.child(card(s, t, c, i, selected == Some(i), cx));
    }
    column(t, "column", col)
}

struct Face {
    glyph: Option<IconName>,
    mark: Option<&'static str>,
    kicker: String,
    headline: String,
    /// The top finding, quoted in the serif under the headline.
    quote: Option<String>,
    sub: String,
    meta: String,
    action: &'static str,
}

fn face(c: &Card, selected: bool) -> Face {
    match c {
        Card::Setup { missing, count, .. } => Face {
            glyph: Some(IconName::TriangleAlert),
            mark: None,
            kicker: "Setup".into(),
            headline: match missing.first() {
                Some(c) => format!("Reviews can't run: {}", c.detail),
                None => "This daemon is read-only".into(),
            },
            quote: None,
            sub: match missing.first().and_then(|c| c.fix.as_ref()) {
                Some(f) => crate::kit::sentence(&f.as_str().replace('`', "")),
                None => "It can look, but not start, retry, cancel or post. Start it with --allow-commands.".into(),
            },
            meta: count.words(),
            action: "Show setup",
        },
        Card::Approve { pr, findings, .. } => Face {
            glyph: Some(IconName::Send),
            mark: None,
            kicker: "Awaiting your approval".into(),
            headline: pr.title.to_string(),
            quote: None,
            sub: if findings.is_empty() {
                "A clean review: no findings. Nothing posts until you approve the proof.".into()
            } else {
                format!(
                    "Posts {} · {}. Nothing posts until you approve the proof.",
                    queue::count(findings.len(), "finding"),
                    queue::severity_summary(findings)
                )
            },
            meta: format!("Ready at {} · nothing posted yet", pr.head_sha.short()),
            action: "Review post",
        },
        Card::Posting { pr, status, .. } => Face {
            glyph: Some(IconName::CircleAlert),
            mark: None,
            kicker: "Posting needs a look".into(),
            headline: pr.title.to_string(),
            quote: None,
            sub: if *status == PublicationStatus::Failed {
                "GitHub refused the post. Open the proof to see what it said.".into()
            } else {
                "GitHub never confirmed the post. The daemon is checking whether the review is there.".into()
            },
            meta: String::new(),
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
                headline: pr.title.to_string(),
                quote: Some(top.title.to_string()),
                // The pane lists every finding once the card is selected, so the card stops counting them then.
                sub: if more.is_empty() || selected {
                    String::new()
                } else {
                    format!("Also open: {}", queue::severity_summary(more))
                },
                meta: format!(
                    "{}{} · {:.0}% sure · {} evidence",
                    file,
                    top.line.map_or(String::new(), |l| format!(":{l}")),
                    top.confidence * 100.,
                    top.evidence_count
                ),
                action: "Open finding",
            }
        }
        Card::Failed { pr, run } => {
            let reason = run.failure_reason.as_ref().map_or("No reason given".to_string(), |r| r.to_string());
            let (glyph, kicker, sub, meta, action) = match queue::ending(run) {
                Some(Ending::Cancelled) => (
                    IconName::CircleSlash,
                    "Cancelled",
                    "Stopped before it finished. Nothing was posted.".to_string(),
                    String::new(),
                    "Start again",
                ),
                Some(Ending::TooLarge) => (
                    IconName::CircleX,
                    "Too large to review",
                    reason,
                    "Next: exclude generated files or split the PR, then retry".into(),
                    "Retry",
                ),
                _ => (
                    IconName::CircleX,
                    "Review failed",
                    reason,
                    run.attempt.map_or(String::new(), |a| format!("Attempt {a}")),
                    "Retry",
                ),
            };
            Face {
                glyph: Some(glyph),
                mark: None,
                kicker: kicker.into(),
                headline: pr.title.to_string(),
                quote: None,
                sub,
                meta,
                action,
            }
        }
        Card::Start { pr } => Face {
            glyph: Some(IconName::Play),
            mark: None,
            kicker: "Not reviewed yet".into(),
            headline: pr.title.to_string(),
            quote: None,
            sub: String::new(),
            meta: format!("by {}", pr.author.as_ref().map_or("unknown", |a| a.as_str())),
            action: "Start review",
        },
    }
}

fn hue_of(t: &Colors, c: &Card) -> Hsla {
    match c {
        Card::Setup { .. } => t.warn,
        Card::Approve { .. } => t.accent,
        Card::Failed { run, .. } if queue::ending(run) == Some(Ending::Cancelled) => t.muted,
        Card::Posting { .. } | Card::Failed { .. } => t.danger,
        Card::Findings { findings, .. } => t.severity(findings[0].severity),
        Card::Start { .. } => t.muted,
    }
}

fn card(s: &Shell, t: &Colors, c: &Card, i: usize, selected: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let f = face(c, selected);
    // A selected card whose PR page fills the pane docks: its title, quote, reason and button are all on that page,
    // so the card keeps only its kicker and its meta line, and the title is said once beside the sidebar.
    let docked = selected && matches!(c, Card::Findings { .. } | Card::Failed { .. } | Card::Start { .. });
    let proof_open =
        selected && matches!(crate::ui::front(&s.model), Some(pr_review_tui::model::Modal::Publication { .. }));
    // On a read-only daemon a card's command stays visible, dimmed, with the reason in its tooltip.
    let command = match c {
        Card::Start { .. } => Some(pr_review_tui::keymap::Mutation::Start),
        Card::Failed { run, .. } => Some(queue::redo(run)),
        _ => None,
    };
    let refused =
        command.filter(|_| queue::read_only(&s.model)).map(|mu| crate::detail::why_not(&s.model, mu, f.action));
    let hue = hue_of(t, c);
    let place = c.pr().map(|p| format!("{} #{}", p.repository, p.number)).unwrap_or_default();
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
        .rounded(radius::LG)
        .border_1()
        .cursor_pointer()
        .bg(if selected { t.selected } else { t.surface })
        .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
        .shadow(vec![lift])
        .hover(move |d| d.bg(crate::kit::lift(t, if selected { t.selected } else { t.surface })))
        .active(|d| d.bg(t.press))
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
                .child(div().text_size(text::CAPTION).text_color(t.faint).child(place)),
        )
        .when(!docked, |d| {
            d.child(
                div()
                    .text_size(text::LEAD)
                    .line_height(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .text_ellipsis()
                    .line_clamp(2)
                    .child(f.headline),
            )
        })
        .children(f.quote.filter(|_| !docked).map(|q| {
            div()
                .font_family(text::SERIF)
                .italic()
                .text_size(text::LEAD)
                .line_height(px(21.))
                .text_color(t.text)
                .text_ellipsis()
                .line_clamp(2)
                .child(format!("\u{201c}{q}\u{201d}"))
        }))
        .when(!f.sub.is_empty() && !docked, |d| {
            d.child(
                div()
                    .text_size(text::SMALL)
                    .line_height(text::LINE_TIGHT)
                    .text_color(t.muted)
                    .text_ellipsis()
                    .line_clamp(2)
                    .child(crate::detail::md_line(t, &f.sub)),
            )
        })
        // The meta line and the action sit on their own rows, so neither ever cuts the other short.
        .when(!f.meta.is_empty(), |d| {
            d.child(div().text_size(text::SMALL).line_height(text::LINE_TIGHT).text_color(t.muted).child(f.meta))
        })
        // With its proof open beside it, a parked card's Review post has nothing left to open.
        .when(!docked && !(proof_open && matches!(c, Card::Approve { .. })), |d| {
            let b = match refused {
                Some(_) => crate::kit::locked_button(t, ("card-go", i), f.action),
                None => button(
                    t,
                    ("card-go", i),
                    f.action,
                    key,
                    if selected { Tone::Primary } else { Tone::Secondary },
                    on(cx, move |s, cx| s.primary(i, cx)),
                ),
            };
            d.child(div().flex().justify_end().pt(space::S1).child(match refused {
                Some(why) => crate::kit::tip(b, why),
                None => b,
            }))
        })
}

/// The reader is clear: said once, at the head of the done list, only by a window with a live snapshot.
fn clear(t: &Colors, m: &pr_review_tui::model::Model) -> Div {
    let reviewed = queue::done(m).len();
    let running = queue::running(m).len();
    let body = match (m.domain.prs.is_empty(), running) {
        (true, _) => {
            "Tracked PRs show up here as soon as the reviewer picks one up. Start one with pr-review review <url>."
                .to_string()
        }
        (false, 0) => format!(
            "{} reviewed. New decisions land here: a post to approve, a finding to judge, a failure to retry.",
            queue::count(reviewed, "PR")
        ),
        (false, r) => format!(
            "{} reviewed, {} running. Findings land here as the verifier keeps them.",
            queue::count(reviewed, "PR"),
            queue::count(r, "review")
        ),
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .pt(space::S4)
        .pb(space::S2)
        .child(
            div()
                .size(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::PILL)
                .bg(t.ok.alpha(0.12))
                .child(icon(IconName::Check, px(20.), t.ok)),
        )
        .child(
            div()
                .font_family(text::SERIF)
                .text_size(text::DISPLAY)
                .font_weight(FontWeight::SEMIBOLD)
                .child("You're clear"),
        )
        .child(div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(body))
}

pub fn done_column(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let done = queue::done(m);
    let mut col = div();
    if queue::cards(m).is_empty() {
        col = col.child(clear(t, m));
    }
    if done.is_empty() {
        return column(
            t,
            "column",
            col.child(
                div()
                    .pt(px(48.))
                    .text_center()
                    .text_size(text::BODY)
                    .text_color(t.muted)
                    .child("Finished reviews collect here."),
            ),
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
                .rounded(radius::LG)
                .border_1()
                .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
                .bg(if selected { t.selected } else { t.surface })
                .cursor_pointer()
                .hover(move |d| d.bg(crate::kit::lift(t, if selected { t.selected } else { t.surface })))
                .active(|d| d.bg(t.press))
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
    column(t, "column", col)
}
