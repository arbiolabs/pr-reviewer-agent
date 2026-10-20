//! Every committed protocol fixture against the Rust wire types.

use std::fs;
use std::path::{Path, PathBuf};

use pr_review_tui::protocol::*;

fn protocol_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol")
}

fn fixtures(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), fs::read_to_string(&p).unwrap()))
        .collect();
    out.sort();
    out
}

#[test]
fn every_valid_fixture_parses() {
    let all = fixtures(&protocol_dir().join("fixtures"));
    assert!(all.len() >= 40, "found only {} fixtures", all.len());
    for (name, body) in all {
        if name.starts_with("server.") {
            ServerMessage::parse(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
        } else {
            let parsed: ClientMessage = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
            // What the client would send for it is the fixture itself.
            let reencoded = serde_json::to_value(&parsed).unwrap();
            let original: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(reencoded, original, "{name} does not round-trip");
        }
    }
}

fn invalid(name: &str) -> String {
    fs::read_to_string(protocol_dir().join(format!("fixtures/invalid/{name}.json"))).unwrap()
}

fn sequenced(msg: ServerMessage) -> SequencedEvent {
    match msg {
        ServerMessage::Event(Event::Sequenced(e)) => e,
        other => panic!("not a sequenced event: {other:?}"),
    }
}

#[test]
fn invalid_fixtures_are_rejected_or_handled_as_their_why_says() {
    let rejected_server = [
        "server.command.result.accepted-no-operation",
        "server.event.heartbeat.no-epoch",
        "server.event.missing-prev-sequence",
        "server.event.sequence-zero",
        "server.finding.line-zero",
        "server.snapshot.account-bad-shape",
    ];
    for name in rejected_server {
        assert!(ServerMessage::parse(&invalid(name)).is_err(), "{name} parsed");
    }
    let rejected_client = [
        "client.command.account-select.no-account-id",
        "client.command.extra-field",
        "client.command.unknown-command",
        "client.command.unknown-param",
        "client.diff.get.limit-zero",
        "client.hello.no-request-id",
        "client.unknown-type",
    ];
    for name in rejected_client {
        assert!(serde_json::from_str::<ClientMessage>(&invalid(name)).is_err(), "{name} parsed");
    }

    // Unknown enum strings become Unknown instead of failing the message (protocol/README.md, forward compatibility),
    // and a known event type never falls through to the unknown-event shape.
    let e = sequenced(ServerMessage::parse(&invalid("server.event.finding.bad-severity")).unwrap());
    let EventBody::FindingUpdated(f) = e.body else { panic!("{:?}", e.body) };
    assert_eq!(f.severity, Severity::Unknown);

    let e = sequenced(ServerMessage::parse(&invalid("server.event.phase.cancelled")).unwrap());
    let EventBody::PhaseChanged(p) = e.body else { panic!("{:?}", e.body) };
    assert_eq!((p.from, p.to), (Phase::Verify, Phase::Unknown));

    let ServerMessage::Error { code, .. } = ServerMessage::parse(&invalid("server.error.unknown-code")).unwrap() else {
        panic!()
    };
    assert_eq!(code, ErrorCode::Unknown);

    let e = sequenced(ServerMessage::parse(&invalid("server.event.tool.activity.long-summary")).unwrap());
    let EventBody::ToolActivity(a) = e.body else { panic!() };
    assert_eq!(a.summary.as_str(), "x".repeat(200));

    // A v2 welcome parses; the reducer turns it into Incompatible (tests/reducer.rs).
    let ServerMessage::Welcome { protocol_version, .. } =
        ServerMessage::parse(&invalid("server.welcome.version-2")).unwrap()
    else {
        panic!()
    };
    assert_eq!(protocol_version, 2);

    // A publication status outside the closed list is Unknown, never a parse failure.
    let e = sequenced(ServerMessage::parse(&invalid("server.event.publication.bad-status")).unwrap());
    let EventBody::PublicationChanged(p) = e.body else { panic!("{:?}", e.body) };
    assert_eq!(p.status, PublicationStatus::Unknown);

    let covered = rejected_server.len() + rejected_client.len() + 6;
    assert_eq!(covered, fixtures(&protocol_dir().join("fixtures/invalid")).len(), "an invalid fixture is untested");
}

#[test]
fn m3_messages_parse_into_the_client_enums() {
    let read = |name: &str| fs::read_to_string(protocol_dir().join(format!("fixtures/{name}.json"))).unwrap();

    let ServerMessage::PublicationPreview(p) = ServerMessage::parse(&read("server.publication.preview")).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (p.run_id.as_str(), p.status, p.event.as_str(), p.comments.len()),
        ("rev-184-a", PublicationStatus::AwaitingApproval, "COMMENT", 1)
    );
    assert_eq!(p.preview_id.as_str().len(), 64);
    assert!(p.body.as_str().ends_with(&format!("<!-- durable-review:rev-184-a:{} -->\n", p.preview_id)));
    assert_eq!(
        (p.comments[0].path.as_str(), p.comments[0].line.get(), p.comments[0].side.as_str()),
        ("src/booking.ts", 42, "RIGHT")
    );

    // The approval names exactly the preview it saw: the fixture command carries the preview's hash in its key.
    let ClientMessage::Command(c) = serde_json::from_str(&read("client.command.review.publish")).unwrap() else {
        panic!()
    };
    let Command::Publish(params) = &c.command else { panic!() };
    assert_eq!(params.preview_id, p.preview_id);
    assert_eq!(c.idempotency_key, c.command.idempotency_key());

    let get: ClientMessage = serde_json::from_str(&read("client.publication.get")).unwrap();
    assert!(matches!(&get, ClientMessage::PublicationGet { run_id, .. } if run_id.as_str() == "rev-184-a"));

    for (name, status, review) in [
        ("server.event.review.publication_changed", PublicationStatus::AwaitingApproval, None),
        ("server.event.review.publication_changed.published", PublicationStatus::Published, Some(3141)),
    ] {
        let e = sequenced(ServerMessage::parse(&read(name)).unwrap());
        let EventBody::PublicationChanged(changed) = e.body else { panic!("{name}: {:?}", e.body) };
        assert_eq!(
            (changed.run_id.as_str(), changed.status, changed.review_id),
            ("rev-184-a", status, review),
            "{name}"
        );
        assert_eq!(changed.url.is_some(), review.is_some());
    }

    for (name, mode, granted) in [
        ("server.welcome", None, 0),
        ("server.welcome.engine", Some(DaemonMode::Engine), 5),
        ("server.welcome.read-only", Some(DaemonMode::ReadOnly), 1),
    ] {
        let ServerMessage::Welcome { mode: m, capabilities, read_only_reason, .. } =
            ServerMessage::parse(&read(name)).unwrap()
        else {
            panic!()
        };
        assert_eq!((m, capabilities.len()), (mode, granted), "{name}");
        assert_eq!(read_only_reason.is_some(), mode == Some(DaemonMode::ReadOnly), "{name}");
    }
}

