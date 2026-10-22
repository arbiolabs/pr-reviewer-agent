//! The window: sidebar, toolbar, the live strip, the triage queue and the detail pane. Every intent is a message to
//! the TUI's reducer (`Msg::Action`, `Msg::Pick`); the desktop keeps only what floats over the window and which list
//! the home shows.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::app::dispatch;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{Link, Modal, Model, Screen};
use pr_review_tui::net::NetCmd;
use pr_review_tui::protocol::{Phase, Pr, PrId, Run};
use pr_review_tui::update::Msg;
use pr_review_tui::view::Hit;
use tokio::sync::mpsc;

use crate::kit::{OnClick, Tone, button, icon, keycap, overline};
use crate::queue::{self, Card};
use crate::script::Desk;
use crate::tokens::{self, Colors, layout, motion, radius, space, text};
use crate::{detail, diff, overlay};

pub enum Overlay {
    Palette { query: String, selected: usize },
    Keys,
}

pub struct Shell {
    pub model: Model,
    pub cmds: mpsc::Sender<NetCmd>,
    /// The home shows the done list instead of the queue.
    pub done_tab: bool,
    pub overlay: Option<Overlay>,
    pub focus: FocusHandle,
    /// No motion: the system asks for less, or a snapshot needs a still frame.
    pub still: bool,
}

/// The theme's tokens for the window's current appearance.
pub fn colors(cx: &App) -> Colors {
    use gpui_kit::component::ActiveTheme;
    if cx.theme().is_dark() { tokens::dark() } else { tokens::light() }
}

impl Shell {
    pub fn new(model: Model, cmds: mpsc::Sender<NetCmd>, still: bool, desk: &[Desk], cx: &mut Context<Self>) -> Self {
        let mut s = Shell { model, cmds, done_tab: false, overlay: None, focus: cx.focus_handle(), still };
        for d in desk {
            match d {
                Desk::Done(on) => s.done_tab = *on,
                Desk::Palette(q) => s.overlay = Some(Overlay::Palette { query: q.clone(), selected: 0 }),
                Desk::Keys => s.overlay = Some(Overlay::Keys),
            }
        }
        s
    }

    pub fn send(&mut self, msg: Msg, cx: &mut Context<Self>) {
        if dispatch(&mut self.model, msg, &self.cmds) {
            cx.quit();
        }
        self.model.dirty = false;
        cx.notify();
    }

    pub fn act(&mut self, a: Action, cx: &mut Context<Self>) {
        self.send(Msg::Action(a), cx);
    }

    /// Closes the reducer's panels, so a pick lands on the page and not under a panel.
    fn close_modals(&mut self, cx: &mut Context<Self>) {
        for _ in 0..8 {
            match self.model.top_modal() {
                None => return,
                Some(Modal::Input { .. }) => self.act(Action::InputCancel, cx),
                Some(_) => self.act(Action::Back, cx),
            }
        }
    }

