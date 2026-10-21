//! Each playback script through the real reducer ends in the state its `.expect.json` describes.

mod common;

use std::time::{Duration, Instant};

use common::*;
use pr_review_tui::model::{ACTIVITY_CAP, Link};
use pr_review_tui::protocol::ClientMessage;
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

/// Feeds a script to a fresh model and returns it with the messages the client tried to send.
fn replay(name: &str) -> (pr_review_tui::model::Model, Vec<&'static str>) {
    let mut m = model();
    let mut sent = Vec::new();
    let mut record = |fx: Vec<Effect>| {
        for e in fx {
            if let Effect::Send(c) = e {
                sent.push(match c {
                    ClientMessage::Hello { .. } => "hello",
                    ClientMessage::SnapshotGet { .. } => "snapshot.get",
                    ClientMessage::Resume { .. } => "resume",
                    ClientMessage::FindingGet { .. } => "finding.get",
                    _ => "other",
                });
            }
        }
    };
    record(update(&mut m, Msg::Net(NetEvent::Opened)));
    for msg in script(name) {
        record(update(&mut m, Msg::Net(NetEvent::Message(msg))));
    }
    (m, sent)
}

#[test]
fn every_script_reaches_its_expected_state() {
    let mut names: Vec<String> = std::fs::read_dir(protocol_dir().join("playback"))
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.strip_suffix(".ndjson").map(String::from))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "accounts",
            "approval",
            "burst",
            "duplicate",
            "epoch-change",
            "gap",
            "hostile",
            "normal",
            "showcase",
            "superseded"
        ]
    );
    for name in names {
        let (m, _) = replay(&name);
        assert_eq!(summary(&m), expect(&name), "{name}");
    }
}

#[test]
fn accounts_shows_the_new_selection_only_once_the_event_arrives() {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    let msgs = script("accounts");
    let selected = |m: &pr_review_tui::model::Model| {
        m.domain.accounts.as_ref().unwrap().iter().filter(|a| a.selected).map(|a| a.id.to_string()).collect::<Vec<_>>()
    };
    for msg in &msgs[..3] {
        update(&mut m, Msg::Net(NetEvent::Message(msg.clone())));
    }
    assert_eq!(selected(&m), Vec::<String>::new(), "accepted alone must not change the selection");
    update(&mut m, Msg::Net(NetEvent::Message(msgs[3].clone())));
    assert_eq!(selected(&m), ["github.com/octo-work"]);
}

#[test]
fn normal_negotiates_then_snapshots_once() {
    let (m, sent) = replay("normal");
    assert_eq!(sent, ["hello", "snapshot.get", "finding.get"], "the landing PR prefetches its finding");
    assert_eq!(m.link, Link::Live);
}

#[test]
fn gap_requests_one_fresh_snapshot_and_waits_for_it() {
    let (m, sent) = replay("gap");
    assert_eq!(sent, ["hello", "snapshot.get", "finding.get", "snapshot.get"]);
    assert_eq!(m.link, Link::Resyncing);
}

#[test]
fn epoch_change_reloads_and_goes_live_on_the_new_epoch() {
    let (m, sent) = replay("epoch-change");
    assert_eq!(sent, ["hello", "snapshot.get", "finding.get", "snapshot.get"]);
    assert_eq!(m.link, Link::Live);
}

#[test]
fn burst_is_fast_and_keeps_at_most_the_activity_cap() {
    let start = Instant::now();
    let (m, _) = replay("burst");
    let took = start.elapsed();
    assert_eq!(m.domain.tool_activity_count, 5_000);
    assert_eq!(m.domain.activity.len(), ACTIVITY_CAP);
    assert_eq!(m.domain.activity.back().map(|a| a.sequence), Some(5_010));
    // Budget: 5,000 events through the reducer in under 500 ms, even unoptimized.
    assert!(took < Duration::from_millis(500), "burst took {took:?}");
}

