//! Setup and the states around the board: the daemon's checks, mode and accounts; the overview when nothing is
//! selected; the offline page with the command that starts the daemon; and the skeleton while the link comes up.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::Link;
use pr_review_tui::protocol::{CheckStatus, DaemonMode, Finding};

use crate::detail::*;
use crate::kit::{OnClick, Tone, button, callout, chip, display, icon, icon_button, overline, summary};
use crate::queue;
use crate::tokens::{Colors, radius, space, text};
use crate::ui::{Shell, on};

fn check_name(id: &str) -> &str {
    match id {
        "git" => "git",
        "config" => "Configuration",
        "model" => "Model credentials",
        "github" => "GitHub token",
        "jev" => "Risk check (Jev)",
        other => other,
    }
}

/// Setup: the daemon's checks, its mode and the GitHub accounts, with the fix for each gap.
pub fn setup(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    div().flex().flex_1().min_h_0().child(page("setup", setup_body(s, t, cx), None, t, false))
}

/// Why this daemon can't act, as a sentence for the reader rather than the daemon's own words.
pub fn read_only_sentence(m: &pr_review_tui::model::Model) -> String {
    let missing = m.server.as_ref().is_some_and(|s| s.missing().next().is_some());
    if missing {
        "This daemon is read-only: it can look, but not start, retry, cancel or post. Fix the blocking checks, then \
         start it with --allow-commands."
            .into()
    } else {
        "This daemon is read-only: it can look, but not start, retry, cancel or post. Start it with --allow-commands."
            .into()
    }
}

/// A check in a first-run reader's words where the daemon's assume what it is: Jev says what it does for a
/// review and what its absence costs. Every other check keeps the daemon's detail and fix.
fn plain(c: &pr_review_tui::protocol::SetupCheck) -> (String, Option<String>) {
    match (c.id.as_str(), c.status) {
        ("jev", CheckStatus::Warn | CheckStatus::Missing) => (
            "Jev rates how risky each change is, so risky PRs get a deeper review. Without a key it is skipped, and \
             every PR gets the standard review."
                .into(),
            Some("Set `TYPESAFE_API_KEY` in the daemon's environment to turn it on.".into()),
        ),
        _ => (c.detail.to_string(), c.fix.as_ref().map(|f| f.to_string())),
    }
}

/// A fix from a setup check, with its `commands` as mono chips the reader can copy, and the prose between them.
fn fix_line(t: &Colors, fix: &str) -> Div {
    let mut row = div().flex().flex_wrap().items_center().gap_x(space::S2).gap_y(space::S1).mt(space::S1);
    for (i, part) in fix.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        if i % 2 == 0 {
            // Punctuation after a command sits against its chip, not a gap away from it.
            let text = part.trim();
            let glued = i > 0 && text.starts_with([',', '.', ';', ':']);
            row = row.child(
                div()
                    .max_w(relative(1.))
                    .when(glued, |d| d.ml(px(-6.)))
                    .text_size(text::SMALL)
                    .text_color(t.muted)
                    .child(text.to_string()),
            );
        } else {
            let copy = part.to_string();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pl(space::S2)
                    .rounded(radius::SM)
                    .bg(t.code_bg)
                    .border_1()
                    .border_color(t.border)
                    .child(div().font_family(text::MONO).text_size(px(11.)).text_color(t.text).child(part.to_string()))
                    .child(icon_button(
                        t,
                        SharedString::from(format!("copy-{i}-{}", part.len())),
                        IconName::Copy,
                        "Copy the command",
                        px(24.),
                        Some(Box::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }) as OnClick),
                    )),
            );
        }
    }
    row
}

/// The command that starts the daemon this window dials.
fn daemon_command(m: &pr_review_tui::model::Model) -> String {
    match m.diag.endpoint.strip_prefix("unix:") {
        Some(path) => format!("pr-review daemon --allow-commands --socket {path}"),
        None => "pr-review daemon --allow-commands".to_string(),
    }
}

