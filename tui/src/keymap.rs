//! `(Context, KeyEvent) -> Action` as one table. A focused text input captures every key, so no shortcut fires while
//! the user types. `y` is bound only in the confirmation modal.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::{Modal, Screen};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Global,
    Inbox,
    Review,
    Diff,
    Timeline,
    Settings,
    Help,
    FindingModal,
    ConfirmModal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    Start,
    Retry,
    Cancel,
    Publish,
}

impl Mutation {
    pub fn capability(self) -> &'static str {
        match self {
            Mutation::Start => "review.start",
            Mutation::Retry => "review.retry",
            Mutation::Cancel => "review.cancel",
            Mutation::Publish => "review.publish",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    FocusNext,
    FocusPrev,
    Open,
    Back,
    Goto(Screen),
    Search,
    JumpTo,
    OpenDiff,
    OpenTimeline,
    Watch,
    ScrollDown,
    ScrollUp,
    NextFinding,
    PrevFinding,
    Refresh,
    Help,
    Accounts,
    Mutation(Mutation),
    CycleScope,
    CycleCategory,
    CyclePhase,
    InputChar(char),
    InputBackspace,
    InputSubmit,
    InputCancel,
    Confirm,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

use Action as A;
use Context as C;
use KeyCode as K;

#[rustfmt::skip]
const TABLE: &[(Context, KeyCode, KeyModifiers, Action)] = &[
    (C::Global, K::Char('q'), NONE, A::Quit),
    (C::Global, K::Char('c'), CTRL, A::Quit),
    (C::Global, K::Char('?'), NONE, A::Help),
    (C::Global, K::Char('?'), SHIFT, A::Help),
    (C::Global, K::Esc, NONE, A::Back),
    (C::Global, K::Char('j'), NONE, A::Down),
    (C::Global, K::Down, NONE, A::Down),
    (C::Global, K::Char('k'), NONE, A::Up),
    (C::Global, K::Up, NONE, A::Up),
    (C::Global, K::PageDown, NONE, A::PageDown),
    (C::Global, K::Char('d'), CTRL, A::PageDown),
    (C::Global, K::PageUp, NONE, A::PageUp),
    (C::Global, K::Char('u'), CTRL, A::PageUp),
    (C::Global, K::Char('g'), NONE, A::Top),
    (C::Global, K::Home, NONE, A::Top),
    (C::Global, K::Char('G'), SHIFT, A::Bottom),
    (C::Global, K::Char('G'), NONE, A::Bottom),
    (C::Global, K::End, NONE, A::Bottom),
    (C::Global, K::Char('h'), NONE, A::Left),
    (C::Global, K::Left, NONE, A::Left),
    (C::Global, K::Char('l'), NONE, A::Right),
    (C::Global, K::Right, NONE, A::Right),
    (C::Global, K::Tab, NONE, A::FocusNext),
    (C::Global, K::BackTab, SHIFT, A::FocusPrev),
    (C::Global, K::BackTab, NONE, A::FocusPrev),
    (C::Global, K::Enter, NONE, A::Open),
    (C::Global, K::Char('1'), NONE, A::Goto(Screen::Inbox)),
    (C::Global, K::Char('2'), NONE, A::Goto(Screen::Review)),
    (C::Global, K::Char('3'), NONE, A::Goto(Screen::Diff)),
    (C::Global, K::Char('4'), NONE, A::Goto(Screen::Timeline)),
    (C::Global, K::Char('5'), NONE, A::Goto(Screen::Settings)),
    (C::Global, K::Char('/'), NONE, A::Search),
    (C::Global, K::Char('d'), NONE, A::OpenDiff),
    (C::Global, K::Char('t'), NONE, A::OpenTimeline),
    (C::Global, K::Char('r'), NONE, A::Refresh),
    (C::Global, K::Char('A'), SHIFT, A::Accounts),
    (C::Global, K::Char('A'), NONE, A::Accounts),
    (C::Global, K::Char('s'), NONE, A::Mutation(Mutation::Start)),
    (C::Global, K::Char('R'), SHIFT, A::Mutation(Mutation::Retry)),
    (C::Global, K::Char('R'), NONE, A::Mutation(Mutation::Retry)),
    (C::Global, K::Char('c'), NONE, A::Mutation(Mutation::Cancel)),
    (C::Global, K::Char('p'), NONE, A::Mutation(Mutation::Publish)),
    (C::Global, K::Char('w'), NONE, A::Watch),
    (C::Review, K::Char('J'), SHIFT, A::ScrollDown),
    (C::Review, K::Char('J'), NONE, A::ScrollDown),
    (C::Review, K::Char('K'), SHIFT, A::ScrollUp),
    (C::Review, K::Char('K'), NONE, A::ScrollUp),
    (C::Review, K::Char('n'), NONE, A::NextFinding),
    (C::Review, K::Char('N'), SHIFT, A::PrevFinding),
    (C::Review, K::Char('N'), NONE, A::PrevFinding),
    (C::Diff, K::Char('n'), NONE, A::NextFinding),
    (C::Diff, K::Char('N'), SHIFT, A::PrevFinding),
    (C::Diff, K::Char('N'), NONE, A::PrevFinding),
    (C::Diff, K::Char(':'), NONE, A::JumpTo),
    (C::Diff, K::Char(':'), SHIFT, A::JumpTo),
    (C::Diff, K::Char('/'), NONE, A::JumpTo),
    (C::Timeline, K::Char('f'), NONE, A::CycleScope),
    (C::Timeline, K::Char('v'), NONE, A::CycleCategory),
    (C::Timeline, K::Char('P'), SHIFT, A::CyclePhase),
    (C::Timeline, K::Char('P'), NONE, A::CyclePhase),
    (C::ConfirmModal, K::Char('y'), NONE, A::Confirm),
    (C::ConfirmModal, K::Char('n'), NONE, A::Back),
];

pub fn context(screen: Screen, modal: Option<&Modal>) -> Context {
    match modal {
        Some(Modal::Help) => C::Help,
        Some(Modal::Finding { .. }) => C::FindingModal,
        Some(Modal::Confirm { .. }) => C::ConfirmModal,
        Some(Modal::Input { .. }) | None => match screen {
            Screen::Inbox => C::Inbox,
            Screen::Review => C::Review,
            Screen::Diff => C::Diff,
            Screen::Timeline => C::Timeline,
            Screen::Settings => C::Settings,
        },
    }
}

/// Resolves a key against the context's own bindings, then the global ones.
pub fn resolve(screen: Screen, modal: Option<&Modal>, key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if let Some(Modal::Input { .. }) = modal {
        return match (key.code, key.modifiers) {
            (K::Esc, _) => Some(A::InputCancel),
            (K::Enter, _) => Some(A::InputSubmit),
            (K::Backspace, _) => Some(A::InputBackspace),
            (K::Char('c'), CTRL) => Some(A::InputCancel),
            (K::Char(c), m) if !m.intersects(CTRL | KeyModifiers::ALT) => Some(A::InputChar(c)),
            _ => None,
        };
    }
    let ctx = context(screen, modal);
    let lookup = |c: Context| {
        TABLE.iter().find(|(tc, code, mods, _)| *tc == c && *code == key.code && *mods == key.modifiers).map(|t| t.3)
    };
    lookup(ctx).or_else(|| lookup(C::Global))
}

/// The help page's rows by context, read from the same table so help never drifts from behavior.
pub fn help_rows() -> Vec<(Context, String, &'static str)> {
    let describe = |a: Action| -> Option<&'static str> {
        Some(match a {
            A::Quit => "quit; reviews keep running",
            A::Help => "this help",
            A::Back => "close / back",
            A::Down | A::Up => "move",
            A::PageDown | A::PageUp => "page down / up",
            A::Top | A::Bottom => "first / last",
            A::Left | A::Right => "pane left / right",
            A::FocusNext => "cycle panes",
            A::Open => "open selected",
            A::Goto(_) => "inbox, review, diff, timeline, settings",
            A::Search => "search PRs",
            A::OpenDiff => "diff at the finding",
            A::OpenTimeline => "timeline for selection",
            A::Watch => "watch the running review",
            A::ScrollDown | A::ScrollUp => "scroll the explanation",
            A::Refresh => "refresh snapshot",
            A::Accounts => "GitHub accounts (Settings)",
            A::Mutation(_) => "start/retry/cancel/publish",
            A::NextFinding | A::PrevFinding => "next / previous finding",
            A::JumpTo => "jump to path:line",
            A::CycleScope => "timeline scope",
            A::CycleCategory => "timeline category",
            A::CyclePhase => "timeline phase",
            _ => return None,
        })
    };
    let mut rows: Vec<(Context, String, &'static str)> = Vec::new();
    for (ctx, code, mods, action) in TABLE {
        let Some(text) = describe(*action) else { continue };
        let key = match code {
            K::Char(c) if mods.contains(CTRL) => format!("C-{c}"),
            K::Char(c) => c.to_string(),
            K::Enter => "Enter".into(),
            K::Esc => "Esc".into(),
            K::Tab => "Tab".into(),
            K::Up | K::Down | K::Left | K::Right | K::BackTab | K::Home | K::End | K::PageUp | K::PageDown => continue,
            _ => continue,
        };
        match rows.iter_mut().find(|(c, _, t)| c == ctx && *t == text) {
            Some((_, keys, _)) if !keys.split(' ').any(|k| k == key) => keys.push_str(&format!(" {key}")),
            Some(_) => {}
            None => rows.push((*ctx, key, text)),
        }
    }
    rows
}
