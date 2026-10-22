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
    assert_eq!(m.top_modal(), Some(&Modal::Help { scroll: 0 }));
    let before = m.ui.selected_finding.clone();
    press(&mut m, KeyCode::Char('j'));
    assert_eq!(m.ui.selected_finding, before);

    press(&mut m, KeyCode::Esc);
    assert!(matches!(m.top_modal(), Some(Modal::Finding { scroll: 0, .. })));
    // The reader takes j, and a page that fits its rows does not scroll past its end.
    press(&mut m, KeyCode::Char('j'));
    assert!(matches!(m.top_modal(), Some(Modal::Finding { scroll: 0, .. })));
    assert_eq!(m.ui.selected_finding, before);
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
    assert_eq!(m.notice.as_ref().map(|n| n.text.as_str()), Some("Can't start: the daemon does not grant it"));
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
        &pr_review_tui::model::ConfirmState::Refused {
            code: Some(pr_review_tui::protocol::ErrorCode::CapabilityDenied),
            message: pr_review_tui::protocol::Text::new("review.start is not available in this version"),
        }
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
    assert_eq!(shown(&m), [99], "tool calls");
    press(&mut m, KeyCode::Char('v'));
    assert_eq!(shown(&m), [] as [u64; 0], "findings");
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
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { state: ConfirmState::Accepted { .. }, .. })));
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
    let Some(Modal::Confirm { state: ConfirmState::Refused { code, message }, .. }) = m.top_modal() else { panic!() };
    assert_eq!(code.unwrap().to_string(), "NOT_FOUND");
    assert_eq!(message.as_str(), "gh lists no account github.com/diego-arbio-dev");
}

#[test]
fn a_confirm_panel_offers_y_only_while_y_can_send() {
    let footer = |m: &mut Model| {
        render(m, 120, 38).0.lines().last().unwrap_or_default().trim().split("    ").next().unwrap().to_string()
    };
    let mut m = accounts_live();
    press(&mut m, KeyCode::Char('A'));
    press(&mut m, KeyCode::Enter);
    assert_eq!(footer(&mut m), "y confirm  n/Esc cancel");
    let fx = press(&mut m, KeyCode::Char('y'));
    assert_eq!(footer(&mut m), "n/Esc close", "requested");
    let [Effect::Send(ClientMessage::Command(c))] = &fx[..] else { panic!("{fx:?}") };
    let rejected =
        r#"{"type":"command.result","requestId":"RID","status":"rejected","code":"NOT_FOUND","message":"no"}"#;
    send(&mut m, &rejected.replace("RID", c.request_id.as_str()));
    assert_eq!(footer(&mut m), "n/Esc close", "refused");
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
        Some("Can't select env/GITHUB_TOKEN: set by the daemon env")
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
    assert_eq!(m.notice.as_ref().map(|n| n.text.as_str()), Some("The reviewer does not track acme/shop#999"));
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
    assert_eq!(m.timeline.as_ref().and_then(|t| t.selected), Some(99), "lands on the newest row, not its heading");
    press(&mut m, KeyCode::Enter);
    assert_eq!((open(&m), m.top_modal()), (Some(99), None));
    press(&mut m, KeyCode::Enter);
    assert_eq!(open(&m), None, "Enter again folds a non-finding event");
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
    // At 38 rows the card fits, so the wheel scrolls a shorter terminal.
    update(&mut m, Msg::Resize(120, 16));
    mouse(&mut m, K::ScrollDown, (60, 10));
    mouse(&mut m, K::ScrollDown, (60, 10));
    mouse(&mut m, K::ScrollUp, (60, 10));
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
    assert!(
        m.notice.as_ref().is_some_and(|n| n.text == "No file in this diff matches \"src/server/routes/orders.ts\""),
        "{:?}",
        m.notice
    );
}