/// No daemon answers: what to run, in a copyable block, and a retry.
pub fn offline(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let (title, why, retrying) = match &m.link {
        Link::Disconnected { reason, retry_in_ms } => {
            ("Reviewer offline", crate::kit::sentence(reason.as_str()), retry_in_ms.is_some())
        }
        Link::Stalled { reason } => ("Not connected", crate::kit::sentence(reason.as_str()), false),
        Link::Incompatible { message, .. } => ("This daemon speaks another protocol", message.to_string(), false),
        Link::Unusable { message } => ("This endpoint can't work", message.to_string(), false),
        _ => ("Not connected", "The reviewer has not answered yet.".to_string(), false),
    };
    let cmd = daemon_command(m);
    let copy = cmd.clone();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .justify_center()
        .gap(space::S6)
        .w_full()
        .max_w(px(640.))
        .pb(px(96.))
        .child(
            div()
                .size(px(48.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::PILL)
                .bg(t.danger.alpha(0.12))
                .child(icon(IconName::Unplug, px(22.), t.danger)),
        )
        .child(div().flex().flex_col().gap(space::S2).child(display(t, title)).child(summary(t, why)))
        .when(retrying, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .text_size(text::SMALL)
                    .text_color(t.muted)
                    .child(spinner(t, s.still, "retry-spin"))
                    .child("Retrying on its own. Start the daemon and this window connects."),
            )
        })
        .child(
            div().flex().flex_col().gap(space::S2).child(overline(t, "The daemon this window dials")).child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .pl(space::S3)
                    .pr(space::S1)
                    .py(space::S1)
                    .rounded(radius::SM)
                    .bg(t.code_bg)
                    .border_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(text::MONO)
                            .text_size(text::CODE)
                            .text_color(t.text)
                            .child(cmd),
                    )
                    .child(icon_button(
                        t,
                        "copy-cmd",
                        IconName::Copy,
                        "Copy the command",
                        px(32.),
                        Some(Box::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }) as OnClick),
                    )),
            ),
        )
        .child(
            div()
                .flex()
                .gap(space::S2)
                .child(button(
                    t,
                    "retry-now",
                    "Retry",
                    Some("⌘R"),
                    Tone::Primary,
                    on(cx, |s, cx| s.act(Action::Refresh, cx)),
                ))
                .child(button(
                    t,
                    "offline-setup",
                    "Setup",
                    Some("⌘,"),
                    Tone::Secondary,
                    on(cx, |s, cx| s.act(Action::Goto(pr_review_tui::model::Screen::Settings), cx)),
                )),
        )
}

/// A loader that turns while something is on its way; still under reduced motion or in a snapshot.
pub fn spinner(t: &Colors, still: bool, id: impl Into<ElementId>) -> AnyElement {
    let glyph = icon(IconName::LoaderCircle, px(14.), t.muted);
    if still {
        return glyph.into_any_element();
    }
    glyph
        .with_animation(id, Animation::new(std::time::Duration::from_millis(900)).repeat(), |g, v| {
            g.with_transformation(Transformation::rotate(percentage(v)))
        })
        .into_any_element()
}

/// Dialing, or waiting for the first snapshot: the PR page's shapes, greyed, where the page's parts will land.
pub fn connecting(t: &Colors, still: bool) -> Div {
    let bone = |w: Pixels, h: Pixels| div().w(w).h(h).rounded(px(4.)).bg(t.border.alpha(0.7));
    let card = |inner: Div| {
        inner.flex().flex_col().gap(space::S3).p(space::S4).rounded(radius::LG).border_1().border_color(t.border)
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S6)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .child(spinner(t, still, "connect-spin"))
                        .child(overline(t, "Connecting to the reviewer")),
                )
                .child(bone(px(420.), px(26.)))
                .child(bone(px(320.), px(14.))),
        )
        .child(
            div()
                .flex()
                .gap(space::S5)
                .pb(space::S2)
                .border_b_1()
                .border_color(t.border)
                .child(bone(px(64.), px(12.)))
                .child(bone(px(56.), px(12.))),
        )
        .child(card(div().child(bone(px(560.), px(5.))).child(bone(px(300.), px(12.)))))
        .child(card(div().child(bone(px(320.), px(14.))).child(bone(px(240.), px(11.)))))
        .child(card(div().child(bone(px(280.), px(14.))).child(bone(px(220.), px(11.)))))
}

