//! The window: one title bar with the traffic lights, the sidebar, the live strip, the triage queue and the detail
//! pane. Every intent is a message to the TUI's reducer (`Msg::Action`, `Msg::Pick`); the desktop keeps only what
//! floats over the window, which list the home shows, and where the live log is held.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::app::dispatch;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{AddState, ConfirmState, Link, Modal, Model, PrRef, Screen};
use pr_review_tui::net::NetCmd;
use pr_review_tui::protocol::{Command, Pr, PrId, Run, UsageBy};
use pr_review_tui::update::{Msg, can_add, can_confirm};
use pr_review_tui::view::Hit;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::kit::{
    Gauge, OnClick, STEPS, Tone, button, count_badge, focusable, icon, keycap, lift, overline, progress, rail,
    severity_count, step_of, tip,
};
use crate::queue::{self, Card, RowState, Stage, phase_word};
use crate::script::Desk;
use crate::tokens::{self, Colors, layout, motion, radius, space, text};
use crate::{detail, diff, overlay};

gpui_kit::actions!(
    pr_review,
    [
        Quit,
        CloseWindow,
        Palette,
        Refresh,
        OpenGitHub,
        Settings,
        Shortcuts,
        Post,
        GoBack,
        NeedsYou,
        DoneList,
        ArchivedList,
        UsageView,
        FocusNext,
        FocusPrev,
        AddPr,
        Paste,
        StartReviewer,
        StopReviewer,
        RestartReviewer,
        ReviewAgain
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
        KeyBinding::new("cmd-,", Settings, None),
        KeyBinding::new("cmd-shift-/", Shortcuts, None),
        KeyBinding::new("cmd-enter", Post, None),
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-1", NeedsYou, None),
        KeyBinding::new("cmd-2", DoneList, None),
        KeyBinding::new("cmd-3", ArchivedList, None),
        KeyBinding::new("cmd-4", UsageView, None),
        KeyBinding::new("cmd-n", AddPr, None),
        KeyBinding::new("cmd-v", Paste, None),
        // Ours, not the kit root's: the window records that the keyboard owns focus, so the ring shows.
        KeyBinding::new("tab", FocusNext, Some("pr-review")),
        KeyBinding::new("shift-tab", FocusPrev, Some("pr-review")),
    ]
}

/// The name the window, the app menu and the title bar show.
pub const APP_NAME: &str = "PR Reviewer Agent";
/// The app icon's eye as a glyph, and its pupil, drawn on top in the accent.
pub const MARK: &str = "brand/mark.svg";
pub const MARK_PUPIL: &str = "brand/mark-pupil.svg";

/// View → Appearance picks a theme: the menu's data, so the item for the theme in use carries the check.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = pr_review, no_json)]
pub struct SetTheme(pub tokens::Choice);

pub fn menus(chosen: tokens::Choice) -> Vec<Menu> {
    let themes = tokens::Choice::all().map(|c| MenuItem::action(c.name(), SetTheme(c)).checked(c == chosen));
    vec![
        Menu::new(APP_NAME).items([MenuItem::action(format!("Quit {APP_NAME}"), Quit)]),
        Menu::new("File")
            .items([MenuItem::action("Review a PR…", AddPr), MenuItem::action("Close Window", CloseWindow)]),
        Menu::new("View").items([
            MenuItem::action("Needs You", NeedsYou),
            MenuItem::action("Done", DoneList),
            MenuItem::action("Archived", ArchivedList),
            MenuItem::action("Usage", UsageView),
            MenuItem::action("Back", GoBack),
            MenuItem::separator(),
            MenuItem::action("Command Palette", Palette),
            MenuItem::action("Settings", Settings),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Appearance").items(themes)),
        ]),
        Menu::new("Review").items([
            MenuItem::action("Open on GitHub", OpenGitHub),
            MenuItem::action("Post the Preview", Post),
            MenuItem::action("Review Again…", ReviewAgain),
            MenuItem::separator(),
            MenuItem::action("Refresh from the Daemon", Refresh),
        ]),
        Menu::new("Reviewer").items([
            MenuItem::action("Start Reviewer", StartReviewer),
            MenuItem::action("Stop Reviewer…", StopReviewer),
            MenuItem::action("Restart Reviewer", RestartReviewer),
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
    ConfirmPost(Arm),
    /// The sheet that adds a PR by its URL. `sent` once ⏎ sent it: the reducer's `adding` then says how it went, and
    /// the sheet closes when the PR it added is selected.
    AddPr {
        url: String,
        sent: bool,
    },
    /// The sheet that asks before a stop pauses `running` reviews, or before a LaunchAgent that launchd would
    /// restart is unloaded instead.
    ConfirmStop {
        restart: bool,
        running: usize,
    },
}

/// The post sheet's guard against the chord that opened it. ⌘⏎ both opens the sheet and posts from it, so a
/// double press would post before the sheet could be read. The sheet arms ⌘⏎ only once ⏎ has come up since it
/// opened (⏎ comes up, or ⌘ does) and `DELAY` has passed: a double press, a held chord and its key repeat all land
/// on an unarmed sheet.
/// A sheet opened by a click or Y needs no release. The sheet's Post button acts at once: it sits far from the
/// button that opened the sheet, so no double click reaches it.
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    opened: Instant,
    released: bool,
}

impl Arm {
    pub const DELAY: Duration = Duration::from_millis(500);

    pub fn new(opened: Instant, by_chord: bool) -> Arm {
        Arm { opened, released: !by_chord }
    }

    /// ⏎ came up.
    pub fn release(&mut self) {
        self.released = true;
    }

    pub fn ready(&self, now: Instant) -> bool {
        self.released && now.duration_since(self.opened) >= Self::DELAY
    }
}

/// The list the home shows beside the PR page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Queue,
    Done,
    Archived,
}

pub struct Shell {
    pub model: Model,
    pub cmds: mpsc::Sender<NetCmd>,
    /// How the daemon starts and stops on this machine, and what the app last asked of it.
    pub reviewer: crate::reviewer::Reviewer,
    /// The offline page shows the command that starts the daemon by hand.
    pub show_command: bool,
    /// The home shows the done list instead of the queue. The archived list is the reducer's `show_archived`, so
    /// its cursor rule and the window read one flag.
    pub done_tab: bool,
    pub overlay: Option<Overlay>,
    pub focus: FocusHandle,
    /// No motion: the system asks for less, or a snapshot needs a still frame.
    pub still: bool,
    /// A snapshot: the window ignores the keyboard and the mouse, so what it shows is only the model.
    pub snapshot: bool,
    /// The live log held at an event, counted from 0, or `None` to follow the run.
    pub scrub: Option<usize>,
    /// The waterfall span whose detail shows, by id (or by name, from a script), or `None`.
    pub span: Option<String>,
    /// The window's width, as the last frame laid it out: a page's action bar fits itself to it.
    pub width: Pixels,
    /// The action bar's More menu is open.
    pub more_open: bool,
    /// The timeline's "Verifier discarded" groups that are open, by the sequence of their CANDIDATES_DISPOSED.
    pub open_discards: Vec<u64>,
    /// The diff lists only the files with findings.
    pub diff_flagged_only: bool,
    /// The diff's Generated group is open.
    pub diff_generated_open: bool,
    /// Settings → Agent: the model picker's search and choice.
    pub picker: crate::agent::Picker,
    /// Settings → Risk check (Jev): the TypeSafe key field and what the Keychain holds.
    pub jev: crate::jev::KeyField,
    /// A sheet was open in the last frame.
    pub sheet_was_open: bool,
    /// The footer's avatar for the publishing account.
    pub avatar: crate::account::Avatar,
    /// A PR the add sheet sent while the reviewer was stopped: it goes to the daemon once the start brings it up.
    pub queued_add: Option<String>,
}

/// A page the reader went into: a finding, the proof, the diff, Settings or Usage. The live strip steps aside there.
pub fn deep(m: &Model) -> bool {
    front(m).is_some() || matches!(m.ui.screen, Screen::Diff | Screen::Settings) || m.usage.by.is_some()
}

/// The reader's theme choice, a global so every view and the menu bar read the same one.
struct Chosen(tokens::Choice);

impl Global for Chosen {}

pub fn chosen(cx: &App) -> tokens::Choice {
    cx.try_global::<Chosen>().map_or(tokens::Choice::System, |c| c.0)
}

/// The theme the window paints with now: the choice, with System resolved against macOS's appearance.
pub fn active(cx: &App) -> &'static tokens::Theme {
    chosen(cx).theme(ThemeMode::from(cx.window_appearance()).is_dark())
}

