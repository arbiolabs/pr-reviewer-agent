mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{ConfirmState, Identity, Link, Modal, Model, Pane, Screen};
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
fn a_new_pr_takes_its_sorted_place_and_the_cursor_stays_on_its_pr() {
    let mut m = live();
    let order = |m: &Model| m.inbox().iter().map(|p| format!("{}#{}", p.repository, p.number)).collect::<Vec<_>>();
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));
    // Another repository sorts by name; inside one, newer numbers come first.
    for (seq, repo, number) in [(109, "acme/api", 7), (110, "acme/shop", 190), (111, "acme/shop", 1)] {
        let json = pr_event(seq, number, "new").replace("acme/shop", repo);
        send(&mut m, &json);
        assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")), "after {repo}#{number}");
    }
    assert_eq!(
        order(&m),
        ["acme/api#7", "acme/shop#190", "acme/shop#186", "acme/shop#185", "acme/shop#184", "acme/shop#1"]
    );
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#186")));
}

#[test]
fn the_inbox_order_holds_still_while_prs_change_and_selection_follows_the_id() {
    let mut m = live();
    let order = |m: &Model| m.inbox().iter().map(|p| p.number).collect::<Vec<_>>();
    assert_eq!(order(&m), [186, 185, 184]);
    // The client lands on the PR with the most severe current finding.
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#184")));
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));

    // Activity on #184 does not move it: the row under the cursor stays the one the user pressed toward.
    send(&mut m, &pr_event(109, 184, "Booking API v2"));
    assert_eq!(order(&m), [186, 185, 184]);
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#185")));
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#186")));
}

#[test]
fn the_top_modal_takes_keys_and_esc_pops_one_at_a_time() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Enter);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { .. })));
    assert_eq!(sent(&fx), ["diff.get"], "the detail came with the PR; the reader adds the diff at its anchor");

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
    let [
        Effect::Send(ClientMessage::Resume { stream_epoch, after_sequence, .. }),
        Effect::Send(ClientMessage::FindingGet { .. }),
    ] = &fx[..]
    else {
        panic!("{fx:?}")
    };
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
    let [Effect::Send(ClientMessage::DiffGet { request_id, head_sha, .. })] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(head_sha.as_str(), "4444444444444444444444444444444444444444");
    assert_eq!(m.ui.screen, Screen::Diff);

    let page = read("fixtures/server.diff.page.json").replace("req-diff", request_id.as_str());
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
            "CAPABILITY_DENIED: review.start is not available in this version"
        ))
    );
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.top_modal(), None);
}

fn diff_loaded() -> Model {
    let mut m = review_184();
    press(&mut m, KeyCode::Char('d'));
    let id = m.diff.as_ref().and_then(|d| d.pending.clone()).unwrap();
    send(&mut m, &read("fixtures/server.diff.page.json").replace("req-diff", id.as_str()));
    m
}

#[test]
fn jump_to_path_and_line_selects_that_file_and_row() {
    let mut m = diff_loaded();
    press(&mut m, KeyCode::Char(':'));
    for c in "booking.ts:43".chars() {
        press(&mut m, KeyCode::Char(c));
    }
    press(&mut m, KeyCode::Enter);
    let d = m.diff.as_ref().unwrap();
    assert_eq!((d.selected_file.as_ref().map(|p| p.as_str()), d.cursor), (Some("src/booking.ts"), 5));

    press(&mut m, KeyCode::Char(':'));
    for c in "assets/logo.png".chars() {
        press(&mut m, KeyCode::Char(c));
    }
    press(&mut m, KeyCode::Enter);
    let d = m.diff.as_ref().unwrap();
    assert_eq!((d.selected_file.as_ref().map(|p| p.as_str()), d.cursor), (Some("assets/logo.png"), 0));
}

#[test]
fn reaching_the_last_file_requests_the_next_diff_page() {
    let mut m = diff_loaded();
    m.ui.focus = Pane::Sidebar;
    let mut fx = Vec::new();
    for _ in 0..3 {
        fx.extend(press(&mut m, KeyCode::Char('j')));
    }
    let [Effect::Send(ClientMessage::DiffGet { cursor, .. })] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(cursor.as_ref().map(|c| c.as_str()), Some("c-2"));
}

