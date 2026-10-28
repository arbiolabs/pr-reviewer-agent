//! Settings and the states around the board: the theme picker, the daemon's checks, mode and accounts; the overview
//! when nothing is selected; the offline page with the command that starts the daemon; and the skeleton while the
//! link comes up.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::model::Link;
use pr_review_tui::protocol::{CheckStatus, DaemonMode, Finding};

use crate::detail::*;
use crate::kit::{
    OnClick, Tone, button, callout, chip, count_badge, display, focusable, icon, icon_button, overline, segmented,
    summary,
};
use crate::queue;
use crate::reviewer::{Ask, Control, shell_quote};
use crate::tokens::{Choice, Colors, radius, space, text};
use crate::ui::{Shell, click, on};

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

/// Settings: a tab row, then the tab's cards. Review holds the model, Jev and the review message; Reviewer the setup
/// checks and the daemon; Accounts the GitHub accounts; Appearance the themes.
pub fn settings(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    div().flex().flex_1().min_h_0().child(tracked_page(
        "settings",
        setup_body(s, t, cx),
        None,
        t,
        false,
        Some(&s.settings_scroll),
    ))
}

/// A theme in miniature, in its own colors: its window, a card with two lines of text, its accent and the four
/// severities.
fn preview(p: Colors) -> Div {
    let bar = |w: f32, c: Hsla| div().h(px(3.)).w(relative(w)).rounded(px(2.)).bg(c);
    let dot = |c: Hsla| div().flex_none().size(px(6.)).rounded(radius::PILL).bg(c);
    div().flex_1().min_w_0().overflow_hidden().h_full().bg(p.bg).p(px(6.)).child(
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(5.))
            .rounded(px(4.))
            .bg(p.surface)
            .border_1()
            .border_color(p.border_strong)
            .child(bar(0.8, p.text))
            .child(bar(0.55, p.muted))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().flex_none().h(px(6.)).w(px(12.)).rounded(radius::PILL).bg(p.accent))
                    .children([p.critical, p.high, p.medium, p.low].map(dot)),
            ),
    )
}

/// The theme picker: a tile per choice, the one in use outlined in the accent with a check beside its name. A
/// click applies it at once.
fn themes(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let current = crate::ui::chosen(cx);
    let tile = |i: usize, choice: Choice| {
        let on = choice == current;
        let look = match choice {
            Choice::System => {
                div().flex().size_full().child(preview(crate::tokens::light())).child(preview(crate::tokens::dark()))
            }
            Choice::Fixed(th) => div().flex().size_full().child(preview((th.colors)())),
        };
        let inner = div()
            .flex()
            .flex_col()
            .gap(space::S2)
            .w(px(100.))
            .child(
                div()
                    .h(px(64.))
                    .rounded(radius::SM)
                    .overflow_hidden()
                    .border_2()
                    .border_color(if on { t.accent } else { t.border })
                    .child(look),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(text::SMALL)
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(choice.name())
                    .when(on, |d| d.child(icon(IconName::Check, px(12.), t.accent))),
            );
        focusable(("theme", i), inner, t, radius::SM, click(cx, move |s, cx| s.set_theme(choice, cx)))
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .p(space::S4)
        .child(div().flex().flex_wrap().gap(space::S3).children(Choice::all().enumerate().map(|(i, c)| tile(i, c))))
        .child(div().text_size(text::SMALL).text_color(t.muted).child(if s.snapshot {
            "System follows macOS between Light and Dark.".to_string()
        } else {
            format!(
                "System follows macOS between Light and Dark. Saved as [desktop] theme in {}.",
                s.reviewer.config_path.display()
            )
        }))
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
            "Jev rates each change's risk so risky PRs get a deeper review. Without a key, every PR gets the \
             standard one."
                .into(),
            Some(if cfg!(target_os = "macos") { "Save a TypeSafe key under Risk check (Jev) on the Review tab, or set `TYPESAFE_API_KEY` in the daemon's environment." } else { "Set `TYPESAFE_API_KEY` in the daemon's environment to turn it on." }.into()),
        ),
        _ => (c.detail.to_string(), c.fix.as_ref().map(|f| f.to_string())),
    }
}

