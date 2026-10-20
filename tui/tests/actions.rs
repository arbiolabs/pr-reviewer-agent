//! s, R, c and p: a confirmation naming the exact target, nothing sent without y, and success shown only once the
//! event that proves it arrives, never on `accepted`.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{ConfirmState, Modal, Model, Preview};
use pr_review_tui::protocol::{ClientMessage, Command, ServerMessage};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

fn press(m: &mut Model, code: KeyCode) -> Vec<Effect> {
    update(m, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn send(m: &mut Model, json: &str) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(ServerMessage::parse(json).unwrap())))
}

fn server(m: &mut Model, msg: ServerMessage) -> Vec<Effect> {
    update(m, Msg::Net(NetEvent::Message(msg)))
}

fn commands(fx: &[Effect]) -> Vec<&pr_review_tui::protocol::CommandRequest> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Send(ClientMessage::Command(c)) => Some(c),
            _ => None,
        })
        .collect()
}

/// An engine daemon granting every command, on the fixture snapshot, with `pr` selected.
fn engine(welcome: &str, pr: &str) -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    send(&mut m, &read(welcome));
    send(&mut m, &read("fixtures/server.snapshot.json"));
    m.ui.selected_pr = Some(pr_review_tui::protocol::PrId::new(pr));
    m
}

fn prompt(m: &Model) -> String {
    match m.top_modal() {
        Some(Modal::Confirm { prompt, .. }) => prompt.clone(),
        other => panic!("{other:?}"),
    }
}

fn confirm_state(m: &Model) -> ConfirmState {
    match m.top_modal() {
        Some(Modal::Confirm { state, .. } | Modal::Publication { publish: state, .. }) => state.clone(),
        other => panic!("{other:?}"),
    }
}

fn event(seq: u64, pr: &str, run: &str, kind: &str, data: &str) -> String {
    format!(
        r#"{{"type":"event","streamEpoch":"epoch-1","sequence":{seq},"prevSequence":{},"occurredAt":"2026-10-20T13:01:00Z","prId":"{pr}","runId":"{run}","eventType":"{kind}","data":{data}}}"#,
        seq - 1
    )
}

fn run_json(id: &str, pr: &str, head: &str, phase: &str, status: &str) -> String {
    format!(
        r#"{{"id":"{id}","prId":"{pr}","baseSha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","headSha":"{head}","phase":"{phase}","status":"{status}","model":"m","startedAt":"2026-10-20T13:01:00Z","failureReason":"model timed out"}}"#
    )
}

const H186: &str = "6666666666666666666666666666666666666666";

#[test]
fn start_names_the_pr_and_head_sends_only_on_y_and_settles_on_run_started_not_on_accepted() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#186");
    assert!(commands(&press(&mut m, KeyCode::Char('s'))).is_empty());
    assert_eq!(prompt(&m), "Start a review of #186 at 6666666?");
    assert!(commands(&press(&mut m, KeyCode::Esc)).is_empty(), "Esc sends nothing");
    assert_eq!(m.top_modal(), None);

    press(&mut m, KeyCode::Char('s'));
    assert!(commands(&press(&mut m, KeyCode::Char('n'))).is_empty(), "n sends nothing");
    press(&mut m, KeyCode::Char('s'));
    let fx = press(&mut m, KeyCode::Char('y'));
    let [c] = commands(&fx)[..] else { panic!("{fx:?}") };
    assert_eq!(c.idempotency_key, format!("review.start:acme/shop#186:{H186}"));
    let rid = c.request_id.clone();
    assert!(commands(&press(&mut m, KeyCode::Char('y'))).is_empty(), "a second y must not resend");

    let accepted = read("fixtures/server.command.result.accepted.json").replace("req-cmd", rid.as_str());
    send(&mut m, &accepted);
    assert!(matches!(confirm_state(&m), ConfirmState::Accepted(_)), "accepted alone keeps the panel waiting");
    assert!(m.notice.is_none(), "{:?}", m.notice);

    let started = run_json("rev-186-a", "acme/shop#186", H186, "COLLECT_CONTEXT", "RUNNING");
    send(&mut m, &event(109, "acme/shop#186", "rev-186-a", "run.started", &started));
    assert_eq!(m.top_modal(), None, "the event closes the panel");
    assert_eq!(m.notice.as_ref().unwrap().text, "review.start done: run rev-186- started at 6666666");
}

