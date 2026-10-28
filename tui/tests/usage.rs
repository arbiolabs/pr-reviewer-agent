mod common;

use common::*;
use pr_review_tui::model::Model;
use pr_review_tui::protocol::{ClientMessage, RequestId, ServerMessage, UsageBy, UsageGroupBy};
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
        [Effect::Send(ClientMessage::UsageGet { request_id, by, periods: None, group: None })] => {
            (request_id.clone(), *by)
        }
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
        let req = ClientMessage::UsageGet { request_id: RequestId::new("u-1"), by, periods: Some(3), group: None };
        match pr_review_tui::playback::answer(&dir, &[], &req).as_slice() {
            [ServerMessage::UsageReport(r)] => {
                assert_eq!((r.by, r.request_id.as_ref().map(|i| i.as_str())), (by, Some("u-1")));
                assert!(r.totals.cost > 0.0);
            }
            other => panic!("{other:?}"),
        }
    }
}

fn asked_grouped(fx: &[Effect]) -> (RequestId, UsageBy, UsageGroupBy) {
    match fx {
        [Effect::Send(ClientMessage::UsageGet { request_id, by, periods: None, group: Some(g) })] => {
            (request_id.clone(), *by, *g)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_breakdown_asks_for_its_split_over_the_same_range_and_keeps_it_across_groupings() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Week))));
    send(&mut m, &reply(UsageBy::Week, &id));
    let (id, by, group) = asked_grouped(&update(&mut m, Msg::UsageGroup(UsageGroupBy::Pr)));
    assert_eq!((by, group), (UsageBy::Week, UsageGroupBy::Pr));
    send(&mut m, &read("fixtures/server.usage.report.week.pr.json").replace("req-usage", id.as_str()));
    let r = m.usage.report.as_ref().unwrap();
    assert_eq!(r.group, Some(UsageGroupBy::Pr));
    assert!(r.groups.iter().any(|g| g.pr_id.is_some() && g.number.is_some()));
    assert_eq!(update(&mut m, Msg::Tick(10)), [], "the answer is current");
    let (_, by, group) = asked_grouped(&update(&mut m, Msg::Usage(Some(UsageBy::Month))));
    assert_eq!((by, group), (UsageBy::Month, UsageGroupBy::Pr));
}