    fn home(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.model.ui.screen, Screen::Inbox | Screen::Review) {
            self.act(Action::Goto(Screen::Review), cx);
        }
    }

    pub fn select_pr(&mut self, pr: PrId, cx: &mut Context<Self>) {
        self.close_modals(cx);
        self.home(cx);
        self.send(Msg::Pick(Hit::Pr(pr)), cx);
    }

    /// Selecting a card shows its decision: a parked review opens the exact preview of what would post.
    pub fn select_card(&mut self, i: usize, cx: &mut Context<Self>) {
        let cards = queue::cards(&self.model);
        let Some(card) = cards.get(i) else { return };
        let (pr, approve) = match card {
            Card::Setup { .. } => return self.act(Action::Goto(Screen::Settings), cx),
            Card::Approve { pr, .. } => (pr.id.clone(), true),
            c => (c.pr().map(|p| p.id.clone()).unwrap_or_else(|| unreachable!("only setup has no PR")), false),
        };
        self.done_tab = false;
        self.select_pr(pr, cx);
        if approve {
            self.send(Msg::Pick(Hit::Ready), cx);
        }
    }

    /// The card's main action: what its button says.
    pub fn primary(&mut self, i: usize, cx: &mut Context<Self>) {
        let cards = queue::cards(&self.model);
        let Some(card) = cards.get(i) else { return };
        let selected = queue::card_of(&cards, self.model.ui.selected_pr.as_ref()) == Some(i);
        let setup = matches!(card, Card::Setup { .. });
        drop(cards);
        if !selected || setup {
            self.select_card(i, cx);
        }
        let m = &self.model;
        let msg = match queue::cards(m).get(i) {
            Some(Card::Approve { .. }) if !matches!(m.top_modal(), Some(Modal::Publication { .. })) => {
                Some(Msg::Pick(Hit::Ready))
            }
            Some(Card::Findings { findings, .. }) => {
                let id = m.ui.selected_finding.clone().or(findings.first().map(|f| f.id.clone()));
                if let Some(id) = id.clone().filter(|id| m.ui.selected_finding.as_ref() != Some(id)) {
                    self.send(Msg::Pick(Hit::Finding(id)), cx);
                }
                id.map(|id| Msg::Pick(Hit::Finding(id)))
            }
            Some(Card::Failed { .. }) => Some(Msg::Action(Action::Mutation(Mutation::Retry))),
            Some(Card::Start { .. }) => Some(Msg::Action(Action::Mutation(Mutation::Start))),
            Some(Card::Posting { .. }) => Some(Msg::Action(Action::Mutation(Mutation::Publish))),
            _ => None,
        };
        if let Some(msg) = msg {
            self.send(msg, cx);
        }
    }

    /// The PR on GitHub, built from its repository and number; the desktop opens only github.com URLs.
    pub fn open_github(&mut self) {
        let Some(p) = self.model.selected_pr() else { return };
        let repo = p.repository.as_str();
        if repo.split('/').count() == 2 && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) {
            let url = format!("https://github.com/{repo}/pull/{}", p.number);
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
    }

    fn move_card(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.done_tab {
            let done = queue::done(&self.model);
            let at = done.iter().position(|(p, _)| Some(&p.id) == self.model.ui.selected_pr.as_ref());
            let next = at.map_or(0, |i| (i as isize + delta).clamp(0, done.len() as isize - 1) as usize);
            if let Some((p, _)) = done.get(next) {
                let id = p.id.clone();
                self.select_pr(id, cx);
            }
            return;
        }
        let cards = queue::cards(&self.model);
        if cards.is_empty() {
            return;
        }
        let next = match queue::card_of(&cards, self.model.ui.selected_pr.as_ref()) {
            Some(i) => (i as isize + delta).clamp(0, cards.len() as isize - 1) as usize,
            None => 0,
        };
        drop(cards);
        self.select_card(next, cx);
    }

    pub fn set_tab(&mut self, done: bool, cx: &mut Context<Self>) {
        self.done_tab = done;
        self.close_modals(cx);
        self.home(cx);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let (key, cmd, ctrl, shift) = (k.key.as_str(), k.modifiers.platform, k.modifiers.control, k.modifiers.shift);
        let typed = k.key_char.clone().filter(|_| !cmd && !ctrl);
        if overlay::palette_key(self, key, cmd, typed.as_deref(), window, cx) {
            return;
        }
        if matches!(self.overlay, Some(Overlay::Keys)) {
            if matches!(key, "escape" | "?") || typed.as_deref() == Some("?") {
                self.overlay = None;
                cx.notify();
            }
            return;
        }
        let m = &self.model;
        if cmd {
            match key {
                "k" => self.overlay = Some(Overlay::Palette { query: String::new(), selected: 0 }),
                "q" | "w" => cx.quit(),
                "1" => self.set_tab(false, cx),
                "2" => self.set_tab(true, cx),
                "," => self.act(Action::Goto(Screen::Settings), cx),
                "r" => self.act(Action::Refresh, cx),
                "o" if matches!(m.top_modal(), Some(Modal::Publication { .. })) => self.act(Action::OpenUrl, cx),
                "o" => self.open_github(),
                "enter" => self.act(Action::Confirm, cx),
                _ => return,
            }
            cx.notify();
            return;
        }
        if let Some(Modal::Input { .. }) = m.top_modal() {
            let a = match key {
                "escape" => Action::InputCancel,
                "enter" => Action::InputSubmit,
                "backspace" => Action::InputBackspace,
                _ => match typed.as_deref().and_then(|s| s.chars().next()) {
                    Some(c) => Action::InputChar(c),
                    None => return,
                },
            };
            return self.act(a, cx);
        }
        let ch = typed.as_deref().unwrap_or(key);
        if ch == "?" {
            self.overlay = Some(Overlay::Keys);
            return cx.notify();
        }
        if ch == "/" {
            self.overlay = Some(Overlay::Palette { query: String::new(), selected: 0 });
            return cx.notify();
        }
        let modal = m.top_modal().cloned();
        let screen = m.ui.screen;
        let a = match (modal.as_ref(), screen, key, ch) {
            (Some(Modal::Confirm { .. }), _, "enter", _) | (Some(Modal::Confirm { .. }), _, _, "y") => Action::Confirm,
            (Some(Modal::Confirm { .. }), _, _, "n") => Action::Back,
            (Some(Modal::Publication { .. }), _, _, "y") => Action::Confirm,
            (Some(Modal::Publication { .. }), _, _, "m") => Action::ToggleRaw,
            (Some(Modal::Publication { .. }), _, _, "d") => Action::OpenDiff,
            (Some(Modal::Finding { .. }), _, _, "n") => Action::NextFinding,
            (Some(Modal::Finding { .. }), _, _, "N") => Action::PrevFinding,
            (Some(Modal::Finding { .. }), _, _, "d") => Action::OpenDiff,
            (Some(Modal::Help { .. }), _, _, _) => Action::Back,
            (_, _, "escape", _) => Action::Back,
            (None, Screen::Diff | Screen::Settings, "down", _) | (None, Screen::Diff | Screen::Settings, _, "j") => {
                Action::Down
            }
            (None, Screen::Diff | Screen::Settings, "up", _) | (None, Screen::Diff | Screen::Settings, _, "k") => {
                Action::Up
            }
            (None, Screen::Diff | Screen::Settings, "enter", _) => Action::Open,
            (None, Screen::Diff, _, "n") => Action::NextFinding,
            (None, Screen::Diff, _, "N") => Action::PrevFinding,
            (None, Screen::Diff, _, ":") => Action::JumpTo,
            (Some(Modal::Finding { .. } | Modal::Publication { .. }) | None, Screen::Inbox | Screen::Review, ..) => {
                match (key, ch) {
                    ("down", _) | (_, "j") => return self.move_card(1, cx),
                    ("up", _) | (_, "k") => return self.move_card(-1, cx),
                    ("enter", _) if modal.is_none() => {
                        let cards = queue::cards(m);
                        if let Some(i) = queue::card_of(&cards, m.ui.selected_pr.as_ref()) {
                            drop(cards);
                            return self.primary(i, cx);
                        }
                        return;
                    }
                    (_, "n") => Action::NextFinding,
                    (_, "N") => Action::PrevFinding,
                    (_, "d") => Action::OpenDiff,
                    (_, "s") => Action::Mutation(Mutation::Start),
                    (_, "R") => Action::Mutation(Mutation::Retry),
                    (_, "c") => Action::Mutation(Mutation::Cancel),
                    (_, "p") => Action::Mutation(Mutation::Publish),
                    _ => return,
                }
            }
            _ => return,
        };
        let _ = shift;
        self.act(a, cx);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = colors(cx);
        let m = &self.model;
        let body = match m.ui.screen {
            Screen::Diff => diff::view(self, &t, cx).into_any_element(),
            Screen::Settings => crate::setup::setup(self, &t, cx).into_any_element(),
            _ => div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(if self.done_tab {
                    crate::cards::done_column(self, &t, cx)
                } else {
                    crate::cards::queue_column(self, &t, cx)
                })
                .child(detail::pane(self, &t, window, cx))
                .into_any_element(),
        };
        div()
            .id("shell")
            .key_context("pr-review")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .size_full()
            .flex()
            .bg(t.bg)
            .font_family(text::UI)
            .text_size(text::BODY)
            .text_color(t.text)
            .child(sidebar(self, &t, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(toolbar(self, &t, cx))
                    .children(banner(m, &t))
                    .child(strip(self, &t, cx))
                    .child(body),
            )
            .children(overlay::layer(self, &t, cx))
    }
}

