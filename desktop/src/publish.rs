//! The proof: exactly what approving a parked review would post, and what happened once approved.

use gpui_kit::assets::IconName;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::{ConfirmState, DiffKey, Model, Preview};
use pr_review_tui::protocol::{PublicationPreview, PublicationStatus, RunId};
use pr_review_tui::update::{can_confirm, publish_block, published_url};

use crate::code::Lexer;
use crate::detail::*;
use crate::kit::{OnClick, Tone, button, callout, icon, summary};
use crate::queue;
use crate::tokens::{Colors, layout, radius, space, text};
use crate::ui::{Overlay, Shell, on};

const MARKER: &str = "<!-- durable-review:";

/// Markdown as GitHub will render it, or with `raw` the exact text that will be posted.
fn markdown(t: &Colors, src: &str, raw: bool) -> Div {
    let mut out = div().flex().flex_col().gap(space::S1).text_size(text::BODY).line_height(text::LINE_BODY);
    if raw {
        return out.font_family(text::MONO).text_size(text::CODE).children(src.lines().map(|l| {
            div()
                .min_h(text::LINE_CODE)
                .text_color(if l.starts_with(MARKER) { t.faint } else { t.text })
                .child(l.to_string())
        }));
    }
    for line in src.lines() {
        if line.starts_with(MARKER) {
            continue;
        }
        out = out.child(
            if let Some(h) = line.trim_start_matches('#').strip_prefix(' ').filter(|_| line.starts_with('#')) {
                div().pt(space::S1).text_size(text::LEAD).font_weight(FontWeight::BOLD).child(md_line(t, h))
            } else if let Some(item) = line.strip_prefix("- ") {
                div()
                    .flex()
                    .gap(space::S2)
                    .pl(space::S1)
                    .child(div().text_color(t.faint).child("•"))
                    .child(div().flex_1().child(md_line(t, item)))
            } else if line.trim().is_empty() {
                div().h(space::S2)
            } else {
                div().child(md_line(t, line))
            },
        );
    }
    if !raw && src.contains(MARKER) {
        out = out.child(
            div()
                .pt(space::S2)
                .mt(space::S1)
                .border_t_1()
                .border_color(t.border)
                .text_size(text::CAPTION)
                .text_color(t.faint)
                .child("Also sent, and hidden by GitHub: a durable-review marker comment."),
        );
    }
    out
}

/// The kind of review, as GitHub names it.
fn event_words(event: &str) -> &'static str {
    match event {
        "APPROVE" => "an approval",
        "REQUEST_CHANGES" => "a request for changes",
        "COMMENT" => "a comment, not an approval or a request for changes",
        _ => "a review",
    }
}

