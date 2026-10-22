//! What floats over the window: the command palette, the sheets (a post, a cancel, a refusal), the shortcut
//! sheet, the jump-to input and the reducer's notices.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{ConfirmState, InputPurpose, Modal, Model, Preview, Screen, fuzzy};
use pr_review_tui::protocol::{Command, PrId};
use pr_review_tui::update::{can_confirm, can_mutate, write_block};

use crate::detail::who;
use crate::kit::{Tone, button, icon, keycap, overline};
use crate::queue::{self, Card};
use crate::tokens::{Colors, layout, radius, space, text};
use crate::ui::{Overlay, Shell, on};

#[derive(Clone)]
enum Run {
    Card(usize),
    Pr(PrId),
    Direct(Mutation),
    Act(Action),
    GitHub,
    Tab(bool),
    Keys,
}

struct Entry {
    group: &'static str,
    title: String,
    key: Option<&'static str>,
    /// Why it would do nothing now. Shown, never hidden, so the palette also teaches what exists.
    blocked: Option<String>,
    run: Run,
}

fn mutation(m: &Model, mu: Mutation, title: String, key: &'static str, otherwise: &str) -> Entry {
    let blocked = (!can_mutate(m, mu)).then(|| match write_block(m, mu.capability()) {
        Some(why) => why.to_string(),
        None if m.selected_pr().is_none() => "no PR is selected".into(),
        None => otherwise.to_string(),
    });
    let run =
        if matches!(mu, Mutation::Start | Mutation::Retry) { Run::Direct(mu) } else { Run::Act(Action::Mutation(mu)) };
    Entry { group: "Review", title, key: Some(key), blocked, run }
}