#[test]
fn a_rejected_command_shows_its_code_and_message() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#186");
    press(&mut m, KeyCode::Char('s'));
    let fx = press(&mut m, KeyCode::Char('y'));
    let rid = commands(&fx)[0].request_id.clone();
    let rejected = format!(
        r#"{{"type":"command.result","requestId":"{rid}","status":"rejected","code":"REVISION_STALE","message":"the head is now 7777777"}}"#
    );
    send(&mut m, &rejected);
    let ConfirmState::Failed(e) = confirm_state(&m) else { panic!() };
    assert_eq!(e.as_str(), "REVISION_STALE: the head is now 7777777");
    let started = run_json("rev-186-a", "acme/shop#186", H186, "COLLECT_CONTEXT", "RUNNING");
    send(&mut m, &event(109, "acme/shop#186", "rev-186-a", "run.started", &started));
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { .. })), "a rejected panel stays until Esc");
}

#[test]
fn cancel_and_retry_name_their_target_and_settle_on_the_run_ending_or_moving() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#184");
    press(&mut m, KeyCode::Char('R'));
    assert_eq!(m.top_modal(), None, "nothing failed to retry");
    assert!(m.notice.as_ref().unwrap().text.contains("its latest run has not failed"));

    press(&mut m, KeyCode::Char('c'));
    assert_eq!(prompt(&m), "Cancel the running review of #184?");
    let fx = press(&mut m, KeyCode::Char('y'));
    assert_eq!(commands(&fx)[0].idempotency_key, "review.cancel:rev-184-a");
    // An ordinary phase step is not the cancel taking effect.
    let step = r#"{"from":"VERIFY","to":"PREPARE_PUBLICATION","event":"CANDIDATES_DISPOSED","detail":null}"#;
    send(&mut m, &event(109, "acme/shop#184", "rev-184-a", "review.phase_changed", step));
    assert!(matches!(m.top_modal(), Some(Modal::Confirm { .. })));
    let failed = run_json("rev-184-a", "acme/shop#184", "4444444444444444444444444444444444444444", "FAILED", "FAILED");
    send(&mut m, &event(110, "acme/shop#184", "rev-184-a", "run.failed", &failed));
    assert_eq!(m.top_modal(), None);
    assert_eq!(m.notice.as_ref().unwrap().text, "review.cancel done: run rev-184- failed: model timed out");

    press(&mut m, KeyCode::Char('R'));
    assert_eq!(prompt(&m), "Retry run rev-184-?");
    let fx = press(&mut m, KeyCode::Char('y'));
    assert_eq!(commands(&fx)[0].idempotency_key, "review.retry:rev-184-a");
}

#[test]
fn a_read_only_daemon_says_why_the_action_keys_do_nothing() {
    let mut m = engine("fixtures/server.welcome.read-only.json", "acme/shop#186");
    assert!(commands(&press(&mut m, KeyCode::Char('s'))).is_empty());
    assert_eq!(m.top_modal(), None);
    assert_eq!(m.notice.as_ref().unwrap().text, "review.start unavailable: daemon is read-only");

    let off = read("fixtures/server.welcome.read-only.json").replace(
        "another pr-review is running on /srv/pr-review/.data",
        "review commands are off; start the daemon with --allow-commands or set control.allow_commands",
    );
    let mut m = engine("fixtures/server.welcome.read-only.json", "acme/shop#186");
    send(&mut m, &off);
    send(&mut m, &read("fixtures/server.snapshot.json"));
    press(&mut m, KeyCode::Char('c'));
    assert_eq!(
        m.notice.as_ref().unwrap().text,
        "review.cancel unavailable: commands are off (start the daemon with --allow-commands)"
    );
}

/// The approval script up to the run parking: #184's run is AWAITING_APPROVAL with its publication awaiting.
fn parked() -> Model {
    let mut m = model();
    update(&mut m, Msg::Net(NetEvent::Opened));
    for msg in script("approval").into_iter().take(5) {
        server(&mut m, msg);
    }
    m.ui.selected_pr = Some(pr_review_tui::protocol::PrId::new("acme/shop#184"));
    m
}

/// `p` on the parked run, answered with the fixture preview.
fn previewing() -> Model {
    let mut m = parked();
    let fx = press(&mut m, KeyCode::Char('p'));
    let get = fx
        .iter()
        .find_map(|e| match e {
            Effect::Send(ClientMessage::PublicationGet { request_id, run_id }) => {
                Some((request_id.clone(), run_id.clone()))
            }
            _ => None,
        })
        .expect("p asks for the preview");
    assert_eq!(get.1.as_str(), "rev-184-a");
    assert!(matches!(m.top_modal(), Some(Modal::Publication { preview: Preview::Loading(_), .. })));
    send(&mut m, &read("fixtures/server.publication.preview.json").replace("req-pub", get.0.as_str()));
    m
}