pub fn publication(
    s: &Shell,
    t: &Colors,
    run_id: &RunId,
    preview: &Preview,
    publish: &ConfirmState,
    raw: bool,
    cx: &Context<Shell>,
) -> Div {
    let m = &s.model;
    let run = m.domain.runs.get(run_id);
    let pr = run.and_then(|r| m.domain.prs.get(&r.pr_id));
    let posted = m.domain.publications.get(run_id).map(|x| x.status);
    let done = posted == Some(PublicationStatus::Published);
    let url = published_url(m, run_id);
    let kicker = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .child(icon(
            if done { IconName::CircleCheck } else { IconName::Send },
            px(12.),
            if done { t.ok } else { t.accent },
        ))
        .child(
            div()
                .text_size(text::CAPTION)
                .font_weight(FontWeight::BOLD)
                .text_color(if done { t.ok } else { t.accent })
                .child(if done { "PUBLISHED" } else { "PROOF · NOTHING IS POSTED YET" }),
        )
        .children(url.as_ref().filter(|_| done).map(|u| {
            div().font_family(text::MONO).text_size(text::CAPTION).text_color(t.faint).truncate().child(u.clone())
        }));
    let target = pr.map_or("this PR".to_string(), |p| format!("{} #{}", p.repository, p.number));
    let mut body = div().flex().flex_col().gap(space::S6);
    let p = match preview {
        Preview::Ready(p) => p,
        Preview::Loading(_) => {
            return page(
                "detail-pub",
                body.child(kicker).child(callout(
                    t,
                    t.muted,
                    IconName::LoaderCircle,
                    "Loading the stored review…",
                    None,
                )),
                None,
                t,
                false,
            );
        }
        Preview::Failed(e) => {
            return page(
                "detail-pub",
                body.child(kicker).child(callout(
                    t,
                    t.danger,
                    IconName::CircleAlert,
                    "The proof is unavailable",
                    Some(format!("{e}. ⌘R asks again.")),
                )),
                None,
                t,
                false,
            );
        }
    };
    let n = p.comments.len();
    let sentence = if done {
        format!("As {} at {}: one review with {}.", who(m), p.head_sha.short(), queue::count(n, "inline comment"))
    } else {
        format!(
            "One review with {}, posted as {}, at {}. It posts as {}, and only when you choose Post.",
            queue::count(n, "inline comment"),
            event_words(p.event.as_str()),
            p.head_sha.short(),
            who(m)
        )
    };
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(kicker)
            .child(crate::kit::display(
                t,
                if done { format!("Posted to {target}") } else { format!("What will be posted to {target}") },
            ))
            .child(summary(t, sentence)),
    );
    if let Some(c) = state_callout(m, t, p, publish, posted) {
        body = body.child(c);
    }
    let frame = |head: Div, content: Div| {
        div()
            .flex()
            .flex_col()
            .rounded(radius::LG)
            .border_1()
            .border_color(t.border_strong)
            .bg(t.surface)
            .overflow_hidden()
            .child(
                head.flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(34.))
                    .px(space::S4)
                    .border_b_1()
                    .border_color(t.border)
                    .bg(t.raised)
                    .text_size(text::SMALL)
                    .text_color(t.muted),
            )
            .child(content.p(space::S4))
    };
    let who = who(m);
    body = body.child(section(
        t,
        "Review body",
        frame(
            div()
                .child(icon(IconName::Bot, px(13.), t.faint))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(who.clone()))
                .child(if done { "commented" } else { "will comment" })
                .child(div().flex_1())
                .child(button(
                    t,
                    "p-raw",
                    if raw { "Raw markdown" } else { "As GitHub shows it" },
                    Some("M"),
                    Tone::Ghost,
                    on(cx, |s, cx| s.act(Action::ToggleRaw, cx)),
                )),
            markdown(t, p.body.as_str(), raw),
        ),
    ));
    let key = run.map(|r| DiffKey { pr: r.pr_id.clone(), base: r.base_sha.clone(), head: r.head_sha.clone() });
    let mut comments = div().flex().flex_col().gap(space::S3);
    for cm in &p.comments {
        let context = key.as_ref().and_then(|k| diff_context(m, k, cm.path.as_str(), cm.line.get()));
        let mut content = div().flex().flex_col().gap(space::S3);
        if let Some(lines) = context {
            let mut lx = Lexer::for_path(cm.path.as_str());
            let mut rows = div().py(space::S1).font_family(text::MONO).text_size(text::CODE);
            for (n, l, hit) in lines {
                rows = rows.children(code_line(
                    t,
                    &mut lx,
                    &l,
                    layout::COLS_WIDE,
                    Some(n),
                    None,
                    hit.then(|| t.accent.alpha(0.12)),
                    hit.then_some(t.accent),
                ));
            }
            content = content.child(
                div().rounded(radius::SM).border_1().border_color(t.border).bg(t.code_bg).overflow_hidden().child(rows),
            );
        }
        content = content.child(markdown(t, cm.body.as_str(), raw));
        comments = comments.child(frame(
            div()
                .child(icon(IconName::FileText, px(13.), t.faint))
                .child(
                    div()
                        .font_family(text::MONO)
                        .text_size(px(11.))
                        .text_color(t.text)
                        .child(format!("{}:{}", cm.path, cm.line)),
                )
                .child(div().flex_1())
                .child(if cm.side.as_str() == "LEFT" { "old side" } else { "new side" }),
            content,
        ));
    }
    if n == 0 {
        comments = comments
            .child(div().text_size(text::BODY).text_color(t.muted).child("None: the summary is the whole review."));
    }
    body = body.child(section(t, &format!("Inline comments · {n}"), comments));

    // While the sheet asks, the page's own buttons step back, so the sheet holds the only live controls.
    let asking = matches!(s.overlay, Some(Overlay::ConfirmPost));
    let ready = can_confirm(m) && !asking;
    let footer = if done {
        Bar {
            lead: vec![],
            note: None,
            trail: vec![Item::new(
                "p-view",
                "View on GitHub",
                "⌘⇧O",
                Tone::Primary,
                url.map(|_| -> OnClick {
                    Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::OpenUrl, cx)))
                }),
            )],
        }
    } else {
        let post = match ready {
            true => Item::new(
                "p-post",
                "Post to GitHub…",
                "⌘⏎",
                Tone::Primary,
                Some(Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.post(cx))) as OnClick),
            ),
            false => Item {
                why: (!asking).then(|| match publish_block(m, p) {
                    Some(_) if m.server.as_ref().and_then(|s| s.commands_off()).is_some() => {
                        why_not(m, pr_review_tui::keymap::Mutation::Publish, "post")
                    }
                    Some(w) => format!("Can't post: {w}"),
                    None => "Can't post right now.".into(),
                }),
                ..Item::new("p-post", "Post to GitHub…", "⌘⏎", Tone::Primary, None)
            },
        };
        Bar {
            lead: vec![],
            note: Some((IconName::TriangleAlert, "Public and final once posted.".into())),
            trail: vec![
                Item::new(
                    "p-close",
                    "Not now",
                    "esc",
                    Tone::Ghost,
                    (!asking).then(|| crate::ui::click(cx, |s, cx| s.act(Action::Back, cx))),
                ),
                post,
            ],
        }
    };
    let footer = bar_view(s, t, footer, cx);
    page("detail-pub", body, Some(footer), t, false)
}

