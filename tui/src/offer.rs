//! The footer's hints: the keys that act now, in one fixed order, and which of them outlast the others when the footer
//! runs out of room. Each hint asks `crate::update` the question its key's guard asks, so the footer offers a key
//! exactly when pressing it acts.

use crate::keymap::Mutation;
use crate::model::{Modal, Model, Pane, Screen};
use crate::update as u;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub keys: &'static str,
    pub label: &'static str,
}

impl Hint {
    pub fn text(&self) -> String {
        format!("{} {}", self.keys, self.label)
    }
}

/// Most important first. Where the footer runs out of room the last of these goes first: the keys that leave or
/// explain a view, then the ones that act on what it shows, then the ones that move elsewhere or narrow it.
pub const RANK: [&str; 26] = [
    "?", "Esc", "n/Esc", "y", "Enter", "Enter/l", "j/k", "p", "R", "c", "d", "q", "n/N", "h", "s", "/", ":", "J/K",
    "m", "o", "t", "w", "f", "v", "P", "r",
];

fn rank(keys: &str) -> usize {
    RANK.iter().position(|k| *k == keys).unwrap_or(RANK.len())
}

/// Every key that acts now, in the order the footer writes them.
pub fn offered(m: &Model) -> Vec<Hint> {
    let mut out = Vec::new();
    let mut offer = |on: bool, keys: &'static str, label: &'static str| {
        debug_assert!(RANK.contains(&keys), "{keys} has no rank");
        if on {
            out.push(Hint { keys, label });
        }
    };
    match m.top_modal() {
        Some(Modal::Input { .. }) => {}
        Some(Modal::Confirm { .. }) => {
            let asking = u::can_confirm(m);
            offer(asking, "y", "confirm");
            offer(true, "n/Esc", if asking { "cancel" } else { "close" });
        }
        Some(Modal::Publication { raw, run_id, .. }) => {
            offer(u::can_confirm(m), "y", "publish");
            offer(u::can_move(m), "j/k", "scroll");
            offer(true, "m", if *raw { "styled" } else { "raw markdown" });
            offer(u::published_url(m, run_id).is_some(), "o", "open on GitHub");
            offer(u::can_diff(m), "d", "diff");
            offer(true, "Esc", "close");
        }
        Some(Modal::Help { .. }) => {
            offer(u::can_move(m), "j/k", "scroll");
            offer(true, "Esc", "close");
            offer(true, "q", "quit");
        }
        Some(Modal::Finding { .. }) => {
            offer(u::can_move(m), "j/k", "scroll");
            offer(u::can_turn(m), "n/N", "finding");
            offer(u::can_diff(m), "d", "diff at anchor");
            offer(u::can_timeline(m), "t", "timeline");
            offer(true, "Esc", "close");
        }
        None => {
            let (screen, side) = (m.ui.screen, m.ui.focus == Pane::Sidebar);
            let page = matches!(screen, Screen::Inbox | Screen::Review);
            let rail = screen == Screen::Inbox || page && side;
            let moves = match (screen, side) {
                (Screen::Review, false) => "finding",
                (Screen::Inbox | Screen::Review, _) => "pull request",
                (Screen::Diff, true) => "file",
                (Screen::Diff, false) => "line",
                (Screen::Timeline, _) => "move",
                (Screen::Settings, _) => "account",
            };
            offer(u::can_move(m), "j/k", moves);
            let open = u::can_open(m);
            let unfold = m.timeline.as_ref().is_some_and(|t| t.open.is_some() && t.open == t.selected);
            match screen {
                // In a side pane Enter and l both move into the page.
                Screen::Review | Screen::Diff if side => {
                    offer(open, "Enter/l", if screen == Screen::Review { "findings" } else { "code" })
                }
                Screen::Settings => offer(open, "Enter", "select account"),
                Screen::Timeline => offer(open, "Enter", if unfold { "fold or read" } else { "detail" }),
                _ => offer(open, "Enter", "read"),
            }
            let search = m.diff.as_ref().is_some_and(|d| d.search.is_some());
            let turns = screen == Screen::Diff || screen == Screen::Review && side;
            offer(turns && u::can_turn(m), "n/N", if search { "match" } else { "finding" });
            if page {
                offer(u::can_mutate(m, Mutation::Publish), "p", "preview & publish");
                offer(u::can_mutate(m, Mutation::Start), "s", "start review");
                offer(u::can_mutate(m, Mutation::Retry), "R", "retry");
                offer(u::can_mutate(m, Mutation::Cancel), "c", "cancel review");
                offer(u::can_diff(m), "d", "diff");
                offer(u::can_scroll_doc(m), "J/K", "scroll");
            }
            let back = if screen == Screen::Review { "pull requests" } else { "files" };
            offer(!rail && u::can_focus(m, Pane::Sidebar), "h", back);
            offer(u::can_search(m), "/", if screen == Screen::Timeline { "filter" } else { "search" });
            offer(screen == Screen::Diff && u::can_jump(m), ":", "jump");
            let timeline = screen == Screen::Timeline;
            offer(timeline && u::can_filter_timeline(m), "f", "scope");
            offer(timeline && u::can_cycle_kind(m), "v", "kind");
            offer(timeline && u::can_cycle_phase(m), "P", "phase");
            offer(page && u::can_timeline(m), "t", "timeline");
            offer(u::can_watch(m), "w", "watch live");
            offer(screen == Screen::Settings && u::can_refresh(m), "r", "refresh");
            let esc = match (&m.timeline, screen) {
                (Some(t), Screen::Timeline) if t.open.is_some() => "fold",
                _ if search && screen == Screen::Diff => "clear search",
                _ => "back",
            };
            offer(u::can_back(m), "Esc", esc);
            offer(true, "?", "help");
            offer(true, "q", "quit");
        }
    }
    out
}

/// The hints that fit in `width` columns, written `   a  b  c`: the least important go first, the rest keep their
/// order.
pub fn fit(mut hints: Vec<Hint>, width: usize) -> Vec<Hint> {
    while hints.len() > 1 && crate::text::width(&line(&hints)) > width {
        let least = (0..hints.len()).max_by_key(|&i| rank(hints[i].keys)).unwrap_or(0);
        hints.remove(least);
    }
    hints
}

pub fn line(hints: &[Hint]) -> String {
    format!("   {}", hints.iter().map(Hint::text).collect::<Vec<_>>().join("  "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(keys: &'static str, label: &'static str) -> Hint {
        Hint { keys, label }
    }

    #[test]
    fn the_review_footer_at_60_columns_keeps_diff_and_quit_over_the_pull_request_list() {
        let review = vec![
            hint("j/k", "finding"),
            hint("Enter", "read"),
            hint("d", "diff"),
            hint("J/K", "scroll"),
            hint("h", "pull requests"),
            hint("t", "timeline"),
            hint("?", "help"),
            hint("q", "quit"),
        ];
        assert_eq!(line(&fit(review, 58)), "   j/k finding  Enter read  d diff  ? help  q quit");
    }
}