/// Every command, contextual ones first: each card's decision, the selected PR's actions with the reason any
/// can't run, then going to a PR, then the views.
fn entries(s: &Shell) -> Vec<Entry> {
    let m = &s.model;
    let mut out = Vec::new();
    for (i, c) in queue::cards(m).iter().enumerate() {
        let title = match c {
            Card::Setup { .. } => "Show setup: reviews can't run yet".to_string(),
            Card::Approve { pr, .. } => format!("Review and post #{}", pr.number),
            Card::Posting { pr, .. } => format!("Check the posting on #{}", pr.number),
            Card::Findings { pr, findings } => {
                format!("Judge {} in #{}", queue::count(findings.len(), "finding"), pr.number)
            }
            Card::Failed { pr, .. } => format!("Retry the failed review of #{}", pr.number),
            Card::Start { pr } => format!("Start a review of #{}", pr.number),
        };
        out.push(Entry { group: "Needs you", title, key: None, blocked: None, run: Run::Card(i) });
    }
    for (p, _) in queue::running(m) {
        out.push(Entry {
            group: "Needs you",
            title: format!("Watch #{} live", p.number),
            key: None,
            blocked: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    let n = m.selected_pr().map_or(String::new(), |p| format!(" #{}", p.number));
    out.push(mutation(
        m,
        Mutation::Publish,
        format!("Review and post the proof of{n}"),
        "P",
        "nothing is waiting for approval",
    ));
    out.push(mutation(
        m,
        Mutation::Start,
        format!("Start a review of{n} at its head"),
        "S",
        "a review already holds this head",
    ));
    out.push(mutation(
        m,
        Mutation::Retry,
        format!("Retry the failed review of{n}"),
        "⇧R",
        "its latest run has not failed",
    ));
    out.push(mutation(m, Mutation::Cancel, format!("Cancel the review of{n}"), "C", "no review is running on it"));
    out.push(Entry {
        group: "Review",
        title: format!("Open{n} on GitHub"),
        key: Some("⌘O"),
        blocked: m.selected_pr().is_none().then(|| "no PR is selected".to_string()),
        run: Run::GitHub,
    });
    for p in m.inbox() {
        out.push(Entry {
            group: "Go to",
            title: format!("#{} {} · {}", p.number, p.title, p.repository),
            key: None,
            blocked: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    let selected = m.selected_pr().is_some();
    out.extend([
        Entry { group: "View", title: "Needs you".into(), key: Some("⌘1"), blocked: None, run: Run::Tab(false) },
        Entry { group: "View", title: "Done".into(), key: Some("⌘2"), blocked: None, run: Run::Tab(true) },
        Entry {
            group: "View",
            title: "Diff of the selected PR".into(),
            key: Some("D"),
            blocked: (!selected).then(|| "no PR is selected".to_string()),
            run: Run::Act(Action::OpenDiff),
        },
        Entry {
            group: "View",
            title: "Timeline of the selected PR".into(),
            key: Some("T"),
            blocked: (!selected).then(|| "no PR is selected".to_string()),
            run: Run::Act(Action::OpenTimeline),
        },
        Entry {
            group: "View",
            title: "Setup and accounts".into(),
            key: Some("⌘,"),
            blocked: None,
            run: Run::Act(Action::Goto(Screen::Settings)),
        },
        Entry {
            group: "App",
            title: "Refresh from the daemon".into(),
            key: Some("⌘R"),
            blocked: None,
            run: Run::Act(Action::Refresh),
        },
        Entry { group: "App", title: "Keyboard shortcuts".into(), key: Some("?"), blocked: None, run: Run::Keys },
    ]);
    out
}

/// How well `query` matches: 0 a prefix of the title, 1 every word a prefix of a title word, 2 the initials of the
/// title's words, 3 a scattered subsequence. `None` when it does not match at all.
pub fn score(query: &str, title: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let title_l = title.to_lowercase();
    let words: Vec<&str> = title_l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let q_words: Vec<&str> = q.split_whitespace().collect();
    if title_l.starts_with(&q) {
        return Some(0);
    }
    if q_words.iter().all(|qw| words.iter().any(|w| w.starts_with(qw))) {
        return Some(1);
    }
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if !q.contains(' ') && initials.contains(&q) {
        return Some(2);
    }
    fuzzy(query, title).then_some(3)
}

/// The entries `query` matches, best first. Groups stay together: a group sorts by its best entry, and its entries
/// by their own score, runnable before blocked, then in the order `entries` gives.
fn matching(s: &Shell, query: &str) -> Vec<Entry> {
    let found: Vec<(u8, usize, Entry)> = entries(s)
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| score(query, &e.title).or_else(|| fuzzy(query, e.group).then_some(3)).map(|sc| (sc, i, e)))
        .collect();
    if query.trim().is_empty() {
        return found.into_iter().map(|(_, _, e)| e).collect();
    }
    let mut groups: Vec<(&'static str, u8, usize)> = Vec::new();
    for (sc, i, e) in &found {
        match groups.iter_mut().find(|g| g.0 == e.group) {
            Some(g) => g.1 = g.1.min(*sc),
            None => groups.push((e.group, *sc, *i)),
        }
    }
    let mut found = found;
    found.sort_by_key(|(sc, i, e)| {
        let g = groups.iter().find(|g| g.0 == e.group).map_or((3, 0), |g| (g.1, g.2));
        (g, *sc, e.blocked.is_some(), *i)
    });
    found.into_iter().map(|(_, _, e)| e).collect()
}

fn run(s: &mut Shell, r: Run, cx: &mut Context<Shell>) {
    s.overlay = None;
    match r {
        Run::Card(i) => s.select_card(i, cx),
        Run::Pr(id) => {
            s.done_tab = queue::done(&s.model).iter().any(|(p, _)| p.id == id);
            s.select_pr(id, cx)
        }
        Run::Direct(mu) => s.direct(mu, cx),
        Run::Act(a) => s.act(a, cx),
        Run::GitHub => s.open_github(cx),
        Run::Tab(done) => s.set_tab(done, cx),
        Run::Keys => s.overlay = Some(Overlay::Keys),
    }
    cx.notify();
}

/// Keys while the palette is open; true when it took the key.
pub fn palette_key(s: &mut Shell, key: &str, typed: Option<&str>, cx: &mut Context<Shell>) -> bool {
    let Some(Overlay::Palette { query, selected }) = &s.overlay else { return false };
    let (mut q, mut sel) = (query.clone(), *selected);
    let n = matching(s, &q).len();
    match key {
        "escape" => s.overlay = None,
        "enter" => {
            if let Some(e) = matching(s, &q).into_iter().nth(sel)
                && e.blocked.is_none()
            {
                run(s, e.run, cx);
            }
            return true;
        }
        _ => {
            match key {
                "down" => sel = (sel + 1).min(n.saturating_sub(1)),
                "up" => sel = sel.saturating_sub(1),
                "backspace" => {
                    q.pop();
                    sel = 0;
                }
                _ => match typed {
                    Some(c) if !c.chars().any(char::is_control) => {
                        q.push_str(c);
                        sel = 0;
                    }
                    _ => return true,
                },
            }
            s.overlay = Some(Overlay::Palette { query: q, selected: sel });
        }
    }
    cx.notify();
    true
}

fn scrim(t: &Colors, id: &'static str, cx: &Context<Shell>, close: bool) -> Stateful<Div> {
    div().id(id).absolute().inset_0().bg(t.scrim).occlude().when(close, |d| {
        d.on_click(cx.listener(|s, _: &ClickEvent, _, cx| {
            s.overlay = None;
            cx.notify();
        }))
    })
}

fn sheet(t: &Colors, w: Pixels) -> Div {
    div()
        .w(w)
        .flex()
        .flex_col()
        .rounded(radius::LG)
        .border_1()
        .border_color(t.border_strong)
        .bg(t.surface)
        .overflow_hidden()
        .shadow(vec![BoxShadow {
            color: t.shadow,
            offset: point(px(0.), px(16.)),
            blur_radius: px(48.),
            spread_radius: px(0.),
            inset: false,
        }])
}

pub fn layer(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Vec<AnyElement> {
    let m = &s.model;
    let mut out = Vec::new();
    if let Some(n) = &m.notice {
        out.push(toast(IconName::Info, n.text.clone(), t.text, t.bg));
    }
    match m.top_modal() {
        Some(Modal::Confirm { prompt, command, state }) => {
            // A start or retry in flight is a quiet status, not a sheet; a refusal or a question needs the reader.
            let quiet = matches!(command, Command::Start(_) | Command::Retry(_))
                && matches!(state, ConfirmState::Requested(_) | ConfirmState::Accepted { .. });
            if quiet {
                out.push(toast(
                    IconName::LoaderCircle,
                    format!("{} Sending…", prompt.trim_end_matches('?')),
                    t.text,
                    t.bg,
                ));
            } else {
                out.push(confirm(s, t, prompt, command, state, cx));
            }
        }
        Some(Modal::Input { purpose, buffer, .. }) => out.push(input(t, *purpose, buffer, cx)),
        Some(Modal::Help { .. }) => out.push(keys(t, cx, true)),
        _ => {}
    }
    match &s.overlay {
        Some(Overlay::Palette { query, selected }) => out.push(palette(s, t, query, *selected, cx)),
        Some(Overlay::Keys) => out.push(keys(t, cx, false)),
        Some(Overlay::ConfirmPost) => out.push(confirm_post(s, t, cx)),
        None => {}
    }
    out
}

fn toast(glyph: IconName, words: String, bg: Hsla, fg: Hsla) -> AnyElement {
    div()
        .absolute()
        .bottom(layout::BAR + space::S4)
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S4)
                .h(px(36.))
                .rounded(radius::PILL)
                .bg(bg)
                .text_color(fg)
                .text_size(text::BODY)
                .child(icon(glyph, px(14.), fg))
                .child(words),
        )
        .into_any_element()
}

fn palette(s: &Shell, t: &Colors, query: &str, selected: usize, cx: &Context<Shell>) -> AnyElement {
    let found = matching(s, query);
    let mut list = div().id("palette-list").flex().flex_col().max_h(px(440.)).overflow_y_scroll().p(space::S2);
    let mut group = "";
    if found.is_empty() {
        list = list.child(
            div()
                .p(space::S4)
                .text_size(text::BODY)
                .text_color(t.muted)
                .child(format!("Nothing matches \u{201c}{query}\u{201d}")),
        );
    }
    for (i, e) in found.into_iter().enumerate() {
        if e.group != group {
            group = e.group;
            list = list.child(div().px(space::S3).pt(space::S3).pb(space::S1).child(overline(t, group)));
        }
        let on_it = i == selected;
        let blocked = e.blocked.is_some();
        let r = e.run.clone();
        list = list.child(
            div()
                .id(("pal", i))
                .flex()
                .items_center()
                .gap(space::S3)
                .h(px(36.))
                .px(space::S3)
                .rounded(radius::SM)
                .cursor_pointer()
                .when(on_it, |d| {
                    d.bg(if blocked { t.selected } else { t.accent }).text_color(if blocked {
                        t.text
                    } else {
                        t.on_accent
                    })
                })
                .when(!on_it, |d| d.hover(|d| d.bg(t.hover)))
                .when(blocked, |d| d.opacity(0.6))
                .when(!blocked, |d| d.on_click(cx.listener(move |s, _: &ClickEvent, _, cx| run(s, r.clone(), cx))))
                .child(div().flex_1().min_w_0().truncate().text_size(text::BODY).child(e.title))
                .children(e.blocked.map(|why| {
                    div()
                        .flex_none()
                        .text_size(text::CAPTION)
                        .text_color(if on_it { t.muted } else { t.faint })
                        .child(why)
                }))
                .child(match e.key {
                    Some(k) => keycap(t, k, on_it && !blocked),
                    None => div()
                        .w(px(18.))
                        .text_center()
                        .text_size(text::CAPTION)
                        .text_color(if on_it && !blocked { t.on_accent.alpha(0.6) } else { t.faint })
                        .child("—"),
                })
                .when(on_it && !blocked, |d| d.child(keycap(t, "⏎", true))),
        );
    }
    scrim(t, "palette-scrim", cx, true)
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.))
        .child(
            sheet(t, layout::PALETTE)
                .id("palette")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .h(px(52.))
                        .px(space::S4)
                        .border_b_1()
                        .border_color(t.border)
                        .child(icon(IconName::Search, px(16.), t.muted))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .flex_1()
                                .text_size(text::LEAD)
                                .child(if query.is_empty() {
                                    div().text_color(t.faint).child("Type a command, a PR number or a title")
                                } else {
                                    div().text_color(t.text).child(query.to_string())
                                })
                                .child(div().w(px(2.)).h(px(18.)).ml(px(1.)).bg(t.accent)),
                        )
                        .child(keycap(t, "esc", false)),
                )
                .child(list)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S4)
                        .h(px(36.))
                        .px(space::S4)
                        .border_t_1()
                        .border_color(t.border)
                        .bg(t.raised)
                        .text_size(text::CAPTION)
                        .text_color(t.faint)
                        .children([("↑↓", "choose"), ("⏎", "run"), ("esc", "close")].map(|(k, what)| {
                            div().flex().items_center().gap(px(6.)).child(keycap(t, k, false)).child(what)
                        }))
                        .child(div().flex_1())
                        .child("dimmed actions say why they can't run"),
                ),
        )
        .into_any_element()
}

