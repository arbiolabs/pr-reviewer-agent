mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{Link, Modal, Model, Pane, Screen};
use pr_review_tui::protocol::{ClientMessage, Phase, PrId, RunStatus, ServerMessage};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

fn live() -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read("fixtures/server.welcome.json"));
    send(&mut m, &read("fixtures/server.snapshot.json"));
    assert_eq!(m.link, Link::Live);
    m
}

/// Inbox -> #184 (third, oldest change) -> Enter: the Review screen with finding f-1 selected.
fn review_184() -> Model {
    let mut m = live();
    press(&mut m, KeyCode::Char('j'));
    press(&mut m, KeyCode::Char('j'));
    press(&mut m, KeyCode::Enter);
    assert_eq!((m.ui.screen, m.ui.focus), (Screen::Review, Pane::Main));
    assert_eq!(m.ui.selected_finding.as_ref().map(|f| f.as_str()), Some("f-1"));
    m
}

fn press(m: &mut Model, code: KeyCode) -> Vec<Effect> {
    update(m, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn send(m: &mut Model, json: &str) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(ServerMessage::parse(json).unwrap())))
}

fn pr_event(seq: u64, number: u64, title: &str) -> String {
    format!(
        r#"{{"type":"event","streamEpoch":"epoch-1","sequence":{seq},"prevSequence":{prev},"occurredAt":"2026-10-20T13:00:00Z","prId":"acme/shop#{number}","eventType":"pr.updated","data":{{"id":"acme/shop#{number}","repository":"acme/shop","number":{number},"title":"{title}","headSha":"{number}0","baseSha":"b","lifecycle":"ACTIVE"}}}}"#,
        prev = seq - 1
    )
}

#[test]
fn selection_follows_the_id_across_a_reorder() {
    let mut m = live();
    let order = |m: &Model| m.inbox().iter().map(|p| p.number).collect::<Vec<_>>();
    assert_eq!(order(&m), [186, 185, 184]);
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));

    send(&mut m, &pr_event(109, 184, "Booking API v2"));
    assert_eq!(order(&m), [184, 186, 185]);
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));

    // Moving down from the re-sorted position lands on the PR now below it, not on a stale index.
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#186")));
}

#[test]
fn the_top_modal_takes_keys_and_esc_pops_one_at_a_time() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Enter);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { .. })));
    assert!(matches!(&fx[..], [Effect::Send(ClientMessage::FindingGet { .. })]), "{fx:?}");

    press(&mut m, KeyCode::Char('?'));
    assert_eq!(m.top_modal(), Some(&Modal::Help));
    let before = m.ui.selected_finding.clone();
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(m.ui.selected_finding, before);

    press(&mut m, KeyCode::Esc);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { scroll: 0, .. })));
    press(&mut m, KeyCode::Char('j'));
    assert!(matches!(m.top_modal(), Some(Modal::Finding { scroll: 1, .. })));
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.top_modal(), None);
    assert_eq!(m.ui.screen, Screen::Review);
}

#[test]
fn a_focused_text_input_makes_every_shortcut_inert() {
    let mut m = live();
    press(&mut m, KeyCode::Char('/'));
    let mut fx = Vec::new();
    for c in ['q', 's', 'R', 'c', 'p', 'y', '2', 'r', 'd'] {
        fx.extend(press(&mut m, KeyCode::Char(c)));
    }
    assert_eq!(fx, []);
    assert_eq!(m.ui.screen, Screen::Inbox);
    assert_eq!(m.ui.pr_filter, "qsRcpy2rd");
    assert!(m.inbox().is_empty());

    press(&mut m, KeyCode::Esc);
    assert_eq!(m.top_modal(), None);
    assert_eq!(m.ui.pr_filter, "");
    assert_eq!(m.inbox().len(), 3);

    // Outside the input, q quits and only quits: it sends nothing to the server.
    assert_eq!(press(&mut m, KeyCode::Char('q')), [Effect::Quit]);
}

#[test]
fn search_filters_and_keeps_a_matching_selection() {
    let mut m = live();
    press(&mut m, KeyCode::Char('/'));
    for c in "cache".chars() {
        press(&mut m, KeyCode::Char(c));
    }
    press(&mut m, KeyCode::Enter);
    assert_eq!(m.inbox().iter().map(|p| p.number).collect::<Vec<_>>(), [185]);
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));
    assert_eq!(m.ui.pr_filter, "cache");
}

#[test]
fn y_and_write_shortcuts_never_send_anything() {
    let mut m = live();
    for screen in Screen::ALL {
        m.ui.screen = screen;
        assert_eq!(press(&mut m, KeyCode::Char('y')), []);
        assert_eq!(press(&mut m, KeyCode::Char('s')), []);
    }
    assert_eq!(
        m.notice.as_ref().map(|n| n.text.as_str()),
        Some("review.start unavailable: the server grants no write capability (read-only)")
    );
}

