//! The detail pane: the decision behind the selected card. A PR with its findings and its timeline, a finding in
//! two columns (the argument, then the code it rests on), the exact publication a parked review would post, a review
//! working live, and the setup checks.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{Identity, Modal, Model};
use pr_review_tui::protocol::{
    EvidenceKind, Finding, FindingId, FindingStatus, HistoryEntry, Lifecycle, LineKind, Pr, PublicationStatus, Run,
    RunStatus, Severity,
};
use pr_review_tui::update::{Msg, can_mutate, write_block};
use pr_review_tui::view::Hit;

use crate::code::{Inline, Lexer, Tok, clip, inline_markdown, wrap};
use crate::kit::{
    Gauge, OnClick, Tone, button, callout, chip, clock, display, heading, icon, icon_button, overline, progress,
    severity_badge, step_of, summary, tip,
};
use crate::queue::{self, Stage, phase_word};
use crate::tokens::{Colors, layout, radius, severity_mark, space, text};
use crate::ui::{Shell, on};

/// The scrolling page and the action bar under it. The page keeps a bottom inset the height of the bar, so the
/// last row never sits under it.
pub fn page(id: &'static str, body: Div, footer: Option<Div>, t: &Colors, wide: bool) -> Div {
    let has_bar = footer.is_some();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(
            div().id(id).flex_1().min_h_0().overflow_y_scroll().child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S6)
                    .px(space::S8)
                    .pt(space::S6)
                    .pb(if has_bar { layout::BAR + space::S6 } else { space::S8 })
                    .when(!wide, |d| d.max_w(px(760.)))
                    .child(body),
            ),
        )
        .children(footer.map(|f| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S2)
                .h(layout::BAR)
                .px(space::S8)
                .border_t_1()
                .border_color(t.border)
                .bg(t.raised)
                .child(f)
        }))
}

pub fn pane(s: &Shell, t: &Colors, _: &mut Window, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    match crate::ui::front(m) {
        Some(Modal::Publication { run_id, preview, publish, raw, .. }) => {
            return crate::publish::publication(s, t, run_id, preview, publish, *raw, cx);
        }
        Some(Modal::Finding { id, .. }) => return finding_page(s, t, id, cx),
        _ => {}
    }
    match queue::stage(m) {
        Stage::Offline => return page("detail", crate::setup::offline(s, t, cx), None, t, false),
        Stage::Connecting => return page("detail", crate::setup::connecting(t), None, t, false),
        Stage::Ready => {}
    }
    let Some(pr) = m.selected_pr() else {
        return page(
            "detail",
            if queue::setup_issues(m) > 0 { crate::setup::setup_body(s, t, cx) } else { crate::setup::board(s, t) },
            None,
            t,
            false,
        );
    };
    if m.ui.screen == pr_review_tui::model::Screen::Timeline {
        return crate::timeline::timeline(s, t, pr, cx);
    }
    match m.domain.latest_run(&pr.id) {
        Some(r) if r.active() => crate::live::live(s, t, pr, r, cx),
        run => pr_page(s, t, pr, run, cx),
    }
}

pub fn empty(t: &Colors, title: &str, body: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .pt(px(160.))
        .items_center()
        .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().text_size(text::BODY).text_color(t.muted).child(body.to_string()))
}

/// Who posts: the account the daemon uses for GitHub.
pub fn who(m: &Model) -> String {
    match m.identity() {
        Some(Identity::Selected(a) | Identity::GhActive(a)) => a.login.to_string(),
        Some(Identity::Env) => "the daemon's GITHUB_TOKEN".into(),
        _ => "the daemon's GitHub identity".into(),
    }
}

/// The kicker, the serif title, the meta line and the one-sentence state under every PR heading.
pub fn pr_header(t: &Colors, pr: &Pr, kicker: Div, sentence: String) -> Div {
    let state = match pr.lifecycle {
        Lifecycle::Merged => Some(("Merged", t.accent)),
        Lifecycle::Closed => Some(("Closed", t.muted)),
        _ => None,
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .child(kicker)
                .child(div().text_size(text::CAPTION).text_color(t.faint).child("·"))
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(t.faint)
                        .child(format!("{} #{}", pr.repository, pr.number)),
                )
                .child(div().text_size(text::CAPTION).text_color(t.faint).child("·"))
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(t.faint)
                        .child(format!("by {}", pr.author.as_ref().map_or("unknown", |a| a.as_str()))),
                )
                .child(div().text_size(text::CAPTION).text_color(t.faint).child("·"))
                .child(
                    div()
                        .font_family(text::MONO)
                        .text_size(text::CAPTION)
                        .text_color(t.faint)
                        .child(pr.head_sha.short().to_string()),
                )
                .children(state.map(|(w, c)| chip(t, w, c))),
        )
        .child(display(t, pr.title.to_string()))
        .child(summary(t, sentence))
}