pub fn click(cx: &Context<Shell>, f: impl Fn(&mut Shell, &mut Context<Shell>) + 'static) -> OnClick {
    Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| f(s, cx)))
}

pub fn on(cx: &Context<Shell>, f: impl Fn(&mut Shell, &mut Context<Shell>) + 'static) -> Option<OnClick> {
    Some(click(cx, f))
}

fn sidebar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let cards = queue::cards(m);
    let running = queue::running(m);
    let done = queue::done(m);
    let home = matches!(m.ui.screen, Screen::Inbox | Screen::Review);
    let setup_issues = m.server.as_ref().map_or(0, |s| s.missing().count() + usize::from(s.commands_off().is_some()));
    let item = |id: &'static str, glyph: IconName, label: &str, count: usize, hue: Hsla, active: bool, f: OnClick| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(30.))
            .px(space::S2)
            .rounded(radius::SM)
            .cursor_pointer()
            .when(active, |d| d.bg(t.selected))
            .hover(|d| d.bg(t.surface_hover))
            .active(|d| d.bg(t.surface_press))
            .on_click(f)
            .child(icon(glyph, px(15.), if active { t.accent } else { t.muted }))
            .child(
                div()
                    .flex_1()
                    .text_size(text::BODY)
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(if active { t.text } else { t.muted })
                    .child(label.to_string()),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .px(px(6.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(radius::PILL)
                        .bg(hue.alpha(0.16))
                        .text_size(text::CAPTION)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hue)
                        .child(count.to_string()),
                )
            })
    };
    let needs = cards.len();
    let selected = m.ui.selected_pr.clone();
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(layout::SIDEBAR)
        .h_full()
        .bg(t.sidebar)
        .border_r_1()
        .border_color(t.border)
        .child(div().h(layout::TOOLBAR).flex_none().on_mouse_down(MouseButton::Left, |_, w, _| w.start_window_move()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(space::S3)
                .child(item(
                    "nav-queue",
                    IconName::Inbox,
                    "Needs you",
                    needs,
                    t.accent,
                    home && !s.done_tab,
                    click(cx, |s, cx| s.set_tab(false, cx)),
                ))
                .child(item(
                    "nav-done",
                    IconName::CircleCheck,
                    "Done",
                    done.len(),
                    t.ok,
                    home && s.done_tab,
                    click(cx, |s, cx| s.set_tab(true, cx)),
                ))
                .child(div().h(space::S4))
                .child(div().px(space::S2).pb(space::S1).child(overline(t, "Running")))
                .when(running.is_empty(), |d| {
                    d.child(
                        div()
                            .px(space::S2)
                            .h(px(26.))
                            .text_size(text::SMALL)
                            .text_color(t.faint)
                            .child("Nothing running"),
                    )
                })
                .children(running.iter().map(|(p, r)| {
                    let id = p.id.clone();
                    let active = selected.as_ref() == Some(&p.id);
                    div()
                        .id(SharedString::from(format!("nav-run-{}", p.number)))
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .h(px(30.))
                        .px(space::S2)
                        .rounded(radius::SM)
                        .cursor_pointer()
                        .when(active && home, |d| d.bg(t.selected))
                        .hover(|d| d.bg(t.surface_hover))
                        .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.select_pr(id.clone(), cx)))
                        .child(live_dot(t, s.still, ("nav-dot", p.number as usize)))
                        .child(
                            div()
                                .text_size(text::SMALL)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(format!("#{}", p.number)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(text::SMALL)
                                .text_color(t.muted)
                                .child(phase_word(r.phase)),
                        )
                })),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(space::S3)
                .pb(space::S3)
                .child(item(
                    "nav-setup",
                    IconName::Settings,
                    "Setup",
                    setup_issues,
                    t.warn,
                    m.ui.screen == Screen::Settings,
                    click(cx, |s, cx| s.act(Action::Goto(Screen::Settings), cx)),
                ))
                .child(connection(m, t)),
        )
}

