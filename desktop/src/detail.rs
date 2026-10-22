//! The detail pane: the decision behind the selected card. A PR and its findings, a finding with its cited code and
//! evidence, the exact publication a parked review would post, a review working live, and the setup checks.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::Modal;
use pr_review_tui::protocol::{
    EvidenceKind, Finding, FindingId, FindingStatus, Lifecycle, Pr, PublicationStatus, Run, RunStatus,
};
use pr_review_tui::update::can_mutate;
use pr_review_tui::view::Hit;

use crate::code::{Inline, Tok, hash_comments, inline_markdown, tokens};
use crate::kit::{OnClick, Tone, button, callout, chip, clock, heading, icon, overline, severity_badge};
use crate::queue;
use crate::tokens::{Colors, layout, radius, severity_mark, space, text};
use crate::ui::{Shell, on, phase_word};

/// The scrolling page and the action bar under it.
pub fn page(id: &'static str, body: Div, footer: Option<Div>, t: &Colors) -> Div {
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
                    .gap(space::S5)
                    .px(space::S8)
                    .pt(space::S6)
                    .pb(space::S8)
                    .max_w(px(760.))
                    .child(body),
            ),
        )
        .children(footer.map(|f| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S2)
                .h(px(56.))
                .px(space::S8)
                .border_t_1()
                .border_color(t.border)
                .bg(t.raised)
                .child(f)
        }))
}

pub fn pane(s: &Shell, t: &Colors, _: &mut Window, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    match m.ui.modals.iter().rev().find(|x| matches!(x, Modal::Finding { .. } | Modal::Publication { .. })) {
        Some(Modal::Publication { run_id, preview, publish, raw, .. }) => {
            return crate::publish::publication(s, t, run_id, preview, publish, *raw, cx);
        }
        Some(Modal::Finding { id, .. }) => return finding_page(s, t, id, cx),
        _ => {}
    }
    let Some(pr) = m.selected_pr() else {
        let issues = m.server.as_ref().is_some_and(|sv| sv.missing().next().is_some() || sv.commands_off().is_some());
        return page(
            "detail",
            if issues { crate::setup::setup_body(s, t, cx) } else { crate::setup::board(s, t) },
            None,
            t,
        );
    };
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

pub fn pr_header(t: &Colors, pr: &Pr, kicker: Div) -> Div {
    let state = match pr.lifecycle {
        Lifecycle::Merged => Some(("Merged", t.accent)),
        Lifecycle::Closed => Some(("Closed", t.muted)),
        _ => None,
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(kicker)
        .child(
            div()
                .text_size(text::DISPLAY)
                .line_height(px(30.))
                .font_weight(FontWeight::BOLD)
                .child(pr.title.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .text_size(text::SMALL)
                .text_color(t.muted)
                .child(format!("{} #{}", pr.repository, pr.number))
                .child("·")
                .child(format!("by {}", pr.author.as_ref().map_or("unknown", |a| a.as_str())))
                .child("·")
                .child(div().font_family(text::MONO).child(pr.head_sha.short().to_string()))
                .children(state.map(|(w, c)| chip(t, w, c))),
        )
}

fn pr_page(s: &Shell, t: &Colors, pr: &Pr, run: Option<&Run>, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let findings = m.domain.pr_findings(&pr.id);
    let (current, stale): (Vec<&Finding>, Vec<&Finding>) = findings.iter().partition(|f| !f.stale);
    let mut body = div().flex().flex_col().gap(space::S5).child(pr_header(t, pr, overline(t, "Pull request")));

    body = body.child(match run {
        None => callout(
            t,
            t.muted,
            IconName::Play,
            "No review yet",
            Some("Start one and its progress shows live in the strip above.".into()),
        ),
        Some(r) if r.parked() => callout(
            t,
            t.accent,
            IconName::Send,
            "Parked for your approval",
            Some("The review is stored. Nothing posts to GitHub until you approve the exact preview.".into()),
        ),
        Some(r) if r.status == RunStatus::Failed => callout(
            t,
            t.danger,
            IconName::CircleX,
            format!("Review failed{}", r.attempt.map_or(String::new(), |a| format!(" on attempt {a}"))),
            r.failure_reason.as_ref().map(|x| x.to_string()),
        ),
        Some(r) => {
            let posted = m.domain.publications.get(&r.id).map(|p| p.status);
            let (hue, title) = match posted {
                Some(PublicationStatus::Published) => (t.ok, "Review posted to GitHub".to_string()),
                Some(PublicationStatus::Failed) => (t.danger, "Posting failed".to_string()),
                Some(PublicationStatus::Uncertain) => {
                    (t.warn, "Posting unconfirmed: the daemon is checking GitHub".to_string())
                }
                _ if current.is_empty() => (t.ok, "Reviewed: nothing found".to_string()),
                _ => (
                    t.text,
                    format!(
                        "Reviewed: {} to judge",
                        queue::count(current.iter().filter(|f| queue::open_finding(f)).count(), "finding")
                    ),
                ),
            };
            callout(
                t,
                hue,
                if hue == t.text { IconName::Eye } else { IconName::CircleCheck },
                title,
                Some(format!(
                    "{} at {} with {}{}.",
                    phase_word(r.phase),
                    r.completed_at.as_ref().map_or(clock(r.started_at.as_str()), |c| clock(c.as_str())),
                    r.model,
                    if posted.is_none() && !current.is_empty() { ". Nothing was posted" } else { "" }
                )),
            )
        }
    });

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
            .when(current.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(text::BODY)
                        .text_color(t.muted)
                        .child("The reviewer reported nothing on this head."),
                )
            })
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
    page("detail-pr", body, Some(pr_actions(s, t, pr, run, cx)), t)
}

fn pr_actions(s: &Shell, t: &Colors, pr: &Pr, run: Option<&Run>, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let mu = |mu: Mutation| if can_mutate(m, mu) { on(cx, move |s, cx| s.act(Action::Mutation(mu), cx)) } else { None };
    let mut bar = div().flex().items_center().gap(space::S2).flex_1();
    match run {
        Some(r) if r.parked() => {
            bar = bar.child(button(t, "a-post", "Review post", Some("P"), Tone::Primary, mu(Mutation::Publish)))
        }
        Some(r) if r.status == RunStatus::Failed => {
            bar = bar.child(button(t, "a-retry", "Retry review", Some("⇧R"), Tone::Primary, mu(Mutation::Retry)))
        }
        None => bar = bar.child(button(t, "a-start", "Start review", Some("S"), Tone::Primary, mu(Mutation::Start))),
        _ => {}
    }
    if m.ui.selected_finding.is_some() {
        bar = bar.child(button(t, "a-open", "Open finding", Some("⏎"), Tone::Secondary, {
            let id = m.ui.selected_finding.clone();
            id.map(|id| -> OnClick {
                Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| {
                    s.send(pr_review_tui::update::Msg::Pick(Hit::Finding(id.clone())), cx)
                }))
            })
        }));
    }
    bar =
        bar.child(button(t, "a-diff", "Diff", Some("D"), Tone::Secondary, on(cx, |s, cx| s.act(Action::OpenDiff, cx))));
    if pr.lifecycle == Lifecycle::Active && run.is_some_and(|r| r.parked()) {
        bar = bar.child(button(t, "a-cancel", "Cancel", Some("C"), Tone::Ghost, mu(Mutation::Cancel)));
    }
    bar.child(div().flex_1()).child(button(
        t,
        "a-gh",
        "Open on GitHub",
        Some("⌘O"),
        Tone::Ghost,
        on(cx, |s, _| s.open_github()),
    ))
}