/// A fix's parts: prose, and the commands and environment variables the reader can copy. A command is what the
/// daemon set in backticks; a variable is a bare `UPPER_CASE` word, which the daemon leaves unquoted.
fn fix_parts(fix: &str) -> Vec<(bool, String)> {
    let var = |w: &str| {
        let w = w.trim_end_matches([',', '.', ';', ':']);
        w.len() > 2 && w.contains('_') && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    let mut out: Vec<(bool, String)> = Vec::new();
    for (i, part) in fix.split('`').enumerate() {
        if i % 2 == 1 {
            out.push((true, part.to_string()));
            continue;
        }
        for word in part.split(' ').filter(|w| !w.is_empty()) {
            if var(word) {
                let name = word.trim_end_matches([',', '.', ';', ':']);
                out.push((true, name.to_string()));
                let tail = &word[name.len()..];
                if !tail.is_empty() {
                    out.push((false, tail.to_string()));
                }
            } else {
                match out.last_mut() {
                    Some((false, prose)) => {
                        prose.push(' ');
                        prose.push_str(word);
                    }
                    _ => out.push((false, word.to_string())),
                }
            }
        }
    }
    out
}

/// Every command the failing and limited checks name, one per line, for Copy all commands.
fn all_commands(m: &pr_review_tui::model::Model) -> Vec<String> {
    let checks = m.server.as_ref().and_then(|s| s.checks.as_ref());
    checks
        .into_iter()
        .flatten()
        .filter(|c| c.status != CheckStatus::Ok)
        .filter_map(|c| plain(c).1)
        .flat_map(|f| f.split('`').skip(1).step_by(2).map(str::to_string).collect::<Vec<_>>())
        .filter(|c| c.contains(' '))
        .collect()
}

/// Mono text the reader can copy, with room before its copy button.
pub fn copy_chip(t: &Colors, id: impl Into<ElementId>, text: &str) -> Div {
    let copy = text.to_string();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1)
        .pl(space::S2)
        .rounded(radius::SM)
        .bg(t.code_bg)
        .border_1()
        .border_color(t.border)
        .child(div().font_family(text::MONO).text_size(px(11.)).text_color(t.text).child(text.to_string()))
        .child(icon_button(
            t,
            id,
            IconName::Copy,
            "Copy",
            px(24.),
            Some(Box::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
            }) as OnClick),
        ))
}

/// A fix from a setup check: prose, with every command and variable as a chip the reader can copy.
fn fix_line(t: &Colors, check: &str, fix: &str) -> Div {
    let mut row = div().flex().flex_wrap().items_center().gap_x(space::S2).gap_y(space::S1).mt(space::S1);
    for (i, (code, part)) in fix_parts(fix).into_iter().enumerate() {
        row = row.child(if code {
            copy_chip(t, SharedString::from(format!("copy-{check}-{i}")), &part)
        } else {
            // Punctuation after a chip sits against it, not a gap away from it.
            let glued = i > 0 && part.starts_with([',', '.', ';', ':']);
            div()
                .max_w(relative(1.))
                .when(glued, |d| d.ml(px(-6.)))
                .text_size(text::SMALL)
                .text_color(t.muted)
                .child(part.trim().to_string())
        });
    }
    row
}

/// The command that starts the daemon this window dials, as a shell reads it.
fn daemon_command(m: &pr_review_tui::model::Model) -> String {
    match m.diag.endpoint.strip_prefix("unix:") {
        Some(path) => format!("pr-review daemon --allow-commands --socket {}", shell_quote(path)),
        None => "pr-review daemon --allow-commands".to_string(),
    }
}