/// Where the PR stands, in one sentence.
pub fn pr_sentence(m: &Model, pr: &Pr, run: Option<&Run>) -> String {
    let findings = m.domain.pr_findings(&pr.id);
    let current: Vec<&Finding> = findings.iter().copied().filter(|f| !f.stale).collect();
    let open: Vec<&Finding> = current.iter().copied().filter(|f| queue::open_finding(f)).collect();
    let Some(r) = run else {
        return "Not reviewed yet. Start a review and it runs live here.".into();
    };
    let when = r.completed_at.as_ref().map_or(clock(r.started_at.as_str()), |c| clock(c.as_str()));
    if r.parked() {
        return if open.is_empty() {
            format!("Waiting for your approval: a clean review, posted as {}.", who(m))
        } else {
            format!(
                "Waiting for your approval: {} ({}), posted as {}.",
                queue::count(open.len(), "finding"),
                queue::severity_summary(&open),
                who(m)
            )
        };
    }
    if r.active() {
        let calls = m
            .domain
            .activity
            .iter()
            .filter(|a| {
                a.pr_id.as_ref() == Some(&pr.id) && matches!(a.kind, pr_review_tui::model::ActivityKind::Tool { .. })
            })
            .count();
        return format!(
            "{} since {}, {} so far.",
            phase_word(r.phase),
            clock(r.started_at.as_str()),
            queue::count(calls, "tool call")
        );
    }
    if r.status == RunStatus::Failed {
        let reason =
            r.failure_reason.as_ref().map_or("no reason given".to_string(), |x| crate::kit::lead(x.as_str(), 1));
        return if crate::cards::too_large(&reason) {
            format!("Too large to review: {reason}")
        } else {
            format!("The review failed{}: {reason}", r.attempt.map_or(String::new(), |a| format!(" on attempt {a}")))
        };
    }
    let posted = m.domain.publications.get(&r.id).map(|p| p.status);
    match posted {
        Some(PublicationStatus::Published) => {
            format!("Review posted at {when} with {}.", queue::count(current.len(), "finding"))
        }
        Some(PublicationStatus::Failed) => "GitHub refused the post. Open the proof to see what it said.".into(),
        Some(PublicationStatus::Uncertain) => "Posting unconfirmed: the daemon is checking GitHub.".into(),
        _ if current.is_empty() => format!("Reviewed at {when} with {}. Nothing found.", r.model),
        _ => {
            let worst = open.first().map(|f| severity_mark(f.severity).1.to_lowercase());
            format!(
                "{} to judge{}. Nothing was posted.",
                queue::count(open.len(), "finding"),
                worst.map_or(String::new(), |w| format!(", one {w}"))
            )
        }
    }
}

/// A tab of the detail pane: Findings or Timeline.
fn tab(t: &Colors, id: &'static str, label: &str, count: Option<usize>, active: bool, on: OnClick) -> Stateful<Div> {
    let row = div()
        .id(id)
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(36.))
        .px(space::S1)
        .border_b_2()
        .border_color(if active { t.accent } else { t.accent.alpha(0.) })
        .cursor_pointer()
        .text_size(text::BODY)
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
        .text_color(if active { t.text } else { t.muted })
        .hover(|d| d.text_color(t.text))
        .on_click(on)
        .child(label.to_string())
        .children(count.map(|n| div().text_size(text::SMALL).text_color(t.faint).child(n.to_string())));
    crate::kit::focusable(row, t)
}

pub fn tabs(t: &Colors, findings: usize, timeline: bool, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::S5)
        .border_b_1()
        .border_color(t.border)
        .child(tab(
            t,
            "tab-findings",
            "Findings",
            Some(findings),
            !timeline,
            crate::ui::click(cx, |s, cx| {
                if s.model.ui.screen == pr_review_tui::model::Screen::Timeline {
                    s.act(Action::Back, cx)
                }
            }),
        ))
        .child(tab(
            t,
            "tab-timeline",
            "Timeline",
            None,
            timeline,
            crate::ui::click(cx, |s, cx| {
                if s.model.ui.screen != pr_review_tui::model::Screen::Timeline {
                    s.act(Action::OpenTimeline, cx)
                }
            }),
        ))
}

fn pr_page(s: &Shell, t: &Colors, pr: &Pr, run: Option<&Run>, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let findings = m.domain.pr_findings(&pr.id);
    let (current, stale): (Vec<&Finding>, Vec<&Finding>) = findings.iter().partition(|f| !f.stale);
    let mut body = div().flex().flex_col().gap(space::S6).child(pr_header(
        t,
        pr,
        overline(t, "Pull request"),
        pr_sentence(m, pr, run),
    ));
    body = body.child(tabs(t, current.len(), false, cx));
    body = body.child(run_block(s, t, run, &current));
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(heading(t, "Findings"))
                    .child(div().text_size(text::SMALL).text_color(t.faint).child(current.len().to_string()))
                    .child(div().flex_1())
                    .when(current.len() > 1, |d| {
                        d.child(div().text_size(text::SMALL).text_color(t.faint).child("n / N to walk · ⏎ opens"))
                    }),
            )
            .when(current.is_empty(), |d| d.child(no_findings(t, pr, run)))
            .children(current.iter().map(|f| finding_row(s, t, f, cx))),
    );
    if !stale.is_empty() {
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(heading(t, "From an earlier head"))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(
                    "These came from a run a newer push superseded. The next review says whether they still hold.",
                ))
                .children(stale.iter().map(|f| finding_row(s, t, f, cx))),
        );
    }
    page("detail-pr", body, Some(pr_actions(s, t, pr, run, cx)), t, false)
}