/// The reducer's confirmation: a cancel asks with Keep as the default; a refused start or retry explains.
fn confirm(
    s: &Shell,
    t: &Colors,
    prompt: &str,
    command: &Command,
    state: &ConfirmState,
    cx: &Context<Shell>,
) -> AnyElement {
    let (verb, tone, glyph, hue) = match command {
        Command::Start(_) => ("Start review", Tone::Primary, IconName::Play, t.accent),
        Command::Retry(_) => ("Retry", Tone::Primary, IconName::RotateCw, t.accent),
        Command::Cancel(_) => ("Cancel review", Tone::Danger, IconName::CircleX, t.danger),
        Command::SelectAccount(_) => ("Use account", Tone::Primary, IconName::User, t.accent),
        _ => ("Confirm", Tone::Primary, IconName::Check, t.accent),
    };
    let status = match state {
        ConfirmState::Asking => None,
        ConfirmState::Requested(_) => Some(("Sending…".to_string(), t.running)),
        ConfirmState::Accepted { .. } => {
            Some(("Accepted. Waiting for the daemon to report it.".to_string(), t.running))
        }
        ConfirmState::Refused { message, .. } => Some((message.to_string(), t.danger)),
        ConfirmState::TimedOut => Some(("No reply in time. ⏎ sends it again under the same key.".to_string(), t.warn)),
    };
    let ready = can_confirm(&s.model);
    let done = matches!(state, ConfirmState::Accepted { .. } | ConfirmState::Refused { .. });
    let keep = matches!(command, Command::Cancel(_));
    scrim(t, "confirm-scrim", cx, false)
        .flex()
        .items_center()
        .justify_center()
        .child(
            sheet(t, layout::SHEET)
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(hue.alpha(0.14))
                        .child(icon(glyph, px(18.), hue)),
                )
                .child(
                    div()
                        .text_size(text::TITLE)
                        .line_height(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(prompt.to_string()),
                )
                .child(div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(
                    match command {
                        Command::Cancel(_) => {
                            "The run stops where it is and its work so far is lost. Nothing it found is posted."
                        }
                        Command::Start(_) | Command::Retry(_) => {
                            "The reviewer reads the diff with read-only tools. Nothing posts without your approval."
                        }
                        _ => "The daemon uses this account for GitHub reads and for posting.",
                    },
                ))
                .children(status.map(|(s, c)| div().text_size(text::BODY).text_color(c).child(s)))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(
                            t,
                            "c-no",
                            if done {
                                "Close"
                            } else if keep {
                                "Keep running"
                            } else {
                                "Not now"
                            },
                            Some("esc"),
                            if keep && !done { Tone::Secondary } else { Tone::Ghost },
                            on(cx, |s, cx| s.act(Action::Back, cx)),
                        ))
                        .when(!done, |d| {
                            d.child(button(
                                t,
                                "c-yes",
                                verb,
                                Some("⏎"),
                                tone,
                                ready.then(|| -> crate::kit::OnClick {
                                    Box::new(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Confirm, cx)))
                                }),
                            ))
                        }),
                ),
        )
        .into_any_element()
}