/// A reviewer action as a button: live when it can run, else dashed with the reason in its tooltip.
fn ask_button(s: &Shell, t: &Colors, ask: Ask, key: Option<&str>, tone: Tone, cx: &Context<Shell>) -> AnyElement {
    let id = SharedString::from(format!("ask-{}", ask.title().to_lowercase().replace(' ', "-")));
    match s.reviewer.block(&s.model, ask) {
        None => button(t, id, ask.title(), key, tone, on(cx, move |s, cx| s.ask(ask, cx))).into_any_element(),
        Some(why) => {
            crate::kit::tip(button(t, id, ask.title(), key, tone, None), crate::kit::sentence(&why)).into_any_element()
        }
    }
}

/// No daemon answers: why, and Start reviewer; or how the start or the stop the reader asked for is going. The
/// command that starts it by hand waits under a disclosure.
pub fn offline(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let socket = s.reviewer.socket.as_ref().map_or(String::new(), |p| p.display().to_string());
    let (glyph, hue, title, why, retrying) = match (&s.reviewer.control, &m.link) {
        (Control::Starting { .. }, _) => (
            IconName::LoaderCircle,
            t.warn,
            "Starting the reviewer",
            format!(
                "Waiting for it to listen on {socket}. This window connects then, or gives up after {} s.",
                crate::reviewer::START_TIMEOUT_MS / 1000
            ),
            false,
        ),
        (Control::Stopping { .. }, _) => (
            IconName::LoaderCircle,
            t.warn,
            "Stopping the reviewer",
            "Running reviews pause and resume on the next start.".into(),
            false,
        ),
        (Control::Stopped, _) => (
            IconName::CircleStop,
            t.muted,
            "Reviewer stopped",
            "You stopped it. Reviews that were running resume when it starts again.".into(),
            false,
        ),
        (Control::StartFailed(why), _) => {
            (IconName::CircleX, t.danger, "The reviewer didn't start", why.clone(), false)
        }
        (_, Link::Disconnected { reason, retry_in_ms }) => (
            IconName::Unplug,
            t.danger,
            "Reviewer offline",
            crate::kit::sentence(reason.as_str()),
            retry_in_ms.is_some(),
        ),
        (_, Link::Stalled { reason }) => {
            (IconName::Unplug, t.danger, "Not connected", crate::kit::sentence(reason.as_str()), false)
        }
        (_, Link::Incompatible { message, .. }) => {
            (IconName::Unplug, t.danger, "This daemon speaks another protocol", message.to_string(), false)
        }
        (_, Link::Unusable { message }) => {
            (IconName::Unplug, t.danger, "This endpoint can't work", message.to_string(), false)
        }
        _ => (IconName::Unplug, t.danger, "Not connected", "The reviewer has not answered yet.".to_string(), false),
    };
    let starting = matches!(s.reviewer.control, Control::Starting { .. } | Control::Stopping { .. });
    let cmd = daemon_command(m);
    let copy = cmd.clone();
    let log = s.reviewer.launcher.as_ref().ok().and_then(crate::reviewer::Launcher::log);
    let failed = matches!(s.reviewer.control, Control::StartFailed(_));
    let badge =
        div().size(px(48.)).flex().items_center().justify_center().rounded(radius::PILL).bg(hue.alpha(0.12)).child(
            if starting { spinner(t, s.still, "offline-spin") } else { icon(glyph, px(22.), hue).into_any_element() },
        );
    div()
        .flex()
        .flex_col()
        .flex_1()
        .justify_center()
        .gap(space::S6)
        .w_full()
        .max_w(px(640.))
        .pb(px(96.))
        .child(badge)
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
                    .child("Retrying on its own. Start the reviewer and this window connects."),
            )
        })
        .child(
            div()
                .flex()
                .gap(space::S2)
                .child(ask_button(s, t, Ask::Start, Some("⌘R"), Tone::Primary, cx))
                .when_some(log.filter(|_| failed), |d, log| {
                    d.child(button(
                        t,
                        "open-log",
                        "Open the log",
                        None,
                        Tone::Secondary,
                        on(cx, move |_, _| {
                            let _ = std::process::Command::new("open").arg(&log).spawn();
                        }),
                    ))
                })
                .child(button(
                    t,
                    "offline-setup",
                    "Settings",
                    Some("⌘,"),
                    Tone::Secondary,
                    on(cx, |s, cx| s.act(Action::Goto(pr_review_tui::model::Screen::Settings), cx)),
                )),
        )
        .children(
            s.reviewer
                .launcher
                .as_ref()
                .err()
                .map(|e| div().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.warn).child(e.clone())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(div().flex().child(button(
                    t,
                    "show-command",
                    if s.show_command { "Hide the command" } else { "Run it yourself" },
                    None,
                    Tone::Ghost,
                    on(cx, |s, cx| {
                        s.show_command = !s.show_command;
                        cx.notify();
                    }),
                )))
                .when(s.show_command, |d| {
                    d.child(
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
                    )
                }),
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
    // Every row's mark sits in one slot, as wide as an icon and as tall as the row's first line, so the dot and the
    // icons share a column and each centres on the line beside it.
    let lead = |mark: AnyElement| {
        div().flex().flex_none().items_center().justify_center().w(px(16.)).h(text::LINE_BODY).child(mark)
    };
    let (hue, word) = crate::ui::link_word(s, t);
    let server = m.server.as_ref();
    let mode = match server.and_then(|s| s.mode) {
        Some(DaemonMode::Engine) => ("Engine: reviews run here", t.ok),
        Some(DaemonMode::ReadOnly) => ("Read-only: this app can look, not act", t.warn),
        _ => ("Unknown mode", t.muted),
    };
    let fact = |glyph: IconName, label: &str, value: String| {
        row().child(lead(icon(glyph, px(15.), t.faint).into_any_element())).child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(text::SMALL)
                        .line_height(text::LINE_BODY)
                        .text_color(t.muted)
                        .child(label.to_string()),
                )
                .child(div().font_family(text::MONO).text_size(px(11.)).text_color(t.text).truncate().child(value)),
        )
    };
    let runs_as = match &s.reviewer.launcher {
        Ok(l) => l.words(),
        Err(e) => e.clone(),
    };
    let log = match s.reviewer.launcher.as_ref().map(crate::reviewer::Launcher::log) {
        Ok(Some(log)) => log.display().to_string(),
        Ok(None) => "None: its LaunchAgent sets no StandardErrorPath".into(),
        Err(_) => crate::reviewer::spawn_log().display().to_string(),
    };
    let mut conn = div()
        .child(
            row()
                .child(lead(div().size(px(8.)).rounded(radius::PILL).bg(hue).into_any_element()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).line_height(text::LINE_BODY).child(word))
                        // The default socket sits under Application Support, long enough to push the buttons out of the
                        // card; the path gives way and its tooltip keeps it whole.
                        .child(crate::kit::tip(
                            div()
                                .id("reviewer-endpoint")
                                .font_family(text::MONO)
                                .text_size(px(11.))
                                .text_color(t.muted)
                                .truncate()
                                .child(m.diag.endpoint.clone()),
                            m.diag.endpoint.clone(),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(space::S2)
                        .child(ask_button(s, t, Ask::Start, None, Tone::Secondary, cx))
                        .child(ask_button(s, t, Ask::Restart, None, Tone::Secondary, cx))
                        .child(ask_button(s, t, Ask::Stop, None, Tone::Danger, cx)),
                ),
        )
        .child(
            row().child(lead(icon(IconName::Cpu, px(15.), mode.1).into_any_element())).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).line_height(text::LINE_BODY).child(mode.0))
                    .children(server.and_then(|s| s.read_only_reason.as_ref()).map(|r| {
                        div().text_size(text::SMALL).text_color(t.muted).child(crate::kit::sentence(r.as_str()))
                    })),
            ),
        )
        .child(fact(IconName::Server, "Runs as", runs_as))
        .child(fact(IconName::FileText, "Log", log));
    if let Some(sv) = server {
        conn = conn.child(
            row().child(lead(icon(IconName::Info, px(15.), t.faint).into_any_element())).child(
                div()
                    .text_size(text::SMALL)
                    .line_height(text::LINE_BODY)
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
                            .children(fix.map(|f| fix_line(t, c.id.as_str(), &f))),
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
    let body = div().flex().flex_col().gap(space::S5);
    let body = match s.settings_tab.tab {
        Tab::Review => body
            .child(crate::agent::section(s, t, cx))
            .child(crate::jev::section(s, t, cx))
            .child(crate::message::section(s, t, cx)),
        Tab::Reviewer => body
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .child(overline(t, "Setup checks"))
                    .child(display(t, "What the reviewer needs"))
                    .child(summary(t, "The checks pr-review setup --check runs, as the daemon reports them.")),
            )
            .child(lead)
            .children({
                let commands = all_commands(m).join("\n");
                (!commands.is_empty()).then(|| {
                    div().flex().child(button(
                        t,
                        "copy-all",
                        "Copy all commands",
                        None,
                        Tone::Secondary,
                        Some(Box::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            cx.write_to_clipboard(ClipboardItem::new_string(commands.clone()))
                        }) as OnClick),
                    ))
                })
            })
            .child(card("Checks", list))
            .child(card("Reviewer", conn)),
        Tab::Accounts => body.child(card("GitHub accounts", accounts)),
        Tab::Appearance => body.child(card("Appearance", themes(s, t, cx))),
    };
    div().flex().flex_col().gap(space::S6).child(tab_control(s, t, cx)).child(body)
}

