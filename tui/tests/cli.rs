//! The binary's own flags, run as a user runs them.

use std::process::Command;

#[test]
fn version_prints_the_crate_version_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_pr-review-tui")).arg("--version").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("pr-review-tui {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn the_readme_starts_the_client_on_a_playback_that_loads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let start = &readme[readme.find("## Get started").expect("a Get started section")..];
    let first_block = start.split("```bash\n").nth(1).and_then(|b| b.split("```").next()).unwrap();
    let script = first_block.split("--playback ").nth(1).expect("the first command plays a session").trim();
    let loaded = pr_review_tui::playback::load(&root.join(script)).unwrap();
    assert!(!loaded.steps.is_empty());
}

#[tokio::test]
async fn a_playback_line_after_a_command_waits_for_that_command_and_the_command_is_accepted() {
    use pr_review_tui::net::NetCmd;
    use pr_review_tui::protocol::{ClientMessage, CommandOutcome, ServerMessage};
    use pr_review_tui::update::{Msg, NetEvent};
    use std::time::Duration;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol");
    let script = pr_review_tui::playback::load(&root.join("playback/showcase.ndjson")).unwrap();
    let gate = script.steps.iter().position(|s| s.after.as_deref() == Some("review.publish")).expect("a gated line");
    // Everything up to the gate, without delays, then the gate and one line after it.
    let mut steps = script.steps;
    steps.truncate(gate + 2);
    for s in &mut steps {
        s.delay = Duration::ZERO;
    }
    let script = pr_review_tui::playback::Script { steps, fixtures: root.join("fixtures") };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(pr_review_tui::playback::run(script, tx, cmd_rx));
    let mut before = 0;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
        before += 1;
    }
    assert_eq!(before, gate + 1, "Opened and every line before the gate, nothing after");

    let publish = std::fs::read_to_string(root.join("fixtures/client.command.review.publish.json")).unwrap();
    let ClientMessage::Command(c) = serde_json::from_str(&publish).unwrap() else { panic!() };
    cmd_tx.send(NetCmd::Send(ClientMessage::Command(c))).await.unwrap();
    let Some(Msg::Net(NetEvent::Message(ServerMessage::CommandResult(r)))) = rx.recv().await else { panic!() };
    assert!(
        matches!(r.outcome, CommandOutcome::Accepted { ref operation_id } if operation_id.as_str() == "review.publish:rev-184-a")
    );
    for _ in 0..2 {
        let next = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.unwrap();
        assert!(matches!(next, Some(Msg::Net(NetEvent::Message(ServerMessage::Event(_))))), "{next:?}");
    }
}