#[test]
fn showcase_answers_every_finding_with_quoted_code_and_a_hunk_holding_its_line() {
    use pr_review_tui::playback;
    use pr_review_tui::protocol::{EvidenceKind, ServerMessage};
    use serde_json::json;

    let (m, _) = replay("showcase");
    let fixtures = protocol_dir().join("fixtures");
    let ask =
        |req: serde_json::Value| playback::answer(&fixtures, &[], &serde_json::from_value(req).unwrap()).remove(0);
    assert_eq!(m.domain.findings.len(), 8);
    for f in m.domain.findings.values() {
        let ServerMessage::FindingDetail { finding: d, .. } =
            ask(json!({ "type": "finding.get", "requestId": "r", "findingId": f.id.as_str() }))
        else {
            panic!("{}: no detail", f.title)
        };
        let code = d.evidence.iter().find(|e| e.kind == EvidenceKind::CodeRef).expect("a code_ref");
        let (start, end) = (code.start.unwrap(), code.end.unwrap());
        assert_eq!(code.text.as_str().lines().count() as u32, end - start + 1, "{}", f.title);

        let run = &m.domain.runs[&f.run_id];
        let req = json!({ "type": "diff.get", "requestId": "r", "prId": f.pr_id.as_str(),
            "baseSha": run.base_sha.as_str(), "headSha": run.head_sha.as_str() });
        let ServerMessage::DiffPage(page) = ask(req) else { panic!("{}: no diff", f.title) };
        let line = f.line.unwrap().get();
        let file = page.files.iter().find(|d| d.path == f.file).expect("the finding's file");
        let row = file.hunks.iter().flat_map(|h| &h.lines).find(|l| l.new == Some(line)).expect("the finding's line");
        assert_eq!(code.path.as_ref(), Some(&f.file), "{}", f.title);
        assert!((start..=end).contains(&line), "{}: the first code_ref quotes the flagged line", f.title);
        assert_eq!(code.text.as_str().lines().nth((line - start) as usize), Some(row.text.as_str()), "{}", f.title);
    }
}

#[test]
fn r_in_the_showcase_resyncs_from_the_played_snapshot_and_replays_what_followed() {
    use crossterm::event::KeyCode;
    use pr_review_tui::model::Look;
    let before = Sim::new(Look::default()).play("showcase", 30);
    let after = Sim::new(Look::default()).play("showcase", 30).key(KeyCode::Char('r'));
    assert_eq!(after.m.link, Link::Live, "r leaves the client live, not resyncing forever");
    assert_eq!(summary(&after.m)["runs"], summary(&before.m)["runs"]);
    assert_eq!(summary(&after.m)["findings"], summary(&before.m)["findings"]);
    assert_eq!(after.m.stream.last_applied, before.m.stream.last_applied);
}

/// Plays every line the script would play now, answering the client the way `--playback` does.
fn drive(p: &mut pr_review_tui::playback::Player, m: &mut pr_review_tui::model::Model, mut fx: Vec<Effect>) {
    loop {
        let mut replies = Vec::new();
        for e in fx.drain(..) {
            if let Effect::Send(req) = e {
                replies.extend(p.answer(&req));
            }
        }
        if replies.is_empty() {
            let Some((i, _)) = p.upcoming() else { return };
            replies.push(p.play(i));
        }
        for r in replies {
            fx.extend(update(m, Msg::Net(NetEvent::Message(r))));
        }
    }
}

/// `drive`, counting the messages it hands the client; a resync that never converges runs past `limit`.
fn drive_bounded(
    p: &mut pr_review_tui::playback::Player,
    m: &mut pr_review_tui::model::Model,
    mut fx: Vec<Effect>,
    limit: usize,
    name: &str,
) -> usize {
    let mut delivered = 0;
    loop {
        let mut replies = Vec::new();
        for e in fx.drain(..) {
            if let Effect::Send(req) = e {
                replies.extend(p.answer(&req));
            }
        }
        if replies.is_empty() {
            let Some((i, _)) = p.upcoming() else { return delivered };
            replies.push(p.play(i));
        }
        for r in replies {
            delivered += 1;
            assert!(delivered <= limit, "{name} still resyncing after {limit} messages: {:?}", m.link);
            fx.extend(update(m, Msg::Net(NetEvent::Message(r))));
        }
    }
}