/// The tab row at the top of Settings: each tab's name, its key and, when something in it wants attention, a count.
fn tab_control(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    segmented(
        t,
        Tab::ALL.map(|tab| {
            let badge = badge(&s.model, &s.jev.stored, tab);
            let words = match badge {
                Some(b) => format!("{}: {} to look at", tab.name(), b.n),
                None => tab.name().to_string(),
            };
            (
                format!("settings-{}", tab.id()),
                tab.name(),
                tab.key(),
                tab == s.settings_tab.tab,
                words,
                click(cx, move |s, cx| s.settings_on(tab, cx)),
                badge.map(|b| count_badge(t, b.n, if b.danger { t.danger } else { t.warn })),
            )
        }),
    )
}

/// Settings' tabs, in the order the control shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Review,
    Reviewer,
    Accounts,
    Appearance,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Review, Tab::Reviewer, Tab::Accounts, Tab::Appearance];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Review => "Review",
            Tab::Reviewer => "Reviewer",
            Tab::Accounts => "Accounts",
            Tab::Appearance => "Appearance",
        }
    }

    /// What a script and an element id call it.
    pub fn id(self) -> &'static str {
        match self {
            Tab::Review => "review",
            Tab::Reviewer => "reviewer",
            Tab::Accounts => "accounts",
            Tab::Appearance => "appearance",
        }
    }

    /// 1 to 4 on the Settings page. Nothing else there takes a digit, and a focused field keeps its keys.
    pub fn key(self) -> &'static str {
        match self {
            Tab::Review => "1",
            Tab::Reviewer => "2",
            Tab::Accounts => "3",
            Tab::Appearance => "4",
        }
    }
}

