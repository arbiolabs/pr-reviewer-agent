//! The window: one title bar with the traffic lights, the sidebar, the live strip, the triage queue and the detail
//! pane. Every intent is a message to the TUI's reducer (`Msg::Action`, `Msg::Pick`); the desktop keeps only what
//! floats over the window, which list the home shows, and where the live log is held.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::app::dispatch;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{ConfirmState, Link, Modal, Model, Screen};
use pr_review_tui::net::NetCmd;
use pr_review_tui::protocol::{Command, Pr, PrId, Run};
use pr_review_tui::update::{Msg, can_confirm};
use pr_review_tui::view::Hit;
use tokio::sync::mpsc;

use crate::kit::{
    Gauge, OnClick, STEPS, Tone, button, count_badge, focusable, icon, keycap, lift, overline, progress, rail,
    severity_count, step_of, tip,
};
use crate::queue::{self, Card, RowState, Stage, phase_word};
use crate::script::Desk;
use crate::tokens::{self, Colors, layout, motion, radius, space, text};
use crate::{detail, diff, overlay};
use pr_review_tui::protocol::PublicationStatus;

gpui_kit::actions!(
    pr_review,
    [
        Quit,
        CloseWindow,
        Palette,
        Refresh,
        OpenGitHub,
        Setup,
        Shortcuts,
        Post,
        GoBack,
        NeedsYou,
        DoneList,
        AppearanceSystem,
        AppearanceDark,
        AppearanceLight
    ]
);

/// The ⌘ shortcuts, bound to the menu actions so the menu bar shows them.
pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-k", Palette, None),
        KeyBinding::new("cmd-r", Refresh, None),
        KeyBinding::new("cmd-shift-o", OpenGitHub, None),
        KeyBinding::new("cmd-,", Setup, None),
        KeyBinding::new("cmd-shift-/", Shortcuts, None),
        KeyBinding::new("cmd-enter", Post, None),
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-1", NeedsYou, None),
        KeyBinding::new("cmd-2", DoneList, None),
    ]
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("pr-review").items([MenuItem::action("Quit pr-review", Quit)]),
        Menu::new("File").items([MenuItem::action("Close Window", CloseWindow)]),
        Menu::new("View").items([
            MenuItem::action("Needs You", NeedsYou),
            MenuItem::action("Done", DoneList),
            MenuItem::action("Back", GoBack),
            MenuItem::separator(),
            MenuItem::action("Command Palette", Palette),
            MenuItem::action("Setup", Setup),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Appearance").items([
                MenuItem::action("Follow System", AppearanceSystem),
                MenuItem::action("Dark", AppearanceDark),
                MenuItem::action("Light", AppearanceLight),
            ])),
        ]),
        Menu::new("Review").items([
            MenuItem::action("Open on GitHub", OpenGitHub),
            MenuItem::action("Post the Preview", Post),
            MenuItem::separator(),
            MenuItem::action("Refresh from the Daemon", Refresh),
        ]),
        Menu::new("Help").items([MenuItem::action("Keyboard Shortcuts", Shortcuts)]),
    ]
}

pub enum Overlay {
    Palette {
        query: String,
        selected: usize,
    },
    Keys,
    /// The sheet that names what a post does before ⌘⏎ sends the reducer's `Confirm`.
    ConfirmPost,
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
    /// A snapshot: the window ignores the keyboard and the mouse, so what it shows is only the model.
    pub snapshot: bool,
    /// The live log held at an event, counted from 0, or `None` to follow the run.
    pub scrub: Option<usize>,
    /// The window's width, as the last frame laid it out: a page's action bar fits itself to it.
    pub width: Pixels,
    /// The action bar's More menu is open.
    pub more_open: bool,
}

/// A page the reader went into: a finding, the proof, the diff or setup. The live strip steps aside there.
pub fn deep(m: &Model) -> bool {
    front(m).is_some() || matches!(m.ui.screen, Screen::Diff | Screen::Settings)
}

/// The theme's tokens for the window's current appearance.
pub fn colors(cx: &App) -> Colors {
    use gpui_kit::component::ActiveTheme;
    if cx.theme().is_dark() { tokens::dark() } else { tokens::light() }
}