/// With nothing selected: the board, what waits and what runs, and where the keyboard starts.
pub fn board(s: &Shell, t: &Colors) -> Div {
    let m = &s.model;
    let cards = queue::cards(m);
    let open: Vec<&Finding> = m.domain.findings.values().filter(|f| queue::open_finding(f)).collect();
    let tile = |n: usize, label: &str, hue: Hsla| {
        div()
            .flex()
            .flex_col()
            .gap(space::S1)
            .flex_1()
            .p(space::S4)
            .rounded(radius::LG)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .font_family(text::SERIF)
                    .text_size(text::DISPLAY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if n == 0 { t.faint } else { hue })
                    .child(n.to_string()),
            )
            .child(div().text_size(text::SMALL).text_color(t.muted).child(label.to_string()))
    };
    let sev = |x| open.iter().filter(|f| f.severity == x).count();
    use pr_review_tui::protocol::Severity as S;
    div()
        .flex()
        .flex_col()
        .gap(space::S6)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(overline(t, "The board"))
                .child(display(
                    t,
                    if cards.is_empty() {
                        "Nothing needs you".to_string()
                    } else {
                        format!("{} waiting on you", queue::count(cards.len(), "decision"))
                    },
                ))
                .child(summary(t, queue::summary(m))),
        )
        .child(
            div()
                .flex()
                .gap(space::S4)
                .child(tile(sev(S::Critical), "◆ critical open", t.critical))
                .child(tile(sev(S::High), "▲ high open", t.high))
                .child(tile(queue::running(m).len(), "reviews running", t.running))
                .child(tile(queue::done(m).len(), "done", t.ok)),
        )
        .child(
            div()
                .text_size(text::BODY)
                .text_color(t.muted)
                .child("J and K walk the queue. ⏎ does what the card says. ⌘K finds anything."),
        )
}

