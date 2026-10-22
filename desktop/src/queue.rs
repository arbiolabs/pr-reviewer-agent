//! The triage queue and the sidebar: the model read as the decisions waiting on the reader, most urgent first, and
//! every PR grouped by repository with its state. Views over the TUI's `Model`, never state of their own, so neither
//! can drift from what the reducer knows.

use pr_review_tui::model::{Link, Model};
use pr_review_tui::protocol::{
    Finding, FindingStatus, Lifecycle, Phase, Pr, PrId, PublicationStatus, Run, RunStatus, SetupCheck, Severity,
};

/// One decision. Its order in the queue is `Card::rank`.
#[derive(Debug)]
pub enum Card<'a> {
    /// Reviews can't run, or can't be acted on, until the reader fixes the daemon's setup.
    Setup {
        missing: Vec<&'a SetupCheck>,
        count: SetupCount,
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
            // A cancel was the reader's own choice: it waits with the reviews nobody started.
            Card::Failed { run, .. } if ending(run) == Some(Ending::Cancelled) => 8,
            Card::Failed { .. } => 5,
            Card::Start { .. } => 8,
        }
    }

    /// The queue's sections, so the eye finds the tier before the card.
    pub fn tier(&self) -> Tier {
        match self.rank() {
            0 => Tier::Setup,
            1..=2 => Tier::Decide,
            3..=4 => Tier::Blocking,
            5..=6 => Tier::Unblock,
            _ => Tier::Later,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Setup,
    Decide,
    Blocking,
    Unblock,
    Later,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Setup => "Before reviews can run",
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

/// How much evidence a finding cites, as every surface prints it: the loaded detail's own list once it is here,
/// and the summary's count until then, so a row and the page it opens never disagree.
pub fn evidence_count(m: &Model, f: &Finding) -> usize {
    m.details.get(&f.id).map_or(f.evidence_count as usize, |d| d.evidence.len())
}

/// What setup still wants, in the one vocabulary the sidebar badge, the setup card and the setup page share:
/// `blocking` checks keep reviews from running (a missing check, or commands that are off on a daemon with nothing
/// missing), `limited` checks let them run with less.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupCount {
    pub blocking: usize,
    pub limited: usize,
}

impl SetupCount {
    /// "1 blocking · 2 limited", or an empty string when nothing wants attention.
    pub fn words(self) -> String {
        [(self.blocking, "blocking"), (self.limited, "limited")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} {w}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// The daemon runs no review commands: it says it is read-only, or it answers live and grants none of them.
pub fn read_only(m: &Model) -> bool {
    let off = m.server.as_ref().is_some_and(|s| s.commands_off().is_some());
    let none = m.link.accepts_events()
        && m.server.is_some()
        && !["review.start", "review.retry", "review.cancel", "review.publish"].iter().any(|c| m.may_write(c));
    off || none
}

pub fn setup_count(m: &Model) -> SetupCount {
    let Some(s) = &m.server else { return SetupCount::default() };
    let missing = s.missing().count();
    let limited = s
        .checks
        .as_ref()
        .map_or(0, |cs| cs.iter().filter(|c| c.status == pr_review_tui::protocol::CheckStatus::Warn).count());
    SetupCount { blocking: if missing == 0 && s.commands_off().is_some() { 1 } else { missing }, limited }
}

pub fn cards(m: &Model) -> Vec<Card<'_>> {
    let d = &m.domain;
    let mut out = Vec::new();
    if let Some(s) = &m.server {
        let missing: Vec<&SetupCheck> = s.missing().collect();
        if !missing.is_empty() || s.commands_off().is_some() {
            out.push(Card::Setup { missing, count: setup_count(m) });
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

/// The decisions in `cards`: every card but setup, which is a precondition for deciding, not a decision.
pub fn decisions(cards: &[Card]) -> usize {
    cards.iter().filter(|c| !matches!(c, Card::Setup { .. })).count()
}

/// Reviews the reviewer is working on now, oldest first.
pub fn running(m: &Model) -> Vec<(&Pr, &Run)> {
    let mut v: Vec<(&Pr, &Run)> =
        m.domain.runs.values().filter(|r| r.active()).filter_map(|r| Some((m.domain.prs.get(&r.pr_id)?, r))).collect();
    v.sort_by(|a, b| (&a.1.started_at, &a.1.id).cmp(&(&b.1.started_at, &b.1.id)));
    v
}

/// How a run that stopped short ended: the reader cancelled it, the daemon refused it for size, or it broke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Cancelled,
    TooLarge,
    Failed,
}

/// `None` for a run that did not fail. The engine records a cancel as a failed run whose reason starts with
/// "cancelled", so the reason is the only place it says so.
pub fn ending(r: &Run) -> Option<Ending> {
    if r.status != RunStatus::Failed {
        return None;
    }
    let reason = r.failure_reason.as_ref().map_or(String::new(), |x| x.as_str().to_lowercase());
    Some(if reason.starts_with("cancel") {
        Ending::Cancelled
    } else if reason.contains("too large") {
        Ending::TooLarge
    } else {
        Ending::Failed
    })
}

/// What gets a stopped run going again: a cancel was the reader's choice, so it starts fresh; anything else
/// retries from the last checkpoint.
pub fn redo(r: &Run) -> pr_review_tui::keymap::Mutation {
    use pr_review_tui::keymap::Mutation;
    if ending(r) == Some(Ending::Cancelled) { Mutation::Start } else { Mutation::Retry }
}

/// What the gauge says about one step: only what the run did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Done,
    /// The step the run is on, or waits at.
    Now,
    Waiting,
    /// Where a failed or cancelled run stopped.
    Stopped,
    /// A step the run finished without: a completed review nobody posted.
    Skipped,
}

/// The six steps of `r`, from its status, its phase and its publication. A completed review that posted nothing
/// never shows Post as done.
pub fn marks(m: &Model, r: &Run) -> [Mark; 6] {
    if r.status == RunStatus::Running {
        return running_marks(r.phase);
    }
    if r.status == RunStatus::Failed || r.status == RunStatus::Superseded {
        return upto(stopped_at(m, r), Mark::Stopped);
    }
    match m.domain.publications.get(&r.id).map(|p| p.status) {
        Some(PublicationStatus::Published) => [Mark::Done; 6],
        Some(PublicationStatus::Failed) => upto(5, Mark::Stopped),
        Some(PublicationStatus::Publishing | PublicationStatus::Uncertain) => upto(5, Mark::Now),
        _ => upto(5, Mark::Skipped),
    }
}

/// The steps before `at` done, `at` itself `mark`, the rest waiting.
fn upto(at: usize, mark: Mark) -> [Mark; 6] {
    std::array::from_fn(|i| match i.cmp(&at) {
        std::cmp::Ordering::Less => Mark::Done,
        std::cmp::Ordering::Equal => mark,
        std::cmp::Ordering::Greater => Mark::Waiting,
    })
}

/// A run at work in `phase`.
pub fn running_marks(phase: Phase) -> [Mark; 6] {
    upto(crate::kit::step_of(phase).min(5), Mark::Now)
}

/// The step a run stopped in, from the last phase change the window saw for it; Collect when it saw none.
pub fn stopped_at(m: &Model, r: &Run) -> usize {
    use crate::kit::step_of;
    m.domain
        .activity
        .iter()
        .rev()
        .filter(|a| a.pr_id.as_ref() == Some(&r.pr_id))
        .find_map(|a| match &a.kind {
            pr_review_tui::model::ActivityKind::Phase { to, .. } if step_of(*to) < 6 => Some(step_of(*to)),
            _ => None,
        })
        .unwrap_or(0)
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

/// A run's status as the log says it: "Review started", never "Run running".
pub fn run_words(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "Review started",
        RunStatus::Completed => "Review completed",
        RunStatus::Failed => "Review failed",
        RunStatus::Superseded => "Superseded by a newer head",
        RunStatus::Unknown => "Review changed state",
    }
}

/// "1 step", "2 steps": every count the window prints goes through here.
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

/// A sidebar row's state: one glyph, one word, one badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Running(Phase),
    Parked,
    Failed,
    Cancelled,
    /// The worst open severity and how many open findings.
    Findings(Severity, usize),
    Posted,
    Clean,
    Merged,
    Closed,
    New,
}

impl RowState {
    pub fn word(self) -> &'static str {
        match self {
            RowState::Running(p) => match p {
                Phase::CollectContext => "Collecting",
                p => phase_word(p),
            },
            RowState::Parked => "To approve",
            RowState::Failed => "Failed",
            RowState::Cancelled => "Cancelled",
            RowState::Findings(..) => "To judge",
            RowState::Posted => "Posted",
            RowState::Clean => "Clean",
            RowState::Merged => "Merged",
            RowState::Closed => "Closed",
            RowState::New => "Not reviewed",
        }
    }
}

pub struct Row<'a> {
    pub pr: &'a Pr,
    pub state: RowState,
}