pub fn status_word(f: &Finding) -> Option<(&'static str, &'static str)> {
    Some(match f.status {
        FindingStatus::Detected => return None,
        FindingStatus::Published => ("Posted", "on GitHub"),
        FindingStatus::StillPresent => ("Still present", "the newest head still has it"),
        FindingStatus::PotentiallyResolved => ("Looks resolved", "the code it cites changed; not yet confirmed"),
        FindingStatus::Outdated => ("Outdated", "its lines are gone"),
        FindingStatus::Resolved => ("Resolved", "a later review confirmed the fix"),
        FindingStatus::Uncertain => ("Uncertain", "the reviewer could not confirm it either way"),
        FindingStatus::Unknown => ("Unknown", ""),
    })
}

pub fn finding_row(s: &Shell, t: &Colors, f: &Finding, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let selected = m.ui.selected_finding.as_ref() == Some(&f.id);
    let (mark, word) = severity_mark(f.severity);
    let hue = t.severity(f.severity);
    let id = f.id.clone();
    let detail = m.details.get(&f.id);
    let place = format!("{}{}", f.file, f.line.map_or(String::new(), |l| format!(":{l}")));
    div()
        .id(SharedString::from(format!("f-{}", f.id.as_str())))
        .flex()
        .flex_col()
        .gap(space::S2)
        .px(space::S3)
        .py(px(10.))
        .rounded(radius::MD)
        .border_1()
        .border_color(if selected { t.accent.alpha(0.75) } else { t.border })
        .bg(if selected { t.selected } else { t.surface })
        .cursor_pointer()
        .hover(|d| d.border_color(if selected { t.accent } else { t.border_strong }))
        .active(|d| d.bg(t.surface_press))
        .when(f.stale, |d| d.opacity(0.75))
        .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
            s.send(pr_review_tui::update::Msg::Pick(Hit::Finding(id.clone())), cx)
        }))
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
                .children(status_word(f).map(|(w, _)| chip(t, w, t.muted))),
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
                    .line_clamp(3)
                    .child(x.explanation.to_string())
            }))
        })
}