/// The page-sized panel in front: a finding or the publication, under any sheet.
pub fn front(m: &Model) -> Option<&Modal> {
    m.ui.modals.iter().rev().find(|x| matches!(x, Modal::Finding { .. } | Modal::Publication { .. }))
}

impl Shell {
    pub fn new(
        model: Model,
        cmds: mpsc::Sender<NetCmd>,
        still: bool,
        snapshot: bool,
        desk: &[Desk],
        cx: &mut Context<Self>,
    ) -> Self {
        let mut s = Shell {
            model,
            cmds,
            done_tab: false,
            overlay: None,
            focus: cx.focus_handle(),
            still,
            snapshot,
            scrub: None,
            width: px(1280.),
            more_open: false,
        };
        for d in desk {
            match d {
                Desk::Done(on) => s.done_tab = *on,
                Desk::Palette(q) => s.overlay = Some(Overlay::Palette { query: q.clone(), selected: 0 }),
                Desk::Keys => s.overlay = Some(Overlay::Keys),
                Desk::ConfirmPost => s.post(cx),
                Desk::Scrub(n) => s.scrub = Some(n.saturating_sub(1)),
            }
        }
        s
    }

    /// The home shows the done list: the reader asked for it, or a live queue is empty and the done list is what
    /// there is to show, under a line that says the reader is clear.
    pub fn shows_done(&self) -> bool {
        self.done_tab || (queue::stage(&self.model) == Stage::Ready && queue::cards(&self.model).is_empty())
    }

    /// A color for the sidebar and the title bar: a little see-through where the window blurs what is behind it,
    /// and solid in a snapshot, whose pixels must not depend on the desktop.
    pub fn glass(&self, c: Hsla) -> Hsla {
        if self.snapshot { c } else { c.alpha(0.9) }
    }

