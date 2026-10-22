//! The snapshot harness's `--actions` script, and a playback played in virtual time, so the same flags always reach
//! the same model however fast the machine is.

use pr_review_tui::app::dispatch;
use pr_review_tui::keymap::{Action, Mutation};
use pr_review_tui::model::{Model, Screen};
use pr_review_tui::net::{NetCmd, QUEUE};
use pr_review_tui::playback::{Player, Script};
use pr_review_tui::update::{Msg, NetEvent};
use pr_review_tui::view::Hit;
use tokio::sync::mpsc;

/// The TUI's tick cadence.
pub const TICK_MS: u64 = 125;

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Act(Action),
    /// Selects the PR with this number, as a click on its row does.
    Pr(u64),
    /// Selects the selected PR's nth finding, counted from 1, as a click on it does.
    Finding(usize),
    /// Milliseconds before the next step.
    Wait(u64),
}

#[rustfmt::skip]
const NAMES: &[(&str, Action)] = &[
    ("quit", Action::Quit), ("up", Action::Up), ("down", Action::Down), ("page-up", Action::PageUp),
    ("page-down", Action::PageDown), ("top", Action::Top), ("bottom", Action::Bottom), ("left", Action::Left),
    ("right", Action::Right), ("focus-next", Action::FocusNext), ("focus-prev", Action::FocusPrev),
    ("open", Action::Open), ("back", Action::Back), ("search", Action::Search), ("jump-to", Action::JumpTo),
    ("open-diff", Action::OpenDiff), ("open-timeline", Action::OpenTimeline), ("watch", Action::Watch),
    ("scroll-down", Action::ScrollDown), ("scroll-up", Action::ScrollUp), ("next-finding", Action::NextFinding),
    ("prev-finding", Action::PrevFinding), ("refresh", Action::Refresh), ("help", Action::Help),
    ("accounts", Action::Accounts), ("start", Action::Mutation(Mutation::Start)),
    ("retry", Action::Mutation(Mutation::Retry)), ("cancel", Action::Mutation(Mutation::Cancel)),
    ("publish", Action::Mutation(Mutation::Publish)), ("cycle-scope", Action::CycleScope),
    ("cycle-category", Action::CycleCategory), ("cycle-phase", Action::CyclePhase),
    ("backspace", Action::InputBackspace), ("submit", Action::InputSubmit), ("cancel-input", Action::InputCancel),
    ("confirm", Action::Confirm), ("open-url", Action::OpenUrl), ("toggle-raw", Action::ToggleRaw),
];

const SCREENS: &[(&str, Screen)] = &[
    ("inbox", Screen::Inbox),
    ("review", Screen::Review),
    ("diff", Screen::Diff),
    ("timeline", Screen::Timeline),
    ("settings", Screen::Settings),
];

/// Comma-separated steps: an action name, `goto:<screen>`, `type:<text>`, `pr:<number>`, `finding:<n>` or
/// `wait:<ms>`.
pub fn parse(s: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for t in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let number = |v: &str| v.parse::<u64>().map_err(|_| format!("{t:?}: {v:?} is not a number"));
        match t.split_once(':') {
            Some(("wait", v)) => steps.push(Step::Wait(number(v)?)),
            Some(("pr", v)) => steps.push(Step::Pr(number(v)?)),
            Some(("finding", v)) => match number(v)? {
                0 => return Err(format!("{t:?}: findings count from 1")),
                n => steps.push(Step::Finding(n as usize)),
            },
            Some(("type", v)) => steps.extend(v.chars().map(|c| Step::Act(Action::InputChar(c)))),
            Some(("goto", v)) => match SCREENS.iter().find(|(n, _)| *n == v) {
                Some((_, s)) => steps.push(Step::Act(Action::Goto(*s))),
                None => return Err(format!("{t:?}: unknown screen; known: {}", names(SCREENS))),
            },
            _ => match NAMES.iter().find(|(n, _)| *n == t) {
                Some((_, a)) => steps.push(Step::Act(*a)),
                None => return Err(format!("unknown action {t:?}; known: {}", names(NAMES))),
            },
        }
    }
    Ok(steps)
}