fn state_callout(
    m: &Model,
    t: &Colors,
    p: &PublicationPreview,
    publish: &ConfirmState,
    posted: Option<PublicationStatus>,
) -> Option<Div> {
    Some(match (posted, publish) {
        // The kicker, the title and the sentence already say it posted.
        (Some(PublicationStatus::Published), _) => return None,
        (Some(PublicationStatus::Publishing), _) => {
            callout(t, t.running, IconName::LoaderCircle, "Posting to GitHub…", None)
        }
        (Some(PublicationStatus::Failed), _) => {
            callout(t, t.danger, IconName::CircleX, "GitHub refused the post", None)
        }
        (Some(PublicationStatus::Uncertain), _) => callout(
            t,
            t.warn,
            IconName::CircleAlert,
            "Unconfirmed",
            Some("The daemon is checking whether GitHub has the review.".into()),
        ),
        (_, ConfirmState::Requested(_)) => {
            callout(t, t.running, IconName::LoaderCircle, "Sending your approval…", None)
        }
        (_, ConfirmState::Accepted { .. }) => callout(
            t,
            t.running,
            IconName::LoaderCircle,
            "Approved. Waiting for GitHub to confirm…",
            Some("Accepted is not posted: this changes once GitHub has the review.".into()),
        ),
        (_, ConfirmState::Refused { message, code }) => callout(
            t,
            t.danger,
            IconName::CircleX,
            "The daemon refused",
            Some(format!("{message}{}", code.map_or(String::new(), |c| format!(" ({c})")))),
        ),
        (_, ConfirmState::TimedOut) => callout(
            t,
            t.warn,
            IconName::CircleAlert,
            "No reply in time",
            Some("⌘⏎ sends the same approval again, under the same key, so it can't post twice.".into()),
        ),
        (_, ConfirmState::Asking) => {
            let why = publish_block(m, p)?;
            callout(t, t.warn, IconName::TriangleAlert, "This proof can't be posted", Some(why))
        }
    })
}

/// The lines around `line` in the loaded diff, for a comment's context.
fn diff_context(m: &Model, key: &DiffKey, path: &str, line: u32) -> Option<Vec<(u32, String, bool)>> {
    let d = m.diff.as_ref().filter(|d| &d.key == key)?;
    let file = d.files.iter().find(|f| f.path.as_str() == path)?;
    let lines: Vec<_> = file.hunks.iter().flat_map(|h| h.lines.iter()).filter(|l| l.new.is_some()).collect();
    let at = lines.iter().position(|l| l.new == Some(line))?;
    let from = at.saturating_sub(2);
    Some(
        lines[from..(at + 2).min(lines.len() - 1) + 1]
            .iter()
            .map(|l| (l.new.unwrap_or(0), l.text.to_string(), l.new == Some(line)))
            .collect(),
    )
}