#[test]
fn unknown_fields_and_event_types_are_ignored() {
    let body = fs::read_to_string(protocol_dir().join("fixtures/server.event.extra-fields.json")).unwrap();
    let e = sequenced(ServerMessage::parse(&body).unwrap());
    let EventBody::FindingUpdated(f) = e.body else { panic!() };
    assert_eq!((f.id.as_str(), f.line.map(|l| l.get())), ("f-1", Some(42)));

    let body = fs::read_to_string(protocol_dir().join("fixtures/server.event.unknown-type.json")).unwrap();
    let e = sequenced(ServerMessage::parse(&body).unwrap());
    assert_eq!((e.sequence.get(), e.prev_sequence), (109, 108));
    assert_eq!(e.body, EventBody::Other(Text::new("review.reinvestigation_requested")));
}

#[test]
fn accounts_parse_from_snapshot_and_event_and_are_optional_in_old_snapshots() {
    let body = fs::read_to_string(protocol_dir().join("fixtures/server.snapshot.accounts.json")).unwrap();
    let ServerMessage::Snapshot(s) = ServerMessage::parse(&body).unwrap() else { panic!() };
    let ids: Vec<_> = s.accounts.unwrap().iter().map(|a| (a.id.to_string(), a.state, a.gh_active)).collect();
    assert_eq!(
        ids,
        [
            ("env/GITHUB_TOKEN".to_string(), AccountState::Ok, false),
            ("github.com/diego-arbio-dev".to_string(), AccountState::Ok, true),
            ("github.com/octo-work".to_string(), AccountState::Ok, false),
            ("github.com/old-bot".to_string(), AccountState::Error, false),
        ]
    );

    let body = fs::read_to_string(protocol_dir().join("fixtures/server.snapshot.json")).unwrap();
    let ServerMessage::Snapshot(s) = ServerMessage::parse(&body).unwrap() else { panic!() };
    assert_eq!(s.accounts, None);

    let body = fs::read_to_string(protocol_dir().join("fixtures/server.event.accounts-updated.json")).unwrap();
    let e = sequenced(ServerMessage::parse(&body).unwrap());
    let EventBody::AccountsUpdated(a) = e.body else { panic!("{:?}", e.body) };
    let selected: Vec<_> = a.iter().filter(|a| a.selected).map(|a| a.id.as_str()).collect();
    assert_eq!(selected, ["github.com/octo-work"]);

    let body = fs::read_to_string(protocol_dir().join("fixtures/client.command.account-select.json")).unwrap();
    let ClientMessage::Command(c) = serde_json::from_str(&body).unwrap() else { panic!() };
    assert_eq!(c.idempotency_key, c.command.idempotency_key());
}

#[test]
fn hostile_snapshot_strings_arrive_sanitized() {
    let body = fs::read_to_string(protocol_dir().join("fixtures/server.snapshot.hostile.json")).unwrap();
    let ServerMessage::Snapshot(s) = ServerMessage::parse(&body).unwrap() else { panic!() };
    for pr in &s.prs {
        let t = pr.title.as_str();
        assert!(!t.chars().any(|c| c.is_control() && c != '\n' && c != '\t'), "{t:?}");
    }
}