fn names<T>(table: &[(&str, T)]) -> String {
    table.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// The message a step sends now, `None` for a wait.
fn message(m: &Model, step: &Step) -> Result<Option<Msg>, String> {
    Ok(Some(match step {
        Step::Wait(_) => return Ok(None),
        Step::Act(a) => Msg::Action(*a),
        Step::Pr(n) => match m.domain.prs.values().find(|p| p.number == *n) {
            Some(p) => Msg::Pick(Hit::Pr(p.id.clone())),
            None => return Err(format!("pr:{n}: no such PR at {} ms", m.now_ms)),
        },
        Step::Finding(i) => {
            let findings = m.ui.selected_pr.as_ref().map(|pr| m.domain.pr_findings(pr)).unwrap_or_default();
            match findings.get(i - 1) {
                Some(f) => Msg::Pick(Hit::Finding(f.id.clone())),
                None => return Err(format!("finding:{i}: the selected PR has {} findings", findings.len())),
            }
        }
    }))
}

/// `playback::run` with its clock replaced: script lines, ticks and steps apply in time order up to `until_ms`, and
/// a command is answered at once and rearms the next line's delay, as the playback task does.
pub fn play(script: Script, m: &mut Model, steps: &[Step], until_ms: u64) -> Result<(), String> {
    let mut sim = Sim::new(script);
    sim.apply(m, Msg::Net(NetEvent::Opened));
    sim.rearm();
    let mut due = Vec::new();
    let mut at = 0;
    for s in steps {
        match s {
            Step::Wait(ms) => at += ms,
            _ => due.push((at, s)),
        }
    }
    let mut due = due.into_iter().peekable();
    let mut tick = TICK_MS;
    loop {
        let step_at = due.peek().map(|(t, _)| *t);
        let next = [sim.line_at, step_at, Some(tick)].into_iter().flatten().min().unwrap_or(tick);
        if next > until_ms {
            return Ok(());
        }
        sim.now = next;
        if sim.line_at == Some(next) {
            let Some((i, _)) = sim.player.upcoming() else { unreachable!("a line was due") };
            let line = sim.player.play(i);
            sim.apply(m, Msg::Net(NetEvent::Message(line)));
            sim.rearm();
        } else if step_at == Some(next) {
            let Some((_, s)) = due.next() else { unreachable!("a step was due") };
            if let Some(msg) = message(m, s)? {
                sim.apply(m, msg);
            }
        } else {
            sim.apply(m, Msg::Tick(next));
            tick += TICK_MS;
        }
    }
}

struct Sim {
    player: Player,
    cmds: mpsc::Sender<NetCmd>,
    sent: mpsc::Receiver<NetCmd>,
    now: u64,
    line_at: Option<u64>,
}

impl Sim {
    fn new(script: Script) -> Sim {
        let (cmds, sent) = mpsc::channel(QUEUE);
        Sim { player: Player::new(script), cmds, sent, now: 0, line_at: None }
    }

    fn rearm(&mut self) {
        self.line_at = self.player.upcoming().map(|(_, d)| self.now + d.as_millis() as u64);
    }

    /// A quit is ignored: the harness quits once it has captured.
    fn apply(&mut self, m: &mut Model, msg: Msg) {
        dispatch(m, msg, &self.cmds);
        let mut answered = false;
        while let Ok(cmd) = self.sent.try_recv() {
            if let NetCmd::Send(req) = cmd {
                for reply in self.player.answer(&req) {
                    dispatch(m, Msg::Net(NetEvent::Message(reply)), &self.cmds);
                }
            }
            answered = true;
        }
        if answered {
            self.rearm();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pr_review_tui::model::{Link, Look, Modal};
    use std::path::Path;

    fn showcase() -> Script {
        pr_review_tui::playback::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/showcase.ndjson"),
        )
        .unwrap()
    }

    fn model() -> Model {
        Model::new("test", Look::default(), Screen::Review, None)
    }

    #[test]
    fn steps_parse_from_names_and_prefixes() {
        assert_eq!(
            parse("down, goto:diff,wait:300,type:ab,pr:1287,finding:2,start").unwrap(),
            [
                Step::Act(Action::Down),
                Step::Act(Action::Goto(Screen::Diff)),
                Step::Wait(300),
                Step::Act(Action::InputChar('a')),
                Step::Act(Action::InputChar('b')),
                Step::Pr(1287),
                Step::Finding(2),
                Step::Act(Action::Mutation(Mutation::Start)),
            ]
        );
        assert!(parse("sideways").unwrap_err().contains("known: quit, up"));
        assert!(parse("goto:nowhere").is_err());
        assert!(parse("finding:0").is_err());
    }

    #[test]
    fn the_showcase_plays_in_virtual_time_and_steps_land_on_the_model() {
        let mut m = model();
        play(showcase(), &mut m, &parse("wait:20000,pr:1287,finding:2,open").unwrap(), 20_500).unwrap();
        assert_eq!(m.link, Link::Live);
        assert_eq!(m.now_ms, 20_500);
        assert_eq!(m.selected_pr().map(|p| p.number), Some(1287));
        assert!(matches!(m.top_modal(), Some(Modal::Finding { .. })), "{:?}", m.top_modal());

        let mut again = model();
        play(showcase(), &mut again, &parse("wait:20000,pr:1287,finding:2,open").unwrap(), 20_500).unwrap();
        assert_eq!(format!("{:?}", again.domain), format!("{:?}", m.domain));
        assert_eq!(again.ui.selected_finding, m.ui.selected_finding);
    }

    #[test]
    fn a_step_naming_a_missing_pr_fails_loudly() {
        let err = play(showcase(), &mut model(), &[Step::Pr(1)], 1_000).unwrap_err();
        assert_eq!(err, "pr:1: no such PR at 0 ms");
    }
}
