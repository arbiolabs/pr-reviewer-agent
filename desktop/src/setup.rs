//! Setup and the board: the daemon's checks, mode and accounts, and the overview when nothing is selected.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::keymap::Action;
use pr_review_tui::protocol::{CheckStatus, DaemonMode, Finding};

use crate::detail::*;
use crate::kit::{OnClick, Tone, button, callout, chip, icon, overline};
use crate::queue;
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

fn check_name(id: &str) -> &str {
    match id {
        "git" => "git",
        "config" => "Configuration",
        "model" => "Model credentials",
        "github" => "GitHub token",
        "jev" => "Jev tier",
        other => other,
    }
}

/// Setup: the daemon's checks, its mode and the GitHub accounts, with the fix for each gap.
pub fn setup(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    div().flex().flex_1().min_h_0().child(page("setup", setup_body(s, t, cx), None, t))
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
            .rounded(radius::MD)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .text_size(text::DISPLAY)
                    .font_weight(FontWeight::BOLD)
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
        .gap(space::S5)
        .child(overline(t, "The board"))
        .child(div().text_size(text::DISPLAY).font_weight(FontWeight::BOLD).child(if !m.link.accepts_events() {
            "Waiting for the reviewer".to_string()
        } else if cards.is_empty() {
            "Nothing needs you".to_string()
        } else {
            format!("{} waiting on you", queue::count(cards.len(), "decision"))
        }))
        .child(
            div()
                .flex()
                .gap(space::S3)
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
                .rounded(radius::MD)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .overflow_hidden()
                .child(content),
        )
    };
    let row =
        || div().flex().items_start().gap(space::S3).px(space::S4).py(space::S3).border_b_1().border_color(t.border);
    let (hue, word) = crate::ui::link_word(m, t);
    let server = m.server.as_ref();
    let mode = match server.and_then(|s| s.mode) {
        Some(DaemonMode::Engine) => ("Engine: reviews run here", t.ok),
        Some(DaemonMode::ReadOnly) => ("Read-only: another pr-review owns the data", t.warn),
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
                    .children(
                        server
                            .and_then(|s| s.read_only_reason.as_ref())
                            .map(|r| div().text_size(text::SMALL).text_color(t.muted).child(r.to_string())),
                    ),
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
                            .child(div().text_size(text::SMALL).text_color(t.muted).child(c.detail.to_string()))
                            .children(c.fix.as_ref().map(|f| {
                                div()
                                    .mt(space::S1)
                                    .px(space::S2)
                                    .py(space::S1)
                                    .rounded(radius::XS)
                                    .bg(t.code_bg)
                                    .border_1()
                                    .border_color(t.border)
                                    .font_family(text::MONO)
                                    .text_size(px(11.))
                                    .text_color(t.text)
                                    .child(f.to_string())
                            })),
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
                        .child(button(
                            t,
                            ("acct", i),
                            if a.selected { "In use" } else { "Use for GitHub" },
                            None,
                            Tone::Secondary,
                            block.is_none().then(|| -> OnClick {
                                Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| {
                                    s.model.ui.selected_account = Some(id.clone());
                                    s.act(Action::Open, cx);
                                }))
                            }),
                        )),
                );
            }
        }
    }
    let missing = server.map_or(0, |s| s.missing().count());
    let lead = if missing > 0 {
        callout(
            t,
            t.danger,
            IconName::CircleX,
            format!("{} before reviews can run", queue::count(missing, "fix")),
            Some("Run the commands below in a terminal, then restart the daemon.".into()),
        )
    } else if server.and_then(|s| s.commands_off()).is_some() {
        callout(
            t,
            t.warn,
            IconName::TriangleAlert,
            "Reviews run, but this app can't start or post them",
            server.and_then(|s| s.commands_off()).map(String::from),
        )
    } else {
        callout(t, t.ok, IconName::CircleCheck, "Everything the reviewer needs is in place", None)
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
                .child(overline(t, "Setup"))
                .child(div().text_size(text::DISPLAY).font_weight(FontWeight::BOLD).child("What the reviewer needs"))
                .child(
                    div()
                        .text_size(text::BODY)
                        .text_color(t.muted)
                        .child("The same checks as pr-review setup --check, read from the daemon."),
                ),
        )
        .child(lead)
        .child(card("Checks", list))
        .child(card("Connection", conn))
        .child(card("GitHub accounts", accounts))
}