fn frame(m: &mut Model) -> String {
    render(m, 120, 60).0
}

#[test]
fn the_preview_shows_the_exact_body_and_every_comment_from_the_fixture() {
    let mut m = previewing();
    let f = frame(&mut m);
    for want in [
        "acme/shop #184 at 4444444 · as event COMMENT · 1 inline comment",
        "## Automated PR review",
        "Found 1 actionable issue.",
        "- High: Finding f-1 (src/booking.ts:42)",
        "Review performed with isolated read-only tools. Human review is still required.",
        "hidden marker  <!-- durable-review:rev-184-a:8984214f",
        "src/booking.ts:42  RIGHT",
        "**High: Finding f-1**",
        "Two concurrent bookings can both observe slot.free before either sets held.",
        "y publishes exactly this to GitHub",
    ] {
        assert!(f.contains(want), "missing {want:?} in\n{f}");
    }
}

#[test]
fn y_sends_exactly_one_publish_naming_the_preview_and_only_published_reads_as_done() {
    let mut m = previewing();
    let fx = press(&mut m, KeyCode::Char('y'));
    let [c] = commands(&fx)[..] else { panic!("{fx:?}") };
    let Command::Publish(p) = &c.command else { panic!() };
    assert_eq!(p.run_id.as_str(), "rev-184-a");
    assert_eq!(p.preview_id.as_str(), "8984214f9b29fcaf2ced0509cf489c6b44205df8bf23ee27c492494f5a09a956");
    assert_eq!(p.expected_head_sha.as_str(), "4444444444444444444444444444444444444444");
    assert_eq!(c.idempotency_key, format!("review.publish:rev-184-a:{}", p.preview_id));
    let rid = c.request_id.clone();
    assert!(commands(&press(&mut m, KeyCode::Char('y'))).is_empty(), "a second y must not resend");
    assert!(frame(&mut m).contains("requested… waiting for the daemon"));

    let rest = script("approval");
    let accepted = read("fixtures/server.command.result.accepted.json").replace("req-cmd", rid.as_str());
    send(&mut m, &accepted);
    let f = frame(&mut m);
    assert!(f.contains("approval recorded; publishing…"), "{f}");
    assert!(!f.contains("published on GitHub"), "accepted is not published: {f}");

    for msg in rest.into_iter().skip(7).take(2) {
        server(&mut m, msg);
    }
    assert!(frame(&mut m).contains("publishing…"));
    assert_eq!(press(&mut m, KeyCode::Char('o')), [], "o does nothing before it is published");

    server(&mut m, script("approval").remove(9));
    let f = frame(&mut m);
    assert!(f.contains("✓ published on GitHub as review 3141"), "{f}");
    assert!(f.contains("https://github.com/acme/shop/pull/184#pullrequestreview-3141"), "{f}");
    assert_eq!(
        press(&mut m, KeyCode::Char('o')),
        [Effect::OpenUrl("https://github.com/acme/shop/pull/184#pullrequestreview-3141".into())]
    );
}

#[test]
fn a_stale_preview_turns_y_off_and_says_why() {
    // A newer head on the PR.
    let mut m = previewing();
    let pr = r#"{"id":"acme/shop#184","repository":"acme/shop","number":184,"title":"Booking API","headSha":"9999999999999999999999999999999999999999","baseSha":"b","lifecycle":"ACTIVE"}"#;
    send(&mut m, &event(14, "acme/shop#184", "rev-184-a", "pr.updated", pr));
    assert!(commands(&press(&mut m, KeyCode::Char('y'))).is_empty());
    let f = frame(&mut m);
    assert!(f.contains("y is off: stale: the PR head is now 9999999; this preview is for 4444444"), "{f}");

    // The run leaving AWAITING_APPROVAL, here cancelled from elsewhere.
    let mut m = previewing();
    let out = r#"{"from":"AWAITING_APPROVAL","to":"FAILED","event":"CANCELLED","detail":null}"#;
    send(&mut m, &event(14, "acme/shop#184", "rev-184-a", "review.phase_changed", out));
    assert!(commands(&press(&mut m, KeyCode::Char('y'))).is_empty());
    assert!(frame(&mut m).contains("y is off: stale: the run is now FAILED, no longer awaiting approval"));
}

#[test]
fn p_on_a_run_that_is_not_parked_opens_nothing() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#184");
    assert_eq!(press(&mut m, KeyCode::Char('p')), []);
    assert_eq!(m.top_modal(), None);
    assert_eq!(
        m.notice.as_ref().unwrap().text,
        "review.publish unavailable for #184: no review is awaiting approval on it"
    );
}