/// The tab a typed character picks: only while setup is on screen and no field (model search, Jev key, the
/// template editor) has the keyboard, since there a digit is text.
pub fn tab_for_key(shown: bool, typing: bool, ch: &str) -> Option<Tab> {
    Tab::ALL.into_iter().find(|t| shown && !typing && t.key() == ch)
}

/// The tab Settings shows. It lasts the session and is never saved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tabs {
    pub tab: Tab,
    /// Settings was on screen in the last frame.
    shown: bool,
}

impl Default for Tabs {
    fn default() -> Tabs {
        Tabs { tab: Tab::Review, shown: false }
    }
}

impl Tabs {
    /// Each frame, with whether Settings is on screen. Arriving there while the reviewer isn't ready puts its checks
    /// in front; otherwise the last tab stays. True when the tab changed.
    pub fn sync(&mut self, shown: bool, m: &pr_review_tui::model::Model) -> bool {
        let arrived = shown && !self.shown;
        self.shown = shown;
        let was = self.tab;
        if arrived && !ready(m) {
            self.tab = Tab::Reviewer;
        }
        self.tab != was
    }

    /// The control, a key, the palette or a script asked for `tab`, which stands even when Settings opens with it.
    pub fn open_on(&mut self, tab: Tab) {
        self.tab = tab;
        self.shown = true;
    }
}