/// The active theme's tokens. Every view takes its colors from here.
pub fn colors(cx: &App) -> Colors {
    (active(cx).colors)()
}

/// Makes `choice` the window's look: the views' tokens, gpui-kit's own colors and mode, and the menu's check.
pub fn apply_theme(choice: tokens::Choice, cx: &mut App) {
    cx.set_global(Chosen(choice));
    let theme = active(cx);
    // `change` loads the kit's own theme for the mode, so the palette goes over it in a second edit.
    Theme::change(if theme.dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |k| (theme.colors)().paint_kit(&mut k.colors));
    cx.set_menus(menus(choice));
}

/// The PR on GitHub, built from its repository and number. Only a plain `owner/repo` makes one, so the desktop opens
/// nothing but github.com pull URLs.
pub fn github_pr_url(p: &Pr) -> Option<String> {
    let repo = p.repository.as_str();
    (repo.split('/').count() == 2 && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)))
        .then(|| format!("https://github.com/{repo}/pull/{}", p.number))
}

/// The page-sized panel in front: a finding or the publication, under any sheet.
pub fn front(m: &Model) -> Option<&Modal> {
    m.ui.modals.iter().rev().find(|x| matches!(x, Modal::Finding { .. } | Modal::Publication { .. }))
}

impl Shell {
    pub fn new(
        model: Model,
        cmds: mpsc::Sender<NetCmd>,
        reviewer: crate::reviewer::Reviewer,
        still: bool,
        snapshot: bool,
        desk: &[Desk],
        cx: &mut Context<Self>,
    ) -> Self {
        let mut s = Shell {
            model,
            cmds,
            reviewer,
            show_command: false,
            done_tab: false,
            overlay: None,
            focus: cx.focus_handle(),
            still,
            snapshot,
            scrub: None,
            span: None,
            width: px(1280.),
            more_open: false,
            open_discards: Vec::new(),
            diff_flagged_only: false,
            diff_generated_open: false,
            sheet_was_open: false,
            avatar: Default::default(),
            picker: crate::agent::Picker::default(),
            jev: crate::jev::KeyField::new(),
            queued_add: None,
        };
        for d in desk {
            match d {
                Desk::Tab(list) => s.set_tab(*list, cx),
                Desk::Palette(q) => s.overlay = Some(Overlay::Palette { query: q.clone(), selected: 0 }),
                Desk::Keys => s.overlay = Some(Overlay::Keys),
                Desk::ConfirmPost => s.post(false, cx),
                Desk::AddPr(url) => {
                    s.overlay = Some(Overlay::AddPr { url: url.clone(), sent: s.model.adding.is_some() })
                }
                Desk::Scrub(n) => s.scrub = Some(n.saturating_sub(1)),
                Desk::Span(name) => s.span = Some(name.clone()),
                Desk::Reviewer(state) => crate::reviewer::stage(&mut s, *state),
                Desk::Flagged => s.diff_flagged_only = true,
                Desk::Discards => {
                    s.open_discards =
                        s.model.timeline.iter().flat_map(|tl| &tl.events).map(|e| e.sequence.get()).collect()
                }
            }
        }
        s
    }

    /// The home shows the done list: the reader asked for it, or a live queue is empty and the done list is what
    /// there is to show, under a line that says the reader is clear.
    pub fn shows_done(&self) -> bool {
        !self.model.ui.show_archived
            && (self.done_tab || (queue::stage(&self.model) == Stage::Ready && queue::cards(&self.model).is_empty()))
    }

    /// The list the home shows now.
    pub fn list(&self) -> List {
        if self.model.ui.show_archived {
            List::Archived
        } else if self.shows_done() {
            List::Done
        } else {
            List::Queue
        }
    }

