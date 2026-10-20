#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use pr_review_tui::model::{Look, Model, Screen};
use pr_review_tui::protocol::ServerMessage;
use serde_json::{Value, json};

pub fn protocol_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol")
}

pub fn read(rel: &str) -> String {
    fs::read_to_string(protocol_dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The playback script's server messages, in order.
pub fn script(name: &str) -> Vec<ServerMessage> {
    read(&format!("playback/{name}.ndjson"))
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            serde_json::from_value(v["message"].clone()).unwrap()
        })
        .collect()
}

pub fn expect(name: &str) -> Value {
    serde_json::from_str(&read(&format!("playback/{name}.expect.json"))).unwrap()
}

pub fn model() -> Model {
    Model::new("test", Look::default(), Screen::Inbox, None)
}

/// The client state in the shape of `playback/*.expect.json`.
pub fn summary(m: &Model) -> Value {
    let mut v = json!({
        "streamEpoch": m.stream.epoch.as_ref().map(|e| e.as_str()),
        "lastSequence": m.stream.last_applied,
        "resyncRequested": m.diag.resyncs > 0,
        "toolActivityCount": m.domain.tool_activity_count,
        "prs": m.domain.prs.values().map(|p| json!({"id": p.id.as_str(), "headSha": p.head_sha.as_str(), "title": p.title.as_str()})).collect::<Vec<_>>(),
        "runs": m.domain.runs.values().map(|r| json!({"id": r.id.as_str(), "phase": r.phase, "status": r.status})).collect::<Vec<_>>(),
        "findings": m.domain.findings.values().map(|f| json!({"id": f.id.as_str(), "title": f.title.as_str(), "status": f.status, "stale": f.stale})).collect::<Vec<_>>(),
    });
    if let Some(accounts) = &m.domain.accounts {
        let mut accounts: Vec<_> = accounts.iter().collect();
        accounts.sort_by(|a, b| a.id.cmp(&b.id));
        v["accounts"] = accounts
            .iter()
            .map(|a| json!({"id": a.id.as_str(), "state": a.state, "ghActive": a.gh_active, "selected": a.selected}))
            .collect();
    }
    v
}