#[test]
fn a_daemon_that_ignores_the_breakdown_is_said_so_and_not_asked_again() {
    let mut m = live();
    update(&mut m, Msg::UsageGroup(UsageGroupBy::Repo));
    let (id, ..) = asked_grouped(&update(&mut m, Msg::Usage(Some(UsageBy::Day))));
    let old = read("fixtures/server.usage.report.day.json")
        .replace("req-usage", id.as_str())
        .replace(r#""group": "time","#, "");
    assert!(!old.contains(r#""group""#));
    send(&mut m, &old);
    assert!(m.usage.error.as_ref().is_some_and(|e| e.as_str().contains("older")));
    assert_eq!(update(&mut m, Msg::Tick(10)), []);
}

#[test]
fn playback_answers_each_breakdown_with_its_recording() {
    let dir = protocol_dir().join("fixtures");
    for by in UsageBy::ALL {
        for group in UsageGroupBy::ALL {
            let req =
                ClientMessage::UsageGet { request_id: RequestId::new("u-1"), by, periods: None, group: Some(group) };
            match pr_review_tui::playback::answer(&dir, &[], &req).as_slice() {
                [ServerMessage::UsageReport(r)] => {
                    assert_eq!((r.by, r.group), (by, Some(group)));
                    assert_eq!(r.groups.is_empty(), group == UsageGroupBy::Time);
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

#[test]
fn a_reconnect_keeps_the_breakdown_on_screen_selected_and_asks_the_new_daemon_for_it() {
    let mut m = live();
    let (id, _) = asked(&update(&mut m, Msg::Usage(Some(UsageBy::Week))));
    send(&mut m, &reply(UsageBy::Week, &id));
    let (id, ..) = asked_grouped(&update(&mut m, Msg::UsageGroup(UsageGroupBy::Pr)));
    send(&mut m, &read("fixtures/server.usage.report.week.pr.json").replace("req-usage", id.as_str()));
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "gone".into(), retry_in_ms: Some(10) }));
    update(&mut m, Msg::Net(NetEvent::Opened));
    let fx = send(&mut m, &read("fixtures/server.welcome.read-only.json"));
    assert_eq!(m.usage.group, UsageGroupBy::Pr, "the reader's breakdown outlives the connection");
    let fx: Vec<_> = fx.into_iter().filter(|e| matches!(e, Effect::Send(ClientMessage::UsageGet { .. }))).collect();
    let (_, by, group) = asked_grouped(&fx);
    assert_eq!((by, group), (UsageBy::Week, UsageGroupBy::Pr));
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    By(UsageBy),
    Group(UsageGroupBy),
    Answer,
    Reconnect,
    ReviewEnded,
}

/// A daemon that answers each usage.get in the order it was asked, from the recordings, and loses what it was
/// asked when the connection drops.
struct Daemon {
    asked: Vec<ClientMessage>,
    answers: Vec<(UsageBy, Option<UsageGroupBy>, ServerMessage)>,
    welcome: ServerMessage,
    snapshot: ServerMessage,
    ended: ServerMessage,
}

impl Daemon {
    fn new() -> Daemon {
        let dir = protocol_dir().join("fixtures");
        let mut answers = vec![];
        for by in UsageBy::ALL {
            for group in UsageGroupBy::ALL.map(Some).into_iter().chain([None]) {
                let req = ClientMessage::UsageGet { request_id: RequestId::new("x"), by, periods: None, group };
                answers.push((by, group, pr_review_tui::playback::answer(&dir, &[], &req).remove(0)));
            }
        }
        let parse = |p: &str| ServerMessage::parse(&read(p)).unwrap();
        Daemon {
            asked: vec![],
            answers,
            welcome: parse("fixtures/server.welcome.read-only.json"),
            snapshot: parse("fixtures/server.snapshot.json"),
            ended: parse("fixtures/server.event.review.completed.json"),
        }
    }

    fn deliver(&mut self, m: &mut Model, msg: Msg) {
        for e in update(m, msg) {
            if let Effect::Send(c @ ClientMessage::UsageGet { .. }) = e {
                self.asked.push(c);
            }
        }
    }

    fn answer(&mut self, m: &mut Model) -> bool {
        if self.asked.is_empty() {
            return false;
        }
        let ClientMessage::UsageGet { request_id, by, group, .. } = self.asked.remove(0) else { unreachable!() };
        let Some((.., ServerMessage::UsageReport(r))) = self.answers.iter().find(|a| (a.0, a.1) == (by, group)) else {
            unreachable!()
        };
        let mut r = r.clone();
        r.request_id = Some(request_id);
        self.deliver(m, Msg::Net(NetEvent::Message(ServerMessage::UsageReport(r))));
        true
    }

    fn apply(&mut self, m: &mut Model, e: Ev) {
        match e {
            Ev::By(by) => self.deliver(m, Msg::Usage(Some(by))),
            Ev::Group(g) => self.deliver(m, Msg::UsageGroup(g)),
            Ev::Answer => {
                self.answer(m);
            }
            Ev::Reconnect => {
                self.asked.clear();
                self.deliver(m, Msg::Net(NetEvent::Lost { reason: "gone".into(), retry_in_ms: Some(10) }));
                self.deliver(m, Msg::Net(NetEvent::Opened));
                self.deliver(m, Msg::Net(NetEvent::Message(self.welcome.clone())));
                self.deliver(m, Msg::Net(NetEvent::Message(self.snapshot.clone())));
            }
            Ev::ReviewEnded => self.deliver(m, Msg::Net(NetEvent::Message(self.ended.clone()))),
        }
    }
}

#[test]
fn any_order_of_switches_replies_and_reconnects_settles_on_the_selection() {
    let alphabet = [
        Ev::By(UsageBy::Week),
        Ev::By(UsageBy::Month),
        Ev::Group(UsageGroupBy::Time),
        Ev::Group(UsageGroupBy::Pr),
        Ev::Group(UsageGroupBy::Repo),
        Ev::Answer,
        Ev::Reconnect,
        Ev::ReviewEnded,
    ];
    let mut d = Daemon::new();
    let n = 4;
    for i in 0..alphabet.len().pow(n) {
        let seq: Vec<Ev> = (0..n).map(|k| alphabet[i / alphabet.len().pow(k) % alphabet.len()]).collect();
        let mut m = live();
        d.asked.clear();
        d.apply(&mut m, Ev::By(UsageBy::Day));
        let mut want = (UsageBy::Day, UsageGroupBy::Time);
        for e in &seq {
            d.apply(&mut m, *e);
            match *e {
                Ev::By(by) => want.0 = by,
                Ev::Group(g) => want.1 = g,
                _ => {}
            }
        }
        while d.answer(&mut m) {}
        let u = &m.usage;
        let shown = u.report.as_ref().map(|r| (r.by, r.group.unwrap_or_default()));
        assert_eq!((u.by, u.group, shown), (Some(want.0), want.1, Some(want)), "{seq:?}");
        assert!(u.request.is_none() && u.error.is_none(), "{seq:?}");
    }
}