#[test]
fn a_snapshot_that_never_arrives_ends_the_resync_with_an_error_and_r_asks_again() {
    let mut m = live();
    let fx = press(&mut m, KeyCode::Char('r'));
    assert_eq!(sent(&fx), ["snapshot.get"]);
    assert_eq!(m.link, Link::Resyncing);
    let late = m.request_timeout_ms + 1;
    update(&mut m, Msg::Tick(late));
    assert!(matches!(m.link, Link::Stalled { .. }), "{:?}", m.link);
    let (frame, _) = common::render(&mut m, 120, 38);
    assert!(frame.contains("the snapshot did not arrive; r asks again"), "{frame}");
    let fx = press(&mut m, KeyCode::Char('r'));
    assert_eq!(sent(&fx), ["snapshot.get"], "r retries");
    assert_eq!(m.link, Link::Resyncing);
    send(&mut m, &read("fixtures/server.snapshot.json"));
    assert_eq!(m.link, Link::Live);
}

#[test]
fn a_command_result_for_a_request_the_client_never_sent_is_ignored() {
    let mut m = live();
    send(&mut m, &read("fixtures/server.command.result.accepted.json"));
    assert_eq!(m.notice, None);
    let mut lines = script("approval");
    lines.truncate(12);
    let stray = ServerMessage::parse(&read("fixtures/server.command.result.rejected.json")).unwrap();
    lines.insert(6, stray.clone());
    lines.push(stray);
    let mut s = common::Sim::new(pr_review_tui::model::Look::default());
    for msg in lines {
        s.server(msg);
    }
    assert_eq!(s.m.notice, None, "a stray command.result in the stream pops no toast");
    let (frame, _) = common::render(&mut s.m, 120, 38);
    assert!(!frame.contains("Refused"), "{frame}");
}