/// The run's gauge and its state, or what stands in its way.
fn run_block(s: &Shell, t: &Colors, run: Option<&Run>, current: &[&Finding]) -> Div {
    let m = &s.model;
    let frame = |inner: Div| {
        div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .p(space::S4)
            .rounded(radius::LG)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(inner)
    };
    match run {
        None => callout(
            t,
            t.muted,
            IconName::Play,
            "No review yet",
            Some("Start one and its progress shows live in the strip above.".into()),
        ),
        Some(r) if r.parked() => frame(
            div()
                .flex()
                .flex_col()
                .gap(space::S3)
                .child(progress(t, step_of(r.phase), t.accent, false, Gauge::Full))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
                    "Parked for your approval · {} · run {}. Nothing posts to GitHub until you approve the proof.",
                    r.model,
                    r.id.short()
                ))),
        ),
        Some(r) if r.status == RunStatus::Failed => {
            let reason = r.failure_reason.as_ref().map_or("No reason given.".to_string(), |x| x.to_string());
            let large = crate::cards::too_large(&reason);
            frame(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S3)
                    .child(progress(t, step_of(r.phase), t.danger, true, Gauge::Full))
                    .child(callout(
                        t,
                        t.danger,
                        IconName::CircleX,
                        if large {
                            "Too large to review".to_string()
                        } else {
                            format!("Review failed{}", r.attempt.map_or(String::new(), |a| format!(" on attempt {a}")))
                        },
                        Some(if large {
                            format!("{reason}\n\nNext step: exclude generated files or split the PR, then retry (⇧R).")
                        } else {
                            format!("{reason}\n\nNext step: retry (⇧R). The reviewer resumes from its last checkpoint.")
                        }),
                    )),
            )
        }
        Some(r) => {
            let posted = m.domain.publications.get(&r.id).map(|p| p.status);
            let (hue, word) = match posted {
                Some(PublicationStatus::Published) => (t.ok, "Review posted to GitHub"),
                Some(PublicationStatus::Failed) => (t.danger, "Posting failed"),
                Some(PublicationStatus::Uncertain) => (t.warn, "Posting unconfirmed"),
                _ if current.is_empty() => (t.ok, "Reviewed: nothing found"),
                _ => (t.accent, "Reviewed, nothing posted"),
            };
            frame(
                div().flex().flex_col().gap(space::S3).child(progress(t, 6, hue, false, Gauge::Full)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .text_size(text::SMALL)
                        .text_color(t.muted)
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(hue).child(word))
                        .child("·")
                        .child(format!(
                            "{} at {} · {} · run {}",
                            phase_word(r.phase),
                            r.completed_at.as_ref().map_or(clock(r.started_at.as_str()), |c| clock(c.as_str())),
                            r.model,
                            r.id.short()
                        )),
                ),
            )
        }
    }
}

