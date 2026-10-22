//! The triage queue: the model read as the decisions waiting on the reader, most urgent first. A view over the TUI's
//! `Model`, never state of its own, so the queue can't drift from what the reducer knows.

use pr_review_tui::model::Model;
use pr_review_tui::protocol::{
    Finding, FindingStatus, Lifecycle, Phase, Pr, PrId, PublicationStatus, Run, RunStatus, SetupCheck, Severity,
};

/// One decision. Its order in the queue is `Card::rank`.
#[derive(Debug)]
pub enum Card<'a> {
    /// Reviews can't run, or can't be acted on, until the reader fixes the daemon's setup.
    Setup {
        missing: Vec<&'a SetupCheck>,
        commands_off: Option<&'static str>,
    },
    /// A review parked for approval: the reviewer is waiting on the reader.
    Approve {
        pr: &'a Pr,
        findings: Vec<&'a Finding>,
    },
    /// A posting that failed or that GitHub never confirmed.
    Posting {
        pr: &'a Pr,
        status: PublicationStatus,
    },
    /// Current findings no one has posted or judged, most severe first.
    Findings {
        pr: &'a Pr,
        findings: Vec<&'a Finding>,
    },
    Failed {
        pr: &'a Pr,
        run: &'a Run,
    },
    /// An open PR no review has looked at.
    Start {
        pr: &'a Pr,
    },
}

impl<'a> Card<'a> {
    pub fn pr(&self) -> Option<&'a Pr> {
        match self {
            Card::Setup { .. } => None,
            Card::Approve { pr, .. }
            | Card::Posting { pr, .. }
            | Card::Findings { pr, .. }
            | Card::Failed { pr, .. }
            | Card::Start { pr } => Some(pr),
        }
    }

    /// Lower is more urgent. A parked review comes first, since the reviewer is blocked on the reader; then what is
    /// wrong in the code, by severity; then what is stuck; then what has not started.
    fn rank(&self) -> u8 {
        match self {
            Card::Setup { .. } => 0,
            Card::Approve { .. } => 1,
            Card::Posting { .. } => 2,
            Card::Findings { findings, .. } => match findings.first().map(|f| f.severity) {
                Some(Severity::Critical) => 3,
                Some(Severity::High) => 4,
                _ => 7,
            },
            Card::Failed { .. } => 5,
            Card::Start { .. } => 8,
        }
    }

    /// The queue's sections, so the eye finds the tier before the card.
    pub fn tier(&self) -> Tier {
        match self.rank() {
            0..=2 => Tier::Decide,
            3..=4 => Tier::Blocking,
            5..=6 => Tier::Unblock,
            _ => Tier::Later,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Decide,
    Blocking,
    Unblock,
    Later,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Decide => "Decide now",
            Tier::Blocking => "Blocking findings",
            Tier::Unblock => "Unblock",
            Tier::Later => "When you have a minute",
        }
    }
}

/// A finding the reader still has to judge: current, and neither posted nor settled.
pub fn open_finding(f: &Finding) -> bool {
    !f.stale && matches!(f.status, FindingStatus::Detected | FindingStatus::StillPresent | FindingStatus::Uncertain)
}

pub fn cards(m: &Model) -> Vec<Card<'_>> {
    let d = &m.domain;
    let mut out = Vec::new();
    if let Some(s) = &m.server {
        let missing: Vec<&SetupCheck> = s.missing().collect();
        let commands_off = s.commands_off();
        if !missing.is_empty() || commands_off.is_some() {
            out.push(Card::Setup { missing, commands_off });
        }
    }
    for pr in m.inbox() {
        let run = d.latest_run(&pr.id);
        let findings: Vec<&Finding> = d.pr_findings(&pr.id).into_iter().filter(|f| open_finding(f)).collect();
        let publication = run.and_then(|r| d.publications.get(&r.id)).map(|p| p.status);
        let card = match (run, publication) {
            (Some(r), _) if r.parked() => Card::Approve { pr, findings },
            (Some(r), _) if r.active() => continue,
            (Some(_), Some(status @ (PublicationStatus::Failed | PublicationStatus::Uncertain))) => {
                Card::Posting { pr, status }
            }
            _ if pr.lifecycle != Lifecycle::Active => continue,
            (Some(r), _) if r.status == RunStatus::Failed => Card::Failed { pr, run: r },
            (None, _) => Card::Start { pr },
            _ if !findings.is_empty() => Card::Findings { pr, findings },
            _ => continue,
        };
        out.push(card);
    }
    out.sort_by_key(Card::rank);
    out
}

