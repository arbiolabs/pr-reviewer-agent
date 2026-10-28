//! Archiving is the reader's preference, kept by the daemon. The PR list leaves archived PRs out and `E` shows them
//! instead; `e` on the Review page archives the PR, or unarchives an archived one, and the cursor moves on once the
//! daemon's `pr.updated` confirms it.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{Model, Screen};
use pr_review_tui::playback::{Player, load};
use pr_review_tui::protocol::{ClientMessage, Command, CommandRequest, PrId, PrParams, ServerMessage};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};

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

fn key(m: &mut Model, c: char) -> Vec<CommandRequest> {
    let mods = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    let fx = update(m, Msg::Key(KeyEvent::new(KeyCode::Char(c), mods)));
    fx.into_iter()
        .filter_map(|e| match e {
            Effect::Send(ClientMessage::Command(c)) => Some(c),
            _ => None,
        })
        .collect()
}

fn pr_updated(seq: u64, number: u64, archived: bool) -> String {
    let at = if archived { r#","archivedAt":"2026-10-20T13:02:00Z""# } else { "" };
    format!(
        r#"{{"type":"event","streamEpoch":"epoch-1","sequence":{seq},"prevSequence":{},"occurredAt":"2026-10-20T13:02:00Z","prId":"acme/shop#{number}","eventType":"pr.updated","data":{{"id":"acme/shop#{number}","repository":"acme/shop","number":{number},"title":"T","headSha":"4444444444444444444444444444444444444444","baseSha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","lifecycle":"ACTIVE"{at}}}}}"#,
        seq - 1
    )
}

fn numbers(prs: Vec<&pr_review_tui::protocol::Pr>) -> Vec<u64> {
    prs.iter().map(|p| p.number).collect()
}

fn notice(m: &Model) -> &str {
    m.notice.as_ref().map_or("", |n| n.text.as_str())
}

#[test]
fn the_inbox_leaves_archived_prs_out_and_the_archived_list_holds_them() {
    let mut m = connected("fixtures/server.welcome.read-only.json");
    send(&mut m, &read("fixtures/server.event.pr.updated.archived.json"));
    assert_eq!((numbers(m.inbox()), numbers(m.archived())), (vec![186, 185], vec![184]));
    assert_eq!(numbers(m.pr_list()), vec![186, 185]);
}

#[test]
fn e_archives_the_pr_on_a_read_only_daemon_and_the_cursor_moves_to_the_next_one_then_e_on_the_archived_list_brings_it_back()
 {
    let mut m = connected("fixtures/server.welcome.read-only.json");
    m.ui.screen = Screen::Review;
    m.ui.selected_pr = Some(PrId::new("acme/shop#185"));

    let sent = key(&mut m, 'e');
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].command, Command::Archive(PrParams { pr_id: PrId::new("acme/shop#185") }));
    assert_eq!(sent[0].idempotency_key, "pr.archive:acme/shop#185");
    send(&mut m, &pr_updated(109, 185, true));
    assert_eq!(numbers(m.pr_list()), vec![186, 184]);
    assert_eq!(m.ui.selected_pr, Some(PrId::new("acme/shop#184")));
    assert_eq!(notice(&m), "Archived #185");

    assert!(key(&mut m, 'E').is_empty());
    assert_eq!((numbers(m.pr_list()), m.ui.selected_pr.clone()), (vec![185], Some(PrId::new("acme/shop#185"))));
    let sent = key(&mut m, 'e');
    assert_eq!(sent[0].command, Command::Unarchive(PrParams { pr_id: PrId::new("acme/shop#185") }));
    send(&mut m, &pr_updated(110, 185, false));
    // The archived list is empty now, so the rail goes back to the tracked PRs, on the one brought back.
    assert_eq!(notice(&m), "Unarchived #185");
    assert_eq!(
        (m.ui.show_archived, numbers(m.pr_list()), m.ui.selected_pr.clone()),
        (false, vec![186, 185, 184], Some(PrId::new("acme/shop#185")))
    );
}

#[test]
fn e_sends_nothing_from_the_inbox_list_or_to_a_daemon_that_does_not_grant_it() {
    let mut m = connected("fixtures/server.welcome.read-only.json");
    assert_eq!(m.ui.screen, Screen::Inbox);
    assert!(key(&mut m, 'e').is_empty());
    assert!(key(&mut m, 'E').is_empty(), "nothing is archived");
    assert!(!m.ui.show_archived);

    let mut m = connected("fixtures/server.welcome.json");
    m.ui.screen = Screen::Review;
    assert!(key(&mut m, 'e').is_empty());
    assert!(notice(&m).starts_with("Can't archive #"), "{}", notice(&m));
}

#[test]
fn playback_answers_archive_as_the_daemon_does_with_the_pr_updated_that_confirms_it() {
    let mut p = Player::new(load(&protocol_dir().join("playback/approval.ndjson")).unwrap());
    while let Some((i, _)) = p.upcoming() {
        if p.played.iter().any(|m| matches!(m, ServerMessage::Snapshot(_))) {
            break;
        }
        p.play(i);
    }
    let ServerMessage::Snapshot(s) = p.played.last().unwrap().clone() else { panic!() };
    let pr = s.prs[0].id.clone();
    let request = |command: Command| {
        ClientMessage::Command(CommandRequest {
            request_id: pr_review_tui::protocol::RequestId::new("r"),
            idempotency_key: command.idempotency_key(),
            command,
        })
    };
    let reply = p.answer(&request(Command::Archive(PrParams { pr_id: pr.clone() })));
    assert!(
        matches!(&reply[0], ServerMessage::CommandResult(r) if matches!(r.outcome, pr_review_tui::protocol::CommandOutcome::Accepted { .. }))
    );
    let (i, _) = p.upcoming().unwrap();
    let pr_review_tui::protocol::ServerMessage::Event(pr_review_tui::protocol::Event::Sequenced(e)) = p.play(i) else {
        panic!()
    };
    assert_eq!(e.sequence.get(), s.sequence + 1);
    assert!(matches!(e.body, pr_review_tui::protocol::EventBody::PrUpdated(ref x) if x.id == pr && x.archived()));
    let reply = p.answer(&request(Command::Archive(PrParams { pr_id: PrId::new("nope") })));
    assert!(
        matches!(&reply[0], ServerMessage::CommandResult(r) if matches!(r.outcome, pr_review_tui::protocol::CommandOutcome::Rejected { .. }))
    );
}