    /// The PRs of `list`, in the order the home shows them.
    fn list_prs(&self, list: List) -> Vec<PrId> {
        let m = &self.model;
        match list {
            List::Queue => queue::cards(m).iter().filter_map(|c| c.pr().map(|p| p.id.clone())).collect(),
            List::Done => queue::done(m).iter().map(|(p, _)| p.id.clone()).collect(),
            List::Archived => m.archived().iter().map(|p| p.id.clone()).collect(),
        }
    }

    /// A color for the sidebar and the title bar: a little see-through where the window blurs what is behind it,
    /// and solid in a snapshot, whose pixels must not depend on the desktop.
    pub fn glass(&self, c: Hsla) -> Hsla {
        if self.snapshot { c } else { c.alpha(0.9) }
    }

    /// The reader is on a deep page or the timeline: Back (⌘[) has somewhere to go.
    pub fn away(&self) -> bool {
        !matches!(self.model.ui.screen, Screen::Inbox | Screen::Review)
            || front(&self.model).is_some()
            || self.usage_open()
    }

    /// The Usage page is in front. It is the reducer's `usage.by`, so what shows and what is asked for read one field.
    pub fn usage_open(&self) -> bool {
        self.model.usage.by.is_some()
    }

    /// ⌘4, the sidebar, the menu and the palette: the Usage page, grouped by `by` or as it was last.
    pub fn open_usage(&mut self, by: Option<UsageBy>, cx: &mut Context<Self>) {
        let by = by.or(self.model.usage.by).or(self.model.usage.report.as_ref().map(|r| r.by)).unwrap_or(UsageBy::Day);
        if !self.usage_open() {
            self.close_modals(cx);
            self.home(cx);
        }
        self.send(Msg::Usage(Some(by)), cx);
    }