    /// The reader is on a deep page or the timeline: Back (⌘[) has somewhere to go.
    pub fn away(&self) -> bool {
        !matches!(self.model.ui.screen, Screen::Inbox | Screen::Review) || front(&self.model).is_some()
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
        self.scrub = None;
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
            Some(Card::Failed { .. }) => return self.direct(Mutation::Retry, cx),
            Some(Card::Start { .. }) => return self.direct(Mutation::Start, cx),
            Some(Card::Posting { .. }) => Some(Msg::Action(Action::Mutation(Mutation::Publish))),
            _ => None,
        };
        if let Some(msg) = msg {
            self.send(msg, cx);
        }
    }

    /// A start or a retry is cheap and reversible, so it goes without a sheet: the reducer opens its confirmation
    /// and the desktop answers it at once. The panel stays while the daemon answers, as a quiet status, and reads as
    /// a sheet only when the daemon refuses.
    pub fn direct(&mut self, mu: Mutation, cx: &mut Context<Self>) {
        self.act(Action::Mutation(mu), cx);
        let asking = matches!(
            self.model.top_modal(),
            Some(Modal::Confirm { command: Command::Start(_) | Command::Retry(_), state: ConfirmState::Asking, .. })
        );
        if asking {
            self.act(Action::Confirm, cx);
        }
    }

    /// Posting is the one irreversible act: it opens the sheet that names what posts. Only `confirm_post` sends.
    pub fn post(&mut self, cx: &mut Context<Self>) {
        if matches!(self.model.top_modal(), Some(Modal::Publication { .. })) && can_confirm(&self.model) {
            self.overlay = Some(Overlay::ConfirmPost);
            cx.notify();
        }
    }

    pub fn confirm_post(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.act(Action::Confirm, cx);
    }

    /// ⌘⏎: posts the sheet in front, opens it over a preview, or confirms a reducer panel.
    fn post_key(&mut self, cx: &mut Context<Self>) {
        match (&self.overlay, self.model.top_modal()) {
            (Some(Overlay::ConfirmPost), _) => self.confirm_post(cx),
            (None, Some(Modal::Publication { .. })) => self.post(cx),
            (None, Some(Modal::Confirm { .. })) => self.act(Action::Confirm, cx),
            _ => {}
        }
    }

    pub fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        self.overlay = match self.overlay {
            Some(Overlay::Palette { .. }) => None,
            _ => Some(Overlay::Palette { query: String::new(), selected: 0 }),
        };
        cx.notify();
    }

    /// The PR on GitHub, built from its repository and number; the desktop opens only github.com URLs.
    pub fn open_github(&mut self, cx: &mut Context<Self>) {
        if matches!(self.model.top_modal(), Some(Modal::Publication { .. })) {
            return self.act(Action::OpenUrl, cx);
        }
        let Some(p) = self.model.selected_pr() else { return };
        let repo = p.repository.as_str();
        if repo.split('/').count() == 2 && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) {
            cx.open_url(&format!("https://github.com/{repo}/pull/{}", p.number));
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
        if self.snapshot {
            return;
        }
        let k = &ev.keystroke;
        let (key, mods) = (k.key.as_str(), k.modifiers);
        // ⌘ shortcuts are the menu's actions, bound in `bindings`.
        if mods.platform || mods.control {
            return;
        }
        let typed = k.key_char.clone();
        if overlay::palette_key(self, key, typed.as_deref(), cx) {
            return;
        }
        match self.overlay {
            Some(Overlay::Keys) => {
                if key == "escape" || typed.as_deref() == Some("?") {
                    self.overlay = None;
                    cx.notify();
                }
                return;
            }
            Some(Overlay::ConfirmPost) => {
                // Cancel is the default: ⏎ and esc both close it. Only ⌘⏎ posts.
                if matches!(key, "escape" | "enter") {
                    self.overlay = None;
                    cx.notify();
                }
                return;
            }
            _ => {}
        }
        if key == "tab" {
            if mods.shift {
                window.focus_prev(cx)
            } else {
                window.focus_next(cx)
            }
            return cx.notify();
        }
        // A focused control takes ⏎ and space itself: GPUI clicks it.
        if matches!(key, "enter" | "space") && window.focused(cx).is_some_and(|f| f != self.focus) {
            return;
        }
        let m = &self.model;
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
            // A sheet's safe button is its default: ⏎ keeps a review running, and only ⌘⏎ or Y cancels it.
            (Some(Modal::Confirm { command: Command::Cancel(_), .. }), _, "enter", _) => Action::Back,
            (Some(Modal::Confirm { .. }), _, "enter", _) | (Some(Modal::Confirm { .. }), _, _, "y") => Action::Confirm,
            (Some(Modal::Confirm { .. }), _, _, "n") => Action::Back,
            (Some(Modal::Publication { .. }), _, _, "y") => return self.post(cx),
            (Some(Modal::Publication { .. }), _, _, "m") => Action::ToggleRaw,
            (Some(Modal::Publication { .. }), _, _, "d") => Action::OpenDiff,
            (Some(Modal::Finding { .. }), _, _, "n") => Action::NextFinding,
            (Some(Modal::Finding { .. }), _, _, "N") => Action::PrevFinding,
            (Some(Modal::Finding { .. }), _, _, "d") => Action::OpenDiff,
            (Some(Modal::Finding { .. }), _, _, "t") => Action::OpenTimeline,
            (Some(Modal::Help { .. }), _, _, _) => Action::Back,
            (_, _, "escape", _) => Action::Back,
            (None, Screen::Diff | Screen::Settings | Screen::Timeline, "down", _)
            | (None, Screen::Diff | Screen::Settings | Screen::Timeline, _, "j") => Action::Down,
            (None, Screen::Diff | Screen::Settings | Screen::Timeline, "up", _)
            | (None, Screen::Diff | Screen::Settings | Screen::Timeline, _, "k") => Action::Up,
            (None, Screen::Diff | Screen::Settings | Screen::Timeline, "enter", _) => Action::Open,
            (None, Screen::Diff, _, "n") => Action::NextFinding,
            (None, Screen::Diff, _, "N") => Action::PrevFinding,
            (None, Screen::Diff, _, ":") => Action::JumpTo,
            (None, Screen::Timeline, _, "f") => Action::Search,
            // T toggles the PR's two tabs, so leaving the timeline has a key of its own, apart from Back.
            (None, Screen::Timeline, _, "t") => Action::Back,
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
                    (_, "t") => Action::OpenTimeline,
                    (_, "s") => return self.direct(Mutation::Start, cx),
                    (_, "R") => return self.direct(Mutation::Retry, cx),
                    (_, "c") => Action::Mutation(Mutation::Cancel),
                    (_, "p") => Action::Mutation(Mutation::Publish),
                    _ => return,
                }
            }
            _ => return,
        };
        self.act(a, cx);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.width = window.viewport_size().width;
        let t = colors(cx);
        let m = &self.model;
        let stage = queue::stage(m);
        let finding_open = matches!(front(m), Some(Modal::Finding { .. }));
        let body = match m.ui.screen {
            Screen::Diff => diff::view(self, &t, cx).into_any_element(),
            Screen::Settings => crate::setup::setup(self, &t, cx).into_any_element(),
            _ => div()
                .flex()
                .flex_1()
                .min_h_0()
                .when(stage == Stage::Ready && !finding_open, |d| {
                    d.child(if self.shows_done() {
                        crate::cards::done_column(self, &t, cx)
                    } else {
                        crate::cards::queue_column(self, &t, cx)
                    })
                })
                .when(stage == Stage::Connecting, |d| d.child(crate::cards::skeleton_column(&t)))
                .child(detail::pane(self, &t, window, cx))
                .into_any_element(),
        };
        let snapshot = self.snapshot;
        div()
            .id("shell")
            .key_context("pr-review")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|s, _: &Palette, _, cx| s.toggle_palette(cx)))
            .on_action(cx.listener(|s, _: &Refresh, _, cx| s.act(Action::Refresh, cx)))
            .on_action(cx.listener(|s, _: &OpenGitHub, _, cx| s.open_github(cx)))
            .on_action(cx.listener(|s, _: &Setup, _, cx| s.act(Action::Goto(Screen::Settings), cx)))
            .on_action(cx.listener(|s, _: &Shortcuts, _, cx| {
                s.overlay = Some(Overlay::Keys);
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &Post, _, cx| s.post_key(cx)))
            .on_action(cx.listener(|s, _: &GoBack, _, cx| {
                if s.overlay.is_none() && s.away() {
                    s.act(Action::Back, cx)
                }
            }))
            .on_action(cx.listener(|s, _: &NeedsYou, _, cx| s.set_tab(false, cx)))
            .on_action(cx.listener(|s, _: &DoneList, _, cx| s.set_tab(true, cx)))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(
                cx.listener(|_, _: &AppearanceSystem, window, cx| Theme::sync_system_appearance(Some(window), cx)),
            )
            .on_action(
                cx.listener(|_, _: &AppearanceDark, window, cx| Theme::change(ThemeMode::Dark, Some(window), cx)),
            )
            .on_action(
                cx.listener(|_, _: &AppearanceLight, window, cx| Theme::change(ThemeMode::Light, Some(window), cx)),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(text::UI)
            .text_size(text::BODY)
            .text_color(t.text)
            .child(titlebar(self, &t, cx))
            .child(
                div().flex().flex_1().min_h_0().child(sidebar(self, &t, cx)).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .bg(t.bg)
                        .children(banner(m, &t))
                        .when(stage == Stage::Ready && !deep(m), |d| d.child(strip(self, &t, cx)))
                        .when(stage == Stage::Connecting, |d| d.child(strip_skeleton(&t)))
                        .child(body),
                ),
            )
            .children(overlay::layer(self, &t, cx))
            // A snapshot shows only the model: the real pointer never hovers or presses anything under this.
            .when(snapshot, |d| d.child(div().id("still").absolute().inset_0().occlude()))
    }
}