#[test]
fn every_script_played_by_the_player_settles_live_and_r_settles_again() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pr_review_tui::playback::{Player, load};
    for name in [
        "accounts",
        "approval",
        "burst",
        "duplicate",
        "epoch-change",
        "gap",
        "hostile",
        "normal",
        "showcase",
        "superseded",
    ] {
        let mut p = Player::new(load(&protocol_dir().join(format!("playback/{name}.ndjson"))).unwrap());
        let limit = 2 * script(name).len() + 20;
        let mut m = model();
        let fx = update(&mut m, Msg::Net(NetEvent::Opened));
        drive_bounded(&mut p, &mut m, fx, limit, name);
        assert_eq!(m.link, Link::Live, "{name}");
        if name == "gap" {
            assert_eq!(m.stream.last_applied, 14, "the resync caught up past the gap");
        }
        let fx = update(&mut m, Msg::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)));
        drive_bounded(&mut p, &mut m, fx, limit, name);
        assert_eq!(m.link, Link::Live, "{name} after r");
    }
}

#[test]
fn every_script_played_by_the_player_reaches_its_expected_state() {
    use pr_review_tui::playback::{Player, load};
    use pr_review_tui::protocol::{Command, CommandRequest, RequestId, RunId, RunParams};
    let mut names: Vec<String> = std::fs::read_dir(protocol_dir().join("playback"))
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.strip_suffix(".ndjson").map(String::from))
        .collect();
    names.sort();
    assert_eq!(names.len(), 10);
    for name in names {
        let path = protocol_dir().join(format!("playback/{name}.ndjson"));
        let mut gates: Vec<String> = load(&path).unwrap().steps.into_iter().filter_map(|s| s.after).collect();
        gates.dedup();
        let mut p = Player::new(load(&path).unwrap());
        let mut m = model();
        let mut fx = update(&mut m, Msg::Net(NetEvent::Opened));
        // The reader acts each time the script waits for them, in the order the script expects.
        for gate in gates.into_iter().map(Some).chain([None]) {
            drive(&mut p, &mut m, std::mem::take(&mut fx));
            let Some(gate) = gate else { break };
            let run_id = RunId::new(gate.split(':').nth(1).unwrap_or("-"));
            let req = CommandRequest {
                request_id: RequestId::new("reader"),
                idempotency_key: gate,
                command: Command::Cancel(RunParams { run_id }),
            };
            for r in p.answer(&ClientMessage::Command(req)) {
                fx.extend(update(&mut m, Msg::Net(NetEvent::Message(r))));
            }
        }
        let (got, want) = (summary(&m), expect(&name));
        if want["resyncRequested"] == true {
            // The raw feed leaves its snapshot.get unanswered; the player answers it, so the client catches up.
            assert_eq!(m.link, Link::Live, "{name}");
            assert_eq!((&got["prs"], &got["streamEpoch"]), (&want["prs"], &want["streamEpoch"]), "{name}");
        } else {
            assert_eq!(got, want, "{name}");
        }
    }
}

#[test]
fn every_command_the_showcase_grants_plays_a_scripted_outcome() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pr_review_tui::model::{Modal, Screen};
    use pr_review_tui::playback::{Player, load};
    use pr_review_tui::protocol::PublicationStatus;
    let mut p = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m = model();
    let fx = update(&mut m, Msg::Net(NetEvent::Opened));
    drive(&mut p, &mut m, fx);
    let key = |c: char| Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    // The PR each command acts on in the showcase, and the keys that send it.
    let plays: [(&str, u64, &[char]); 5] = [
        ("review.publish", 1291, &['p', 'y']),
        ("review.start", 1296, &['s', 'y']),
        ("review.cancel", 338, &['c', 'y']),
        ("review.retry", 345, &['R', 'y']),
        ("account.select", 0, &['5', 'j', '\n', 'y']),
    ];
    let mut granted: Vec<String> = m.server.as_ref().unwrap().capabilities.iter().map(|c| c.to_string()).collect();
    let mut scripted: Vec<String> = plays.iter().map(|p| p.0.to_string()).collect();
    granted.sort();
    scripted.sort();
    assert_eq!(granted, scripted, "every capability the showcase grants has a scripted outcome below");
    for (cap, number, keys) in plays {
        m.ui.modals.clear();
        m.ui.screen = Screen::Review;
        if let Some(pr) = m.domain.prs.values().find(|p| p.number == number) {
            m.ui.selected_pr = Some(pr.id.clone());
        }
        for &k in keys {
            let msg = if k == '\n' { Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) } else { key(k) };
            let fx = update(&mut m, msg);
            drive(&mut p, &mut m, fx);
        }
        match cap {
            "review.publish" => {
                let run = m.domain.latest_run(m.ui.selected_pr.as_ref().unwrap()).unwrap().id.clone();
                assert_eq!(m.domain.publications[&run].status, PublicationStatus::Published, "{cap}");
            }
            _ => {
                assert!(
                    !matches!(m.top_modal(), Some(Modal::Confirm { .. })),
                    "{cap} never settled: {:?}",
                    m.top_modal()
                );
                assert!(m.notice.as_ref().is_some_and(|n| !n.text.contains("rejected")), "{cap}: {:?}", m.notice);
            }
        }
    }
}