/// Every PR, grouped by repository in the inbox's order, each with its state.
pub fn rows(m: &Model) -> Vec<(String, Vec<Row<'_>>)> {
    let d = &m.domain;
    let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
    for pr in m.inbox() {
        let run = d.latest_run(&pr.id);
        let open: Vec<&Finding> = d.pr_findings(&pr.id).into_iter().filter(|f| open_finding(f)).collect();
        let posted =
            run.and_then(|r| d.publications.get(&r.id)).is_some_and(|p| p.status == PublicationStatus::Published);
        let state = match run {
            Some(r) if r.active() => RowState::Running(r.phase),
            Some(r) if r.parked() => RowState::Parked,
            _ if pr.lifecycle == Lifecycle::Merged => RowState::Merged,
            _ if pr.lifecycle == Lifecycle::Closed => RowState::Closed,
            Some(r) if ending(r) == Some(Ending::Cancelled) => RowState::Cancelled,
            Some(r) if r.status == RunStatus::Failed => RowState::Failed,
            _ if !open.is_empty() => RowState::Findings(open[0].severity, open.len()),
            _ if posted => RowState::Posted,
            Some(_) => RowState::Clean,
            None => RowState::New,
        };
        let repo = pr.repository.to_string();
        match groups.last_mut() {
            Some((r, rows)) if *r == repo => rows.push(Row { pr, state }),
            _ => groups.push((repo, vec![Row { pr, state }])),
        }
    }
    groups
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

/// Where the connection stands, as the window lays itself out: nothing to show, a skeleton, or the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// No daemon answers, and no snapshot ever came.
    Offline,
    /// Dialing, or waiting for the first snapshot.
    Connecting,
    /// A snapshot exists; the queue and the strip read it.
    Ready,
}

