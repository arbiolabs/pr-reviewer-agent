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
        ["accounts", "burst", "duplicate", "epoch-change", "gap", "hostile", "normal", "showcase", "superseded"]
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
    assert_eq!(sent, ["hello", "snapshot.get"]);
    assert_eq!(m.link, Link::Live);
}

#[test]
fn gap_requests_one_fresh_snapshot_and_waits_for_it() {
    let (m, sent) = replay("gap");
    assert_eq!(sent, ["hello", "snapshot.get", "snapshot.get"]);
    assert_eq!(m.link, Link::Resyncing);
}

#[test]
fn epoch_change_reloads_and_goes_live_on_the_new_epoch() {
    let (m, sent) = replay("epoch-change");
    assert_eq!(sent, ["hello", "snapshot.get", "snapshot.get"]);
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
    let ask = |req: serde_json::Value| playback::answer(&fixtures, &[], &serde_json::from_value(req).unwrap()).unwrap();
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