#[test]
fn timeline_category_filter_hides_other_events() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('4'));
    let [Effect::Send(ClientMessage::TimelineGet { request_id, .. })] = &fx[..] else { panic!("{fx:?}") };
    send(&mut m, &read("fixtures/server.timeline.page.json").replace("req-tl", request_id.as_str()));
    let shown = |m: &Model| {
        let t = m.timeline.as_ref().unwrap();
        pr_review_tui::update::timeline_rows(m, t).iter().map(|(e, _)| e.sequence.get()).collect::<Vec<_>>()
    };
    assert_eq!(shown(&m), [99, 100], "oldest first");
    press(&mut m, KeyCode::Char('v'));
    assert_eq!(shown(&m), [100]);
    press(&mut m, KeyCode::Char('v'));
    assert_eq!(shown(&m), [99]);
}

#[test]
fn a_diff_request_lost_with_the_connection_is_retried_when_reopened() {
    let mut m = review_184();
    press(&mut m, KeyCode::Char('3'));
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "eof".into(), retry_in_ms: Some(250) }));
    assert!(m.diff.as_ref().unwrap().error.is_some());
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read("fixtures/server.welcome.json"));
    press(&mut m, KeyCode::Char('1'));
    let fx = press(&mut m, KeyCode::Char('3'));
    assert!(matches!(&fx[..], [Effect::Send(ClientMessage::DiffGet { .. })]), "{fx:?}");
}

/// accounts.ndjson up to its snapshot: diego-arbio-dev (gh active) and octo-work, account.select offered.
fn accounts_live() -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    for msg in script("accounts").into_iter().take(2) {
        update(&mut m, Msg::Net(NetEvent::Message(msg)));
    }
    m
}

fn identity(m: &Model) -> String {
    match m.identity() {
        Some(Identity::Env) => "env".into(),
        Some(Identity::Selected(a)) => format!("selected {}", a.id),
        Some(Identity::GhActive(a)) => format!("active {}", a.id),
        Some(Identity::Nobody) => "nobody".into(),
        None => "unknown".into(),
    }
}

#[test]
fn account_select_waits_for_accounts_updated_before_showing_the_selection() {
    let mut m = accounts_live();
    assert_eq!(identity(&m), "active github.com/diego-arbio-dev");
    press(&mut m, KeyCode::Char('A'));
    assert_eq!(m.ui.screen, Screen::Settings);
    assert_eq!(press(&mut m, KeyCode::Char('y')), [], "y outside the modal does nothing");
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(press(&mut m, KeyCode::Enter), []);
    let Some(Modal::Confirm { prompt, .. }) = m.top_modal() else { panic!("{:?}", m.top_modal()) };
    assert_eq!(prompt, "Use octo-work on github.com for GitHub reads and publishing?");

    let fx = press(&mut m, KeyCode::Char('y'));
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    let wire = serde_json::to_value(ClientMessage::Command(c.clone())).unwrap();
    let mut want: serde_json::Value =
        serde_json::from_str(&read("fixtures/client.command.account-select.json")).unwrap();
    want["requestId"] = c.request_id.as_str().into();
    assert_eq!(wire, want);

    let accepted =
        r#"{"type":"command.result","requestId":"RID","status":"accepted","operationId":"account.select:11"}"#;
    send(&mut m, &accepted.replace("RID", c.request_id.as_str()));
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { state: ConfirmState::Accepted(_), .. })));
    assert_eq!(identity(&m), "active github.com/diego-arbio-dev", "accepted alone changes nothing");

    let event = script("accounts").pop().unwrap();
    update(&mut m, Msg::Net(NetEvent::Message(event)));
    assert_eq!(m.top_modal(), None);
    assert_eq!(identity(&m), "selected github.com/octo-work");
    assert_eq!(
        m.notice.as_ref().map(|n| n.text.as_str()),
        Some("GitHub reads and publishing now use octo-work on github.com")
    );
}