/// The one irreversible act asks once more, naming exactly what goes out. Cancel is the default; only ⌘⏎ posts.
fn confirm_post(s: &Shell, t: &Colors, cx: &Context<Shell>) -> AnyElement {
    let m = &s.model;
    let Some(Modal::Publication { run_id, preview: Preview::Ready(p), .. }) = m.top_modal() else {
        return div().into_any_element();
    };
    let pr = m.domain.runs.get(run_id).and_then(|r| m.domain.prs.get(&r.pr_id));
    let target = pr.map_or("this PR".to_string(), |p| format!("{}#{}", p.repository, p.number));
    let fact = |label: &str, value: String| {
        div()
            .flex()
            .gap(space::S3)
            .text_size(text::BODY)
            .child(div().w(px(96.)).flex_none().text_color(t.faint).child(label.to_string()))
            .child(div().flex_1().min_w_0().text_color(t.text).child(value))
    };
    scrim(t, "post-scrim", cx, true)
        .flex()
        .items_center()
        .justify_center()
        .child(
            sheet(t, layout::SHEET)
                .id("post-sheet")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .p(space::S6)
                .gap(space::S4)
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::PILL)
                        .bg(t.accent.alpha(0.14))
                        .child(icon(IconName::Send, px(18.), t.accent)),
                )
                .child(
                    div()
                        .text_size(text::TITLE)
                        .line_height(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Post this review to {target}?")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::S2)
                        .child(fact("As", who(m)))
                        .child(fact("At commit", p.head_sha.short().to_string()))
                        .child(fact(
                            "It posts",
                            format!("1 review body and {}", queue::count(p.comments.len(), "inline comment")),
                        ))
                        .child(fact("Review kind", p.event.as_str().to_lowercase().replace('_', " "))),
                )
                .child(
                    div()
                        .text_size(text::BODY)
                        .line_height(text::LINE_BODY)
                        .text_color(t.danger)
                        .child("This can't be undone from here. GitHub keeps the review once it is posted."),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(button(
                            t,
                            "post-no",
                            "Cancel",
                            Some("⏎"),
                            Tone::Secondary,
                            on(cx, |s, cx| {
                                s.overlay = None;
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            t,
                            "post-yes",
                            "Post to GitHub",
                            Some("⌘⏎"),
                            Tone::Primary,
                            on(cx, |s, cx| s.confirm_post(cx)),
                        )),
                ),
        )
        .into_any_element()
}

fn input(t: &Colors, purpose: InputPurpose, buffer: &str, cx: &Context<Shell>) -> AnyElement {
    let label = match purpose {
        InputPurpose::JumpTo => "Jump to path:line",
        InputPurpose::DiffSearch => "Search the diff",
        InputPurpose::TimelineFilter => "Filter events",
        InputPurpose::Search => "Filter PRs",
    };
    scrim(t, "input-scrim", cx, false)
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.))
        .child(
            sheet(t, layout::PALETTE).child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .h(px(52.))
                    .px(space::S4)
                    .child(div().text_size(text::SMALL).text_color(t.muted).child(label))
                    .child(div().flex_1().font_family(text::MONO).text_size(text::LEAD).child(buffer.to_string()))
                    .child(div().w(px(2.)).h(px(18.)).bg(t.accent))
                    .child(keycap(t, "⏎", false)),
            ),
        )
        .into_any_element()
}