pub fn click(cx: &Context<Shell>, f: impl Fn(&mut Shell, &mut Context<Shell>) + 'static) -> OnClick {
    Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| f(s, cx)))
}

pub fn on(cx: &Context<Shell>, f: impl Fn(&mut Shell, &mut Context<Shell>) + 'static) -> Option<OnClick> {
    Some(click(cx, f))
}

/// The title bar: the traffic lights, the page title with its one-sentence state, and the ⌘K field.
fn titlebar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let pr = m.selected_pr();
    let number = pr.map_or(String::new(), |p| format!("#{}", p.number));
    let pr_title = pr.map_or(String::new(), |p| p.title.to_string());
    let (title, sub) = match (m.ui.screen, front(m)) {
        (_, Some(Modal::Finding { id, .. })) => {
            let all = pr.map(|p| m.domain.pr_findings(&p.id)).unwrap_or_default();
            let at = all.iter().position(|f| &f.id == id).map_or(0, |i| i + 1);
            (format!("{number} · finding {at} of {}", all.len()), pr_title)
        }
        (_, Some(Modal::Publication { run_id, .. })) => {
            let posted = m.domain.publications.get(run_id).is_some_and(|p| p.status == PublicationStatus::Published);
            (format!("{number} · {}", if posted { "posted" } else { "proof" }), pr_title)
        }
        (Screen::Diff, _) => (format!("{number} diff"), pr_title),
        (Screen::Timeline, _) => (format!("{number} timeline"), pr_title),
        (Screen::Settings, _) => ("Setup".into(), "What the reviewer needs to run".into()),
        _ if s.done_tab && !queue::cards(m).is_empty() => {
            ("Done".into(), format!("{} with nothing left for you.", queue::count(queue::done(m).len(), "PR")))
        }
        // The home's title says how much waits, not the list's name the sidebar already shows.
        _ => match (queue::stage(m), queue::cards(m).len()) {
            (Stage::Ready, 0) => {
                let rest = queue::summary(m);
                ("You're clear".into(), crate::kit::sentence(rest.trim_start_matches("You're clear · ")))
            }
            (Stage::Ready, n) => (format!("{} waiting", queue::count(n, "decision")), queue::summary(m)),
            _ => ("pr-review".into(), queue::summary(m)),
        },
    };
    let read_only = queue::read_only(m);
    let away = s.away();
    let search = div()
        .id("search-face")
        .flex()
        .items_center()
        .gap(space::S2)
        .w(px(220.))
        .h(px(30.))
        .px(space::S3)
        .rounded(radius::SM)
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .hover(move |d| d.bg(lift(t, t.surface)))
        .active(|d| d.bg(t.press))
        .child(icon(IconName::Search, px(14.), t.muted))
        .child(div().flex_1().text_size(text::SMALL).text_color(t.muted).child("Search or command"))
        .child(keycap(t, "⌘K", None, false));
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S3)
        .h(layout::TOOLBAR)
        .pl(layout::TRAFFIC_INSET)
        .pr(space::S4)
        .bg(s.glass(t.raised))
        .border_b_1()
        .border_color(t.border)
        .window_control_area(WindowControlArea::Drag)
        .when(away, |d| {
            d.child(button(t, "back", "Back", Some("⌘["), Tone::Ghost, on(cx, |s, cx| s.act(Action::Back, cx))))
        })
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(space::S3)
                .flex_1()
                .min_w_0()
                .child(div().flex_none().text_size(text::LEAD).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().min_w_0().truncate().text_size(text::SMALL).text_color(t.muted).child(sub)),
        )
        .when(read_only, |d| {
            d.child(tip(
                div()
                    .id("read-only")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(5.))
                    .h(px(24.))
                    .px(space::S2)
                    .rounded(radius::SM)
                    .bg(t.warn.alpha(t.tint))
                    .border_1()
                    .border_color(t.warn.alpha(t.tint * 2.))
                    .text_size(text::SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.warn)
                    .child(icon(IconName::Lock, px(12.), t.warn))
                    .child("Read-only"),
                crate::setup::read_only_sentence(m),
            ))
        })
        .child(tip(
            focusable("search", search, t, radius::SM, click(cx, |s, cx| s.toggle_palette(cx))),
            "Search PRs and findings, or run a command",
        ))
}