fn finding_page(s: &Shell, t: &Colors, id: &FindingId, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let Some(f) = m.domain.findings.get(id) else {
        return page("detail-f", empty(t, "This finding is gone", "A newer snapshot no longer has it."), None, t);
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
        .child(div().text_size(text::SMALL).text_color(t.faint).child(format!("{at} of {}", all.len())));
    let mut body = div().flex().flex_col().gap(space::S5).child(crumbs);
    let mut badges = div().flex().items_center().gap(space::S2).child(severity_badge(t, f.severity)).child(chip(
        t,
        f.category.label(),
        t.muted,
    ));
    if let Some((w, why)) = status_word(f) {
        badges = badges.child(chip(t, w, t.accent)).child(div().text_size(text::SMALL).text_color(t.faint).child(why));
    } else {
        badges = badges.child(chip(t, "New in this review", t.accent));
    }
    if f.stale {
        badges = badges.child(chip(t, "From an earlier head", t.warn));
    }
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(badges)
            .child(
                div()
                    .text_size(text::DISPLAY)
                    .line_height(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .child(f.title.to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .child(div().font_family(text::MONO).text_size(text::SMALL).text_color(t.muted).child(format!(
                        "{}{}",
                        f.file,
                        f.line.map_or(String::new(), |l| format!(":{l}"))
                    )))
                    .child(confidence(t, f.confidence)),
            ),
    );
    let Some(d) = m.details.get(id) else {
        let (title, note) = match m.detail_errors.get(id) {
            Some(e) => ("Couldn't load the evidence", e.to_string()),
            None => {
                ("Loading the evidence…", "The explanation, cited code and history come from the daemon.".to_string())
            }
        };
        return page("detail-f", body.child(callout(t, t.muted, IconName::LoaderCircle, title, Some(note))), None, t);
    };
    body = body.child(section(t, "What's wrong", prose(t, d.explanation.as_str())));
    let refs: Vec<_> = d.evidence.iter().filter(|e| e.kind == EvidenceKind::CodeRef && e.path.is_some()).collect();
    if !refs.is_empty() {
        let mut blocks = div().flex().flex_col().gap(space::S3);
        for e in refs {
            let path = e.path.as_ref().map_or("", |p| p.as_str());
            let start = e.start.unwrap_or(1);
            let end = e.end.unwrap_or(start);
            let anchor = f.line.map(|l| l.get()).filter(|l| path == f.file.as_str() && (start..=end).contains(l));
            let title = format!(
                "{path}:{start}{}",
                e.end.filter(|end| *end != start).map_or(String::new(), |end| format!("–{end}"))
            );
            let sha = e.ref_sha.as_ref().map(|s| s.short().to_string());
            blocks = blocks.child(code_block(
                t,
                &title,
                sha,
                e.text.as_str(),
                start,
                anchor,
                f.severity,
                hash_comments(path),
            ));
        }
        body = body.child(section(t, "Cited code", blocks));
    }
    let tools: Vec<_> = d.evidence.iter().filter(|e| e.kind != EvidenceKind::CodeRef || e.path.is_none()).collect();
    let mut checked = div().flex().flex_col().gap(space::S3).child(prose(t, d.reasoning_summary.as_str()));
    for e in tools {
        let (tool, rest) = e.text.as_str().split_once(' ').unwrap_or(("note", e.text.as_str()));
        checked = checked.child(
            div()
                .flex()
                .gap(space::S3)
                .items_start()
                .child(div().pt(px(2.)).child(icon(IconName::Bot, px(14.), t.faint)))
                .child(
                    div()
                        .flex_none()
                        .font_family(text::MONO)
                        .text_size(px(11.))
                        .pt(px(1.))
                        .text_color(t.accent)
                        .child(tool.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(text::BODY)
                        .line_height(text::LINE_BODY)
                        .text_color(t.muted)
                        .child(rest.to_string()),
                ),
        );
    }
    body = body.child(section(t, "How the reviewer checked", checked));
    if let Some(fix) = &d.suggested_fix {
        body = body.child(section(t, "Suggested fix", fix_block(t, fix.as_str(), hash_comments(f.file.as_str()))));
    }
    let history = div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(div().text_size(text::SMALL).text_color(t.muted).child(format!(
            "Tracked across commits by fingerprint {}. Each review says whether it still holds.",
            &f.fingerprint.as_str()[..f.fingerprint.as_str().len().min(8)]
        )))
        .children(d.history.iter().map(|h| {
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
                .child(div().text_size(text::SMALL).text_color(t.faint).child(format!(
                    "{} · run {}",
                    clock(h.at.as_str()),
                    h.run_id.short()
                )))
        }));
    body = body.child(section(t, "History", history));
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
            "f-next",
            "Next finding",
            Some("N"),
            Tone::Secondary,
            (all.len() > 1).then(|| -> OnClick {
                Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::NextFinding, cx)))
            }),
        ))
        .child(div().flex_1())
        .child(button(t, "f-gh", "Open on GitHub", Some("⌘O"), Tone::Ghost, on(cx, |s, _| s.open_github())));
    page("detail-f", body, Some(footer), t)
}

