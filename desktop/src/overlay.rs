//! What floats over the window: the command palette, the confirmation sheet, the shortcut sheet, the jump-to input
//! and the reducer's notices.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{ConfirmState, InputPurpose, Modal, Screen, fuzzy};
use pr_review_tui::protocol::{Command, PrId};
use pr_review_tui::update::can_confirm;

use crate::kit::{Tone, button, icon, keycap, overline};
use crate::queue::{self, Card};
use crate::tokens::{Colors, layout, radius, space, text};
use crate::ui::{Overlay, Shell, on};

#[derive(Clone)]
enum Run {
    Card(usize),
    Pr(PrId),
    Mutate(PrId, Mutation),
    Act(Action),
    Tab(bool),
    Keys,
}

struct Entry {
    group: &'static str,
    title: String,
    key: Option<&'static str>,
    run: Run,
}

/// Every command, contextual ones first: each card's decision, then going to a PR, then the views.
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
        out.push(Entry { group: "Needs you", title, key: None, run: Run::Card(i) });
    }
    for (p, _) in queue::running(m) {
        out.push(Entry {
            group: "Needs you",
            title: format!("Cancel the running review of #{}", p.number),
            key: None,
            run: Run::Mutate(p.id.clone(), Mutation::Cancel),
        });
        out.push(Entry {
            group: "Go to",
            title: format!("Watch #{} live", p.number),
            key: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    for p in m.inbox() {
        out.push(Entry {
            group: "Go to",
            title: format!("#{} {}", p.number, p.title),
            key: None,
            run: Run::Pr(p.id.clone()),
        });
    }
    out.extend([
        Entry { group: "View", title: "Needs you".into(), key: Some("⌘1"), run: Run::Tab(false) },
        Entry { group: "View", title: "Done".into(), key: Some("⌘2"), run: Run::Tab(true) },
        Entry {
            group: "View",
            title: "Setup and accounts".into(),
            key: Some("⌘,"),
            run: Run::Act(Action::Goto(Screen::Settings)),
        },
        Entry {
            group: "View",
            title: "Diff of the selected PR".into(),
            key: Some("D"),
            run: Run::Act(Action::OpenDiff),
        },
        Entry {
            group: "App",
            title: "Refresh from the daemon".into(),
            key: Some("⌘R"),
            run: Run::Act(Action::Refresh),
        },
        Entry { group: "App", title: "Keyboard shortcuts".into(), key: Some("?"), run: Run::Keys },
    ]);
    out
}

fn matching(s: &Shell, query: &str) -> Vec<Entry> {
    entries(s).into_iter().filter(|e| fuzzy(query, &format!("{} {}", e.group, e.title))).collect()
}

fn run(s: &mut Shell, r: Run, cx: &mut Context<Shell>) {
    s.overlay = None;
    match r {
        Run::Card(i) => s.select_card(i, cx),
        Run::Pr(id) => {
            s.done_tab = queue::done(&s.model).iter().any(|(p, _)| p.id == id);
            s.select_pr(id, cx)
        }
        Run::Mutate(id, mu) => {
            s.select_pr(id, cx);
            s.act(Action::Mutation(mu), cx);
        }
        Run::Act(a) => s.act(a, cx),
        Run::Tab(done) => s.set_tab(done, cx),
        Run::Keys => s.overlay = Some(Overlay::Keys),
    }
    cx.notify();
}

/// Keys while the palette is open; true when it took the key.
pub fn palette_key(
    s: &mut Shell,
    key: &str,
    cmd: bool,
    typed: Option<&str>,
    _: &mut Window,
    cx: &mut Context<Shell>,
) -> bool {
    let Some(Overlay::Palette { query, selected }) = &s.overlay else { return false };
    let (mut q, mut sel) = (query.clone(), *selected);
    let n = matching(s, &q).len();
    match key {
        "escape" => s.overlay = None,
        "k" if cmd => s.overlay = None,
        "enter" => {
            if let Some(e) = matching(s, &q).into_iter().nth(sel) {
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
                    Some(c) if !cmd && !c.chars().any(char::is_control) => {
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
        out.push(
            div()
                .absolute()
                .bottom(space::S6)
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
                        .bg(t.text)
                        .text_color(t.bg)
                        .text_size(text::BODY)
                        .child(icon(IconName::Info, px(14.), t.bg))
                        .child(n.text.clone()),
                )
                .into_any_element(),
        );
    }
    match m.top_modal() {
        Some(Modal::Confirm { prompt, command, state }) => out.push(confirm(s, t, prompt, command, state, cx)),
        Some(Modal::Input { purpose, buffer, .. }) => out.push(input(t, *purpose, buffer, cx)),
        Some(Modal::Help { .. }) => out.push(keys(t, cx, true)),
        _ => {}
    }
    match &s.overlay {
        Some(Overlay::Palette { query, selected }) => out.push(palette(s, t, query, *selected, cx)),
        Some(Overlay::Keys) => out.push(keys(t, cx, false)),
        None => {}
    }
    out
}

fn palette(s: &Shell, t: &Colors, query: &str, selected: usize, cx: &Context<Shell>) -> AnyElement {
    let found = matching(s, query);
    let mut list = div().id("palette-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll().p(space::S2);
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
        let r = e.run.clone();
        list = list.child(
            div()
                .id(("pal", i))
                .flex()
                .items_center()
                .gap(space::S3)
                .h(px(34.))
                .px(space::S3)
                .rounded(radius::SM)
                .cursor_pointer()
                .when(on_it, |d| d.bg(t.accent).text_color(t.on_accent))
                .when(!on_it, |d| d.hover(|d| d.bg(t.surface_hover)))
                .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| run(s, r.clone(), cx)))
                .child(div().flex_1().min_w_0().truncate().text_size(text::BODY).child(e.title))
                .children(e.key.map(|k| keycap(t, k, on_it)))
                .when(on_it, |d| d.child(keycap(t, "⏎", true))),
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
                .child(list),
        )
        .into_any_element()
}

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
                        Command::Cancel(_) => "The run stops where it is. Nothing it found so far is posted.",
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
                            if done { "Close" } else { "Not now" },
                            Some("esc"),
                            Tone::Ghost,
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
            ("esc", "back"),
        ],
    ),
    (
        "Decide",
        &[
            ("P", "preview what would post"),
            ("⌘⏎", "post the preview to GitHub"),
            ("S", "start a review"),
            ("⇧R", "retry a failed review"),
            ("C", "cancel a review"),
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
            sheet(t, px(760.))
                .p(space::S6)
                .gap(space::S5)
                .child(div().text_size(text::TITLE).font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
                .child(groups),
        )
        .into_any_element()
}