/// Reviews can run: the window has the daemon's state, the link is up, and no check or read-only mode blocks them.
pub fn ready(m: &pr_review_tui::model::Model) -> bool {
    queue::stage(m) == queue::Stage::Ready && !link_down(m) && queue::setup_count(m).blocking == 0
}

/// The link is down and not on its way back up.
fn link_down(m: &pr_review_tui::model::Model) -> bool {
    matches!(
        m.link,
        Link::Disconnected { .. } | Link::Stalled { .. } | Link::Incompatible { .. } | Link::Unusable { .. }
    )
}

/// A tab's count of what wants attention in it, and whether any of it blocks reviews.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Badge {
    pub n: usize,
    pub danger: bool,
}

/// What a tab's badge says, so nothing that needs the reader hides behind a tab they aren't on. Reviewer counts the
/// blocking and limited checks, and a read-only or unreachable daemon; Review counts the model and Jev checks and a
/// Keychain that can't say whether a Jev key is saved.
pub fn badge(m: &pr_review_tui::model::Model, jev: &crate::jev::Stored, tab: Tab) -> Option<Badge> {
    match tab {
        Tab::Reviewer => {
            let c = queue::setup_count(m);
            let down = link_down(m);
            let n = c.blocking + c.limited;
            (n > 0 || down).then_some(Badge { n: n.max(1), danger: c.blocking > 0 || down })
        }
        Tab::Review => {
            let checks = m.server.as_ref().and_then(|s| s.checks.as_ref()).into_iter().flatten();
            let open: Vec<CheckStatus> = checks
                .filter(|c| matches!(c.id.as_str(), "model" | "jev"))
                .map(|c| c.status)
                .filter(|s| matches!(s, CheckStatus::Missing | CheckStatus::Warn))
                .collect();
            let n = open.len() + usize::from(matches!(jev, crate::jev::Stored::Unknown(_)));
            (n > 0).then_some(Badge { n, danger: open.contains(&CheckStatus::Missing) })
        }
        Tab::Accounts | Tab::Appearance => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Badge, Tab, Tabs, badge, tab_for_key};
    use crate::jev::Stored;
    use pr_review_tui::model::{Link, Model, Screen};

    /// The model a playback leaves at `ms`.
    fn played(file: &str, ms: u64) -> Model {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback").join(file);
        let mut m = Model::new("test", Default::default(), Screen::Review, None);
        crate::script::play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &[], ms).unwrap();
        m
    }

    #[test]
    fn settings_opens_on_review_and_keeps_the_last_tab_for_the_session() {
        let m = played("showcase.ndjson", 20_000);
        let mut tabs = Tabs::default();
        tabs.sync(true, &m);
        assert_eq!(tabs.tab, Tab::Review);
        tabs.open_on(Tab::Appearance);
        tabs.sync(false, &m);
        tabs.sync(true, &m);
        assert_eq!(tabs.tab, Tab::Appearance, "⌘, reopens on the tab the reader left");
    }

    #[test]
    fn a_reviewer_that_is_not_ready_opens_settings_on_its_checks() {
        let blocked = played("setup.ndjson", 1_000);
        let mut tabs = Tabs::default();
        tabs.sync(true, &blocked);
        assert_eq!(tabs.tab, Tab::Reviewer);
        tabs.open_on(Tab::Appearance);
        tabs.sync(true, &blocked);
        assert_eq!(tabs.tab, Tab::Appearance, "a pick while Settings is open stands");
        tabs.sync(false, &blocked);
        tabs.sync(true, &blocked);
        assert_eq!(tabs.tab, Tab::Reviewer, "coming back while it still blocks puts the checks in front again");

        let mut down = played("showcase.ndjson", 20_000);
        down.link = Link::Disconnected { reason: pr_review_tui::protocol::Text::new("gone"), retry_in_ms: Some(1_000) };
        let mut tabs = Tabs::default();
        tabs.sync(true, &down);
        assert_eq!(tabs.tab, Tab::Reviewer);
    }

    #[test]
    fn a_tab_with_something_to_fix_carries_a_badge_and_the_others_none() {
        let blocked = played("setup.ndjson", 1_000);
        let on = |m: &Model, jev: &Stored| Tab::ALL.map(|tab| badge(m, jev, tab));
        // Model credentials missing, the GitHub token and Jev limited.
        assert_eq!(
            on(&blocked, &Stored::NotSet),
            [Some(Badge { n: 2, danger: true }), Some(Badge { n: 3, danger: true }), None, None]
        );

        let fine = played("showcase.ndjson", 20_000);
        assert_eq!(on(&fine, &Stored::NotSet), [None; 4]);
        assert_eq!(
            badge(&fine, &Stored::Unknown("locked".into()), Tab::Review),
            Some(Badge { n: 1, danger: false }),
            "a Keychain that can't answer about the Jev key"
        );

        let mut read_only = played("showcase.ndjson", 20_000);
        read_only.server.as_mut().unwrap().mode = Some(pr_review_tui::protocol::DaemonMode::ReadOnly);
        assert_eq!(badge(&read_only, &Stored::NotSet, Tab::Reviewer), Some(Badge { n: 1, danger: true }));

        let mut down = played("showcase.ndjson", 20_000);
        down.link = Link::Disconnected { reason: pr_review_tui::protocol::Text::new("gone"), retry_in_ms: None };
        assert_eq!(badge(&down, &Stored::NotSet, Tab::Reviewer), Some(Badge { n: 1, danger: true }));
    }

    #[test]
    fn digits_pick_a_tab_only_while_setup_shows_and_no_field_has_the_keyboard() {
        assert_eq!(Tab::ALL.map(|t| tab_for_key(true, false, t.key())), Tab::ALL.map(Some));
        assert_eq!(tab_for_key(true, true, "3"), None, "a digit typed into the model search or the Jev key");
        assert_eq!(tab_for_key(false, false, "3"), None, "the queue and a PR page keep their keys");
        for taken in ["j", "k", "enter", "escape", "?", "/", "5"] {
            assert_eq!(tab_for_key(true, false, taken), None, "{taken}");
        }
    }

    #[test]
    fn the_daemon_command_quotes_the_socket_path_for_the_shell() {
        let mut m = pr_review_tui::model::Model::new(
            "unix:/Users/me/Library/Application Support/pr-review/reviewer.sock",
            Default::default(),
            pr_review_tui::model::Screen::Review,
            None,
        );
        assert_eq!(
            super::daemon_command(&m),
            "pr-review daemon --allow-commands --socket '/Users/me/Library/Application Support/pr-review/reviewer.sock'"
        );
        m.diag.endpoint = "unix:/tmp/it's/s.sock".into();
        assert_eq!(super::daemon_command(&m), r"pr-review daemon --allow-commands --socket '/tmp/it'\''s/s.sock'");
    }
}