#[test]
fn a_duplicate_event_is_applied_once() {
    let mut m = live();
    let activity = r#"{"type":"event","streamEpoch":"epoch-1","sequence":109,"prevSequence":108,"occurredAt":"t","prId":"acme/shop#184","runId":"rev-184-a","eventType":"tool.activity","data":{"tool":"read_file","summary":"a.ts"}}"#;
    assert_eq!(send(&mut m, activity), []);
    assert_eq!(send(&mut m, activity), []);
    assert_eq!(m.domain.tool_activity_count, 1);
    assert_eq!(m.domain.activity.len(), 1);
    assert_eq!(m.stream.last_applied, 109);
    assert_eq!(m.diag.resyncs, 0);
}

#[test]
fn a_v2_welcome_is_incompatible_and_stops_the_client_dialing() {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    let fx = send(&mut m, &read("fixtures/invalid/server.welcome.version-2.json"));
    assert_eq!(fx, [Effect::Close]);
    assert!(matches!(&m.link, Link::Incompatible { supported, .. } if supported == &[2]));
    // Terminal: a later snapshot is not applied.
    send(&mut m, &read("fixtures/server.snapshot.json"));
    assert!(m.domain.prs.is_empty());
}

#[test]
fn protocol_mismatch_error_is_incompatible() {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    let fx = send(&mut m, &read("fixtures/server.error.protocol-mismatch.json"));
    assert_eq!(fx, [Effect::Close]);
    assert!(matches!(&m.link, Link::Incompatible { supported, .. } if supported == &[1]));
}

#[test]
fn losing_the_connection_keeps_the_run_status_and_resumes_on_reconnect() {
    let mut m = live();
    let run = |m: &Model| {
        let r = m.domain.runs.values().find(|r| r.id.as_str() == "rev-184-a").unwrap();
        (r.phase, r.status)
    };
    assert_eq!(run(&m), (Phase::Verify, RunStatus::Running));
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "heartbeat timeout".into(), retry_in_ms: Some(250) }));
    assert!(matches!(m.link, Link::Disconnected { .. }));
    assert_eq!(run(&m), (Phase::Verify, RunStatus::Running));

    update(&mut m, Msg::Net(NetEvent::Dialing { attempt: 2 }));
    let hello = update(&mut m, Msg::Net(NetEvent::Opened));
    assert!(matches!(&hello[..], [Effect::Send(ClientMessage::Hello { protocol_version: 1, .. })]));
    let fx = send(&mut m, &read("fixtures/server.welcome.json"));
    let [Effect::Send(ClientMessage::Resume { stream_epoch, after_sequence, .. })] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!((stream_epoch.as_str(), *after_sequence), ("epoch-1", 108));
    assert_eq!(m.link, Link::Live);

    // The server cannot replay: fall back to a snapshot.
    let fx = send(&mut m, &read("fixtures/server.resync.required.json"));
    assert!(matches!(&fx[..], [Effect::Send(ClientMessage::SnapshotGet { .. })]), "{fx:?}");
    assert_eq!(m.link, Link::Resyncing);
}

#[test]
fn finding_to_diff_pins_the_findings_revision_and_lands_on_its_line() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('d'));
    let [Effect::Send(ClientMessage::DiffGet { head_sha, .. })] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(head_sha.as_str(), "4444444444444444444444444444444444444444");
    assert_eq!(m.ui.screen, Screen::Diff);

    let page = read("fixtures/server.diff.page.json").replace("req-diff", "req-3");
    send(&mut m, &page);
    let d = m.diff.as_ref().unwrap();
    assert_eq!(d.selected_file.as_ref().map(|p| p.as_str()), Some("src/booking.ts"));
    // Row 0 is the hunk header; head line 42 is the fifth row ("slot.held = true").
    assert_eq!(d.cursor, 4);
}

#[test]
fn y_confirms_only_inside_the_confirmation_modal_and_sends_the_command_once() {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(
        &mut m,
        &read("fixtures/server.welcome.json").replace("\"capabilities\": []", "\"capabilities\": [\"review.start\"]"),
    );
    send(&mut m, &read("fixtures/server.snapshot.json"));
    press(&mut m, KeyCode::Char('g'));
    assert_eq!(press(&mut m, KeyCode::Char('y')), []);

    assert_eq!(press(&mut m, KeyCode::Char('s')), []);
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { .. })), "{:?}", m.top_modal());
    let fx = press(&mut m, KeyCode::Char('y'));
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, "review.start:acme/shop#186:6666666666666666666666666666666666666666");
    assert_eq!(press(&mut m, KeyCode::Char('y')), [], "a second y must not resend");

    let rejected = read("fixtures/server.command.result.rejected.json").replace("req-cmd", c.request_id.as_str());
    send(&mut m, &rejected);
    let Some(Modal::Confirm { state, .. }) = m.top_modal() else { panic!() };
    assert_eq!(
        state,
        &pr_review_tui::model::ConfirmState::Failed(pr_review_tui::protocol::Text::new(
            "CapabilityDenied: review.start is not available in this version"
        ))
    );
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.top_modal(), None);
}