#[test]
fn a_rejected_account_select_shows_the_code_and_message_in_the_modal() {
    let mut m = accounts_live();
    press(&mut m, KeyCode::Char('A'));
    press(&mut m, KeyCode::Enter);
    let fx = press(&mut m, KeyCode::Char('y'));
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, "account.select:github.com/diego-arbio-dev");
    let rejected = r#"{"type":"command.result","requestId":"RID","status":"rejected","code":"NOT_FOUND","message":"gh lists no account github.com/diego-arbio-dev"}"#;
    send(&mut m, &rejected.replace("RID", c.request_id.as_str()));
    let Some(Modal::Confirm { state: ConfirmState::Failed(e), .. }) = m.top_modal() else { panic!() };
    assert_eq!(e.as_str(), "NOT_FOUND: gh lists no account github.com/diego-arbio-dev");
}

#[test]
fn accounts_that_cannot_be_selected_say_why_and_open_no_modal() {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    let caps = "\"capabilities\": [\"account.select\"]";
    send(&mut m, &read("fixtures/server.welcome.json").replace("\"capabilities\": []", caps));
    send(&mut m, &read("fixtures/server.snapshot.accounts.json"));
    assert_eq!(identity(&m), "env");
    let reasons: Vec<_> =
        m.domain.accounts.as_ref().unwrap().iter().map(|a| (a.id.to_string(), m.account_block(a))).collect();
    assert_eq!(
        reasons,
        [
            ("env/GITHUB_TOKEN".to_string(), Some("set by the daemon env")),
            ("github.com/diego-arbio-dev".to_string(), Some("env token wins")),
            ("github.com/octo-work".to_string(), Some("env token wins")),
            ("github.com/old-bot".to_string(), Some("run gh auth login")),
        ]
    );
    press(&mut m, KeyCode::Char('A'));
    assert_eq!(press(&mut m, KeyCode::Enter), []);
    assert_eq!(m.top_modal(), None);
    assert_eq!(
        m.notice.as_ref().map(|n| n.text.as_str()),
        Some("env/GITHUB_TOKEN cannot be selected: set by the daemon env")
    );

    // Without the capability, or off a live stream, a selectable account says so.
    let mut m = accounts_live();
    let a = m.domain.accounts.as_ref().unwrap()[1].clone();
    assert_eq!(m.account_block(&a), None);
    m.server.as_mut().unwrap().capabilities.clear();
    assert_eq!(m.account_block(&a), Some("daemon has no select"));
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "x".into(), retry_in_ms: None }));
    assert_eq!(m.account_block(&a), Some("not live"));
}

#[test]
fn pr_flag_opens_a_daemon_pr_whose_id_is_a_hash() {
    let wanted = pr_review_tui::config::parse_pr("https://github.com/ACME/Shop/pull/185").unwrap();
    let mut m = Model::new("test", pr_review_tui::model::Look::default(), Screen::Review, Some(wanted));
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read("fixtures/server.welcome.json"));
    send(&mut m, &read("fixtures/server.snapshot.json").replace("acme/shop#185", "pr_9f2c41d07a"));
    assert_eq!(m.ui.selected_pr, Some(PrId::new("pr_9f2c41d07a")));
    assert_eq!((m.ui.screen, m.notice.as_ref().map(|n| n.text.as_str())), (Screen::Review, None));

    let wanted = pr_review_tui::config::parse_pr("acme/shop#999").unwrap();
    let mut m = Model::new("test", pr_review_tui::model::Look::default(), Screen::Review, Some(wanted));
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read("fixtures/server.welcome.json"));
    send(&mut m, &read("fixtures/server.snapshot.json"));
    assert_eq!(m.notice.as_ref().map(|n| n.text.as_str()), Some("acme/shop#999 is not tracked by the reviewer"));
}