pub fn stage(m: &Model) -> Stage {
    if m.diag.last_snapshot_ms.is_some() || !m.domain.prs.is_empty() {
        return Stage::Ready;
    }
    match m.link {
        Link::Connecting { .. } | Link::Negotiating | Link::Syncing | Link::Resyncing | Link::Live => Stage::Connecting,
        _ => Stage::Offline,
    }
}

fn one(n: usize) -> String {
    match n {
        1 => "One".into(),
        2 => "Two".into(),
        3 => "Three".into(),
        n => n.to_string(),
    }
}

/// The title bar's sentence: what waits on the reader, in two sentences at most.
pub fn summary(m: &Model) -> String {
    match stage(m) {
        Stage::Offline => return "Not connected to the reviewer.".into(),
        Stage::Connecting => return "Connecting to the reviewer…".into(),
        Stage::Ready => {}
    }
    let cards = cards(m);
    let running = running(m).len();
    let mut parts: Vec<String> = Vec::new();
    let approvals = cards.iter().filter(|c| matches!(c, Card::Approve { .. })).count();
    if approvals > 0 {
        parts.push(format!(
            "{} {} waiting on you.",
            one(approvals),
            if approvals == 1 { "review is" } else { "reviews are" }
        ));
    }
    if let Some(Card::Findings { pr, findings }) = cards.iter().find(|c| matches!(c, Card::Findings { .. })) {
        let (_, word) = crate::tokens::severity_mark(findings[0].severity);
        let article = if matches!(findings[0].severity, Severity::Unknown) { "an" } else { "a" };
        parts.push(format!("#{} has {article} {} finding.", pr.number, word.to_lowercase()));
    }
    if let Some(Card::Failed { pr, .. }) =
        cards.iter().find(|c| matches!(c, Card::Failed { run, .. } if ending(run) != Some(Ending::Cancelled)))
    {
        parts.push(format!("The review of #{} failed.", pr.number));
    }
    if let Some(Card::Setup { .. }) = cards.first() {
        parts.insert(0, "Setup needs you before reviews can run.".into());
    }
    if parts.is_empty() {
        parts.push(match (cards.len(), running) {
            (0, 0) => format!("You're clear · {} reviewed.", count(done(m).len(), "PR")),
            (0, r) => format!("You're clear · {} running.", count(r, "review")),
            (n, _) => format!("{} waiting on you.", count(n, "decision")),
        });
    } else if running > 0 && parts.len() < 2 {
        parts.push(format!("{} {} running.", one(running), if running == 1 { "review is" } else { "reviews are" }));
    }
    parts.truncate(2);
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{Card, RowState, Tier, cards, count, done, rows, run_words, running, summary};
    use crate::script::{parse, play};
    use pr_review_tui::model::{Look, Model, Screen};
    use pr_review_tui::protocol::{Phase, Severity};
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
        assert_eq!(summary(&m), "One review is waiting on you. #1287 has a critical finding.");
    }

    #[test]
    fn a_daemon_that_grants_no_command_reads_as_read_only() {
        let mut m = at(20_000, "");
        assert!(!super::read_only(&m));
        assert_eq!(crate::overlay::read_only_notice(&m, "Can't start: the daemon does not grant it"), None);
        m.server.as_mut().unwrap().capabilities.clear();
        assert!(super::read_only(&m));
        assert_eq!(
            crate::overlay::read_only_notice(&m, "Can't start: the daemon does not grant it").as_deref(),
            Some("Read-only daemon: starting reviews is off. Restart it with --allow-commands.")
        );
    }

    #[test]
    fn counts_agree_with_their_number() {
        assert_eq!(count(1, "step"), "1 step");
        assert_eq!(count(0, "finding"), "0 findings");
        assert_eq!(count(7, "inline comment"), "7 inline comments");
        assert_eq!(run_words(pr_review_tui::protocol::RunStatus::Running), "Review started");
    }

    #[test]
    fn a_posted_review_leaves_the_queue_for_done() {
        let m = at(30_000, "wait:20000,pr:1291,publish,wait:100,confirm");
        assert!(!shape(&m).iter().any(|(_, n)| *n == 1291), "{:?}", shape(&m));
        assert!(done(&m).iter().any(|(p, _)| p.number == 1291));
    }

    #[test]
    fn setup_is_not_counted_as_a_decision() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/setup.ndjson");
        let mut m = Model::new("test", Look::default(), Screen::Review, None);
        play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &[], 1_000).unwrap();
        let cards = cards(&m);
        assert!(matches!(cards.first(), Some(Card::Setup { .. })));
        assert_eq!(super::decisions(&cards), cards.len() - 1);
        assert_eq!(super::decisions(&super::cards(&at(20_000, ""))), 4);
    }

    fn run_of(m: &Model, number: u64) -> pr_review_tui::protocol::Run {
        let pr = m.domain.prs.values().find(|p| p.number == number).unwrap();
        m.domain.latest_run(&pr.id).unwrap().clone()
    }

    #[test]
    fn the_gauge_never_claims_a_post_the_run_did_not_make() {
        use super::Mark::{Done, Now, Skipped, Waiting};
        let m = at(20_000, "");
        assert_eq!(super::marks(&m, &run_of(&m, 1287)), [Done, Done, Done, Done, Done, Skipped]);
        assert_eq!(super::marks(&m, &run_of(&m, 1291)), [Done, Done, Done, Done, Done, Now]);
        assert_eq!(super::marks(&m, &run_of(&m, 338)), [Done, Done, Done, Now, Waiting, Waiting]);
        let posted = at(30_000, "wait:20000,pr:1291,publish,wait:100,confirm");
        assert_eq!(super::marks(&posted, &run_of(&posted, 1291)), [Done; 6]);
    }

    #[test]
    fn a_finding_row_and_its_page_count_the_same_evidence() {
        let mut m = at(20_000, "wait:19000,pr:1287,finding:1,finding:1,finding:3,finding:3");
        assert!(m.details.len() >= 2, "the playback answers finding.get");
        for (id, d) in &m.details {
            let f = &m.domain.findings[id];
            assert_eq!((f.evidence_count as usize, super::evidence_count(&m, f)), (d.evidence.len(), d.evidence.len()));
        }
        let id = m.details.keys().next().unwrap().clone();
        m.details.get_mut(&id).unwrap().evidence.clear();
        assert_eq!(super::evidence_count(&m, &m.domain.findings[&id]), 0, "the loaded detail wins over the summary");
    }

    #[test]
    fn a_cancelled_review_reads_as_cancelled_and_starts_again() {
        use super::Mark::{Done, Stopped, Waiting};
        let m = at(25_000, "wait:20000,pr:338,cancel,wait:100,confirm,wait:2000");
        let r = run_of(&m, 338);
        assert_eq!(super::ending(&r), Some(super::Ending::Cancelled));
        assert_eq!(super::redo(&r), pr_review_tui::keymap::Mutation::Start);
        assert_eq!(super::marks(&m, &r), [Done, Done, Done, Stopped, Waiting, Waiting]);
        let row = rows(&m).into_iter().flat_map(|(_, r)| r).find(|r| r.pr.number == 338).unwrap().state;
        assert_eq!((row, row.word()), (RowState::Cancelled, "Cancelled"));
        assert!(!summary(&m).contains("#338"), "a cancel is not a failure: {}", summary(&m));
        let failed = at(20_000, "");
        assert_eq!(super::ending(&run_of(&failed, 345)), Some(super::Ending::TooLarge));
    }

    #[test]
    fn the_sidebar_groups_every_pr_by_repository_with_its_state() {
        let m = at(20_000, "");
        let got: Vec<(String, Vec<(u64, RowState)>)> = rows(&m)
            .into_iter()
            .map(|(repo, rows)| (repo, rows.iter().map(|r| (r.pr.number, r.state)).collect()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "acme/api".to_string(),
                    vec![(345, RowState::Failed), (341, RowState::Merged), (338, RowState::Running(Phase::Verify))]
                ),
                (
                    "acme/shop".to_string(),
                    vec![
                        (1296, RowState::New),
                        (1291, RowState::Parked),
                        (1287, RowState::Findings(Severity::Critical, 4))
                    ]
                ),
            ]
        );
    }
}