pub fn confidence(t: &Colors, c: f64) -> Div {
    let pct = (c * 100.).round().clamp(0., 100.) as f32;
    let word = if c >= 0.85 {
        "high confidence"
    } else if c >= 0.6 {
        "moderate confidence"
    } else {
        "low confidence"
    };
    div()
        .flex()
        .items_center()
        .gap(space::S2)
        .child(
            div()
                .w(px(64.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(t.border)
                .child(div().h_full().w(px(0.64 * pct)).rounded(px(2.)).bg(t.muted)),
        )
        .child(div().text_size(text::SMALL).text_color(t.muted).child(format!("{pct:.0}% · {word}")))
}

pub fn section(t: &Colors, title: &str, content: impl IntoElement) -> Div {
    div().flex().flex_col().gap(space::S3).child(overline(t, title)).child(content)
}

pub fn prose(t: &Colors, s: &str) -> Div {
    div()
        .max_w(layout::MEASURE)
        .text_size(text::BODY)
        .line_height(text::LINE_READ)
        .text_color(t.text)
        .child(md_line(t, s))
}

/// One line of code, colored by `crate::code::tokens`.
pub fn styled_code(t: &Colors, line: &str, hash: bool) -> StyledText {
    let line = line.replace('\t', "    ");
    let marks: Vec<_> = tokens(&line, hash)
        .into_iter()
        .map(|(r, tok)| {
            let color = match tok {
                Tok::Keyword => t.syn_keyword,
                Tok::Str => t.syn_string,
                Tok::Number => t.syn_number,
                Tok::Comment => t.syn_comment,
                Tok::Call => t.syn_fn,
            };
            (r, HighlightStyle { color: Some(color), ..Default::default() })
        })
        .collect();
    StyledText::new(line).with_highlights(marks)
}

/// A gutter, a marker and a line of code. `wash` tints the row; `mark` draws the anchor bar in the gutter.
pub fn code_row(
    t: &Colors,
    number: Option<u32>,
    line: &str,
    hash: bool,
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
                .child(number.map_or(String::new(), |n| n.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(t.text)
                .child(styled_code(t, line, hash)),
        )
}

#[allow(clippy::too_many_arguments)]
pub fn code_block(
    t: &Colors,
    title: &str,
    sha: Option<String>,
    code: &str,
    start: u32,
    anchor: Option<u32>,
    sev: pr_review_tui::protocol::Severity,
    hash: bool,
) -> Div {
    let hue = t.severity(sev);
    div()
        .flex()
        .flex_col()
        .rounded(radius::MD)
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
                .child(icon(IconName::FileText, px(13.), t.faint))
                .child(
                    div()
                        .flex_1()
                        .font_family(text::MONO)
                        .text_size(px(11.))
                        .text_color(t.muted)
                        .child(title.to_string()),
                )
                .children(
                    anchor
                        .map(|l| div().text_size(text::CAPTION).text_color(hue).child(format!("finding at line {l}"))),
                )
                .children(sha.map(|s| div().font_family(text::MONO).text_size(px(11.)).text_color(t.faint).child(s))),
        )
        .child(div().py(space::S2).font_family(text::MONO).text_size(text::CODE).children(
            code.lines().enumerate().map(|(i, l)| {
                let n = start + i as u32;
                let hit = anchor == Some(n);
                code_row(t, Some(n), l, hash, hit.then(|| t.severity_wash(sev)), hit.then_some(hue))
            }),
        ))
}

/// A suggested fix: its prose paragraphs as prose, its code as code.
pub fn fix_block(t: &Colors, fix: &str, hash: bool) -> Div {
    let mut out = div().flex().flex_col().gap(space::S3);
    for para in fix.split("\n\n") {
        let code = para.contains('\n') || para.trim_end().ends_with(['{', ';', ')', '}']);
        out = out.child(if code {
            div()
                .rounded(radius::MD)
                .border_1()
                .border_color(t.border)
                .bg(t.code_bg)
                .py(space::S2)
                .font_family(text::MONO)
                .text_size(text::CODE)
                .children(para.lines().map(|l| code_row(t, None, l, hash, Some(t.add_bg), Some(t.add_fg))))
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
