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
    assert!(matches!(confirm_state(&m), ConfirmState::Accepted { .. }), "accepted alone keeps the panel waiting");
    assert!(m.notice.is_none(), "{:?}", m.notice);

    let started = run_json("rev-186-a", "acme/shop#186", H186, "COLLECT_CONTEXT", "RUNNING");
    send(&mut m, &event(109, "acme/shop#186", "rev-186-a", "run.started", &started));
    assert_eq!(m.top_modal(), None, "the event closes the panel");
    assert_eq!(m.notice.as_ref().unwrap().text, "Started reviewing #186 at 6666666");
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
    assert_eq!(m.notice.as_ref().unwrap().text, "Cancelled the review of #184");

    press(&mut m, KeyCode::Char('R'));
    assert_eq!(prompt(&m), "Retry run rev-184-?");
    let fx = press(&mut m, KeyCode::Char('y'));
    assert_eq!(commands(&fx)[0].idempotency_key, "review.retry:rev-184-a");
}

#[test]
fn the_retry_prompt_names_the_attempt_it_opens_when_the_daemon_counts_them() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#184");
    let failed = run_json("rev-184-a", "acme/shop#184", "4444444444444444444444444444444444444444", "FAILED", "FAILED")
        .replacen('{', r#"{"attempt":1,"#, 1);
    send(&mut m, &event(109, "acme/shop#184", "rev-184-a", "run.failed", &failed));
    press(&mut m, KeyCode::Char('R'));
    assert_eq!(prompt(&m), "Retry run rev-184- (attempt 2)?");
}

