//! `review.add`: a PR the daemon doesn't track yet, sent from its URL, selected once the daemon reports it, and a
//! refusal kept where a client can show it and said in a notice.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{AddState, Adding, InputPurpose, Modal, Model, PrRef, Screen};
use pr_review_tui::protocol::{ClientMessage, Command, CommandRequest, ErrorCode, PrId, ServerMessage, Text};
use pr_review_tui::update::{Effect, Msg, NetEvent, can_add, update};

fn send(m: &mut Model, json: &str) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(ServerMessage::parse(json).unwrap())))
}

fn connected(welcome: &str) -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read(welcome));
    send(&mut m, &read("fixtures/server.snapshot.json"));
    m
}

fn sent(fx: &[Effect]) -> Vec<&CommandRequest> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Send(ClientMessage::Command(c)) => Some(c),
            _ => None,
        })
        .collect()
}

fn add(m: &mut Model, text: &str) -> Option<CommandRequest> {
    sent(&update(m, Msg::AddPr(text.into()))).first().map(|c| (*c).clone())
}

fn notice(m: &Model) -> &str {
    m.notice.as_ref().map_or("", |n| n.text.as_str())
}

const PR_UPDATED: &str = r#"{"type":"event","streamEpoch":"epoch-1","sequence":109,"prevSequence":108,"occurredAt":"2026-10-20T13:01:00Z","prId":"acme/web#9","eventType":"pr.updated","data":{"id":"acme/web#9","repository":"acme/web","number":9,"title":"Docs","headSha":"9999999999999999999999999999999999999999","baseSha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","lifecycle":"ACTIVE"}}"#;

#[test]
fn a_new_pr_is_sent_as_owner_repo_and_selected_once_its_event_arrives() {
    let mut m = connected("fixtures/server.welcome.engine.json");
    m.ui.screen = Screen::Settings;
    let c = add(&mut m, " https://github.com/acme/web/pull/9/files ").unwrap();
    assert_eq!(c.command, Command::Add(pr_review_tui::protocol::AddParams { url: "acme/web#9".into() }));
    assert_eq!(c.idempotency_key, "review.add:acme/web#9");
    assert_eq!(m.adding.as_ref().map(|a| &a.state), Some(&AddState::Sending(c.request_id.clone())));
    assert_eq!(add(&mut m, "acme/web#9"), None, "one add at a time");

    send(
        &mut m,
        &format!(
            r#"{{"type":"command.result","requestId":"{}","status":"accepted","operationId":"review.add:rev-9","prId":"acme/web#9","runId":"rev-9"}}"#,
            c.request_id
        ),
    );
    assert!(matches!(m.adding, Some(Adding { state: AddState::Landing { already: false, .. }, .. })), "{:?}", m.adding);
    send(&mut m, PR_UPDATED);
    assert_eq!(
        (m.adding.clone(), m.ui.selected_pr.clone(), m.ui.screen),
        (None, Some(PrId::new("acme/web#9")), Screen::Review)
    );
    assert_eq!(notice(&m), "Added acme/web#9; its review is starting");
}

#[test]
fn a_tracked_pr_is_selected_whether_the_client_or_the_daemon_knows_it() {
    let mut m = connected("fixtures/server.welcome.engine.json");
    assert_eq!(add(&mut m, "https://github.com/ACME/Shop/pull/185"), None, "the client knows it: nothing is sent");
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));
    assert_eq!(notice(&m), "Already tracking ACME/Shop#185; showing it");

    let c = add(&mut m, "acme/renamed#184").unwrap();
    send(
        &mut m,
        &format!(
            r#"{{"type":"command.result","requestId":"{}","status":"rejected","code":"CONFLICT","message":"acme/shop#184 is already tracked","prId":"acme/shop#184"}}"#,
            c.request_id
        ),
    );
    assert_eq!((m.adding.clone(), m.ui.selected_pr.clone()), (None, Some(PrId::new("acme/shop#184"))));
    assert_eq!(notice(&m), "Already tracking acme/renamed#184; showing it");
}

#[test]
fn a_refusal_stays_on_the_model_and_reads_as_a_notice() {
    let mut m = connected("fixtures/server.welcome.engine.json");
    let c = add(&mut m, "acme/ghost#3").unwrap();
    let why = "cannot read acme/ghost#3: it does not exist, or github.com/octo has no access to it (HTTP 404)";
    send(
        &mut m,
        &format!(
            r#"{{"type":"command.result","requestId":"{}","status":"rejected","code":"NOT_FOUND","message":"{why}"}}"#,
            c.request_id
        ),
    );
    let refused = AddState::Refused { code: Some(ErrorCode::NotFound), message: Text::new(why) };
    assert_eq!(m.adding, Some(Adding { target: PrRef::parse("acme/ghost#3").unwrap(), state: refused }));
    assert_eq!(notice(&m), format!("Can't add acme/ghost#3: {why}"));

    let again = add(&mut m, "acme/ghost#4").unwrap();
    let later = m.now_ms + m.request_timeout_ms;
    update(&mut m, Msg::Tick(later));
    assert_eq!(m.adding.as_ref().map(|a| &a.state), Some(&AddState::TimedOut), "{again:?}");
    assert_eq!(notice(&m), "No reply to adding acme/ghost#4 in time; try again");
}

#[test]
fn nothing_is_sent_for_text_that_names_no_pr_or_to_a_read_only_daemon() {
    let mut m = connected("fixtures/server.welcome.engine.json");
    for text in [
        "",
        "acme/shop",
        "https://gitlab.com/acme/shop/pull/1",
        "https://github.com/acme/shop/issues/1",
        "a b/c#1",
        "acme/shop#0",
    ] {
        assert_eq!(add(&mut m, text), None, "{text:?}");
        assert_eq!(notice(&m), "Can't add a PR: paste its GitHub URL or owner/repo#N", "{text:?}");
    }
    let mut ro = connected("fixtures/server.welcome.read-only.json");
    assert!(!can_add(&ro));
    assert_eq!(add(&mut ro, "acme/web#9"), None);
    assert_eq!(notice(&ro), "Can't add a PR: daemon is read-only");
    update(&mut ro, Msg::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)));
    assert_eq!(ro.top_modal(), None, "a opens no prompt the daemon would refuse");
}

#[test]
fn a_asks_for_the_url_and_enter_sends_it() {
    let mut m = connected("fixtures/server.welcome.engine.json");
    let key = |m: &mut Model, code| update(m, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    key(&mut m, KeyCode::Char('a'));
    assert!(matches!(m.top_modal(), Some(Modal::Input { purpose: InputPurpose::AddPr, .. })));
    for ch in "acme/web#9".chars() {
        key(&mut m, KeyCode::Char(ch));
    }
    let fx = key(&mut m, KeyCode::Enter);
    assert_eq!(sent(&fx).iter().map(|c| c.idempotency_key.as_str()).collect::<Vec<_>>(), ["review.add:acme/web#9"]);
    assert_eq!(m.top_modal(), None);
}