fn connection(m: &Model, t: &Colors) -> Div {
    let (hue, word) = link_word(m, t);
    let endpoint = match m.diag.endpoint.split_once(':').map_or("", |(k, _)| k) {
        "unix" => "socket",
        other => other,
    }
    .to_string();
    div()
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(28.))
        .px(space::S2)
        .child(div().size(px(7.)).rounded(radius::PILL).bg(hue))
        .child(div().text_size(text::SMALL).text_color(t.muted).child(word))
        .child(div().text_size(text::SMALL).text_color(t.faint).child(endpoint))
}

pub fn link_word(m: &Model, t: &Colors) -> (Hsla, String) {
    match &m.link {
        Link::Live => (t.ok, "Live".into()),
        Link::Connecting { attempt } => (t.warn, format!("Connecting · try {attempt}")),
        Link::Negotiating | Link::Syncing => (t.warn, "Syncing".into()),
        Link::Resyncing => (t.warn, "Catching up".into()),
        Link::Disconnected { .. } => (t.danger, "Disconnected".into()),
        Link::Stalled { .. } => (t.danger, "Stalled".into()),
        Link::Incompatible { .. } => (t.danger, "Incompatible daemon".into()),
        Link::Unusable { .. } => (t.danger, "Can't connect".into()),
    }
}