/// A reviewed PR with nothing to judge: a designed state, not a blank.
fn no_findings(t: &Colors, pr: &Pr, run: Option<&Run>) -> Div {
    let (title, body) = match run {
        Some(r) if r.status == RunStatus::Failed => {
            ("No findings: the review did not finish", "Retry it to get a verdict on this head.".to_string())
        }
        Some(_) => (
            "Nothing found on this head",
            format!(
                "The reviewer read the diff at {} with read-only tools and verified no defect.",
                pr.head_sha.short()
            ),
        ),
        None => {
            ("No findings yet", "Start a review and its findings land here as the verifier keeps them.".to_string())
        }
    };
    div()
        .flex()
        .gap(space::S3)
        .p(space::S4)
        .rounded(radius::LG)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .child(div().pt(px(1.)).child(icon(IconName::Check, px(16.), t.ok)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(div().text_size(text::BODY).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(body)),
        )
}

/// A button for a mutation: live when the reducer allows it, dimmed with the reducer's reason when not.
#[allow(clippy::too_many_arguments)]
pub fn mutation_button(
    s: &Shell,
    t: &Colors,
    id: &'static str,
    label: &str,
    key: &str,
    tone: Tone,
    mu: Mutation,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let m = &s.model;
    if can_mutate(m, mu) {
        let direct = matches!(mu, Mutation::Start | Mutation::Retry);
        return button(
            t,
            id,
            label,
            Some(key),
            tone,
            on(cx, move |s, cx| if direct { s.direct(mu, cx) } else { s.act(Action::Mutation(mu), cx) }),
        );
    }
    let why = write_block(m, mu.capability()).map_or_else(
        || match mu {
            Mutation::Start => "a review already holds this head",
            Mutation::Retry => "its latest run has not failed",
            Mutation::Cancel => "no review is running on it",
            Mutation::Publish => "nothing is waiting for approval",
        },
        |w| w,
    );
    tip(button(t, id, label, Some(key), tone, None), format!("Can't {}: {why}", label.to_lowercase()))
}

fn pr_actions(s: &Shell, t: &Colors, pr: &Pr, run: Option<&Run>, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let mut bar = div().flex().items_center().gap(space::S2).flex_1();
    match run {
        Some(r) if r.parked() => {
            bar = bar.child(mutation_button(s, t, "a-post", "Review post", "P", Tone::Primary, Mutation::Publish, cx))
        }
        Some(r) if r.status == RunStatus::Failed => {
            bar = bar.child(mutation_button(s, t, "a-retry", "Retry review", "⇧R", Tone::Primary, Mutation::Retry, cx))
        }
        None => {
            bar = bar.child(mutation_button(s, t, "a-start", "Start review", "S", Tone::Primary, Mutation::Start, cx))
        }
        _ => {}
    }
    if m.ui.selected_finding.is_some() {
        bar = bar.child(button(t, "a-open", "Open finding", Some("⏎"), Tone::Secondary, {
            let id = m.ui.selected_finding.clone();
            id.map(|id| -> OnClick {
                Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| s.send(Msg::Pick(Hit::Finding(id.clone())), cx)))
            })
        }));
    }
    bar =
        bar.child(button(t, "a-diff", "Diff", Some("D"), Tone::Secondary, on(cx, |s, cx| s.act(Action::OpenDiff, cx))));
    if pr.lifecycle == Lifecycle::Active && run.is_some_and(|r| r.parked()) {
        bar = bar.child(mutation_button(s, t, "a-cancel", "Cancel", "C", Tone::Ghost, Mutation::Cancel, cx));
    }
    if let Some(why) = m.server.as_ref().and_then(|sv| sv.commands_off()) {
        bar =
            bar.child(div().ml(space::S2).text_size(text::SMALL).text_color(t.warn).child(format!("Read-only: {why}")));
    }
    bar.child(div().flex_1()).child(button(
        t,
        "a-gh",
        "Open on GitHub",
        Some("⌘O"),
        Tone::Ghost,
        on(cx, |s, cx| s.open_github(cx)),
    ))
}

/// A finding's tracking status as a word and the sentence that explains it, in the reviewer's terms.
pub fn status_sentence(f: &Finding, history: &[HistoryEntry]) -> (&'static str, &'static str) {
    if f.status == FindingStatus::Detected && history.iter().any(|h| h.disposition == FindingStatus::Resolved) {
        return ("Reopened", "It looked resolved on an earlier head, but its code is still here.");
    }
    match f.status {
        FindingStatus::Detected => ("New", "First reported by this run. Not posted to GitHub yet."),
        FindingStatus::Published => ("Posted", "On GitHub as an inline comment."),
        FindingStatus::StillPresent => {
            ("Still present", "Seen again on the newest head: the cited code did not change the outcome.")
        }
        FindingStatus::PotentiallyResolved => (
            "Looks resolved",
            "The cited lines changed on the newest head. It may be fixed; the reviewer has not verified that.",
        ),
        FindingStatus::Outdated => ("Outdated", "The code it cites is no longer in the diff."),
        FindingStatus::Resolved => ("Resolved", "Verified fixed on a later commit."),
        FindingStatus::Uncertain => {
            ("Uncertain", "The verifier could neither confirm nor rule it out. Read it as a question.")
        }
        FindingStatus::Unknown => ("Unknown", "The daemon sent a status this client does not know."),
    }
}

fn status_hue(t: &Colors, f: &Finding) -> Hsla {
    match f.status {
        FindingStatus::StillPresent => t.high,
        FindingStatus::Resolved => t.ok,
        FindingStatus::PotentiallyResolved | FindingStatus::Uncertain => t.warn,
        FindingStatus::Published => t.accent,
        _ => t.accent,
    }
}

