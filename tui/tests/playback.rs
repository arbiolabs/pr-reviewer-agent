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
    assert_eq!(names, ["burst", "duplicate", "epoch-change", "gap", "hostile", "normal", "superseded"]);
    for name in names {
        let (m, _) = replay(&name);
        assert_eq!(summary(&m), expect(&name), "{name}");
    }
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