/// A connection that isn't live says so above everything, with what the app shows meanwhile.
fn banner(m: &Model, t: &Colors) -> Option<Div> {
    let (hue, title, detail) = match &m.link {
        Link::Live => return None,
        Link::Disconnected { reason, retry_in_ms } => (
            t.danger,
            match retry_in_ms {
                Some(ms) => format!("Disconnected. Retrying in {}s", ms.div_ceil(1000)),
                None => "Disconnected".to_string(),
            },
            reason.to_string(),
        ),
        Link::Stalled { reason } => (t.danger, "The snapshot never arrived".into(), format!("{reason} (⌘R)")),
        Link::Incompatible { message, .. } => {
            (t.danger, "This daemon speaks another protocol".into(), message.to_string())
        }
        Link::Unusable { message } => (t.danger, "This endpoint can't work".into(), message.to_string()),
        _ => (t.warn, "Connecting to the reviewer".into(), m.diag.endpoint.clone()),
    };
    let stale = if m.domain.prs.is_empty() { "" } else { " What you see is the last state the daemon sent." };
    Some(
        div()
            .flex()
            .items_center()
            .gap(space::S3)
            .h(px(36.))
            .px(space::S5)
            .bg(hue.alpha(0.10))
            .border_b_1()
            .border_color(hue.alpha(0.30))
            .child(icon(IconName::TriangleAlert, px(14.), hue))
            .child(div().text_size(text::BODY).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(text::SMALL)
                    .text_color(t.muted)
                    .child(format!("{detail}.{stale}")),
            ),
    )
}

fn toolbar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let cards = queue::cards(m);
    let running = queue::running(m).len();
    let (title, sub) = match m.ui.screen {
        Screen::Diff => {
            let pr = m.selected_pr();
            (
                pr.map_or("Diff".into(), |p| format!("#{} diff", p.number)),
                pr.map_or(String::new(), |p| p.title.to_string()),
            )
        }
        Screen::Settings => ("Setup".into(), "What the reviewer needs to run".into()),
        _ if s.done_tab => {
            ("Done".into(), format!("{} with nothing left for you", queue::count(queue::done(m).len(), "PR")))
        }
        _ => (
            "Needs you".into(),
            match (cards.len(), running) {
                (0, 0) => "You're clear".into(),
                (n, 0) => queue::count(n, "decision"),
                (n, r) => format!("{} · {r} running", queue::count(n, "decision")),
            },
        ),
    };
    let away = !matches!(m.ui.screen, Screen::Inbox | Screen::Review);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S3)
        .h(layout::TOOLBAR)
        .px(space::S5)
        .border_b_1()
        .border_color(t.border)
        .on_mouse_down(MouseButton::Left, |_, w, _| w.start_window_move())
        .when(away, |d| {
            d.child(button(t, "back", "Queue", Some("esc"), Tone::Ghost, on(cx, |s, cx| s.act(Action::Back, cx))))
        })
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(space::S2)
                .flex_1()
                .min_w_0()
                .child(div().flex_none().text_size(text::LEAD).font_weight(FontWeight::BOLD).child(title))
                .child(div().min_w_0().truncate().text_size(text::SMALL).text_color(t.muted).child(sub)),
        )
        .child(
            div()
                .id("search")
                .flex()
                .items_center()
                .gap(space::S2)
                .w(px(248.))
                .h(px(28.))
                .px(space::S2)
                .rounded(radius::SM)
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .cursor_pointer()
                .hover(|d| d.border_color(t.border_strong))
                .active(|d| d.bg(t.surface_press))
                .on_click(cx.listener(|s, _: &ClickEvent, _, cx| {
                    s.overlay = Some(Overlay::Palette { query: String::new(), selected: 0 });
                    cx.notify();
                }))
                .child(icon(IconName::Search, px(13.), t.faint))
                .child(div().flex_1().text_size(text::BODY).text_color(t.faint).child("Search or run a command"))
                .child(keycap(t, "⌘K", false)),
        )
}