const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Queue",
        &[
            ("J / K", "next or previous card"),
            ("⏎", "do what the card's button says"),
            ("N / ⇧N", "next or previous finding in the PR"),
            ("D", "open the diff at the finding"),
            ("T", "the PR's timeline"),
            ("⇥", "move between controls"),
            ("esc", "back"),
        ],
    ),
    (
        "Decide",
        &[
            ("P", "open the proof of what would post"),
            ("⌘⏎", "post the proof, after one confirmation"),
            ("S", "start a review (no sheet)"),
            ("⇧R", "retry a failed review (no sheet)"),
            ("C", "cancel a review (asks first)"),
        ],
    ),
    (
        "Anywhere",
        &[
            ("⌘K  /", "command palette"),
            ("⌘1  ⌘2", "needs you, done"),
            ("⌘,", "setup and accounts"),
            ("⌘O", "open on GitHub"),
            ("⌘R", "refresh"),
            ("⌘W", "close the window"),
            ("?", "this sheet"),
        ],
    ),
];

fn keys(t: &Colors, cx: &Context<Shell>, reducer: bool) -> AnyElement {
    let groups = div().flex().gap(space::S6).children(KEYS.iter().map(|(g, rows)| {
        div().flex().flex_col().gap(space::S2).flex_1().child(overline(t, *g)).children(rows.iter().map(|(k, what)| {
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(div().w(px(64.)).flex().child(keycap(t, k, false)))
                .child(div().text_size(text::SMALL).text_color(t.muted).child(what.to_string()))
        }))
    }));
    let s = scrim(t, "keys-scrim", cx, !reducer);
    let s = if reducer { s.on_click(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Back, cx))) } else { s };
    s.flex()
        .items_center()
        .justify_center()
        .child(
            sheet(t, px(780.))
                .p(space::S6)
                .gap(space::S5)
                .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
                .child(groups),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn prefixes_and_initials_rank_before_subsequences() {
        assert_eq!(score("retry", "Review Retry the failed review of #345"), Some(1));
        assert_eq!(score("review retry", "Review Retry the failed review of #345"), Some(0));
        assert_eq!(score("rtf", "Review Retry the failed review"), Some(2));
        assert_eq!(score("rvw", "Review Retry the failed review"), Some(3));
        assert_eq!(score("zzz", "Review Retry"), None);
        assert_eq!(score("", "anything"), Some(0));
    }
}