pub fn finding_row(s: &Shell, t: &Colors, f: &Finding, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let selected = m.ui.selected_finding.as_ref() == Some(&f.id);
    let (mark, word) = severity_mark(f.severity);
    let hue = t.severity(f.severity);
    let id = f.id.clone();
    let detail = m.details.get(&f.id);
    let place = format!("{}{}", f.file, f.line.map_or(String::new(), |l| format!(":{l}")));
    let (status, _) = status_sentence(f, detail.map_or(&[][..], |d| &d.history));
    let row = div()
        .id(SharedString::from(format!("f-{}", f.id.as_str())))
        .flex()
        .flex_col()
        .gap(space::S2)
        .px(space::S3)
        .py(px(10.))
        .rounded(radius::LG)
        .border_1()
        .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
        .bg(if selected { t.selected } else { t.surface })
        .cursor_pointer()
        .hover(|d| d.bg(t.hover).border_color(if selected { t.accent } else { t.border_strong }))
        .active(|d| d.bg(t.press))
        .when(f.stale, |d| d.opacity(0.75))
        .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.send(Msg::Pick(Hit::Finding(id.clone())), cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .w(px(76.))
                        .flex_none()
                        .text_size(text::SMALL)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hue)
                        .child(div().text_size(px(10.)).child(mark))
                        .child(word),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(text::BODY)
                        .font_weight(FontWeight::MEDIUM)
                        .child(f.title.to_string()),
                )
                .when(status != "New", |d| d.child(chip(t, status, status_hue(t, f)))),
        )
        .child(
            div()
                .flex()
                .gap(space::S3)
                .pl(px(88.))
                .text_size(text::SMALL)
                .text_color(t.faint)
                .child(div().font_family(text::MONO).text_size(px(11.)).child(place))
                .child(format!(
                    "{} · {:.0}% sure · {}",
                    f.category.label(),
                    f.confidence * 100.,
                    queue::count(f.evidence_count as usize, "evidence item")
                )),
        )
        .when(selected, |d| {
            d.children(detail.map(|x| {
                div()
                    .pl(px(88.))
                    .max_w(layout::MEASURE)
                    .text_size(text::BODY)
                    .line_height(text::LINE_BODY)
                    .text_color(t.muted)
                    .child(md_line(t, &crate::kit::lead(x.explanation.as_str(), 2)))
            }))
        });
    crate::kit::focusable(row, t)
}

/// The words for a confidence: the number, then what it means for the reader.
pub fn confidence_words(c: f64) -> (&'static str, &'static str) {
    if c >= 0.85 {
        ("High confidence", "The verifier confirmed it against the code.")
    } else if c >= 0.6 {
        ("Moderate confidence", "Likely, with some doubt. Check the evidence.")
    } else {
        ("Low confidence", "Doubtful. Read the evidence before you act.")
    }
}

/// A finding, opened: the argument in a reading column on the left, and the code it rests on at the right.
fn finding_page(s: &Shell, t: &Colors, id: &FindingId, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let Some(f) = m.domain.findings.get(id) else {
        return page("detail-f", empty(t, "This finding is gone", "A newer snapshot no longer has it."), None, t, true);
    };
    let pr = m.domain.prs.get(&f.pr_id);
    let all = m.domain.pr_findings(&f.pr_id);
    let at = all.iter().position(|x| x.id == f.id).map_or(0, |i| i + 1);
    let crumbs = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .child(div().ml(px(-12.)).child(button(
            t,
            "f-back",
            pr.map_or("Back".into(), |p| format!("#{} findings", p.number)).as_str(),
            Some("esc"),
            Tone::Ghost,
            on(cx, |s, cx| s.act(Action::Back, cx)),
        )))
        .child(div().flex_1())
        .child(icon_button(
            t,
            "f-prev",
            IconName::ChevronUp,
            "Previous finding (⇧N)",
            (at > 1).then(|| crate::ui::click(cx, |s, cx| s.act(Action::PrevFinding, cx))),
        ))
        .child(icon_button(
            t,
            "f-next",
            IconName::ChevronDown,
            "Next finding (N)",
            (at < all.len()).then(|| crate::ui::click(cx, |s, cx| s.act(Action::NextFinding, cx))),
        ))
        .child(div().text_size(text::SMALL).text_color(t.faint).child(format!("{at} of {}", all.len())));
    let detail = m.details.get(id);
    let history: &[HistoryEntry] = detail.map_or(&[], |d| &d.history);
    let (status, why) = status_sentence(f, history);
    let mut badges = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(space::S2)
        .child(severity_badge(t, f.severity))
        .child(chip(t, f.category.label(), t.muted))
        .child(chip(t, status, status_hue(t, f)));
    if f.stale {
        badges = badges.child(chip(t, "From an earlier head", t.warn));
    }
    let (conf_word, conf_why) = confidence_words(f.confidence);
    let mut left = div().flex().flex_col().flex_none().w(layout::MEASURE).gap(space::S6).child(
        div().flex().flex_col().gap(space::S3).child(badges).child(display(t, f.title.to_string())).child(
            div().font_family(text::MONO).text_size(text::SMALL).text_color(t.muted).child(format!(
                "{}{}",
                f.file,
                f.line.map_or(String::new(), |l| format!(":{l}"))
            )),
        ),
    );
    let mut right = div().flex().flex_col().flex_1().min_w_0().gap(space::S6);
    match detail {
        None => {
            let (title, note) = match m.detail_errors.get(id) {
                Some(e) => ("Couldn't load the evidence", e.to_string()),
                None => (
                    "Loading the evidence…",
                    "The explanation, cited code and history come from the daemon.".to_string(),
                ),
            };
            left = left.child(callout(t, t.muted, IconName::LoaderCircle, title, Some(note)));
        }
        Some(d) => {
            left = left.child(section(t, "What's wrong", paragraphs(t, d.explanation.as_str(), t.text))).child(
                section(t, "Why the reviewer believes it", paragraphs(t, d.reasoning_summary.as_str(), t.muted)),
            );
            if let Some(fix) = &d.suggested_fix {
                left = left.child(section(t, "Suggested fix", fix_block(t, fix.as_str(), f.file.as_str())));
            }
            left = left
                .child(section(
                    t,
                    "Status",
                    div().max_w(layout::MEASURE).text_size(text::BODY).line_height(text::LINE_READ).child(
                        StyledText::new(format!("{status}. {why}")).with_highlights(vec![(
                            0..status.len() + 1,
                            HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() },
                        )]),
                    ),
                ))
                .child(section(t, "Confidence", confidence(t, f.confidence, conf_word, conf_why)))
                .child(section(t, "History", history_rows(t, f, history)));
            right = right.child(section(t, &format!("Evidence · {}", d.evidence.len()), evidence_blocks(t, f, d)));
            right = right.child(in_the_diff(s, t, f, cx));
        }
    }
    let body = div()
        .flex()
        .flex_col()
        .gap(space::S5)
        .child(crumbs)
        .child(div().flex().gap(space::S8).child(left).child(right));
    let footer = div()
        .flex()
        .items_center()
        .gap(space::S2)
        .flex_1()
        .child(button(
            t,
            "f-diff",
            "Open in diff",
            Some("D"),
            Tone::Primary,
            on(cx, |s, cx| s.act(Action::OpenDiff, cx)),
        ))
        .child(button(
            t,
            "f-next-b",
            "Next finding",
            Some("N"),
            Tone::Secondary,
            (at < all.len()).then(|| -> OnClick {
                Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::NextFinding, cx)))
            }),
        ))
        .child(button(
            t,
            "f-timeline",
            "Timeline",
            Some("T"),
            Tone::Ghost,
            on(cx, |s, cx| s.act(Action::OpenTimeline, cx)),
        ))
        .child(div().flex_1())
        .child(button(t, "f-gh", "Open on GitHub", Some("⌘O"), Tone::Ghost, on(cx, |s, cx| s.open_github(cx))));
    page("detail-f", body, Some(footer), t, true)
}