fn sent(fx: &[Effect]) -> Vec<&'static str> {
    fx.iter()
        .map(|e| match e {
            Effect::Send(ClientMessage::SnapshotGet { .. }) => "snapshot.get",
            Effect::Send(ClientMessage::DiffGet { .. }) => "diff.get",
            Effect::Send(ClientMessage::FindingGet { .. }) => "finding.get",
            Effect::Send(ClientMessage::TimelineGet { .. }) => "timeline.get",
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn a_resync_reissues_the_diff_page_and_finding_detail_it_was_waiting_for() {
    let mut m = review_184();
    press(&mut m, KeyCode::Char('d'));
    let lost = m.diff.as_ref().unwrap().pending.clone().unwrap();
    assert_eq!(sent(&update(&mut m, Msg::Net(NetEvent::Overflow))), ["snapshot.get"]);
    let fx = send(&mut m, &read("fixtures/server.snapshot.json"));
    let [
        Effect::Send(ClientMessage::DiffGet { request_id, cursor: None, .. }),
        Effect::Send(ClientMessage::FindingGet { .. }),
    ] = &fx[..]
    else {
        panic!("{fx:?}")
    };
    assert_ne!(request_id, &lost);
    assert_eq!(m.diff.as_ref().unwrap().pending.as_ref(), Some(request_id));

    let mut m = review_184();
    assert_eq!(sent(&press(&mut m, KeyCode::Enter)), ["diff.get"]);
    assert_eq!(sent(&press(&mut m, KeyCode::Char('r'))), ["snapshot.get"]);
    assert_eq!(sent(&send(&mut m, &read("fixtures/server.snapshot.json"))), ["diff.get", "finding.get"]);
    assert_eq!(m.detail_requests.len(), 1);
}

#[test]
fn an_unanswered_request_times_out_and_clears_its_pending_marker() {
    let mut m = review_184();
    press(&mut m, KeyCode::Enter);
    press(&mut m, KeyCode::Char('d'));
    update(&mut m, Msg::Tick(14_999));
    assert!(m.diff.as_ref().unwrap().pending.is_some());
    update(&mut m, Msg::Tick(15_000));
    let d = m.diff.as_ref().unwrap();
    assert_eq!(
        (d.pending.as_ref(), d.error.as_ref().map(|e| e.as_str())),
        (None, Some("request timed out; r to retry"))
    );
    let errors: Vec<_> = m.detail_errors.iter().map(|(f, e)| (f.as_str(), e.as_str())).collect();
    assert_eq!(errors, [("f-1", "request timed out; r to retry")]);
    assert!(m.detail_requests.is_empty());

    assert_eq!(sent(&press(&mut m, KeyCode::Char('r'))), ["snapshot.get"]);
    assert_eq!(sent(&send(&mut m, &read("fixtures/server.snapshot.json"))), ["diff.get", "finding.get"]);
    assert!(m.diff.as_ref().unwrap().pending.is_some());
    assert!(m.detail_errors.is_empty(), "r retries the card's evidence too");
}

#[test]
fn resync_required_while_resyncing_asks_again_and_ignores_the_stale_snapshot() {
    let mut m = live();
    let fx = update(&mut m, Msg::Net(NetEvent::Overflow));
    let [Effect::Send(ClientMessage::SnapshotGet { request_id: first })] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!(m.link, Link::Resyncing);

    let fx = send(&mut m, r#"{"type":"resync.required","reason":"client fell behind the event stream"}"#);
    let [Effect::Send(ClientMessage::SnapshotGet { request_id: second })] = &fx[..] else { panic!("{fx:?}") };
    assert_ne!(first, second);
    assert_eq!(m.link, Link::Resyncing);

    let stale = read("fixtures/server.snapshot.empty.json").replacen("{", &format!(r#"{{"requestId":"{first}","#), 1);
    send(&mut m, &stale);
    assert_eq!((m.link.clone(), m.domain.prs.len()), (Link::Resyncing, 3));

    send(&mut m, &read("fixtures/server.snapshot.json").replace("req-snapshot", second.as_str()));
    assert_eq!((m.link.clone(), m.domain.prs.len()), (Link::Live, 3));
}

/// Settings -> octo-work -> y: the account.select modal waits in Requested.
fn account_select_requested() -> (Model, pr_review_tui::protocol::RequestId) {
    let mut m = accounts_live();
    press(&mut m, KeyCode::Char('A'));
    press(&mut m, KeyCode::Char('j'));
    press(&mut m, KeyCode::Enter);
    let fx = press(&mut m, KeyCode::Char('y'));
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { state: ConfirmState::Requested(_), .. })));
    (m, c.request_id.clone())
}

#[test]
fn a_snapshot_showing_the_selection_settles_an_account_modal_whose_reply_was_dropped() {
    use pr_review_tui::protocol::{Event, EventBody};
    let (mut m, _) = account_select_requested();
    update(&mut m, Msg::Net(NetEvent::Overflow));
    let mut msgs = script("accounts");
    let Some(ServerMessage::Event(Event::Sequenced(e))) = msgs.pop() else { panic!() };
    let EventBody::AccountsUpdated(accounts) = e.body else { panic!() };
    let ServerMessage::Snapshot(mut snap) = msgs.swap_remove(1) else { panic!() };
    snap.accounts = Some(accounts);
    update(&mut m, Msg::Net(NetEvent::Message(ServerMessage::Snapshot(snap))));
    assert_eq!(m.top_modal(), None);
    assert_eq!(identity(&m), "selected github.com/octo-work");
    assert_eq!(
        m.notice.as_ref().map(|n| n.text.as_str()),
        Some("GitHub reads and publishing now use octo-work on github.com")
    );
}

#[test]
fn an_account_select_with_no_reply_times_out_and_y_sends_it_again() {
    let (mut m, first) = account_select_requested();
    update(&mut m, Msg::Tick(15_000));
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { state: ConfirmState::TimedOut, .. })), "{:?}", m.top_modal());
    let fx = press(&mut m, KeyCode::Char('y'));
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    assert_ne!(c.request_id, first);
    assert_eq!(c.idempotency_key, "account.select:github.com/octo-work");
}