#[allow(clippy::too_many_arguments)]
fn nav_item(
    t: &Colors,
    id: &'static str,
    glyph: IconName,
    label: &str,
    count: usize,
    hue: Hsla,
    active: bool,
    dim: bool,
    f: OnClick,
) -> Stateful<Div> {
    // A list with nothing behind it (offline) is neither selected nor at full strength.
    let active = active && !dim;
    let fg = if dim {
        t.disabled
    } else if active {
        t.text
    } else {
        t.muted
    };
    let row = div()
        .id(SharedString::from(format!("{id}-face")))
        .when(dim, |d| d.opacity(0.55))
        .relative()
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(32.))
        .px(space::S2)
        .rounded(radius::SM)
        .when(active, |d| d.bg(t.selected).child(rail(t)))
        .hover(move |d| d.bg(if active { lift(t, t.selected) } else { t.hover }))
        .active(|d| d.bg(t.press))
        .child(icon(glyph, px(15.), if active && !dim { t.accent } else { fg }))
        .child(
            div()
                .flex_1()
                .text_size(text::BODY)
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(fg)
                .child(label.to_string()),
        )
        .when(count > 0, |d| d.child(count_badge(t, count, hue)));
    focusable(id, row, t, radius::SM, f)
}

/// The sidebar: the smart lists, then every PR grouped by repository with its state, then the connection.
fn sidebar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let stage = queue::stage(m);
    let cards = queue::cards(m);
    let done = queue::done(m);
    let home = matches!(m.ui.screen, Screen::Inbox | Screen::Review | Screen::Timeline) && front(m).is_none();
    // Nothing sits behind the lists while the window is offline, so they step back.
    let dim = stage == Stage::Offline;
    let selected = m.ui.selected_pr.clone();
    let mut list = div()
        .id("sidebar")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .gap(px(2.))
        .px(space::S3)
        .pt(space::S3)
        .pb(space::S4)
        .child(nav_item(
            t,
            "nav-queue",
            IconName::Inbox,
            "Needs you",
            cards.len(),
            t.accent,
            home && !s.done_tab && m.ui.screen != Screen::Timeline,
            dim,
            click(cx, |s, cx| s.set_tab(false, cx)),
        ))
        .child(nav_item(
            t,
            "nav-done",
            IconName::CircleCheck,
            "Done",
            done.len(),
            t.ok,
            home && s.done_tab,
            dim,
            click(cx, |s, cx| s.set_tab(true, cx)),
        ))
        .child(nav_item(
            t,
            "nav-setup",
            IconName::Settings,
            "Setup",
            queue::setup_count(m).blocking,
            t.warn,
            m.ui.screen == Screen::Settings,
            false,
            click(cx, |s, cx| s.act(Action::Goto(Screen::Settings), cx)),
        ));
    let groups = queue::rows(m);
    if stage == Stage::Ready && !groups.is_empty() {
        // One hierarchy: the repository is the group label, lowercase as GitHub writes it, in the sidebar's one
        // label style; the smart lists above need no heading.
        for (i, (repo, rows)) in groups.into_iter().enumerate() {
            list = list.child(
                div()
                    .px(space::S2)
                    .pt(if i == 0 { space::S5 } else { space::S3 })
                    .pb(space::S1)
                    .text_size(text::CAPTION)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child(repo),
            );
            for r in rows {
                let on = selected.as_ref() == Some(&r.pr.id) && m.ui.screen != Screen::Settings;
                list = list.child(pr_row(s, t, r.pr, r.state, on, cx));
            }
        }
    }
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(layout::SIDEBAR)
        .h_full()
        .bg(s.glass(t.sidebar))
        .border_r_1()
        .border_color(t.border)
        .child(list)
        .child(connection(m, t))
}

