mod common;

use common::*;
use pr_review_tui::model::Model;
use pr_review_tui::protocol::{ClientMessage, RequestId, ServerMessage, UsageBy};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

fn send(m: &mut Model, json: &str) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(ServerMessage::parse(json).unwrap())))
}

/// Live on a read-only daemon: usage.get is a read, so no capability gates it.
fn live() -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read("fixtures/server.welcome.read-only.json"));
    send(&mut m, &read("fixtures/server.snapshot.json"));
    m
}

fn asked(fx: &[Effect]) -> (RequestId, UsageBy) {
    match fx {
        [Effect::Send(ClientMessage::UsageGet { request_id, by, periods: None })] => (request_id.clone(), *by),
        other => panic!("{other:?}"),
    }
}

fn reply(by: UsageBy, id: &RequestId) -> String {
    read(&format!("fixtures/server.usage.report.{}.json", by.as_str())).replace("req-usage", id.as_str())
}

#[test]
fn opening_asks_once_and_keeps_the_answer() {
    let mut m = live();
    let (id, by) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Week))));
    assert_eq!(by, UsageBy::Week);
    assert_eq!(update(&mut m, Msg::Tick(10)), [], "one request in flight");
    send(&mut m, &reply(UsageBy::Week, &id));
    let r = m.usage.report.as_ref().unwrap();
    assert_eq!((r.by, r.buckets.len(), r.time_zone.as_str()), (UsageBy::Week, 12, "Europe/Berlin"));
    assert_eq!(update(&mut m, Msg::Tick(20)), []);
}

#[test]
fn the_tui_never_asks() {
    let mut m = live();
    assert_eq!(update(&mut m, Msg::Tick(10)), []);
}

#[test]
fn another_grouping_asks_again_and_shows_the_last_answer_until_it_comes() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Day))));
    send(&mut m, &reply(UsageBy::Day, &id));
    let (id, by) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Month))));
    assert_eq!(by, UsageBy::Month);
    assert_eq!(m.usage.report.as_ref().unwrap().by, UsageBy::Day);
    send(&mut m, &reply(UsageBy::Month, &id));
    assert_eq!(m.usage.report.as_ref().unwrap().by, UsageBy::Month);
}

#[test]
fn a_review_that_ends_refreshes_usage_on_screen_and_only_there() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Day))));
    send(&mut m, &reply(UsageBy::Day, &id));
    assert!(asked(&send(&mut m, &read("fixtures/server.event.review.completed.json"))).1 == UsageBy::Day);

    let mut closed = live();
    let (id, _) = asked(&update(&mut closed, Msg::Usage(Some(UsageBy::Day))));
    send(&mut closed, &reply(UsageBy::Day, &id));
    assert_eq!(update(&mut closed, Msg::Usage(None)), []);
    assert_eq!(send(&mut closed, &read("fixtures/server.event.review.completed.json")), []);
}

#[test]
fn an_error_or_a_timeout_shows_until_opened_again() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Day))));
    let refused = format!(
        r#"{{"type":"error","requestId":"{}","code":"CAPABILITY_DENIED","message":"usage.get is not available in this version"}}"#,
        id.as_str()
    );
    send(&mut m, &refused);
    assert_eq!(m.usage.error.as_ref().map(|e| e.as_str()), Some("usage.get is not available in this version"));
    assert_eq!(update(&mut m, Msg::Tick(10)), []);
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Day))));
    assert!(m.usage.error.is_none());
    let late = m.now_ms + m.request_timeout_ms;
    update(&mut m, Msg::Tick(late));
    assert!(m.usage.request.is_none() && m.usage.error.is_some(), "{id:?} timed out");
}

#[test]
fn a_reconnect_keeps_the_report_and_asks_the_new_daemon() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Week))));
    send(&mut m, &reply(UsageBy::Week, &id));
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "gone".into(), retry_in_ms: Some(10) }));
    update(&mut m, Msg::Net(NetEvent::Opened));
    let fx = send(&mut m, &read("fixtures/server.welcome.read-only.json"));
    assert!(m.usage.report.is_some());
    let fx: Vec<_> = fx.into_iter().filter(|e| matches!(e, Effect::Send(ClientMessage::UsageGet { .. }))).collect();
    assert_eq!(asked(&fx).1, UsageBy::Week);
}

#[test]
fn playback_answers_each_grouping_with_its_recording() {
    let dir = protocol_dir().join("fixtures");
    for by in UsageBy::ALL {
        let req = ClientMessage::UsageGet { request_id: RequestId::new("u-1"), by, periods: Some(3) };
        match pr_review_tui::playback::answer(&dir, &[], &req).as_slice() {
            [ServerMessage::UsageReport(r)] => {
                assert_eq!((r.by, r.request_id.as_ref().map(|i| i.as_str())), (by, Some("u-1")));
                assert!(r.totals.cost > 0.0);
            }
            other => panic!("{other:?}"),
        }
    }
}