/// The showcase script played up to `events` live events past the snapshot.
fn showcase(events: usize) -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    for msg in script("showcase").into_iter().take(2 + events) {
        update(&mut m, Msg::Net(NetEvent::Message(msg)));
    }
    m
}

#[test]
fn w_opens_the_review_the_status_line_names_then_steps_to_the_next_running_one() {
    let mut m = showcase(9);
    let number = |m: &Model| m.selected_pr().map(|p| p.number);
    assert_eq!(number(&m), Some(1287));
    press(&mut m, KeyCode::Char('/'));
    press(&mut m, KeyCode::Char('x'));
    press(&mut m, KeyCode::Enter);
    press(&mut m, KeyCode::Char('w'));
    assert_eq!((m.ui.screen, m.ui.focus, number(&m)), (Screen::Review, Pane::Main, Some(1291)));
    assert_eq!(m.ui.pr_filter, "");
    press(&mut m, KeyCode::Char('w'));
    assert_eq!(number(&m), Some(338));
    press(&mut m, KeyCode::Char('w'));
    assert_eq!(number(&m), Some(1291));
}

#[test]
fn shift_j_scrolls_the_open_card_and_moving_to_another_finding_resets_it() {
    let mut m = showcase(0);
    press(&mut m, KeyCode::Enter);
    let shift = |m: &mut Model, c| update(m, Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)));
    shift(&mut m, 'J');
    shift(&mut m, 'J');
    shift(&mut m, 'K');
    assert_eq!(m.ui.doc_scroll, 1);
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(m.ui.doc_scroll, 0);
    shift(&mut m, 'K');
    assert_eq!(m.ui.doc_scroll, 0);
}

#[test]
fn opening_a_finding_loads_its_revisions_diff_without_leaving_the_screen() {
    let mut m = showcase(0);
    press(&mut m, KeyCode::Enter);
    let fx = press(&mut m, KeyCode::Enter);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { .. })));
    assert_eq!(m.ui.screen, Screen::Review);
    assert!(fx.iter().any(|e| matches!(e, Effect::Send(ClientMessage::DiffGet { .. }))), "{fx:?}");
}