fn pr_row(s: &Shell, t: &Colors, pr: &Pr, state: RowState, on: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let id = pr.id.clone();
    let (glyph, hue): (AnyElement, Hsla) = match state {
        RowState::Running(_) => (live_dot(t, s.still, ("nav-dot", pr.number as usize)), t.running),
        RowState::Parked => (icon(IconName::Send, px(13.), t.accent).into_any_element(), t.accent),
        RowState::Failed => (icon(IconName::CircleX, px(13.), t.danger).into_any_element(), t.danger),
        RowState::Findings(sev, _) => (
            div().text_size(px(10.)).text_color(t.severity(sev)).child(tokens::severity_mark(sev).0).into_any_element(),
            t.severity(sev),
        ),
        RowState::Posted => (icon(IconName::CircleCheck, px(13.), t.ok).into_any_element(), t.ok),
        RowState::Clean => (icon(IconName::Check, px(13.), t.ok).into_any_element(), t.ok),
        RowState::Merged => (icon(IconName::GitMerge, px(13.), t.accent).into_any_element(), t.muted),
        RowState::Closed => (icon(IconName::CircleDashed, px(13.), t.faint).into_any_element(), t.faint),
        RowState::New => (icon(IconName::Circle, px(13.), t.faint).into_any_element(), t.faint),
    };
    let badge: Option<AnyElement> = match state {
        RowState::Findings(sev, n) => Some(severity_count(t, sev, n).into_any_element()),
        RowState::Running(p) => Some(progress(t, step_of(p), t.running, false, Gauge::Pip).into_any_element()),
        _ => None,
    };
    let row = div()
        .id(SharedString::from(format!("nav-pr-{}-face", pr.number)))
        .relative()
        .flex()
        .items_start()
        .gap(space::S2)
        .h(px(44.))
        .px(space::S2)
        .py(px(5.))
        .rounded(radius::SM)
        .when(on, |d| d.bg(t.selected).child(rail(t)))
        .hover(move |d| d.bg(if on { lift(t, t.selected) } else { t.hover }))
        .active(|d| d.bg(t.press))
        .child(div().w(px(14.)).h(px(18.)).flex().items_center().justify_center().flex_none().child(glyph))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                // The title takes the row's whole width on one line; the number, the state and the badge share
                // the line under it, so a title is cut only at its end.
                .child(
                    div()
                        .text_size(text::BODY)
                        .line_height(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .truncate()
                        .child(pr.title.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(16.))
                        .gap(px(6.))
                        .text_size(text::CAPTION)
                        .child(div().flex_none().text_color(t.muted).child(format!("#{}", pr.number)))
                        .child(div().flex_none().text_color(hue).child(state.word()))
                        .child(div().flex_1())
                        .children(badge),
                ),
        );
    focusable(
        SharedString::from(format!("nav-pr-{}", pr.number)),
        row,
        t,
        radius::SM,
        Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| {
            s.done_tab = queue::done(&s.model).iter().any(|(p, _)| p.id == id);
            s.select_pr(id.clone(), cx)
        })),
    )
}

