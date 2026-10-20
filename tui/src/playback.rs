//! `--playback FILE.ndjson`: plays a scripted server session into the app with no socket. The script owns the stream;
//! requests the script cannot know about (diff, finding detail, timeline) are answered from `protocol/fixtures/`. A
//! timeline is the events already played on top of the history the `server.timeline.page.showcase-*` pages recorded
//! before the session opened.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::net::NetCmd;
use crate::protocol::{ClientMessage, EventBody, SequencedEvent, ServerMessage};
use crate::update::{Msg, NetEvent};

pub struct Script {
    pub steps: Vec<(Duration, ServerMessage)>,
    pub fixtures: PathBuf,
}

pub fn load(path: &Path) -> Result<Script, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        let delay = Duration::from_millis(v["delayMs"].as_u64().unwrap_or(0));
        let msg =
            serde_json::from_value(v["message"].clone()).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        steps.push((delay, msg));
    }
    let fixtures = path.parent().unwrap_or(Path::new(".")).join("../fixtures");
    Ok(Script { steps, fixtures })
}

pub async fn run(script: Script, tx: mpsc::Sender<Msg>, mut rx: mpsc::Receiver<NetCmd>) {
    let mut played: Vec<SequencedEvent> = Vec::new();
    if tx.send(Msg::Net(NetEvent::Opened)).await.is_err() {
        return;
    }
    let mut steps = script.steps.into_iter();
    let mut next = steps.next();
    loop {
        let delay = next.as_ref().map(|(d, _)| *d);
        tokio::select! {
            _ = tokio::time::sleep(delay.unwrap_or(Duration::MAX)), if delay.is_some() => {
                let Some((_, msg)) = next.take() else { continue };
                if let ServerMessage::Event(crate::protocol::Event::Sequenced(e)) = &msg {
                    played.push(e.clone());
                }
                if tx.send(Msg::Net(NetEvent::Message(msg))).await.is_err() {
                    return;
                }
                next = steps.next();
            }
            cmd = rx.recv() => match cmd {
                Some(NetCmd::Send(req)) => {
                    if let Some(reply) = answer(&script.fixtures, &played, &req)
                        && tx.send(Msg::Net(NetEvent::Message(reply))).await.is_err()
                    {
                        return;
                    }
                }
                Some(NetCmd::Close) | None => return,
            },
        }
    }
}

/// Every `<prefix>*.json` fixture, by file name, with `requestId` set to `request_id`.
fn fixtures_named(dir: &Path, prefix: &str, request_id: &str) -> Vec<(String, Value)> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            let mut v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(&n)).ok()?).ok()?;
            v["requestId"] = json!(request_id);
            Some((n, v))
        })
        .collect()
}

fn not_found(request_id: &str, what: String) -> Value {
    json!({ "type": "error", "requestId": request_id, "code": "NOT_FOUND", "message": format!("{what} (playback)") })
}

pub fn answer(fixtures: &Path, played: &[SequencedEvent], req: &ClientMessage) -> Option<ServerMessage> {
    let rid = req.request_id().as_str();
    let reply = match req {
        // The script plays the server side of the handshake and the stream.
        ClientMessage::Hello { .. } | ClientMessage::SnapshotGet { .. } | ClientMessage::Resume { .. } => return None,
        ClientMessage::DiffGet { pr_id, base_sha, head_sha, cursor, .. } => {
            // `server.diff.page.last.json` is the only later page; every other diff fixture is a first page.
            fixtures_named(fixtures, "server.diff.page", rid)
                .into_iter()
                .filter(|(n, _)| (n == "server.diff.page.last.json") == cursor.is_some())
                .map(|(_, v)| v)
                .find(|v| {
                    v["prId"] == pr_id.as_str()
                        && v["baseSha"] == base_sha.as_str()
                        && v["headSha"] == head_sha.as_str()
                })
                .unwrap_or_else(|| not_found(rid, format!("no diff for {pr_id} at {}", head_sha.short())))
        }
        ClientMessage::FindingGet { finding_id, .. } => fixtures_named(fixtures, "server.finding.detail", rid)
            .into_iter()
            .map(|(_, v)| v)
            .find(|v| v["finding"]["id"] == finding_id.as_str())
            .unwrap_or_else(|| not_found(rid, format!("no detail for {finding_id}"))),
        ClientMessage::TimelineGet { pr_id, run_id, finding_id, before_sequence, limit, .. } => {
            let limit = u16::from(*limit) as usize;
            let history: Vec<SequencedEvent> = fixtures_named(fixtures, "server.timeline.page.showcase-", rid)
                .into_iter()
                .filter_map(|(_, v)| match serde_json::from_value(v).ok()? {
                    ServerMessage::TimelinePage { events, .. } => Some(events),
                    _ => None,
                })
                .flatten()
                .filter(|e| played.iter().all(|p| p.sequence != e.sequence))
                .collect();
            let mut all: Vec<&SequencedEvent> = played.iter().chain(&history).collect();
            all.sort_by_key(|e| std::cmp::Reverse(e.sequence.get()));
            let matching: Vec<&SequencedEvent> = all
                .into_iter()
                .filter(|e| before_sequence.is_none_or(|b| e.sequence.get() < b))
                .filter(|e| pr_id.is_none() || pr_id == &e.pr_id)
                .filter(|e| run_id.is_none() || run_id == &e.run_id)
                .filter(|e| match (finding_id, &e.body) {
                    (None, _) => true,
                    (Some(id), EventBody::FindingUpdated(f)) => &f.id == id,
                    _ => false,
                })
                .collect();
            let page: Vec<&SequencedEvent> = matching.iter().take(limit).copied().collect();
            let next = (matching.len() > limit).then(|| page.last().map(|e| e.sequence.get())).flatten();
            return Some(ServerMessage::TimelinePage {
                request_id: Some(req.request_id().clone()),
                events: page.into_iter().cloned().collect(),
                next_before_sequence: next,
            });
        }
        ClientMessage::PublicationGet { run_id, .. } => fixtures_named(fixtures, "server.publication.preview", rid)
            .into_iter()
            .map(|(_, v)| v)
            .find(|v| v["runId"] == run_id.as_str())
            .unwrap_or_else(|| not_found(rid, format!("no preview for {run_id}"))),
        ClientMessage::Command(_) => json!({
            "type": "command.result", "requestId": rid, "status": "rejected", "code": "CAPABILITY_DENIED",
            "message": "commands are not available in playback",
        }),
    };
    serde_json::from_value(reply).ok()
}