/// Reviews the reviewer is working on now, oldest first.
pub fn running(m: &Model) -> Vec<(&Pr, &Run)> {
    let mut v: Vec<(&Pr, &Run)> =
        m.domain.runs.values().filter(|r| r.active()).filter_map(|r| Some((m.domain.prs.get(&r.pr_id)?, r))).collect();
    v.sort_by(|a, b| (&a.1.started_at, &a.1.id).cmp(&(&b.1.started_at, &b.1.id)));
    v
}

/// PRs with nothing left for the reader: posted, clean, merged or closed.
pub fn done(m: &Model) -> Vec<(&Pr, Option<&Run>)> {
    let queued: Vec<&PrId> = cards(m).iter().filter_map(|c| c.pr().map(|p| &p.id)).collect();
    m.inbox()
        .into_iter()
        .filter(|p| !queued.contains(&&p.id))
        .filter_map(|p| {
            let run = m.domain.latest_run(&p.id);
            (!run.is_some_and(|r| r.status == RunStatus::Running)).then_some((p, run))
        })
        .collect()
}

/// What a done PR came to, in a few words.
pub fn outcome(m: &Model, pr: &Pr, run: Option<&Run>) -> String {
    let posted =
        run.and_then(|r| m.domain.publications.get(&r.id)).filter(|p| p.status == PublicationStatus::Published);
    let findings = m.domain.pr_findings(&pr.id).into_iter().filter(|f| !f.stale).count();
    match (posted, run.map(|r| r.phase), pr.lifecycle) {
        (Some(_), _, _) => format!("Review posted · {}", count(findings, "finding")),
        (_, _, Lifecycle::Merged) => "Merged".into(),
        (_, _, Lifecycle::Closed) => "Closed".into(),
        (_, Some(Phase::Completed), _) if findings == 0 => "Reviewed · nothing found".into(),
        (_, Some(Phase::Completed), _) => format!("Reviewed · {}", count(findings, "finding")),
        (_, Some(Phase::Superseded), _) => "Superseded by a newer head".into(),
        _ => "No action needed".into(),
    }
}

pub fn count(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// Index of the card for `pr`, the one the detail pane shows.
pub fn card_of(cards: &[Card], pr: Option<&PrId>) -> Option<usize> {
    let pr = pr?;
    cards.iter().position(|c| c.pr().is_some_and(|p| &p.id == pr))
}

/// "2 high, 1 medium": the severities in a list of findings, most severe first.
pub fn severity_summary(findings: &[&Finding]) -> String {
    let mut parts = Vec::new();
    for s in [Severity::Critical, Severity::High, Severity::Medium, Severity::Low] {
        let n = findings.iter().filter(|f| f.severity == s).count();
        if n > 0 {
            parts.push(format!("{n} {}", crate::tokens::severity_mark(s).1.to_lowercase()));
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{Card, Tier, cards, done, running};
    use crate::script::{parse, play};
    use pr_review_tui::model::{Look, Model, Screen};
    use std::path::Path;

    fn at(ms: u64, steps: &str) -> Model {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/showcase.ndjson");
        let mut m = Model::new("test", Look::default(), Screen::Review, None);
        play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &parse(steps).unwrap(), ms).unwrap();
        m
    }

    fn shape(m: &Model) -> Vec<(String, u64)> {
        cards(m)
            .iter()
            .map(|c| {
                let kind = match c {
                    Card::Setup { .. } => "setup",
                    Card::Approve { .. } => "approve",
                    Card::Posting { .. } => "posting",
                    Card::Findings { .. } => "findings",
                    Card::Failed { .. } => "failed",
                    Card::Start { .. } => "start",
                };
                (kind.to_string(), c.pr().map_or(0, |p| p.number))
            })
            .collect()
    }

    #[test]
    fn the_showcase_queue_puts_the_parked_review_first_and_running_ones_in_the_strip() {
        let m = at(20_000, "");
        let s = |k: &str, n| (k.to_string(), n);
        assert_eq!(shape(&m), [s("approve", 1291), s("findings", 1287), s("failed", 345), s("start", 1296)]);
        assert_eq!(cards(&m)[1].tier(), Tier::Blocking);
        assert_eq!(running(&m).iter().map(|(p, _)| p.number).collect::<Vec<_>>(), [338]);
        assert_eq!(done(&m).iter().map(|(p, _)| p.number).collect::<Vec<_>>(), [341]);
    }

    #[test]
    fn a_posted_review_leaves_the_queue_for_done() {
        let m = at(30_000, "wait:20000,pr:1291,publish,wait:100,confirm");
        assert!(!shape(&m).iter().any(|(_, n)| *n == 1291), "{:?}", shape(&m));
        assert!(done(&m).iter().any(|(p, _)| p.number == 1291));
    }
}