fn connection(m: &Model, t: &Colors) -> Stateful<Div> {
    let (hue, word) = link_word(m, t);
    let endpoint = match m.diag.endpoint.split_once(':').map_or("", |(k, _)| k) {
        "unix" => "socket",
        other => other,
    }
    .to_string();
    let row = div()
        .id("connection")
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(40.))
        .px(space::S4)
        .border_t_1()
        .border_color(t.border)
        .child(div().size(px(7.)).rounded(radius::PILL).bg(hue))
        .child(div().text_size(text::SMALL).text_color(t.muted).child(word))
        .child(div().text_size(text::SMALL).text_color(t.faint).child(endpoint));
    tip(row, m.diag.endpoint.clone())
}

pub fn link_word(m: &Model, t: &Colors) -> (Hsla, String) {
    match &m.link {
        Link::Live => (t.ok, "Live".into()),
        Link::Connecting { attempt } => (t.warn, format!("Connecting · try {attempt}")),
        Link::Negotiating | Link::Syncing => (t.warn, "Syncing".into()),
        Link::Resyncing => (t.warn, "Catching up".into()),
        Link::Disconnected { .. } => (t.danger, "Offline".into()),
        Link::Stalled { .. } => (t.danger, "Stalled".into()),
        Link::Incompatible { .. } => (t.danger, "Incompatible daemon".into()),
        Link::Unusable { .. } => (t.danger, "Can't connect".into()),
    }
}