#[test]
fn the_showcases_started_retried_and_running_runs_reach_an_end_and_a_reviewed_head_is_refused() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pr_review_tui::model::{ConfirmState, Modal, Screen};
    use pr_review_tui::playback::{Player, load};
    use pr_review_tui::protocol::{ErrorCode, RunStatus};
    let mut p = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m = model();
    let fx = update(&mut m, Msg::Net(NetEvent::Opened));
    drive(&mut p, &mut m, fx);
    let latest = |m: &pr_review_tui::model::Model, number: u64| {
        let pr = m.domain.prs.values().find(|p| p.number == number).unwrap().id.clone();
        m.domain.latest_run(&pr).map(|r| (r.status, r.id.as_str().to_string()))
    };
    assert_eq!(latest(&m, 338).unwrap().0, RunStatus::Completed, "#338 reaches an end when nobody cancels it");
    let mut keys = |m: &mut pr_review_tui::model::Model, number: u64, keys: &str| {
        m.ui.modals.clear();
        m.ui.screen = Screen::Review;
        m.ui.selected_pr = m.domain.prs.values().find(|p| p.number == number).map(|p| p.id.clone());
        for c in keys.chars() {
            let fx = update(m, Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
            drive(&mut p, m, fx);
        }
    };
    let refused = |m: &pr_review_tui::model::Model| match m.top_modal() {
        Some(Modal::Confirm { state: ConfirmState::Refused { code: ErrorCode::Conflict, message }, .. }) => {
            message.to_string()
        }
        other => panic!("{other:?}"),
    };
    let before = latest(&m, 1287).unwrap();
    for _ in 0..2 {
        keys(&mut m, 1287, "sy");
        let why = refused(&m);
        assert!(why.starts_with("#1287 at ") && why.ends_with(" is already reviewed"), "{why:?}");
        assert_eq!(latest(&m, 1287).unwrap(), before, "a refused start plays nothing");
    }
    assert_eq!(latest(&m, 1296), None, "#1296 is tracked and never reviewed");
    keys(&mut m, 1296, "sy");
    assert_eq!(latest(&m, 1296).map(|r| r.0), Some(RunStatus::Completed), "{:?} {:?}", m.notice, m.top_modal());
    keys(&mut m, 345, "Ry");
    assert_eq!(latest(&m, 345).unwrap().0, RunStatus::Completed, "the retried run");
    // The reader's own start replays as the daemon replays it: accepted for the run it made, which has ended.
    keys(&mut m, 1296, "sy");
    assert_eq!(m.top_modal(), None);
    let why = m.notice.as_ref().unwrap().text.clone();
    assert!(why.starts_with("Can't start: #1296 at ") && why.ends_with(" is already reviewed"), "{why:?}");
    for number in [1287, 341] {
        keys(&mut m, number, "sy");
        let why = refused(&m);
        assert!(why.starts_with(&format!("#{number} at ")) && why.ends_with(" is already reviewed"), "{why:?}");
    }
    let mut p2 = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m2 = model();
    let fx = update(&mut m2, Msg::Net(NetEvent::Opened));
    drive(&mut p2, &mut m2, fx);
    m2.ui.screen = Screen::Review;
    m2.ui.selected_pr = m2.domain.prs.values().find(|p| p.number == 345).map(|p| p.id.clone());
    for c in ['s', 'y'] {
        let fx = update(&mut m2, Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        drive(&mut p2, &mut m2, fx);
    }
    assert!(refused(&m2).ends_with("failed its review; retry it instead"), "a failed head points at R");
}

#[test]
fn a_cancelled_showcase_run_stays_cancelled_while_its_scripted_lines_drop() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pr_review_tui::model::Screen;
    use pr_review_tui::playback::{Player, load};
    use pr_review_tui::protocol::RunStatus;
    use pr_review_tui::protocol::{Event, ServerMessage};
    let mut p = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m = model();
    // The handshake and the first few lines, then a cancel of #338 while it is still analyzing.
    let fx = update(&mut m, Msg::Net(NetEvent::Opened));
    for _ in 0..12 {
        let (i, _) = p.upcoming().unwrap();
        let msg = p.play(i);
        let fx = update(&mut m, Msg::Net(NetEvent::Message(msg)));
        for e in fx {
            if let Effect::Send(req) = e {
                for r in p.answer(&req) {
                    update(&mut m, Msg::Net(NetEvent::Message(r)));
                }
            }
        }
    }
    drop(fx);
    let pr = m.domain.prs.values().find(|p| p.number == 338).unwrap().id.clone();
    m.ui.screen = Screen::Review;
    m.ui.selected_pr = Some(pr.clone());
    update(&mut m, Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
    let fx = update(&mut m, Msg::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)));
    drive(&mut p, &mut m, fx);
    assert_eq!(m.domain.latest_run(&pr).unwrap().status, RunStatus::Failed, "cancelled");
    let run = m.domain.latest_run(&pr).unwrap().id.clone();
    let after_cancel: Vec<String> = p
        .played
        .iter()
        .filter_map(|m| match m {
            ServerMessage::Event(Event::Sequenced(e)) if e.run_id.as_ref() == Some(&run) => {
                Some(format!("{:?}", e.body))
            }
            _ => None,
        })
        .skip_while(|b| !b.contains("Failed"))
        .collect();
    assert_eq!(after_cancel.len(), 2, "the cancel's two lines, and none of the run's scripted end: {after_cancel:#?}");
}