#[test]
fn n_walks_the_findings_stacked_on_one_diff_line_before_moving_on() {
    let mut m = diff_loaded();
    let second = read("fixtures/server.event.finding.updated.json")
        .replace("\"f-1\"", "\"f-9\"")
        .replace("fp-f-1", "fp-f-9")
        .replace("\"high\"", "\"low\"");
    send(&mut m, &second);
    let at = |m: &Model| m.diff.as_ref().map(|d| (d.cursor, d.stack)).unwrap();
    assert_eq!(at(&m), (4, 0));
    press(&mut m, KeyCode::Char('n'));
    assert_eq!(at(&m), (4, 1), "the second finding on line 42");
    press(&mut m, KeyCode::Enter);
    let Some(Modal::Finding { id, .. }) = m.top_modal() else { panic!("{:?}", m.top_modal()) };
    assert_eq!(id.as_str(), "f-9");
    press(&mut m, KeyCode::Esc);
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(at(&m), (5, 0), "moving the cursor leaves the stack");
}

#[test]
fn enter_opens_an_events_detail_inline_and_esc_folds_it_before_leaving() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('4'));
    let [Effect::Send(ClientMessage::TimelineGet { request_id, .. })] = &fx[..] else { panic!("{fx:?}") };
    send(&mut m, &read("fixtures/server.timeline.page.json").replace("req-tl", request_id.as_str()));
    let open = |m: &Model| m.timeline.as_ref().and_then(|t| t.open);
    assert_eq!(m.timeline.as_ref().and_then(|t| t.selected), Some(100), "lands on the newest, at the bottom");
    press(&mut m, KeyCode::Enter);
    assert_eq!((open(&m), m.top_modal()), (Some(100), None));
    press(&mut m, KeyCode::Enter);
    assert_eq!(open(&m), None, "Enter again folds a non-finding event");
    press(&mut m, KeyCode::Char('k'));
    press(&mut m, KeyCode::Enter);
    assert_eq!(open(&m), Some(99));
    press(&mut m, KeyCode::Esc);
    assert_eq!((open(&m), m.ui.screen), (None, Screen::Timeline));
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.ui.screen, Screen::Review);
}

/// Where `text` is drawn on a 120x38 frame of `m`.
fn drawn_at(m: &mut Model, text: &str) -> (u16, u16) {
    use ratatui::{Terminal, backend::TestBackend};
    update(m, Msg::Resize(120, 38));
    let mut term = Terminal::new(TestBackend::new(120, 38)).unwrap();
    term.draw(|f| pr_review_tui::view::view(m, f)).unwrap();
    let buf = term.backend().buffer().clone();
    for y in 0..38 {
        let row: String = (0..120).map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' ')).collect();
        if let Some(x) = row.find(text) {
            return (row[..x].chars().count() as u16, y);
        }
    }
    panic!("{text} is not on screen");
}

fn mouse(m: &mut Model, kind: crossterm::event::MouseEventKind, (column, row): (u16, u16)) {
    let e = crossterm::event::MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    update(m, Msg::Mouse(e));
}

#[test]
fn a_click_selects_the_pr_or_finding_under_it_and_the_wheel_scrolls_the_document() {
    use crossterm::event::{MouseButton, MouseEventKind as K};
    let mut m = showcase(0);
    let at = drawn_at(&mut m, "#1291");
    mouse(&mut m, K::Down(MouseButton::Left), at);
    assert_eq!(m.selected_pr().map(|p| p.number), Some(1291));

    let at = drawn_at(&mut m, "#1287");
    mouse(&mut m, K::Down(MouseButton::Left), at);
    let second = m.domain.pr_findings(&m.ui.selected_pr.clone().unwrap())[1].clone();
    assert_ne!(m.ui.selected_finding.as_ref(), Some(&second.id));
    let at = drawn_at(&mut m, second.title.as_str());
    mouse(&mut m, K::Down(MouseButton::Left), at);
    assert_eq!(m.ui.selected_finding.as_ref(), Some(&second.id));
    assert_eq!(m.top_modal(), None);
    let at = drawn_at(&mut m, second.title.as_str());
    mouse(&mut m, K::Down(MouseButton::Left), at);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { id, .. }) if id == &second.id), "a second click reads it");

    press(&mut m, KeyCode::Esc);
    mouse(&mut m, K::ScrollDown, (60, 20));
    mouse(&mut m, K::ScrollDown, (60, 20));
    mouse(&mut m, K::ScrollUp, (60, 20));
    assert_eq!(m.ui.doc_scroll, 3);
}