/// With a board on screen but the link down, a banner says what the board is: the last state the daemon sent.
fn banner(m: &Model, t: &Colors) -> Option<Div> {
    if m.link.accepts_events() || queue::stage(m) != Stage::Ready {
        return None;
    }
    let (hue, title, detail) = match &m.link {
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
        _ => (t.warn, "Reconnecting to the reviewer".into(), m.diag.endpoint.clone()),
    };
    Some(
        div()
            .flex()
            .items_start()
            .gap(space::S3)
            .min_h(px(36.))
            .px(space::S5)
            .py(space::S2)
            .bg(hue.alpha(0.10))
            .border_b_1()
            .border_color(hue.alpha(0.30))
            .child(div().pt(px(2.)).child(icon(IconName::TriangleAlert, px(14.), hue)))
            .child(
                div()
                    .flex_none()
                    .text_size(text::BODY)
                    .line_height(text::LINE_BODY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(title),
            )
            .child(
                div().flex_1().min_w_0().text_size(text::SMALL).line_height(text::LINE_BODY).text_color(t.muted).child(
                    format!("{} What you see is the last state the daemon sent.", crate::kit::sentence(&detail)),
                ),
            ),
    )
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
            div().text_size(text::SMALL).text_color(t.faint).child(if queue::read_only(m) {
                "No reviews running. This daemon is read-only, so none can start here."
            } else {
                "No reviews running. Start one from a card or ⌘K."
            }),
        );
    }
    row.min_w_0()
        .child(div().pr(space::S1).child(overline(t, "Live")))
        .children(running.into_iter().map(|(p, r)| pill(s, t, p, r, cx)))
}

/// The strip's place while the first snapshot loads, so nothing jumps when it lands.
fn strip_skeleton(t: &Colors) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(layout::STRIP)
        .px(space::S5)
        .border_b_1()
        .border_color(t.border)
        .bg(t.raised)
        .child(div().pr(space::S1).child(overline(t, "Live")))
        .child(div().w(px(260.)).h(px(32.)).rounded(radius::PILL).bg(t.border.alpha(0.7)))
}

fn pill(s: &Shell, t: &Colors, p: &Pr, r: &Run, cx: &Context<Shell>) -> Stateful<Div> {
    let m = &s.model;
    let selected = m.ui.selected_pr.as_ref() == Some(&p.id) && matches!(m.ui.screen, Screen::Inbox | Screen::Review);
    let id = p.id.clone();
    let base = if selected { t.running.alpha(0.14) } else { t.surface };
    let pill = div()
        .id(SharedString::from(format!("pill-{}-face", p.number)))
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(32.))
        .px(space::S3)
        .rounded(radius::PILL)
        .min_w_0()
        .bg(base)
        .border_1()
        .border_color(if selected { t.running.alpha(0.6) } else { t.border })
        .hover(move |d| d.bg(lift(t, base)))
        .active(|d| d.bg(t.press))
        .child(live_dot(t, s.still, ("pill-dot", p.number as usize)))
        .child(div().text_size(text::SMALL).font_weight(FontWeight::SEMIBOLD).child(format!("#{}", p.number)))
        .child(div().min_w_0().truncate().text_size(text::SMALL).text_color(t.muted).child(p.title.to_string()))
        .child(progress(t, step_of(r.phase), t.running, false, Gauge::Strip))
        .child(div().text_size(text::SMALL).text_color(t.running).child(phase_word(r.phase)))
        .child(crate::kit::tabular(div()).flex_none().text_size(text::CAPTION).text_color(t.faint).child(format!(
            "step {} of {}",
            (step_of(r.phase) + 1).min(STEPS.len()),
            STEPS.len()
        )));
    tip(
        focusable(
            SharedString::from(format!("pill-{}", p.number)),
            pill,
            t,
            radius::PILL,
            Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| s.select_pr(id.clone(), cx))),
        )
        .min_w_0(),
        format!("Watch #{} live", p.number),
    )
}