/// The playback's answer to a start of PR `number` at its head, under its own key or `key`, and that head, short.
fn start_answer(
    p: &mut pr_review_tui::playback::Player,
    m: &pr_review_tui::model::Model,
    number: u64,
    key: Option<&str>,
) -> (pr_review_tui::protocol::CommandOutcome, String) {
    use pr_review_tui::protocol::{Command, CommandRequest, RequestId, ServerMessage, StartParams};
    let pr = m.domain.prs.values().find(|p| p.number == number).unwrap();
    let command = Command::Start(StartParams { pr_id: pr.id.clone(), expected_head_sha: pr.head_sha.clone() });
    let idempotency_key = key.map_or_else(|| command.idempotency_key(), String::from);
    let req = ClientMessage::Command(CommandRequest { request_id: RequestId::new("t"), idempotency_key, command });
    match &p.answer(&req)[..] {
        [ServerMessage::CommandResult(r)] => (r.outcome.clone(), pr.head_sha.short().to_string()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_showcase_refuses_a_start_on_a_head_under_review_as_the_daemon_does() {
    use pr_review_tui::playback::{Player, load};
    use pr_review_tui::protocol::{CommandOutcome, ErrorCode, Event, EventBody, RunChange, ServerMessage, Text};
    let mut p = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m = model();
    let fx = update(&mut m, Msg::Net(NetEvent::Opened));
    drive(&mut p, &mut m, fx);
    let conflict =
        |message: String| CommandOutcome::Rejected { code: ErrorCode::Conflict, message: Text::new(&message) };
    let (outcome, head) = start_answer(&mut p, &m, 1291, None);
    assert_eq!(outcome, conflict(format!("#1291 at {head} is waiting for your approval")));
    let (accepted, _) = start_answer(&mut p, &m, 1296, None);
    let (i, _) = p.upcoming().unwrap();
    let ServerMessage::Event(Event::Sequenced(e)) = p.play(i) else { panic!("run.started plays first") };
    let EventBody::Run(RunChange::Started, run) = e.body else { panic!("{:?}", e.body) };
    let operation = Text::new(&format!("review.start:{}", run.id));
    assert_eq!(accepted, CommandOutcome::Accepted { operation_id: operation.clone() }, "the run it starts");
    let (outcome, _) = start_answer(&mut p, &m, 1296, None);
    assert_eq!(outcome, CommandOutcome::Accepted { operation_id: operation }, "its own key replays");
    let (outcome, head) = start_answer(&mut p, &m, 1296, Some("another client"));
    assert_eq!(outcome, conflict(format!("#1296 at {head} is already being reviewed")));
}