fn history_rows(t: &Colors, f: &Finding, history: &[HistoryEntry]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
            "Tracked across commits by fingerprint {}. Each review says whether it still holds.",
            &f.fingerprint.as_str()[..f.fingerprint.as_str().len().min(8)]
        )))
        .children(history.iter().map(|h| {
            let word = match h.disposition {
                FindingStatus::Detected => "Found",
                FindingStatus::Published => "Posted",
                FindingStatus::StillPresent => "Still present",
                FindingStatus::PotentiallyResolved => "Looks resolved",
                FindingStatus::Outdated => "Outdated",
                FindingStatus::Resolved => "Resolved",
                FindingStatus::Uncertain => "Uncertain",
                FindingStatus::Unknown => "Unknown",
            };
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(div().size(px(6.)).rounded(radius::PILL).bg(t.accent))
                .child(div().w(px(110.)).text_size(text::BODY).font_weight(FontWeight::MEDIUM).child(word))
                .child(div().font_family(text::MONO).text_size(text::CAPTION).text_color(t.faint).child(format!(
                    "{} {} · run {}",
                    h.at.as_str().get(..10).unwrap_or(""),
                    clock(h.at.as_str()),
                    h.run_id.short()
                )))
        }))
}

/// The cited code, each block pinned to its path, lines and commit, and the tool summaries between them.
fn evidence_blocks(t: &Colors, f: &Finding, d: &pr_review_tui::protocol::FindingDetail) -> Div {
    let mut blocks = div().flex().flex_col().gap(space::S3);
    for e in &d.evidence {
        blocks = blocks.child(match (e.kind, &e.path) {
            (EvidenceKind::CodeRef, Some(path)) => {
                let start = e.start.unwrap_or(1);
                let end = e.end.unwrap_or(start);
                let anchor = f.line.map(|l| l.get()).filter(|l| path == &f.file && (start..=end).contains(l));
                let title = format!("{path}:L{start}{}", if end != start { format!("–{end}") } else { String::new() });
                let sha = e.ref_sha.as_ref().map(|s| format!("@{}", s.short()));
                code_block(
                    t,
                    &title,
                    sha,
                    e.text.as_str(),
                    start,
                    anchor,
                    f.severity,
                    path.as_str(),
                    layout::COLS_EVIDENCE,
                )
                .into_any_element()
            }
            _ => {
                let (tool, rest) = e.text.as_str().split_once(' ').unwrap_or(("note", e.text.as_str()));
                div()
                    .flex()
                    .gap(space::S3)
                    .items_start()
                    .p(space::S3)
                    .rounded(radius::LG)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.surface)
                    .child(div().pt(px(2.)).child(icon(
                        if e.kind == EvidenceKind::ToolSummary { IconName::SquareTerminal } else { IconName::Bot },
                        px(14.),
                        t.faint,
                    )))
                    .child(
                        div()
                            .flex_none()
                            .font_family(text::MONO)
                            .text_size(px(11.))
                            .pt(px(2.))
                            .text_color(t.accent)
                            .child(tool.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(text::BODY)
                            .line_height(text::LINE_BODY)
                            .text_color(t.muted)
                            .child(rest.to_string()),
                    )
                    .into_any_element()
            }
        });
    }
    blocks
}