#[test]
fn l_on_the_document_stays_put_and_tab_cycles_between_the_rail_and_the_document() {
    let mut m = showcase(0);
    press(&mut m, KeyCode::Enter);
    let before = (m.ui.selected_pr.clone(), m.ui.selected_finding.clone());
    press(&mut m, KeyCode::Char('l'));
    assert_eq!(m.ui.focus, Pane::Main, "the document is the rightmost pane");
    press(&mut m, KeyCode::Char('j'));
    assert_ne!(m.ui.selected_finding, before.1, "j still moves through the findings");
    press(&mut m, KeyCode::Tab);
    assert_eq!(m.ui.focus, Pane::Sidebar);
    press(&mut m, KeyCode::Tab);
    assert_eq!(m.ui.focus, Pane::Main);
    press(&mut m, KeyCode::BackTab);
    assert_eq!(m.ui.focus, Pane::Sidebar);
    assert_eq!(m.ui.selected_pr, before.0);
}

#[test]
fn nothing_but_hello_goes_out_before_the_welcome_and_a_snapshot_reissues_what_waited() {
    let mut m = review_184();
    update(&mut m, Msg::Net(NetEvent::Lost { reason: "eof".into(), retry_in_ms: Some(250) }));
    update(&mut m, Msg::Net(NetEvent::Opened));
    assert_eq!(m.link, Link::Negotiating);
    let mut fx = press(&mut m, KeyCode::Enter);
    press(&mut m, KeyCode::Esc);
    fx.extend(press(&mut m, KeyCode::Char('3')));
    fx.extend(press(&mut m, KeyCode::Char('4')));
    fx.extend(press(&mut m, KeyCode::Char('k')));
    assert_eq!(sent(&fx), Vec::<&str>::new(), "the server answers anything before welcome with an error");
    assert_eq!(m.diff.as_ref().and_then(|d| d.error.as_ref()).map(|e| e.as_str()), Some("not connected; r retries"));

    let welcome = read("fixtures/server.welcome.json").replace("epoch-1", "epoch-2");
    assert_eq!(sent(&send(&mut m, &welcome)), ["snapshot.get", "finding.get"]);
    let fx = send(&mut m, &read("fixtures/server.snapshot.json"));
    assert_eq!(sent(&fx), ["diff.get", "timeline.get", "finding.get"]);
}

#[test]
fn the_diff_a_finding_prefetches_changes_nothing_on_the_review_screen() {
    let mut m = showcase(0);
    press(&mut m, KeyCode::Enter);
    let fx = press(&mut m, KeyCode::Enter);
    let [Effect::Send(ClientMessage::DiffGet { request_id, pr_id, base_sha, head_sha, .. })] = &fx[..] else {
        panic!("{fx:?}")
    };
    let page = read("fixtures/server.diff.page.json")
        .replace("req-diff", request_id.as_str())
        .replace("acme/shop#184", pr_id.as_str())
        .replace("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", base_sha.as_str())
        .replace("4444444444444444444444444444444444444444", head_sha.as_str())
        .replace("\"nextCursor\": \"c-2\"", "\"nextCursor\": null");
    press(&mut m, KeyCode::Esc);
    press(&mut m, KeyCode::Char('h'));
    send(&mut m, &page);
    assert_eq!((m.ui.screen, m.ui.focus), (Screen::Review, Pane::Sidebar));
    assert_eq!(m.notice, None, "the finding's file is not in this diff, and only d says so");

    press(&mut m, KeyCode::Char('l'));
    press(&mut m, KeyCode::Char('d'));
    assert_eq!(m.ui.screen, Screen::Diff);
    assert!(m.notice.as_ref().is_some_and(|n| n.text.ends_with("is not in this diff")), "{:?}", m.notice);
}
