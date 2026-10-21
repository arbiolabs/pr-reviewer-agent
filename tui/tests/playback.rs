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
    assert_eq!(m.domain.findings.len(), 7);
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
            let Some((i, _)) = p.next() else { return };
            replies.push(p.play(i));
        }
        for r in replies {
            fx.extend(update(m, Msg::Net(NetEvent::Message(r))));
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
        ("review.start", 1287, &['s', 'y']),
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