/// The diff hunk the finding sits in, when the diff is loaded, and the way to the full diff.
fn in_the_diff(s: &Shell, t: &Colors, f: &Finding, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let anchor = f.line.map(|l| l.get());
    let hunk = m.diff.as_ref().filter(|dv| dv.key.pr == f.pr_id).and_then(|dv| {
        let file = dv.files.iter().find(|x| x.path == f.file)?;
        let line = anchor?;
        let hunk = file.hunks.iter().find(|h| h.lines.iter().any(|l| l.new == Some(line)))?;
        let at = hunk.lines.iter().position(|l| l.new == Some(line))?;
        let lo = at.saturating_sub(4);
        let hi = (at + 5).min(hunk.lines.len());
        Some((hunk.header.to_string(), hunk.lines[lo..hi].to_vec()))
    });
    let head = div().flex().items_center().child(overline(t, "In the diff")).child(div().flex_1()).child(button(
        t,
        "f-full-diff",
        "Open full diff",
        Some("D"),
        Tone::Secondary,
        on(cx, |s, cx| s.act(Action::OpenDiff, cx)),
    ));
    let body = match hunk {
        Some((header, lines)) => {
            let mut lx = Lexer::for_path(f.file.as_str());
            let hue = t.severity(f.severity);
            let mut rows = div().py(space::S2).font_family(text::MONO).text_size(text::CODE);
            for l in &lines {
                let hit = l.new == anchor && l.kind != LineKind::Del;
                let (wash, sign, sign_fg) = match l.kind {
                    LineKind::Add => (Some(t.add_bg), "+", t.add_fg),
                    LineKind::Del => (Some(t.del_bg), "−", t.del_fg),
                    _ => (None, " ", t.faint),
                };
                let wash = if hit { Some(t.severity_wash(f.severity)) } else { wash };
                for (k, (run, cont)) in runs(t, &mut lx, l.text.as_str(), layout::COLS_EVIDENCE).into_iter().enumerate()
                {
                    rows = rows.child(
                        div()
                            .flex()
                            .child(div().w(px(14.)).flex_none().pl(space::S2).text_color(sign_fg).child(if k == 0 {
                                sign
                            } else {
                                " "
                            }))
                            .child(div().flex_1().min_w_0().child(code_row(
                                t,
                                l.new.or(l.old),
                                run,
                                cont,
                                wash,
                                hit.then_some(hue),
                            ))),
                    );
                }
            }
            div()
                .flex()
                .flex_col()
                .rounded(radius::LG)
                .border_1()
                .border_color(t.border)
                .bg(t.code_bg)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .h(px(30.))
                        .px(space::S3)
                        .border_b_1()
                        .border_color(t.border)
                        .bg(t.surface)
                        .child(icon(IconName::FileDiff, px(13.), t.faint))
                        .child(div().font_family(text::MONO).text_size(px(11.)).text_color(t.muted).child(header)),
                )
                .child(rows)
        }
        None => div()
            .text_size(text::SMALL)
            .line_height(text::LINE_TIGHT)
            .text_color(t.faint)
            .child("The hunk shows here once the diff is loaded. Open full diff loads it, anchored on this line."),
    };
    div().flex().flex_col().gap(space::S3).child(head).child(body)
}

pub fn confidence(t: &Colors, c: f64, word: &str, why: &str) -> Div {
    let pct = (c * 100.).round().clamp(0., 100.) as f32;
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .max_w(layout::MEASURE)
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child(format!("{pct:.0}%")))
                .child(
                    div()
                        .flex_1()
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(t.border)
                        .child(div().h_full().w(relative(pct / 100.)).rounded(px(2.)).bg(t.muted)),
                ),
        )
        .child(
            div()
                .text_size(text::BODY)
                .line_height(text::LINE_READ)
                .text_color(t.muted)
                .child(format!("{word}. {why}")),
        )
}

pub fn section(t: &Colors, title: &str, content: impl IntoElement) -> Div {
    div().flex().flex_col().gap(space::S3).child(overline(t, title)).child(content)
}

/// Prose at the reading measure, one block per paragraph.
pub fn paragraphs(t: &Colors, s: &str, color: Hsla) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .children(s.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).map(|p| prose(t, p).text_color(color)))
}

pub fn prose(t: &Colors, s: &str) -> Div {
    div()
        .max_w(layout::MEASURE)
        .text_size(text::BODY)
        .line_height(text::LINE_READ)
        .text_color(t.text)
        .child(md_line(t, s))
}