#[test]
fn a_read_only_daemon_says_why_the_action_keys_do_nothing() {
    let mut m = engine("fixtures/server.welcome.read-only.json", "acme/shop#186");
    assert!(commands(&press(&mut m, KeyCode::Char('s'))).is_empty());
    assert_eq!(m.top_modal(), None);
    assert_eq!(m.notice.as_ref().unwrap().text, "Can't start a review: daemon is read-only");

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
        "Can't cancel: commands are off (start the daemon with --allow-commands)"
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
fn the_preview_styles_the_payload_and_m_shows_the_raw_markdown_exactly() {
    let mut m = previewing();
    let (f, term) = render(&mut m, 120, 60);
    for want in [
        "acme/shop #184 at 4444444 · as event COMMENT · 1 inline comment",
        "Automated PR review",
        "Found 1 actionable issue.",
        "- High: Finding f-1 (src/booking.ts:42)",
        "Review performed with isolated read-only tools. Human review is still required.",
        "hidden marker  <!-- durable-review:rev-184-a:8984214f",
        "src/booking.ts:42  new side",
        "High: Finding f-1",
        "Two concurrent bookings can both observe slot.free before either sets held.",
        "y publishes exactly this to GitHub",
        "m raw markdown",
    ] {
        assert!(f.contains(want), "missing {want:?} in\n{f}");
    }
    for raw in ["## Automated", "**", "RIGHT"] {
        assert!(!f.contains(raw), "styled view shows {raw:?}:\n{f}");
    }
    let bold = |term: &ratatui::Terminal<ratatui::backend::TestBackend>, text: &str, frame: &str| {
        let (y, line) = frame.lines().enumerate().find(|(_, l)| l.contains(text)).unwrap();
        let x = line[..line.find(text).unwrap()].chars().count() as u16;
        term.backend().buffer()[(x, y as u16)].modifier.contains(ratatui::style::Modifier::BOLD)
    };
    assert!(bold(&term, "Automated PR review", &f), "the heading is styled as one");
    assert!(
        bold(&term, "High: Finding f-1**".trim_end_matches('*'), &f.replace("- High", "- high")),
        "**bold** is bold"
    );

    press(&mut m, KeyCode::Char('m'));
    let raw = frame(&mut m);
    for want in [
        "## Automated PR review",
        "Reviewed commit: `4444444444444444444444444444444444444444`",
        "- High: Finding f-1 (`src/booking.ts:42`)",
        "<!-- durable-review:rev-184-a:8984214f",
        "**High: Finding f-1**",
        "Two concurrent bookings can both observe `slot.free` before either sets `held`.",
        "m styled",
    ] {
        assert!(raw.contains(want), "raw view lacks {want:?}:\n{raw}");
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
    assert!(f.contains("Approved; publishing…"), "{f}");
    assert!(!f.contains("published on GitHub"), "accepted is not published: {f}");

    for msg in rest.into_iter().skip(6).take(2) {
        server(&mut m, msg);
    }
    assert!(frame(&mut m).contains("publishing…"));
    assert_eq!(press(&mut m, KeyCode::Char('o')), [], "o does nothing before it is published");

    server(&mut m, script("approval").remove(8));
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
    assert_eq!(m.notice.as_ref().unwrap().text, "Nothing to publish on #184");
}

fn parked_frame(look: pr_review_tui::model::Look) -> (String, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut m = parked();
    m.look = look;
    m.ui.screen = pr_review_tui::model::Screen::Review;
    render(&mut m, 120, 38)
}

#[test]
fn awaiting_approval_is_its_own_amber_state_in_the_stepper_the_rail_and_the_status_line() {
    use pr_review_tui::model::Look;
    use pr_review_tui::theme::{Palette, rgb};
    let (f, term) = parked_frame(Look::default());
    let top = f.lines().next().unwrap();
    assert!(top.contains("◔ #184 waiting for you · p previews"), "{top}");
    assert!(!top.contains("reviewing") && !top.contains("nothing running"), "{top}");
    assert!(f.contains("● Prepare ◔ Publish"), "{f}");
    assert!(f.contains("◔ waiting for you"), "{f}");
    assert!(f.lines().any(|l| l.starts_with("▌   shop") && l.contains("◔ awaiting you")), "the rail row: {f}");
    assert!(f.contains("◔ Ready to publish"), "{f}");
    assert!(!f.contains("failed") && !f.contains("✓ done"), "neither an error nor done: {f}");
    // Amber: the status line's glyph is drawn in the palette's warn color.
    let buf = term.backend().buffer();
    let x = (0..120).find(|&x| buf[(x, 0)].symbol() == "◔").unwrap();
    assert_eq!(buf[(x, 0)].fg, rgb(Palette::Dark.hex("warn")));

    let (f, _) = parked_frame(Look { color: false, ..Look::default() });
    assert!(f.lines().next().unwrap().contains("◔ #184 waiting for you"), "NO_COLOR keeps the glyph and words: {f}");
    assert!(f.contains("awaiting you"), "{f}");

    let (f, _) = parked_frame(Look { ascii: true, ..Look::default() });
    assert!(f.lines().next().unwrap().contains("~ #184 waiting for you | p previews"), "{f}");
    assert!(f.contains("~ awaiting you"), "{f}");
    assert!(f.contains("+ Prepare ~ Publish"), "{f}");
    assert!(f.is_ascii(), "{f}");
}

#[test]
fn w_steps_through_parked_reviews_then_the_running_ones() {
    let mut m = parked();
    // #185 starts running: w goes to the parked #184 first, since it waits on the reader, then to #185.
    let started =
        run_json("rev-185-b", "acme/shop#185", "5555555555555555555555555555555555555555", "ANALYZE", "RUNNING");
    send(&mut m, &event(14, "acme/shop#185", "rev-185-b", "run.started", &started));
    m.ui.selected_pr = Some(pr_review_tui::protocol::PrId::new("acme/shop#186"));
    let selected = |m: &Model| m.ui.selected_pr.as_ref().unwrap().to_string();
    press(&mut m, KeyCode::Char('w'));
    assert_eq!(selected(&m), "acme/shop#184");
    press(&mut m, KeyCode::Char('w'));
    assert_eq!(selected(&m), "acme/shop#185");
    press(&mut m, KeyCode::Char('w'));
    assert_eq!(selected(&m), "acme/shop#184");
}

#[test]
fn a_running_review_with_no_activity_yet_is_not_nothing_running() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#185");
    let (f, _) = render(&mut m, 120, 38);
    let top = f.lines().next().unwrap();
    assert!(top.contains("reviewing #184 verify"), "{top}");
    assert!(!top.contains("nothing running"), "{top}");
}

#[test]
fn publication_events_show_in_the_timeline_and_a_pr_update_between_runs_is_under_pr() {
    let mut s = Sim::new(pr_review_tui::model::Look::default()).play("approval", 13);
    s.m.ui.selected_pr = Some(pr_review_tui::protocol::PrId::new("acme/shop#184"));
    let pr = r#"{"id":"acme/shop#184","repository":"acme/shop","number":184,"title":"Booking API","headSha":"4444444444444444444444444444444444444444","baseSha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","lifecycle":"ACTIVE","author":"octocat"}"#;
    s.server(
        ServerMessage::parse(&event(19, "acme/shop#184", "", "pr.updated", pr).replace(r#""runId":"","#, "")).unwrap(),
    );
    let mut s = s.key(KeyCode::Char('4'));
    let (f, _) = render(&mut s.m, 120, 50);
    let at = |needle: &str| f.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{f}"));
    assert!(
        at("publication   published  https://github.com/acme/shop/pull/184#pullrequestreview-3141") > at("PUBLISH ─"),
        "{f}"
    );
    assert!(at("publication   publishing") > at("PUBLISH ─"), "{f}");
    assert!(f.contains("publication   awaiting your approval"), "{f}");
    let update = at("PR #184 updated");
    assert!(f.lines().nth(update - 1).unwrap().trim_start().starts_with("PR ─"), "{f}");
    assert!(!f.lines().skip(update.saturating_sub(2)).any(|l| l.contains("BEFORE THE RUN")), "{f}");
}

#[test]
fn publication_rows_sit_under_publish_though_the_run_ends_with_no_phase_change() {
    // The daemon's journal: the publisher's statuses, then review.completed, which names no `from` phase.
    let url = r#","reviewId":3141,"url":"https://github.com/acme/shop/pull/184#pullrequestreview-3141""#;
    let completed =
        run_json("rev-184-a", "acme/shop#184", "4444444444444444444444444444444444444444", "COMPLETED", "COMPLETED");
    let mut s = Sim::new(pr_review_tui::model::Look::default()).play("approval", 5);
    for (seq, kind, data) in [
        (
            14,
            "review.phase_changed",
            r#"{"from":"AWAITING_APPROVAL","to":"PUBLISH","event":"APPROVED","detail":null}"#.to_string(),
        ),
        (15, "review.publication_changed", r#"{"runId":"rev-184-a","status":"publishing"}"#.to_string()),
        (16, "review.publication_changed", format!(r#"{{"runId":"rev-184-a","status":"published"{url}}}"#)),
        (17, "review.completed", completed),
    ] {
        s.server(ServerMessage::parse(&event(seq, "acme/shop#184", "rev-184-a", kind, &data)).unwrap());
    }
    let mut s = s.select("acme/shop#184").key(KeyCode::Char('4'));
    let (f, _) = render(&mut s.m, 120, 50);
    let rows: Vec<&str> = f.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let at = |needle: &str| rows.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{f}"));
    let publishing = at("publication   publishing");
    assert!(rows[publishing - 1].starts_with("PUBLISH ─"), "{f}");
    assert_eq!(at("publication   published"), publishing + 1, "{f}");
    assert!(!rows[publishing].contains("https://"), "{f}");
    assert!(rows[publishing + 1].ends_with("https://github.com/acme/shop/pull/184#pullrequestreview-3141"), "{f}");
}

fn asks_for_the_preview(fx: &[Effect]) -> bool {
    fx.iter().any(
        |e| matches!(e, Effect::Send(ClientMessage::PublicationGet { run_id, .. }) if run_id.as_str() == "rev-184-a"),
    )
}

#[test]
fn the_ready_to_publish_line_takes_the_cursor_and_enter_or_a_click_opens_the_preview() {
    let mut m = parked();
    m.ui.screen = pr_review_tui::model::Screen::Review;
    m.ui.focus = pr_review_tui::model::Pane::Main;
    assert!(m.ui.selected_finding.is_some(), "the first finding holds the cursor");
    press(&mut m, KeyCode::Char('k'));
    assert_eq!(m.ui.selected_finding, None, "k from the first finding moves onto the ready line");
    assert!(asks_for_the_preview(&press(&mut m, KeyCode::Enter)));
    press(&mut m, KeyCode::Esc);
    press(&mut m, KeyCode::Char('j'));
    assert!(m.ui.selected_finding.is_some(), "j moves back to the first finding");

    let (f, _) = render(&mut m, 120, 50);
    let row = f.lines().position(|l| l.contains("Ready to publish")).expect("the ready line") as u16;
    let line = f.lines().nth(row as usize).unwrap();
    let col = line[..line.find("Ready").unwrap()].chars().count() as u16;
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };
    assert!(asks_for_the_preview(&update(&mut m, Msg::Mouse(click))));
}

fn footer(m: &mut Model) -> String {
    frame(m).lines().last().unwrap().to_string()
}

#[test]
fn the_preview_footer_offers_y_only_while_it_can_publish_and_o_only_once_there_is_a_url() {
    let mut m = previewing();
    let f = footer(&mut m);
    assert!(f.contains("y publish") && !f.contains("o open"), "{f}");

    let rid = commands(&press(&mut m, KeyCode::Char('y')))[0].request_id.clone();
    send(&mut m, &read("fixtures/server.command.result.accepted.json").replace("req-cmd", rid.as_str()));
    for msg in script("approval").into_iter().skip(6).take(2) {
        server(&mut m, msg);
    }
    let f = footer(&mut m);
    assert!(!f.contains("y publish") && !f.contains("o open"), "publishing: {f}");

    server(&mut m, script("approval").remove(8));
    let f = footer(&mut m);
    assert!(!f.contains("y publish") && f.contains("o open on GitHub"), "published: {f}");
}

#[test]
fn the_confirmation_is_a_page_with_a_human_title_and_wraps_its_result() {
    for (w, h) in [(80, 24), (120, 38)] {
        let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#186");
        m.ui.screen = pr_review_tui::model::Screen::Review;
        press(&mut m, KeyCode::Char('s'));
        let (frame, _) = render(&mut m, w, h);
        assert!(frame.contains("Start a review of #186 at 6666666?"), "{frame}");
        assert!(!frame.contains("REVIEW.START") && !frame.contains("sends review.start"), "{frame}");
        assert!(frame.contains("review.start"), "the command name stays in a detail line:\n{frame}");
        let body: Vec<&str> = frame.lines().skip(2).take(h as usize - 3).collect();
        assert!(
            body.iter().all(|l| !l.contains("#185") && !l.contains("PULL REQUESTS")),
            "the rail shows through:\n{frame}"
        );
        let fx = press(&mut m, KeyCode::Char('y'));
        let rid = commands(&fx)[0].request_id.clone();
        let long = "the head moved to 7777777 while the request was in flight, so the daemon refused to start a review of a revision that is no longer the head of this pull request";
        send(
            &mut m,
            &format!(
                r#"{{"type":"command.result","requestId":"{rid}","status":"rejected","code":"REVISION_STALE","message":"{long}"}}"#
            ),
        );
        let (frame, _) = render(&mut m, w, h);
        let text = frame.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(text.contains("pull request"), "{w}x{h}: the rejection is cut:\n{frame}");
        assert!(!frame.contains('…'), "{w}x{h}: {frame}");
    }
}

#[test]
fn toasts_say_what_happened_in_plain_words_and_wrap_instead_of_cutting() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#186");
    press(&mut m, KeyCode::Char('R'));
    assert_eq!(m.notice.as_ref().unwrap().text, "Can't retry #186: its latest run has not failed");
    press(&mut m, KeyCode::Char('c'));
    assert_eq!(m.notice.as_ref().unwrap().text, "Can't cancel #186: no review is running on it");
    m.notice = Some(pr_review_tui::model::Notice {
        text: "Can't retry #186: its latest run has not failed, and the daemon keeps the attempt you asked for".into(),
        until_ms: u64::MAX,
    });
    let (frame, _) = render(&mut m, 60, 16);
    let text = frame.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("keeps the attempt you asked for"), "the toast is cut:\n{frame}");
}

#[test]
fn each_timeline_event_sits_under_the_phase_its_run_was_in_when_it_happened() {
    use pr_review_tui::protocol::{Event, Phase, SequencedEvent};
    let m = engine("fixtures/server.welcome.engine.json", "acme/shop#184");
    let run = |phase: &str, status: &str| run_json("rev-184-a", "acme/shop#184", H186, phase, status);
    let phase = |from: &str, to: &str| format!(r#"{{"from":"{from}","to":"{to}","event":"E","detail":null}}"#);
    let finding = r#"{"id":"f-1","prId":"acme/shop#184","runId":"rev-184-a","fingerprint":"fp-f-1","title":"Finding f-1","category":"correctness","severity":"high","status":"PUBLISHED","confidence":0.82,"file":"src/booking.ts","line":42,"evidenceCount":1,"stale":false}"#;
    let pub_ = |status: &str| format!(r#"{{"runId":"rev-184-a","status":"{status}"}}"#);
    let oldest_first = [
        event(109, "acme/shop#184", "rev-184-a", "review.publication_changed", &pub_("awaiting_approval")),
        event(110, "acme/shop#184", "rev-184-a", "run.failed", &run("FAILED", "FAILED")),
        event(111, "acme/shop#184", "rev-184-a", "run.started", &run("COLLECT_CONTEXT", "RUNNING")),
        event(112, "acme/shop#184", "rev-184-a", "review.phase_changed", &phase("AWAITING_APPROVAL", "PUBLISH")),
        event(113, "acme/shop#184", "rev-184-a", "finding.updated", finding),
        event(114, "acme/shop#184", "rev-184-a", "review.completed", &run("COMPLETED", "COMPLETED")),
    ];
    let mut events: Vec<SequencedEvent> = oldest_first
        .iter()
        .map(|j| match ServerMessage::parse(j).unwrap() {
            ServerMessage::Event(Event::Sequenced(e)) => e,
            other => panic!("{other:?}"),
        })
        .collect();
    events.reverse();
    let mut phases = pr_review_tui::update::event_phases(&m, &events);
    phases.reverse();
    assert_eq!(
        phases,
        [
            Some(Phase::AwaitingApproval),
            Some(Phase::Failed),
            Some(Phase::CollectContext),
            Some(Phase::Publish),
            Some(Phase::Publish),
            Some(Phase::Completed),
        ]
    );
}

#[test]
fn enter_opens_the_preview_on_a_parked_pr_with_no_findings() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#184");
    m.domain.findings.retain(|_, f| f.pr_id.as_str() != "acme/shop#184");
    let parked = run_json("rev-184-a", "acme/shop#184", H186, "AWAITING_APPROVAL", "RUNNING");
    send(&mut m, &event(109, "acme/shop#184", "rev-184-a", "run.started", &parked));
    m.ui.screen = pr_review_tui::model::Screen::Review;
    m.ui.focus = pr_review_tui::model::Pane::Main;
    press(&mut m, KeyCode::Char('j'));
    press(&mut m, KeyCode::Enter);
    assert!(matches!(m.top_modal(), Some(Modal::Publication { .. })), "{:?}", m.top_modal());
}

#[test]
fn a_resync_between_accepted_and_published_refetches_the_preview_and_takes_its_status() {
    let mut m = previewing();
    let fx = press(&mut m, KeyCode::Char('y'));
    let rid = commands(&fx)[0].request_id.clone();
    send(&mut m, &read("fixtures/server.command.result.accepted.json").replace("req-cmd", rid.as_str()));
    // The stream breaks; the publication_changed events are lost; the snapshot carries none.
    send(&mut m, &read("fixtures/server.resync.required.unsolicited.json"));
    let fx = send(
        &mut m,
        &read("fixtures/server.snapshot.json").replace("\"requestId\":\"req-snapshot\"", "\"requestId\":null"),
    );
    let get = fx.iter().find_map(|e| match e {
        Effect::Send(ClientMessage::PublicationGet { request_id, .. }) => Some(request_id.clone()),
        _ => None,
    });
    let get = get.expect("the snapshot refetches the open preview");
    let published = read("fixtures/server.publication.preview.json")
        .replace("req-pub", get.as_str())
        .replace("\"awaiting_approval\"", "\"published\"");
    send(&mut m, &published);
    let run = pr_review_tui::protocol::RunId::new("rev-184-a");
    assert_eq!(m.domain.publications[&run].status, pr_review_tui::protocol::PublicationStatus::Published);
    let f = frame(&mut m);
    assert!(f.contains("published on GitHub"), "{f}");
}

#[test]
fn an_accepted_command_with_no_event_after_15_s_says_so_and_esc_closes() {
    let mut m = engine("fixtures/server.welcome.engine.json", "acme/shop#186");
    press(&mut m, KeyCode::Char('s'));
    let rid = commands(&press(&mut m, KeyCode::Char('y')))[0].request_id.clone();
    send(&mut m, &read("fixtures/server.command.result.accepted.json").replace("req-cmd", rid.as_str()));
    let quiet = "The daemon accepted this but nothing happened yet. Esc closes; the timeline will show it if it runs.";
    let t0 = m.now_ms;
    update(&mut m, Msg::Tick(t0 + 14_000));
    let (frame, _) = common::render(&mut m, 120, 38);
    assert!(frame.contains("waiting for the event"), "{frame}");
    m.dirty = false;
    update(&mut m, Msg::Tick(t0 + 15_000));
    assert!(m.dirty, "the deadline redraws");
    let (frame, _) = common::render(&mut m, 120, 38);
    assert!(frame.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ").contains(quiet), "{frame}");
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.top_modal(), None);
}