pub fn setup_body(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let card = |title: &str, content: Div| {
        div().flex().flex_col().gap(space::S3).child(overline(t, title)).child(
            div()
                .flex()
                .flex_col()
                .rounded(radius::LG)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .overflow_hidden()
                .child(content),
        )
    };
    let row =
        || div().flex().items_start().gap(space::S3).px(space::S4).py(px(10.)).border_b_1().border_color(t.border);
    let (hue, word) = crate::ui::link_word(m, t);
    let server = m.server.as_ref();
    let mode = match server.and_then(|s| s.mode) {
        Some(DaemonMode::Engine) => ("Engine: reviews run here", t.ok),
        Some(DaemonMode::ReadOnly) => ("Read-only: this app can look, not act", t.warn),
        _ => ("Unknown mode", t.muted),
    };
    let mut conn = div()
        .child(row().child(div().pt(px(5.)).child(div().size(px(8.)).rounded(radius::PILL).bg(hue))).child(
            div().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::SEMIBOLD).child(word)).child(
                div().font_family(text::MONO).text_size(px(11.)).text_color(t.muted).child(m.diag.endpoint.clone()),
            ),
        ))
        .child(
            row().child(icon(IconName::Cpu, px(15.), mode.1)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(mode.0))
                    .children(server.and_then(|s| s.read_only_reason.as_ref()).map(|r| {
                        div().text_size(text::SMALL).text_color(t.muted).child(crate::kit::sentence(r.as_str()))
                    })),
            ),
        );
    if let Some(sv) = server {
        conn = conn.child(
            row().child(icon(IconName::Info, px(15.), t.faint)).child(
                div()
                    .text_size(text::SMALL)
                    .text_color(t.muted)
                    .child(format!("pr-review {} · protocol v{}", sv.server_version, sv.protocol_version)),
            ),
        );
    }
    let checks = server.and_then(|s| s.checks.as_ref());
    let mut list = div();
    match checks {
        None => {
            list = list.child(row().child(
                div().text_size(text::BODY).text_color(t.muted).child("This daemon doesn't report setup checks."),
            ))
        }
        Some(cs) => {
            for c in cs {
                let (detail, fix) = plain(c);
                let (glyph, hue, word) = match c.status {
                    CheckStatus::Ok => (IconName::CircleCheck, t.ok, "Ready"),
                    CheckStatus::Warn => (IconName::TriangleAlert, t.warn, "Limited"),
                    CheckStatus::Missing => (IconName::CircleX, t.danger, "Missing"),
                    CheckStatus::Unknown => (IconName::CircleAlert, t.muted, "Unknown"),
                };
                list = list.child(
                    row().child(div().pt(px(1.)).child(icon(glyph, px(16.), hue))).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .gap(space::S2)
                                    .items_center()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(check_name(c.id.as_str()).to_string()),
                                    )
                                    .child(chip(t, word, hue)),
                            )
                            .child(div().text_size(text::SMALL).text_color(t.muted).child(detail.to_string()))
                            .children(fix.map(|f| fix_line(t, &f))),
                    ),
                );
            }
        }
    }
    let mut accounts = div();
    match &m.domain.accounts {
        None => {
            accounts =
                accounts.child(row().child(
                    div().text_size(text::BODY).text_color(t.muted).child("This daemon doesn't report accounts."),
                ))
        }
        Some(v) if v.is_empty() => {
            accounts = accounts.child(
                row().child(
                    div()
                        .text_size(text::BODY)
                        .text_color(t.muted)
                        .child("No GitHub accounts. Run gh auth login, or set GITHUB_TOKEN."),
                ),
            )
        }
        Some(v) => {
            for (i, a) in v.iter().enumerate() {
                let id = a.id.clone();
                let block = m.account_block(a);
                accounts = accounts.child(
                    row()
                        .items_center()
                        .child(icon(IconName::User, px(15.), t.muted))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .flex_1()
                                .child(
                                    div()
                                        .flex()
                                        .gap(space::S2)
                                        .items_center()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(format!("{} on {}", a.login, a.host)),
                                        )
                                        .when(a.selected, |d| d.child(chip(t, "Selected", t.accent)))
                                        .when(a.gh_active, |d| d.child(chip(t, "gh active", t.muted))),
                                )
                                .child(
                                    div()
                                        .text_size(text::SMALL)
                                        .text_color(t.faint)
                                        .child(a.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" · ")),
                                ),
                        )
                        .child(match block {
                            None => button(
                                t,
                                ("acct", i),
                                "Use for GitHub",
                                None,
                                Tone::Secondary,
                                Some(Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| {
                                    s.model.ui.selected_account = Some(id.clone());
                                    s.act(Action::Open, cx);
                                })) as OnClick),
                            ),
                            Some(why) => crate::kit::tip(
                                button(
                                    t,
                                    ("acct", i),
                                    if a.selected { "In use" } else { "Use for GitHub" },
                                    None,
                                    Tone::Secondary,
                                    None,
                                ),
                                why,
                            ),
                        }),
                );
            }
        }
    }
    let count = queue::setup_count(m);
    let missing = server.map_or(0, |s| s.missing().count());
    let lead = if missing > 0 {
        callout(
            t,
            t.danger,
            IconName::CircleX,
            format!("Reviews can't run: {}", count.words()),
            Some("Run the commands below in a terminal, then restart the daemon.".into()),
        )
    } else if server.and_then(|s| s.commands_off()).is_some() {
        callout(
            t,
            t.warn,
            IconName::TriangleAlert,
            "This daemon is read-only",
            Some(format!(
                "{} Every action button stays visible and says this when you hover it.",
                read_only_sentence(m)
            )),
        )
    } else if count.limited > 0 {
        callout(
            t,
            t.warn,
            IconName::TriangleAlert,
            format!("Reviews run with less: {}", count.words()),
            Some("The limited checks below say what each one costs.".into()),
        )
    } else {
        callout(t, t.ok, IconName::CircleCheck, "Everything the reviewer needs is in place", None)
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S5)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(overline(t, "Setup"))
                .child(display(t, "What the reviewer needs"))
                .child(summary(t, "The checks pr-review setup --check runs, as the daemon reports them.")),
        )
        .child(lead)
        .child(card("Checks", list))
        .child(card("Connection", conn))
        .child(card("GitHub accounts", accounts))
}