fn style_of(t: &Colors, tok: Tok) -> HighlightStyle {
    let color = match tok {
        Tok::Keyword => t.syn_keyword,
        Tok::Str => t.syn_string,
        Tok::Number => t.syn_number,
        Tok::Comment => t.syn_comment,
        Tok::Call => t.syn_fn,
        Tok::Type => t.syn_type,
    };
    HighlightStyle {
        color: Some(color),
        font_style: (tok == Tok::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// One line of code as the runs it wraps into at `cols`, each colored, with whether it continues the line.
pub fn runs(t: &Colors, lx: &mut Lexer, line: &str, cols: usize) -> Vec<(StyledText, bool)> {
    let line = line.replace('\t', "    ");
    let toks = lx.line(&line);
    wrap(&line, cols)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let marks: Vec<_> = clip(&toks, &r).into_iter().map(|(r, tok)| (r, style_of(t, tok))).collect();
            let s = if r.is_empty() { " ".to_string() } else { line[r].to_string() };
            (StyledText::new(s).with_highlights(marks), i > 0)
        })
        .collect()
}

/// A gutter, a marker and one run of a line. `cont` marks a run that continues the line above; `wash` tints the
/// row; `mark` draws the anchor bar in the gutter.
pub fn code_row(
    t: &Colors,
    number: Option<u32>,
    run: StyledText,
    cont: bool,
    wash: Option<Hsla>,
    mark: Option<Hsla>,
) -> Div {
    div()
        .flex()
        .h(text::LINE_CODE)
        .items_center()
        .when_some(wash, |d, w| d.bg(w))
        .child(div().w(px(3.)).h_full().flex_none().when_some(mark, |d, m| d.bg(m)))
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .pr(space::S3)
                .text_right()
                .text_color(if mark.is_some() { t.text } else { t.faint })
                .child(if cont { "⤷".to_string() } else { number.map_or(String::new(), |n| n.to_string()) }),
        )
        .child(div().flex_1().min_w_0().whitespace_nowrap().text_color(t.text).child(run))
}

#[allow(clippy::too_many_arguments)]
pub fn code_block(
    t: &Colors,
    title: &str,
    sha: Option<String>,
    code: &str,
    start: u32,
    anchor: Option<u32>,
    sev: Severity,
    path: &str,
    cols: usize,
) -> Div {
    let hue = t.severity(sev);
    let mut lx = Lexer::for_path(path);
    let mut rows = div().py(space::S2).font_family(text::MONO).text_size(text::CODE);
    for (i, l) in code.lines().enumerate() {
        let n = start + i as u32;
        let hit = anchor == Some(n);
        for (run, cont) in runs(t, &mut lx, l, cols) {
            rows = rows.child(code_row(t, Some(n), run, cont, hit.then(|| t.severity_wash(sev)), hit.then_some(hue)));
        }
    }
    div()
        .flex()
        .flex_col()
        .rounded(radius::LG)
        .border_1()
        .border_color(t.border)
        .bg(t.code_bg)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .h(px(30.))
                .px(space::S3)
                .border_b_1()
                .border_color(t.border)
                .bg(t.surface)
                .child(icon(IconName::FileCode, px(13.), t.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(text::MONO)
                        .text_size(px(11.))
                        .text_color(t.muted)
                        .child(title.to_string()),
                )
                .children(anchor.map(|l| {
                    div().flex_none().text_size(text::CAPTION).text_color(hue).child(format!("finding at line {l}"))
                }))
                .children(sha.map(|s| {
                    div().flex_none().font_family(text::MONO).text_size(px(11.)).text_color(t.faint).child(s)
                })),
        )
        .child(rows)
}

/// A suggested fix: its prose paragraphs as prose, its code as code.
pub fn fix_block(t: &Colors, fix: &str, path: &str) -> Div {
    let mut out = div().flex().flex_col().gap(space::S3);
    for para in fix.split("\n\n") {
        let code = para.contains('\n') || para.trim_end().ends_with(['{', ';', ')', '}']);
        out = out.child(if code {
            let mut lx = Lexer::for_path(path);
            let mut rows = div()
                .rounded(radius::LG)
                .border_1()
                .border_color(t.border)
                .bg(t.code_bg)
                .py(space::S2)
                .font_family(text::MONO)
                .text_size(text::CODE);
            for l in para.lines() {
                for (run, cont) in runs(t, &mut lx, l, layout::COLS_EVIDENCE) {
                    rows = rows.child(code_row(t, None, run, cont, Some(t.add_bg), Some(t.add_fg)));
                }
            }
            rows
        } else {
            prose(t, para)
        });
    }
    out
}

/// A line of GitHub markdown: `**bold**` and `` `code` ``.
pub fn md_line(t: &Colors, src: &str) -> StyledText {
    let (plain, spans) = inline_markdown(src);
    let marks: Vec<_> = spans
        .into_iter()
        .map(|(r, k)| {
            (
                r,
                match k {
                    Inline::Bold => HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() },
                    Inline::Code => HighlightStyle {
                        color: Some(t.syn_fn),
                        background_color: Some(t.border.alpha(0.6)),
                        ..Default::default()
                    },
                },
            )
        })
        .collect();
    StyledText::new(plain).with_highlights(marks)
}