/// The keys crossterm reads from a late `ESC ] 11 ; rgb:… BEL`: Alt+`]`, the text as plain keys, BEL as Ctrl+G.
fn late_osc11() -> Vec<KeyEvent> {
    let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
    keys.extend("11;rgb:1717/1717/1717".chars().map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    keys.push(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    keys
}

/// Feeds key events through the input thread's path, `since_start` into the session, and returns what reached the
/// reducer.
fn through_input(m: &mut Model, late: &mut pr_review_tui::terminal::LateReply, keys: &[(KeyEvent, u64)]) -> Vec<char> {
    let mut acted = vec![];
    for (k, ms) in keys {
        let at = std::time::Duration::from_millis(*ms);
        if let Some(msg) = pr_review_tui::terminal::input(crossterm::event::Event::Key(*k), late, at) {
            if let KeyCode::Char(c) = k.code {
                acted.push(c);
            }
            update(m, msg);
        }
    }
    acted
}

#[test]
fn a_late_osc11_reply_reaching_the_key_reader_fires_no_action() {
    let fresh = live();
    let mut raw = live();
    for k in late_osc11() {
        update(&mut raw, Msg::Key(k));
    }
    assert!(raw.top_modal().is_some() || raw.ui.screen != fresh.ui.screen, "unfiltered, the reply acts as keys");

    let mut m = live();
    let mut late = pr_review_tui::terminal::LateReply::default();
    let reply: Vec<_> = late_osc11().into_iter().map(|k| (k, 400)).collect();
    assert_eq!(through_input(&mut m, &mut late, &reply), Vec::<char>::new());
    assert_eq!(m.top_modal(), None);
    assert_eq!((m.ui.screen, &m.ui.pr_filter), (fresh.ui.screen, &fresh.ui.pr_filter));
    let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(through_input(&mut m, &mut late, &[(j, 401)]), ['j'], "the next key acts");
    let alt = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
    assert_eq!(through_input(&mut m, &mut late, &[(alt, 3000), (j, 3001)]), [']', 'j'], "after the window");
}

#[test]
fn a_typed_alt_bracket_swallows_only_what_could_be_a_reply_and_only_briefly() {
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let alt = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
    let mut m = live();
    let mut late = pr_review_tui::terminal::LateReply::default();
    let typed = [(alt, 500), (key('1'), 501), (key('j'), 502), (key('k'), 503), (key('1'), 504)];
    assert_eq!(through_input(&mut m, &mut late, &typed), ['j', 'k', '1'], "j cannot be in a reply");

    let mut late = pr_review_tui::terminal::LateReply::default();
    let typed = [(alt, 500), (key('b'), 600), (key('1'), 800), (key('f'), 801)];
    assert_eq!(through_input(&mut m, &mut late, &typed), ['1', 'f'], "200 ms after Alt+] keys act again");
}

fn page_scroll(m: &Model) -> u16 {
    match m.top_modal() {
        Some(Modal::Finding { scroll, .. } | Modal::Publication { scroll, .. } | Modal::Help { scroll }) => *scroll,
        _ => panic!("no page open: {:?}", m.top_modal()),
    }
}

#[test]
fn a_snapshot_keeps_the_finding_readers_place_once_the_detail_is_back() {
    let mut m = review_184();
    send(&mut m, &read("fixtures/server.finding.detail.json"));
    update(&mut m, Msg::Resize(80, 16));
    press(&mut m, KeyCode::Enter);
    for _ in 0..4 {
        press(&mut m, KeyCode::Char('j'));
    }
    assert_eq!(page_scroll(&m), 4);
    send(&mut m, &read("fixtures/server.snapshot.json"));
    assert_eq!(page_scroll(&m), 4, "the detail is reloading");
    send(&mut m, &read("fixtures/server.finding.detail.json"));
    assert_eq!(page_scroll(&m), 4, "the detail came back");
}

#[test]
fn a_snapshot_keeps_the_previews_place_once_the_preview_is_back() {
    let mut s = common::parked(pr_review_tui::model::Look::default());
    s.apply(Msg::Resize(80, 16));
    s = s.key(KeyCode::Char('p'));
    for _ in 0..4 {
        s = s.key(KeyCode::Char('j'));
    }
    assert_eq!(page_scroll(&s.m), 4);
    let snapshot = s.played.iter().find(|m| matches!(m, ServerMessage::Snapshot(_))).unwrap().clone();
    s.server(snapshot);
    assert!(matches!(s.m.top_modal(), Some(Modal::Publication { .. })));
    assert_eq!(page_scroll(&s.m), 4);
}

#[test]
fn a_preview_after_a_resync_mid_publish_brings_back_the_review_and_its_link() {
    let mut s = common::parked(pr_review_tui::model::Look::default()).key(KeyCode::Char('p'));
    let snapshot = s.played.iter().find(|m| matches!(m, ServerMessage::Snapshot(_))).unwrap().clone();
    s.server(snapshot);
    send(&mut s.m, &read("fixtures/server.publication.preview.published.json"));
    let (frame, _) = common::render(&mut s.m, 120, 38);
    assert!(frame.contains("published on GitHub as review 3141"), "{frame}");
    let fx = press(&mut s.m, KeyCode::Char('o'));
    assert_eq!(fx, [Effect::OpenUrl("https://github.com/acme/shop/pull/184#pullrequestreview-3141".into())]);
}

#[test]
fn a_pr_updated_event_retitles_the_pr_in_the_rail_and_the_header() {
    let mut m = review_184();
    send(&mut m, &pr_event(109, 184, "Booking API v2"));
    let (frame, _) = common::render(&mut m, 120, 38);
    assert_eq!(frame.matches("Booking API v2").count(), 2, "{frame}");
}

/// #184's timeline after a retry resumed from a checkpoint: three phase changes in a row, then the first thing that
/// happens.
fn checkpoint_timeline() -> Model {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('4'));
    let rid = fx
        .iter()
        .find_map(|e| match e {
            Effect::Send(ClientMessage::TimelineGet { request_id, .. }) => Some(request_id.as_str().to_string()),
            _ => None,
        })
        .unwrap();
    let ev = |seq: u64, kind: &str, data: &str| {
        format!(
            r#"{{"type":"event","streamEpoch":"epoch-1","sequence":{seq},"prevSequence":{},"occurredAt":"2026-10-20T13:01:{seq}Z","prId":"acme/shop#184","runId":"rev-184-a","eventType":"{kind}","data":{data}}}"#,
            seq - 1
        )
    };
    let phase = |seq, from: &str, to: &str| {
        ev(seq, "review.phase_changed", &format!(r#"{{"from":"{from}","to":"{to}","event":"E","detail":null}}"#))
    };
    let events = [
        ev(14, "tool.activity", r#"{"tool":"read_diff","summary":"src/booking.ts"}"#),
        phase(13, "ANALYZE", "VERIFY"),
        phase(12, "TRIAGE", "ANALYZE"),
        phase(11, "COLLECT_CONTEXT", "TRIAGE"),
    ];
    let page = format!(r#"{{"type":"timeline.page","requestId":"{rid}","events":[{}]}}"#, events.join(","));
    send(&mut m, &page);
    m
}

#[test]
fn a_phase_with_no_rows_gets_no_heading_in_the_timeline() {
    let mut m = checkpoint_timeline();
    let (frame, _) = common::render(&mut m, 120, 38);
    let headings: Vec<&str> = frame
        .lines()
        .filter(|l| l.contains("────"))
        .map(|l| l.trim_start_matches(['▌', ' ']).split(' ').next().unwrap())
        .collect();
    assert_eq!(headings, ["VERIFY"], "{frame}");
}

#[test]
fn v_cycles_the_kinds_that_are_rows_and_never_phase_changes() {
    let mut m = checkpoint_timeline();
    let kinds: Vec<&str> = (0..9)
        .map(|_| {
            press(&mut m, KeyCode::Char('v'));
            m.timeline.as_ref().unwrap().category.map_or("all", |c| c.label())
        })
        .collect();
    assert_eq!(kinds, ["tool", "finding", "run", "pr", "account", "publication", "other", "all", "tool"]);
}

#[test]
fn the_timeline_counts_the_events_it_shows_and_a_filter_counts_against_them() {
    let mut m = checkpoint_timeline();
    let count = |m: &mut Model| {
        let (frame, _) = common::render(m, 120, 38);
        frame.lines().find_map(|l| l.trim().split_once(" · oldest first")).unwrap().0.to_string()
    };
    assert_eq!(count(&mut m), "1 event", "no filter: the tool call; VERIFY is a heading");
    for _ in 0..3 {
        press(&mut m, KeyCode::Char('k'));
        assert_eq!(m.timeline.as_ref().unwrap().selected, Some(14), "the cursor never lands on the heading");
    }
    press(&mut m, KeyCode::Char('v'));
    assert_eq!(count(&mut m), "1 of 1 event", "tool calls");
    press(&mut m, KeyCode::Char('P'));
    assert_eq!(count(&mut m), "0 of 1 event", "tool calls in COLLECT_CONTEXT");
}

#[test]
fn a_finished_run_with_no_findings_says_no_findings_and_only_a_running_one_says_yet() {
    let mut m = review_184();
    let pr = PrId::new("acme/shop#184");
    m.domain.findings.retain(|_, f| f.pr_id != pr);
    let run = m.domain.latest_run(&pr).unwrap().id.clone();
    let says = |m: &mut Model, status: RunStatus| {
        m.domain.runs.get_mut(&run).unwrap().status = status;
        let (frame, _) = common::render(m, 120, 38);
        let at = frame.find("No findings").unwrap();
        frame[at..].split("   ").next().unwrap().trim().to_string()
    };
    assert_eq!(says(&mut m, RunStatus::Failed), "No findings", "cancelled or failed: nothing more will come");
    assert_eq!(says(&mut m, RunStatus::Completed), "No findings");
    assert_eq!(says(&mut m, RunStatus::Running), "No findings yet");
}

#[test]
fn a_selected_row_that_a_live_phase_change_hides_hands_the_cursor_to_the_nearest_row() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('4'));
    let [Effect::Send(ClientMessage::TimelineGet { request_id, .. })] = &fx[..] else { panic!("{fx:?}") };
    let ev = |seq: u64, prev: u64, kind: &str, data: &str| {
        format!(
            r#"{{"type":"event","streamEpoch":"epoch-1","sequence":{seq},"prevSequence":{prev},"occurredAt":"2026-10-20T13:02:{:02}Z","prId":"acme/shop#184","runId":"rev-184-a","eventType":"{kind}","data":{data}}}"#,
            seq % 60
        )
    };
    let tool = |seq| ev(seq, seq - 1, "tool.activity", r#"{"tool":"read_diff","summary":"src/booking.ts"}"#);
    let phase = |seq, prev, from: &str, to: &str| {
        ev(seq, prev, "review.phase_changed", &format!(r#"{{"from":"{from}","to":"{to}","event":"E","detail":null}}"#))
    };
    let page = [
        phase(14, 13, "VERIFY", "PREPARE_PUBLICATION"),
        tool(13),
        tool(12),
        phase(11, 10, "ANALYZE", "VERIFY"),
        tool(10),
    ];
    let page = format!(r#"{{"type":"timeline.page","requestId":"{request_id}","events":[{}]}}"#, page.join(","));
    send(&mut m, &page);
    send(&mut m, &phase(109, 108, "PREPARE_PUBLICATION", "AWAITING_APPROVAL"));
    let selected = |m: &Model| m.timeline.as_ref().unwrap().selected;
    assert_eq!(selected(&m), Some(13), "the newest row, nearest the hidden phase change");
    let (frame, _) = common::render(&mut m, 120, 38);
    assert_eq!(frame.lines().filter(|l| l.trim_start().starts_with('▌')).count(), 1, "{frame}");
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(selected(&m), Some(12));
}

/// A one-entry page: `fixture` with its `list` cut to the first entry, answering `rid`.
fn one_entry_page(fixture: &str, list: &str, rid: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&read(fixture)).unwrap();
    v["requestId"] = rid.into();
    v[list].as_array_mut().unwrap().truncate(1);
    v.to_string()
}

#[test]
fn j_on_the_only_loaded_file_loads_the_next_diff_page() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('3'));
    let Some(Effect::Send(ClientMessage::DiffGet { request_id, .. })) =
        fx.iter().find(|e| matches!(e, Effect::Send(ClientMessage::DiffGet { .. })))
    else {
        panic!("{fx:?}")
    };
    send(&mut m, &one_entry_page("fixtures/server.diff.page.json", "files", request_id.as_str()));
    press(&mut m, KeyCode::Char('h'));
    assert_eq!((m.ui.focus, m.diff.as_ref().unwrap().files.len()), (Pane::Sidebar, 1));
    let fx = press(&mut m, KeyCode::Char('j'));
    let cursors: Vec<_> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Send(ClientMessage::DiffGet { cursor, .. }) => Some(cursor.as_ref().map(|c| c.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(cursors, [Some("c-2")]);
}

#[test]
fn k_on_a_one_event_timeline_page_loads_the_older_events() {
    let mut m = review_184();
    let fx = press(&mut m, KeyCode::Char('4'));
    let [Effect::Send(ClientMessage::TimelineGet { request_id, .. })] = &fx[..] else { panic!("{fx:?}") };
    send(&mut m, &one_entry_page("fixtures/server.timeline.page.json", "events", request_id.as_str()));
    assert_eq!(m.timeline.as_ref().unwrap().events.len(), 1);
    let fx = press(&mut m, KeyCode::Char('k'));
    let before: Vec<_> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Send(ClientMessage::TimelineGet { before_sequence, .. }) => Some(*before_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(before, [Some(99)]);
}

/// The whole showcase played, then PR `number` opened in Review at `w`x`h`.
fn showcase_review(number: u64, w: u16, h: u16) -> Model {
    let mut m = showcase(usize::MAX / 2);
    update(&mut m, Msg::Resize(w, h));
    let id = m.domain.prs.values().find(|p| p.number == number).map(|p| p.id.clone());
    press(&mut m, KeyCode::Char('g'));
    while m.ui.selected_pr != id {
        press(&mut m, KeyCode::Char('j'));
    }
    press(&mut m, KeyCode::Enter);
    m
}

#[test]
fn a_page_that_fits_offers_no_j_and_j_keeps_its_header() {
    let mut m = showcase_review(341, 120, 38);
    let (before, _) = common::render(&mut m, 120, 38);
    assert!(before.contains("No findings") && !before.contains("J/K"), "{before}");
    press(&mut m, KeyCode::Char('J'));
    assert_eq!((m.ui.doc_scroll, common::render(&mut m, 120, 38).0), (0, before));
}

#[test]
fn a_taller_terminal_pulls_the_scroll_back_to_what_still_hides() {
    let mut m = showcase_review(1287, 120, 16);
    for _ in 0..200 {
        press(&mut m, KeyCode::Char('J'));
    }
    let short = m.ui.doc_scroll;
    update(&mut m, Msg::Resize(120, 38));
    let limit = pr_review_tui::view::doc_scroll_limit(&m);
    assert!(short > limit, "{short} {limit}");
    assert_eq!(m.ui.doc_scroll, limit);
}

#[test]
fn an_action_does_what_its_key_does_and_nothing_where_no_key_reaches_it() {
    use pr_review_tui::keymap::Action;
    let mut by_key = live();
    let mut by_action = live();
    press(&mut by_key, KeyCode::Char('j'));
    press(&mut by_key, KeyCode::Enter);
    update(&mut by_action, Msg::Action(Action::Down));
    update(&mut by_action, Msg::Action(Action::Open));
    assert_eq!((by_action.ui.screen, by_action.ui.focus), (Screen::Review, Pane::Main));
    assert_eq!(by_action.ui.selected_pr.as_ref().map(|p| p.as_str()), Some("acme/shop#184"));
    assert_eq!(by_action.ui.selected_pr, by_key.ui.selected_pr);
    assert_eq!(by_action.ui.selected_finding, by_key.ui.selected_finding);
    assert_eq!(update(&mut by_action, Msg::Action(Action::Quit)), [Effect::Quit]);

    // y is bound only in a confirmation, so outside one Confirm sends nothing.
    assert_eq!(update(&mut by_action, Msg::Action(Action::Confirm)), []);

    // A focused input takes only input actions, as it takes every key.
    update(&mut by_action, Msg::Action(Action::Search));
    assert_eq!(update(&mut by_action, Msg::Action(Action::Quit)), []);
    update(&mut by_action, Msg::Action(Action::InputChar('z')));
    assert!(matches!(by_action.top_modal(), Some(Modal::Input { buffer, .. }) if buffer == "z"));
}

#[test]
fn a_pick_selects_like_a_click_and_ignores_a_finding_the_selected_pr_lacks() {
    use pr_review_tui::view::Hit;
    let mut m = showcase(usize::MAX / 2);
    let pr = m.domain.prs.values().find(|p| p.number == 1287).unwrap().id.clone();
    update(&mut m, Msg::Pick(Hit::Pr(pr.clone())));
    assert_eq!(m.selected_pr().map(|p| p.number), Some(1287));

    let other = m.domain.findings.values().find(|f| f.pr_id != pr).unwrap().id.clone();
    let before = m.ui.selected_finding.clone();
    update(&mut m, Msg::Pick(Hit::Finding(other)));
    assert_eq!(m.ui.selected_finding, before, "a finding of another PR is not selectable");

    let second = m.domain.pr_findings(&pr)[1].id.clone();
    update(&mut m, Msg::Pick(Hit::Finding(second.clone())));
    assert_eq!(m.ui.selected_finding.as_ref(), Some(&second));
    assert_eq!(m.top_modal(), None);
    update(&mut m, Msg::Pick(Hit::Finding(second.clone())));
    assert!(matches!(m.top_modal(), Some(Modal::Finding { id, .. }) if id == &second), "a second pick reads it");
}