    fn close_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_open() {
            self.send(Msg::Usage(None), cx);
        }
    }

    pub fn send(&mut self, msg: Msg, cx: &mut Context<Self>) {
        if dispatch(&mut self.model, msg, &self.cmds) {
            cx.quit();
        }
        self.model.dirty = false;
        cx.notify();
    }

    /// Going anywhere leaves Usage, and Back from Usage only leaves it. Refresh there asks for usage again too.
    pub fn act(&mut self, a: Action, cx: &mut Context<Self>) {
        if self.usage_open() {
            match a {
                Action::Back => return self.close_usage(cx),
                Action::Goto(_) => self.close_usage(cx),
                Action::Refresh => self.send(Msg::Usage(self.model.usage.by), cx),
                _ => {}
            }
        }
        self.send(Msg::Action(a), cx);
    }

    /// Switches the theme at once and saves it in the config file's [desktop] section, so the next launch opens
    /// in it. A snapshot never writes the reader's config.
    pub fn set_theme(&mut self, choice: tokens::Choice, cx: &mut Context<Self>) {
        apply_theme(choice, cx);
        if !self.snapshot
            && let Err(e) = pr_review_tui::config::set_desktop_theme(&self.reviewer.config_path, choice.id())
        {
            let until_ms = self.model.now_ms + crate::reviewer::NOTICE_MS;
            self.model.notice = Some(pr_review_tui::model::Notice { text: format!("Theme not saved: {e}"), until_ms });
        }
        cx.notify();
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
        self.close_usage(cx);
        self.close_modals(cx);
        self.home(cx);
        self.scrub = None;
        self.span = None;
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
        self.model.ui.show_archived = false;
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
            Some(Card::Failed { run, .. }) => {
                let mu = queue::redo(run);
                return self.direct(mu, cx);
            }
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
    /// `chord` is true when ⌘⏎ opened it, so the same chord can't post until it is armed.
    pub fn post(&mut self, chord: bool, cx: &mut Context<Self>) {
        if matches!(self.model.top_modal(), Some(Modal::Publication { .. })) && can_confirm(&self.model) {
            self.overlay = Some(Overlay::ConfirmPost(Arm::new(Instant::now(), chord)));
            cx.notify();
        }
    }

    pub fn confirm_post(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.act(Action::Confirm, cx);
    }

    /// ⌘⏎: posts the sheet in front once it is armed, opens it over a preview, or confirms a reducer panel.
    fn post_key(&mut self, cx: &mut Context<Self>) {
        match (&self.overlay, self.model.top_modal()) {
            (Some(Overlay::ConfirmPost(arm)), _) => {
                if arm.ready(Instant::now()) {
                    self.confirm_post(cx)
                }
            }
            (None, Some(Modal::Publication { .. })) => self.post(true, cx),
            (None, Some(Modal::Confirm { .. })) => self.act(Action::Confirm, cx),
            (Some(Overlay::ConfirmStop { .. }), _) => self.confirm_stop(cx),
            _ => {}
        }
    }

    /// ⌘N: the sheet that adds a PR. On a daemon that refuses it the reducer says why, in a toast.
    pub fn add_pr(&mut self, cx: &mut Context<Self>) {
        if !can_add(&self.model) && !self.starts_for_add() {
            return self.send(Msg::AddPr(String::new()), cx);
        }
        self.overlay = Some(Overlay::AddPr { url: String::new(), sent: false });
        cx.notify();
    }

    /// The sheet's field changed: what the daemon said about the last URL no longer applies.
    fn edit_add(&mut self, edit: impl FnOnce(&mut String), cx: &mut Context<Self>) {
        let Some(Overlay::AddPr { url, sent }) = &mut self.overlay else { return };
        if self.model.adding.as_ref().is_some_and(|a| matches!(a.state, AddState::Sending(_))) {
            return;
        }
        edit(url);
        *sent = false;
        self.model.adding = None;
        self.queued_add = None;
        cx.notify();
    }

    /// ⏎ in the sheet: sends the PR it names, once it names one.
    pub fn submit_add(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::AddPr { url, sent }) = &mut self.overlay else { return };
        if PrRef::parse(url).is_none()
            || self.model.adding.as_ref().is_some_and(|a| matches!(a.state, AddState::Sending(_)))
        {
            return;
        }
        *sent = true;
        let url = url.clone();
        if !can_add(&self.model) && self.starts_for_add() {
            self.queued_add = Some(url);
            if !matches!(self.reviewer.control, crate::reviewer::Control::Starting { .. }) {
                self.ask(crate::reviewer::Ask::Start, cx);
            }
            return cx.notify();
        }
        self.send(Msg::AddPr(url), cx);
    }

    fn starts_for_add(&self) -> bool {
        crate::reviewer::starts_for_add(&self.model, &self.reviewer)
    }

    pub fn close_add(&mut self, cx: &mut Context<Self>) {
        if !self
            .model
            .adding
            .as_ref()
            .is_some_and(|a| matches!(a.state, AddState::Sending(_) | AddState::Landing { .. }))
        {
            self.model.adding = None;
        }
        self.queued_add = None;
        self.overlay = None;
        cx.notify();
    }

    pub fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        self.overlay = match self.overlay {
            Some(Overlay::Palette { .. }) => None,
            _ => Some(Overlay::Palette { query: String::new(), selected: 0 }),
        };
        cx.notify();
    }

    pub fn open_github(&mut self, cx: &mut Context<Self>) {
        if matches!(self.model.top_modal(), Some(Modal::Publication { .. })) {
            return self.act(Action::OpenUrl, cx);
        }
        if let Some(url) = self.model.selected_pr().and_then(github_pr_url) {
            cx.open_url(&url);
        }
    }

    fn move_card(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let list @ (List::Done | List::Archived) = self.list() {
            let ids = self.list_prs(list);
            let at = ids.iter().position(|id| Some(id) == self.model.ui.selected_pr.as_ref());
            let next = at.map_or(0, |i| (i as isize + delta).clamp(0, ids.len() as isize - 1) as usize);
            if let Some(id) = ids.get(next) {
                self.select_pr(id.clone(), cx);
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

    pub fn set_tab(&mut self, list: List, cx: &mut Context<Self>) {
        self.close_usage(cx);
        self.done_tab = list == List::Done;
        self.model.ui.show_archived = list == List::Archived;
        self.close_modals(cx);
        self.home(cx);
        // An archived PR's page shows only beside the archived list, and only an archived one shows there.
        let fits = self.model.selected_pr().is_some_and(|p| p.archived() == (list == List::Archived));
        if !fits {
            match self.list_prs(list).first().cloned() {
                Some(id) => self.select_pr(id, cx),
                None => self.model.ui.selected_pr = None,
            }
        }
        cx.notify();
    }

    /// E or the PR page's Archive: archives the open PR, or unarchives it beside the archived list. It hides
    /// nothing else and undoes in one key, so it asks nothing; the reducer moves the selection on once the daemon
    /// confirms.
    pub fn archive(&mut self, cx: &mut Context<Self>) {
        if self.model.ui.screen == Screen::Inbox {
            self.act(Action::Goto(Screen::Review), cx);
        }
        self.act(Action::Archive, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.snapshot || self.typing(window, cx) {
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
            Some(Overlay::ConfirmPost(_) | Overlay::ConfirmStop { .. }) => {
                // Cancel is the default: ⏎ and esc both close it. Only ⌘⏎ posts or stops.
                if matches!(key, "escape" | "enter") {
                    self.overlay = None;
                    cx.notify();
                }
                return;
            }
            Some(Overlay::AddPr { .. }) => {
                match key {
                    "escape" => self.close_add(cx),
                    "enter" => self.submit_add(cx),
                    "backspace" => self.edit_add(
                        |u| {
                            u.pop();
                        },
                        cx,
                    ),
                    _ => {
                        if let Some(c) = typed.filter(|c| !c.chars().any(char::is_control)) {
                            self.edit_add(|u| u.push_str(&c), cx);
                        }
                    }
                }
                return;
            }
            _ => {}
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
        if self.usage_open() && m.top_modal().is_none() {
            match (key, ch) {
                ("escape", _) => self.close_usage(cx),
                (_, "d") => self.open_usage(Some(UsageBy::Day), cx),
                (_, "w") => self.open_usage(Some(UsageBy::Week), cx),
                (_, "m") => self.open_usage(Some(UsageBy::Month), cx),
                _ => {}
            }
            return;
        }
        let modal = m.top_modal().cloned();
        let screen = m.ui.screen;
        let a = match (modal.as_ref(), screen, key, ch) {
            // A sheet's safe button is its default: ⏎ keeps a review running, and only ⌘⏎ or Y cancels it.
            (Some(Modal::Confirm { command: Command::Cancel(_), .. }), _, "enter", _) => Action::Back,
            (Some(Modal::Confirm { .. }), _, "enter", _) | (Some(Modal::Confirm { .. }), _, _, "y") => Action::Confirm,
            (Some(Modal::Confirm { .. }), _, _, "n") => Action::Back,
            // While the approval is on its way, esc does not walk away from it, as its Not now button can't.
            (Some(Modal::Publication { run_id, publish, .. }), _, "escape", _)
                if crate::publish::stage(m, run_id, publish) == crate::publish::Stage::Posting =>
            {
                return;
            }
            (Some(Modal::Publication { .. }), _, _, "y") => return self.post(false, cx),
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
            (None, Screen::Diff, _, "f") => {
                self.diff_flagged_only = !self.diff_flagged_only;
                return cx.notify();
            }
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
                    (_, "S") => Action::Mutation(Mutation::Again),
                    (_, "c") => Action::Mutation(Mutation::Cancel),
                    (_, "p") => Action::Mutation(Mutation::Publish),
                    (_, "e") if modal.is_none() => return self.archive(cx),
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
        self.sync_avatar(cx);
        if self.model.ui.screen == Screen::Settings {
            self.key_input(window, cx);
            self.model_search(window, cx);
        }
        // The PR the sheet sent is selected: the sheet's work is done.
        if matches!(self.overlay, Some(Overlay::AddPr { sent: true, .. })) && self.model.adding.is_none() {
            self.overlay = None;
        }
        let quiet = matches!(
            self.model.top_modal(),
            Some(Modal::Confirm {
                command: Command::Start(_) | Command::Retry(_) | Command::StopDaemon(_),
                state: ConfirmState::Requested(_) | ConfirmState::Accepted { .. },
                ..
            })
        );
        let sheet = self.overlay.is_some()
            || matches!(self.model.top_modal(), Some(Modal::Confirm { .. } | Modal::Input { .. } | Modal::Help { .. }))
                && !quiet;
        crate::kit::sheet_open(sheet);
        // A sheet opening takes the page's tab stops away, and one closing takes its own: a control focused there
        // would hold focus outside the frame, where no key reaches. The window takes focus back on either change.
        if sheet != self.sheet_was_open && !self.snapshot {
            window.focus(&self.focus, cx);
        }
        self.sheet_was_open = sheet;
        // A focused control that a state change removed (a card's button once its proof opens, the proof's Post
        // under its sheet) leaves no element focused, and then no key reaches the window: focus returns to it.
        if !self.snapshot && window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let t = colors(cx);
        {
            use gpui_kit::component::scroll::ScrollbarMode;
            if Theme::global(cx).scrollbar_mode != ScrollbarMode::Always {
                Theme::set_scrollbar_mode(ScrollbarMode::Always, cx);
            }
        }
        let m = &self.model;
        let stage = queue::stage(m);
        let finding_open = matches!(front(m), Some(Modal::Finding { .. }));
        let body = match m.ui.screen {
            Screen::Diff => diff::view(self, &t, cx).into_any_element(),
            Screen::Settings => crate::setup::settings(self, &t, cx).into_any_element(),
            _ if self.usage_open() => crate::usage::view(self, &t, cx).into_any_element(),
            _ => div()
                .flex()
                .flex_1()
                .min_h_0()
                .when(stage == Stage::Ready && !finding_open, |d| {
                    d.child(match self.list() {
                        List::Archived => crate::cards::archived_column(self, &t, cx),
                        List::Done => crate::cards::done_column(self, &t, cx),
                        List::Queue => crate::cards::queue_column(self, &t, cx),
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
            // A press hands focus back to the mouse: the ring goes until the next ⇥.
            .capture_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| {
                if crate::kit::KEYBOARD.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    cx.notify();
                }
            }))
            // macOS sends no key-up for a key pressed under ⌘, so lifting ⌘ is the release of a ⌘⏎ chord; a plain
            // ⏎ coming up counts too.
            .on_key_up(cx.listener(|s, ev: &KeyUpEvent, _, _| {
                if let (Some(Overlay::ConfirmPost(arm)), "enter") = (&mut s.overlay, ev.keystroke.key.as_str()) {
                    arm.release();
                }
            }))
            .on_modifiers_changed(cx.listener(|s, ev: &ModifiersChangedEvent, _, _| {
                if let (Some(Overlay::ConfirmPost(arm)), false) = (&mut s.overlay, ev.modifiers.platform) {
                    arm.release();
                }
            }))
            .on_action(cx.listener(|s, _: &Palette, _, cx| s.toggle_palette(cx)))
            .on_action(cx.listener(|s, _: &Refresh, _, cx| s.refresh(cx)))
            .on_action(cx.listener(|s, _: &StartReviewer, _, cx| s.ask(crate::reviewer::Ask::Start, cx)))
            .on_action(cx.listener(|s, _: &StopReviewer, _, cx| s.ask(crate::reviewer::Ask::Stop, cx)))
            .on_action(cx.listener(|s, _: &RestartReviewer, _, cx| s.ask(crate::reviewer::Ask::Restart, cx)))
            .on_action(cx.listener(|s, _: &OpenGitHub, _, cx| s.open_github(cx)))
            .on_action(cx.listener(|s, _: &Settings, _, cx| s.act(Action::Goto(Screen::Settings), cx)))
            .on_action(cx.listener(|s, a: &SetTheme, _, cx| s.set_theme(a.0, cx)))
            .on_action(cx.listener(|s, _: &Shortcuts, _, cx| {
                s.overlay = Some(Overlay::Keys);
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &Post, _, cx| s.post_key(cx)))
            .on_action(cx.listener(|s, _: &ReviewAgain, _, cx| s.act(Action::Mutation(Mutation::Again), cx)))
            .on_action(cx.listener(|s, _: &GoBack, _, cx| {
                if s.overlay.is_none() && s.away() {
                    s.act(Action::Back, cx)
                }
            }))
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| {
                crate::kit::KEYBOARD.store(true, std::sync::atomic::Ordering::Relaxed);
                window.focus_next(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &FocusPrev, window, cx| {
                crate::kit::KEYBOARD.store(true, std::sync::atomic::Ordering::Relaxed);
                window.focus_prev(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &AddPr, _, cx| s.add_pr(cx)))
            .on_action(cx.listener(|s, _: &Paste, _, cx| {
                let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
                // A URL copied with its line break still names one PR.
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                s.edit_add(|u| u.push_str(text.trim()), cx);
            }))
            .on_action(cx.listener(|s, _: &NeedsYou, _, cx| s.set_tab(List::Queue, cx)))
            .on_action(cx.listener(|s, _: &DoneList, _, cx| s.set_tab(List::Done, cx)))
            .on_action(cx.listener(|s, _: &ArchivedList, _, cx| s.set_tab(List::Archived, cx)))
            .on_action(cx.listener(|s, _: &UsageView, _, cx| s.open_usage(None, cx)))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
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
                        .children(banner(self, &t, cx))
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

/// The app's mark and name, set off from the page title by a hairline.
fn brand(t: &Colors) -> Div {
    let mark = div()
        .relative()
        .size(px(22.))
        .flex_none()
        .child(svg().path(MARK).absolute().size_full().text_color(t.text))
        .child(svg().path(MARK_PUPIL).absolute().size_full().text_color(t.accent));
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .pr(space::S3)
        .border_r_1()
        .border_color(t.border)
        .child(mark)
        .child(div().text_size(text::SMALL).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(APP_NAME))
}

/// The title bar: the traffic lights, the app's mark and name, the page title with its one-sentence state, and the ⌘K field.
fn titlebar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let pr = m.selected_pr();
    let number = pr.map_or(String::new(), |p| format!("#{}", p.number));
    let pr_title = pr.map_or(String::new(), |p| p.title.to_string());
    let (title, sub) = match (m.ui.screen, front(m)) {
        (_, Some(Modal::Finding { id, .. })) => {
            // Counted within the list the PR page shows it in, so "of N" matches that list's count.
            let stale = m.domain.findings.get(id).is_some_and(|f| f.stale);
            let all: Vec<_> = pr
                .map(|p| m.domain.pr_findings(&p.id))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| f.stale == stale)
                .collect();
            let at = all.iter().position(|f| &f.id == id).map_or(0, |i| i + 1);
            let what = if stale { "earlier finding" } else { "finding" };
            (format!("{number} · {what} {at} of {}", all.len()), pr_title)
        }
        (_, Some(Modal::Publication { run_id, publish, .. })) => {
            let word = match crate::publish::stage(m, run_id, publish) {
                crate::publish::Stage::Posted => "posted",
                crate::publish::Stage::Posting => "posting…",
                crate::publish::Stage::Proof => "proof",
            };
            (format!("{number} · {word}"), pr_title)
        }
        (Screen::Diff, _) => (format!("{number} diff"), pr_title),
        (Screen::Timeline, _) => (format!("{number} timeline"), pr_title),
        (Screen::Settings, _) => ("Settings".into(), "What the reviewer needs and how it reviews".into()),
        _ if s.usage_open() => ("Usage".into(), "Tokens and estimated cost of reviews".into()),
        _ if s.list() == List::Archived => (
            "Archived".into(),
            format!("{} out of your lists until you unarchive or review one.", queue::count(m.archived().len(), "PR")),
        ),
        _ if s.done_tab && !queue::cards(m).is_empty() => {
            ("Done".into(), format!("{} with nothing left for you.", queue::count(queue::done(m).len(), "PR")))
        }
        // The home's title says how much waits, not the list's name the sidebar already shows, and nothing under
        // it: the queue right below says what each decision is. Setup is a precondition, not a decision.
        _ => match (queue::stage(m), queue::decisions(&queue::cards(m)), queue::setup_count(m).blocking) {
            (Stage::Ready, 0, 0) => ("You're clear".into(), String::new()),
            (Stage::Ready, 0, _) => ("Setup needed".into(), String::new()),
            (Stage::Ready, n, _) => (format!("{} waiting", queue::count(n, "decision")), String::new()),
            (Stage::Connecting, ..) => ("Connecting".into(), String::new()),
            _ => ("Offline".into(), queue::summary(m)),
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
        .child(brand(t))
        // Back as a macOS toolbar button: a chevron in a square that lights on hover, its shortcut in the tooltip.
        .when(away, |d| {
            d.child(crate::kit::icon_button(
                t,
                "back",
                IconName::ChevronLeft,
                "Back (⌘[)",
                px(28.),
                Some(click(cx, |s, cx| s.act(Action::Back, cx))),
            ))
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
        // The live strip belongs to the home; on a deep page the title bar keeps the one fact: reviews are at work.
        .children({
            let running = queue::running(m);
            (deep(m) && !running.is_empty()).then(|| {
                let first = running[0].0.id.clone();
                let names =
                    running.iter().map(|(p, r)| format!("#{} {}", p.number, phase_word(r.phase))).collect::<Vec<_>>();
                let face = div()
                    .id("live-face")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S2)
                    .h(px(24.))
                    .px(space::S2)
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(t.running.alpha(0.5))
                    .bg(t.running.alpha(t.tint))
                    .hover(move |d| d.bg(t.running.alpha(t.tint * 2.)))
                    .child(live_dot(t, s.still, "title-live"))
                    .child(
                        div()
                            .text_size(text::SMALL)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.running)
                            .child(format!("{} running", running.len())),
                    );
                tip(
                    focusable(
                        "live",
                        face,
                        t,
                        radius::PILL,
                        Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| s.select_pr(first.clone(), cx))),
                    ),
                    format!("Watch live: {}", names.join(", ")),
                )
            })
        })
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
                    .child("Read-only")
                    .cursor_pointer()
                    .on_click(cx.listener(|s, _: &ClickEvent, _, cx| s.act(Action::Goto(Screen::Settings), cx))),
                format!("{} Click for Settings.", crate::setup::read_only_sentence(m)),
            ))
        })
        .child(reviewer_control(s, t, cx))
        .child(tip(
            focusable("search", search, t, radius::SM, click(cx, |s, cx| s.toggle_palette(cx))),
            "Search PRs and findings, or run a command",
        ))
}

/// The reviewer's state in a word, and the one action that changes it: Stop while it runs, Start while it doesn't.
/// A start or stop on its way shows a spinner; an action that can't run now is dimmed with the reason in its tooltip.
fn reviewer_control(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Stateful<Div> {
    use crate::reviewer::{Ask, Control};
    let (hue, word) = link_word(s, t);
    let busy = matches!(s.reviewer.control, Control::Starting { .. } | Control::Stopping { .. });
    let ask = if s.model.link.welcomed() { Ask::Stop } else { Ask::Start };
    let (glyph, name) = match ask {
        Ask::Stop => (IconName::CircleStop, "Stop reviewer"),
        _ => (IconName::CirclePlay, "Start reviewer"),
    };
    let action = if busy {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(24.))
            .child(crate::setup::spinner(t, s.still, "reviewer-spin"))
            .id("reviewer-busy")
    } else {
        match s.reviewer.block(&s.model, ask) {
            None => {
                crate::kit::icon_button(t, "reviewer-toggle", glyph, name, px(24.), on(cx, move |s, cx| s.ask(ask, cx)))
            }
            Some(why) => crate::kit::icon_button(
                t,
                "reviewer-toggle",
                glyph,
                &format!("{name}: {}", crate::kit::sentence(&why)),
                px(24.),
                None,
            ),
        }
    };
    div()
        .id("reviewer")
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(px(30.))
        .pl(space::S3)
        .pr(px(3.))
        .rounded(radius::SM)
        .border_1()
        .border_color(t.border)
        .child(div().size(px(7.)).flex_none().rounded(radius::PILL).bg(hue))
        .child(div().text_size(text::SMALL).text_color(t.muted).child(format!("Reviewer · {word}")))
        .child(action)
}

#[allow(clippy::too_many_arguments)]
fn nav_item(
    t: &Colors,
    id: &'static str,
    glyph: IconName,
    label: &str,
    count: Option<usize>,
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
        // An empty list still counts: zero in the neutral ink, so the column of counts never has a hole.
        .children(count.map(|n| count_badge(t, n, if n == 0 { t.muted } else { hue })));
    focusable(id, row, t, radius::SM, f)
}

/// The pane shows setup: the Settings page, or the home with no PR selected while setup blocks reviews.
pub fn setup_shown(s: &Shell) -> bool {
    let m = &s.model;
    m.ui.screen == Screen::Settings
        || matches!(m.ui.screen, Screen::Inbox | Screen::Review)
            && front(m).is_none()
            && !s.usage_open()
            && m.selected_pr().is_none()
            && queue::stage(m) == Stage::Ready
            && queue::setup_count(m).blocking > 0
}

/// The sidebar: the smart lists, then every PR grouped by repository with its state, then the connection.
fn sidebar(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let m = &s.model;
    let stage = queue::stage(m);
    let cards = queue::cards(m);
    let done = queue::done(m);
    let home = matches!(m.ui.screen, Screen::Inbox | Screen::Review | Screen::Timeline)
        && front(m).is_none()
        && !s.usage_open();
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
            (!dim).then_some(queue::decisions(&cards)),
            t.accent,
            home && s.list() == List::Queue && !setup_shown(s),
            dim,
            click(cx, |s, cx| s.set_tab(List::Queue, cx)),
        ))
        .child(nav_item(
            t,
            "nav-done",
            IconName::CircleCheck,
            "Done",
            (!dim).then_some(done.len()),
            t.ok,
            home && s.list() == List::Done,
            dim,
            click(cx, |s, cx| s.set_tab(List::Done, cx)),
        ))
        .child(nav_item(
            t,
            "nav-archived",
            IconName::Archive,
            "Archived",
            (!dim).then_some(m.archived().len()),
            t.muted,
            home && s.list() == List::Archived,
            dim,
            click(cx, |s, cx| s.set_tab(List::Archived, cx)),
        ))
        .child(nav_item(
            t,
            "nav-usage",
            IconName::ChartColumn,
            "Usage",
            None,
            t.muted,
            s.usage_open() && matches!(m.ui.screen, Screen::Inbox | Screen::Review),
            dim,
            click(cx, |s, cx| s.open_usage(None, cx)),
        ))
        .child(nav_item(
            t,
            "nav-settings",
            IconName::Settings,
            "Settings",
            Some(queue::setup_count(m).blocking).filter(|n| *n > 0),
            t.warn,
            setup_shown(s),
            false,
            click(cx, |s, cx| s.act(Action::Goto(Screen::Settings), cx)),
        ))
        .child(add_row(s, t, dim, cx));
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
        .child(crate::account::footer(s, t, cx))
}

/// The sidebar's way to add a PR: a quiet row under the lists with ⌘N beside it, locked with its reason on a daemon
/// that can't add one.
fn add_row(s: &Shell, t: &Colors, dim: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let dim = dim && !crate::reviewer::starts_for_add(&s.model, &s.reviewer);
    let why = crate::overlay::add_block(&s.model, &s.reviewer).filter(|_| !dim);
    let off = why.is_some() || dim;
    let fg = if off { t.disabled } else { t.muted };
    let row = div()
        .id("nav-add-face")
        .when(dim, |d| d.opacity(0.55))
        .flex()
        .items_center()
        .gap(space::S2)
        .h(px(32.))
        .px(space::S2)
        .rounded(radius::SM)
        .when(!off, |d| d.hover(move |d| d.bg(t.hover)).active(|d| d.bg(t.press)))
        .child(icon(if why.is_some() { IconName::Lock } else { IconName::Plus }, px(15.), fg))
        .child(div().flex_1().text_size(text::BODY).text_color(fg).child("Review a PR…"))
        .child(keycap(t, "⌘N", None, off));
    let row = focusable("nav-add", row, t, radius::SM, click(cx, |s, cx| s.add_pr(cx)));
    match why {
        Some(why) => tip(row, format!("Can't add a PR: {why}. Restart the daemon with --allow-commands.")),
        None => tip(row, "Add a PR by its GitHub URL and start its review"),
    }
}

/// The reader approved `pr`'s proof and it is on its way, while the run still reads as parked.
fn posting(s: &Shell, pr: &Pr) -> bool {
    let m = &s.model;
    m.ui.modals.iter().any(|x| match x {
        Modal::Publication { run_id, publish, .. } => {
            m.domain.runs.get(run_id).is_some_and(|r| r.pr_id == pr.id)
                && crate::publish::stage(m, run_id, publish) == crate::publish::Stage::Posting
        }
        _ => false,
    })
}

fn pr_row(s: &Shell, t: &Colors, pr: &Pr, state: RowState, on: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let id = pr.id.clone();
    let (glyph, hue): (AnyElement, Hsla) = match state {
        RowState::Running(_) => (live_dot(t, s.still, ("nav-dot", pr.number as usize)), t.running),
        RowState::Parked => (icon(IconName::Send, px(13.), t.accent).into_any_element(), t.accent),
        RowState::Failed => (icon(IconName::CircleX, px(13.), t.danger).into_any_element(), t.danger),
        RowState::Cancelled => (icon(IconName::CircleSlash, px(13.), t.muted).into_any_element(), t.muted),
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
        RowState::Running(p) => Some(progress(t, queue::running_marks(p), t.running, Gauge::Pip).into_any_element()),
        _ => None,
    };
    let row =
        div()
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
                            .child(div().flex_none().text_color(hue).child(
                                if state == RowState::Parked && posting(s, pr) { "Posting" } else { state.word() },
                            ))
                            .child(div().flex_1())
                            .children(badge),
                    ),
            );
    let full = pr.title.to_string();
    tip(
        focusable(
            SharedString::from(format!("nav-pr-{}", pr.number)),
            row,
            t,
            radius::SM,
            Box::new(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.done_tab = queue::done(&s.model).iter().any(|(p, _)| p.id == id);
                s.model.ui.show_archived = false;
                s.select_pr(id.clone(), cx)
            })),
        ),
        full,
    )
}