pub fn phase_word(p: Phase) -> &'static str {
    match p {
        Phase::CollectContext => "Collecting context",
        Phase::Triage => "Triage",
        Phase::Analyze => "Analyzing",
        Phase::Verify => "Verifying",
        Phase::PreparePublication => "Preparing",
        Phase::AwaitingApproval => "Awaiting approval",
        Phase::Publish => "Posting",
        Phase::Reconcile => "Reconciling",
        Phase::Completed => "Completed",
        Phase::Failed => "Failed",
        Phase::Superseded => "Superseded",
        Phase::Unknown => "Unknown",
    }
}

/// The phases a run walks, as the stepper and the pills count them.
pub const STEPS: [(&str, Phase); 6] = [
    ("Collect", Phase::CollectContext),
    ("Triage", Phase::Triage),
    ("Analyze", Phase::Analyze),
    ("Verify", Phase::Verify),
    ("Prepare", Phase::PreparePublication),
    ("Post", Phase::Publish),
];

pub fn step_of(p: Phase) -> usize {
    match p {
        Phase::CollectContext => 0,
        Phase::Triage => 1,
        Phase::Analyze => 2,
        Phase::Verify => 3,
        Phase::PreparePublication => 4,
        Phase::AwaitingApproval | Phase::Publish | Phase::Reconcile => 5,
        _ => 6,
    }
}

/// The breathing dot of a review at work; still under reduced motion.
pub fn live_dot(t: &Colors, still: bool, id: impl Into<ElementId>) -> AnyElement {
    let dot = div().size(px(7.)).flex_none().rounded(radius::PILL).bg(t.running);
    if still {
        return dot.into_any_element();
    }
    dot.with_animation(id, Animation::new(motion::PULSE).repeat().with_easing(pulsating_between(0.35, 1.0)), |d, v| {
        d.opacity(v)
    })
    .into_any_element()
}

/// The live strip: every review at work, its phase and progress, one click from its activity.
fn strip(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let running = queue::running(m);
    let row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(layout::STRIP)
        .px(space::S5)
        .border_b_1()
        .border_color(t.border)
        .bg(t.raised);
    if running.is_empty() {
        return row.child(icon(IconName::CircleCheck, px(14.), t.faint)).child(
            div().text_size(text::SMALL).text_color(t.faint).child("No reviews running. Start one from a card or ⌘K."),
        );
    }
    row.child(div().pr(space::S1).child(overline(t, "Live")))
        .children(running.into_iter().map(|(p, r)| pill(s, t, p, r, cx)))
}

fn pill(s: &Shell, t: &Colors, p: &Pr, r: &Run, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let selected = m.ui.selected_pr.as_ref() == Some(&p.id) && matches!(m.ui.screen, Screen::Inbox | Screen::Review);
    let at = step_of(r.phase);
    let id = p.id.clone();
    let tools = m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&p.id)).count();
    div()
        .id(SharedString::from(format!("pill-{}", p.number)))
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(28.))
        .pl(space::S3)
        .pr(space::S3)
        .rounded(radius::PILL)
        .bg(if selected { t.running.alpha(0.14) } else { t.surface })
        .border_1()
        .border_color(if selected { t.running.alpha(0.6) } else { t.border })
        .cursor_pointer()
        .hover(|d| d.border_color(t.running.alpha(0.6)))
        .active(|d| d.bg(t.surface_press))
        .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| s.select_pr(id.clone(), cx)))
        .child(live_dot(t, s.still, ("pill-dot", p.number as usize)))
        .child(div().text_size(text::SMALL).font_weight(FontWeight::SEMIBOLD).child(format!("#{}", p.number)))
        .child(div().max_w(px(180.)).truncate().text_size(text::SMALL).text_color(t.muted).child(p.title.to_string()))
        .child(div().flex().gap(px(2.)).children((0..STEPS.len()).map(|i| {
            div().w(px(10.)).h(px(4.)).rounded(px(2.)).bg(if i < at {
                t.running
            } else if i == at {
                t.running.alpha(0.55)
            } else {
                t.border_strong
            })
        })))
        .child(div().text_size(text::SMALL).text_color(t.running).child(phase_word(r.phase)))
        .when(tools > 0, |d| {
            d.child(div().text_size(text::CAPTION).text_color(t.faint).child(format!("{tools} steps")))
        })
}