/// The connection in a word, with the reviewer's own state when the app started or stopped it.
pub fn link_word(s: &Shell, t: &Colors) -> (Hsla, String) {
    use crate::reviewer::Control;
    match &s.reviewer.control {
        Control::Starting { .. } => return (t.warn, "Starting…".into()),
        Control::Stopping { .. } => return (t.warn, "Stopping…".into()),
        Control::Stopped if !s.model.link.welcomed() => return (t.muted, "Stopped by you".into()),
        Control::StartFailed(_) if !s.model.link.welcomed() => return (t.danger, "Didn't start".into()),
        _ => {}
    }
    match &s.model.link {
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

/// With a board on screen but the link down, a banner says what the board is: the last state the daemon sent. It
/// names a stop the reader asked for apart from a lost connection, and offers the start when one can run.
fn banner(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Option<Div> {
    use crate::reviewer::{Ask, Control};
    let m = &s.model;
    if m.link.accepts_events() || queue::stage(m) != Stage::Ready {
        return None;
    }
    let (hue, glyph, title, detail) = match (&s.reviewer.control, &m.link) {
        (Control::Starting { .. }, _) => {
            (t.warn, IconName::LoaderCircle, "Starting the reviewer".into(), "It connects as soon as it listens".into())
        }
        (Control::Stopping { .. }, _) => {
            (t.warn, IconName::LoaderCircle, "Stopping the reviewer".into(), "Running reviews pause".into())
        }
        (Control::Stopped, _) => (
            t.muted,
            IconName::CircleStop,
            "Reviewer stopped".into(),
            "You stopped it. Reviews that were running resume when it starts again".into(),
        ),
        (Control::StartFailed(why), _) => {
            (t.danger, IconName::CircleX, "The reviewer didn't start".into(), why.clone())
        }
        (_, Link::Disconnected { reason, retry_in_ms }) => (
            t.danger,
            IconName::TriangleAlert,
            match retry_in_ms {
                Some(ms) => format!("Disconnected. Retrying in {}s", ms.div_ceil(1000)),
                None => "Disconnected".to_string(),
            },
            reason.to_string(),
        ),
        (_, Link::Stalled { reason }) => {
            (t.danger, IconName::TriangleAlert, "The snapshot never arrived".into(), format!("{reason} (⌘R)"))
        }
        (_, Link::Incompatible { message, .. }) => {
            (t.danger, IconName::TriangleAlert, "This daemon speaks another protocol".into(), message.to_string())
        }
        (_, Link::Unusable { message }) => {
            (t.danger, IconName::TriangleAlert, "This endpoint can't work".into(), message.to_string())
        }
        _ => (t.warn, IconName::TriangleAlert, "Reconnecting to the reviewer".into(), m.diag.endpoint.clone()),
    };
    let start = s.reviewer.block(m, Ask::Start).is_none() && matches!(m.link, Link::Disconnected { .. });
    Some(
        div()
            .flex()
            .items_center()
            .gap(space::S3)
            .min_h(px(36.))
            .px(space::S5)
            .py(space::S2)
            .bg(hue.alpha(0.10))
            .border_b_1()
            .border_color(hue.alpha(0.30))
            .child(icon(glyph, px(14.), hue))
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
            )
            .when(start, |d| {
                d.child(button(
                    t,
                    "banner-start",
                    Ask::Start.title(),
                    Some("⌘R"),
                    Tone::Secondary,
                    on(cx, |s, cx| s.ask(Ask::Start, cx)),
                ))
            }),
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
/// The pills the strip shows before it folds the rest under "+N running".
const STRIP_PILLS: usize = 2;

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
        // Two pills at most, each capped, so a title never eats the row; the rest wait behind "+N running".
        .children(running.iter().take(STRIP_PILLS).map(|(p, r)| pill(s, t, p, r, cx)))
        .when(running.len() > STRIP_PILLS, |d| {
            let more = running.len() - STRIP_PILLS;
            d.child(tip(
                button(
                    t,
                    "strip-more",
                    &format!("+{more} running"),
                    None,
                    Tone::Secondary,
                    on(cx, |s, cx| {
                        s.overlay = Some(Overlay::Palette { query: "watch".into(), selected: 0 });
                        cx.notify();
                    }),
                ),
                "Every review at work, in the palette",
            ))
        })
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
        .child(progress(t, queue::marks(m, r), t.running, Gauge::Strip))
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
        .min_w_0()
        .max_w(px(520.)),
        format!("Watch #{} live", p.number),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_pr_links_to_github_only_for_a_plain_owner_and_repo() {
        let pr = |repository: &str| -> pr_review_tui::protocol::Pr {
            serde_json::from_value(serde_json::json!({
                "id": "p", "repository": repository, "number": 7, "title": "t",
                "headSha": "a".repeat(40), "baseSha": "b".repeat(40), "lifecycle": "OPEN"
            }))
            .unwrap()
        };
        assert_eq!(super::github_pr_url(&pr("acme/shop")).as_deref(), Some("https://github.com/acme/shop/pull/7"));
        for bad in ["acme", "acme/shop/x", "acme/shop?x=1", "javascript:alert/x", "acme/sh op"] {
            assert_eq!(super::github_pr_url(&pr(bad)), None, "{bad}");
        }
    }

    use super::Arm;
    use std::time::{Duration, Instant};

    #[test]
    fn a_double_cmd_return_opens_the_post_sheet_and_never_posts_from_it() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut arm = Arm::new(t0, true);
        assert!(!arm.ready(ms(120)), "the second press of a double ⌘⏎");
        assert!(!arm.ready(ms(900)), "the chord is still held: its key repeat");
        arm.release();
        assert!(!arm.ready(ms(300)), "released, but sooner than the sheet can be read");
        assert!(arm.ready(ms(500)));
        assert!(Arm::new(t0, false).ready(ms(500)), "a sheet opened by a click needs no release");
        assert!(!Arm::new(t0, false).ready(ms(499)));
    }
}
