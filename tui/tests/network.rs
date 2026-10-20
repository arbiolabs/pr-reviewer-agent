//! Playback scripts through the real network task, against a mock server on a tokio `UnixListener`.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use pr_review_tui::app::dispatch;
use pr_review_tui::model::{ACTIVITY_CAP, Link, Model};
use pr_review_tui::net::{self, NetCmd, NetConfig, QUEUE};
use pr_review_tui::update::Msg;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

fn socket_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pr-review-tui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn script_lines(name: &str) -> Vec<String> {
    read(&format!("playback/{name}.ndjson"))
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["message"].to_string())
        .collect()
}

type Seen = Arc<Mutex<Vec<Value>>>;

/// Records every client line; returns the write half.
fn record(stream: UnixStream, seen: Seen) -> tokio::net::unix::OwnedWriteHalf {
    let (read, write) = stream.into_split();
    tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            seen.lock().unwrap().push(serde_json::from_str(&l).unwrap());
        }
    });
    write
}

async fn send_lines(w: &mut tokio::net::unix::OwnedWriteHalf, lines: &[String]) {
    for l in lines {
        w.write_all(format!("{l}\n").as_bytes()).await.unwrap();
    }
}

struct Client {
    model: Model,
    msgs: mpsc::Receiver<Msg>,
    cmds: mpsc::Sender<NetCmd>,
}

fn client(socket: PathBuf) -> Client {
    let (msg_tx, msgs) = mpsc::channel(QUEUE);
    let (cmds, cmd_rx) = mpsc::channel(QUEUE);
    let cfg = NetConfig { socket, heartbeat_timeout: Duration::from_secs(30), reconnect: true };
    tokio::spawn(net::run(cfg, msg_tx, cmd_rx));
    Client { model: model(), msgs, cmds }
}

impl Client {
    /// Runs the app loop until `done` holds, or fails after five seconds.
    async fn until(&mut self, what: &str, done: impl Fn(&Model) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done(&self.model) {
            match tokio::time::timeout_at(deadline, self.msgs.recv()).await {
                Ok(Some(msg)) => {
                    dispatch(&mut self.model, msg, &self.cmds);
                }
                _ => panic!("timed out waiting for {what}; state: {:#}", summary(&self.model)),
            }
        }
    }
}

fn types(seen: &Seen) -> Vec<String> {
    seen.lock().unwrap().iter().map(|v| v["type"].as_str().unwrap().to_string()).collect()
}

async fn wait_for(seen: &Seen, n: usize) {
    for _ in 0..200 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn play(name: &str) -> (Client, Seen) {
    let dir = socket_dir(name);
    let sock = dir.join("reviewer.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let seen: Seen = Default::default();
    let lines = script_lines(name);
    let server_seen = seen.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut w = record(stream, server_seen);
        send_lines(&mut w, &lines).await;
        std::future::pending::<()>().await;
    });
    let mut c = client(sock);
    let want = expect(name);
    c.until(&format!("{name} expected state"), |m| summary(m) == want).await;
    (c, seen)
}

#[tokio::test]
#[ignore = "design prototype C: evidence prefetch, landing selection and start view change reducer output"]
async fn normal_over_a_socket() {
    let (c, seen) = play("normal").await;
    wait_for(&seen, 2).await;
    assert_eq!(types(&seen), ["hello", "snapshot.get"]);
    assert_eq!(seen.lock().unwrap()[0]["protocolVersion"], 1);
    assert_eq!(c.model.link, Link::Live);
}

#[tokio::test]
#[ignore = "design prototype C: evidence prefetch, landing selection and start view change reducer output"]
async fn gap_over_a_socket_asks_for_a_new_snapshot() {
    let (c, seen) = play("gap").await;
    wait_for(&seen, 3).await;
    assert_eq!(types(&seen), ["hello", "snapshot.get", "snapshot.get"]);
    assert_eq!(c.model.link, Link::Resyncing);
}

#[tokio::test]
async fn burst_over_a_socket_stays_bounded() {
    let start = Instant::now();
    let (c, _) = play("burst").await;
    let took = start.elapsed();
    assert!(c.model.domain.activity.len() <= ACTIVITY_CAP);
    assert!(took < Duration::from_secs(2), "burst over the socket took {took:?}");
}

#[tokio::test]
#[ignore = "design prototype C: evidence prefetch, landing selection and start view change reducer output"]
async fn a_dropped_connection_resumes_after_the_last_applied_sequence() {
    let dir = socket_dir("resume");
    let sock = dir.join("reviewer.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let lines = script_lines("normal");
    let first: Seen = Default::default();
    let second: Seen = Default::default();
    let (s1, s2) = (first.clone(), second.clone());
    tokio::spawn(async move {
        // welcome, snapshot, seq 11, heartbeat; then the daemon "restarts" mid-VERIFY.
        let (stream, _) = listener.accept().await.unwrap();
        let mut w = record(stream, s1);
        send_lines(&mut w, &lines[..4]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(w);
        let (stream, _) = listener.accept().await.unwrap();
        let mut w = record(stream, s2.clone());
        wait_for(&s2, 1).await;
        send_lines(&mut w, &lines[..1]).await;
        wait_for(&s2, 2).await;
        send_lines(&mut w, &lines[4..]).await;
        std::future::pending::<()>().await;
    });
    let mut c = client(sock);
    c.until("seq 11", |m| m.stream.last_applied == 11).await;
    c.until("the disconnect", |m| matches!(m.link, Link::Disconnected { .. })).await;
    let run = &c.model.domain.runs.values().find(|r| r.id.as_str() == "rev-184-a").unwrap();
    assert_eq!(serde_json::to_value((run.phase, run.status)).unwrap(), serde_json::json!(["VERIFY", "RUNNING"]));

    let want = expect("normal");
    c.until("normal expected state after resume", |m| summary(m) == want).await;
    let resume = second.lock().unwrap()[1].clone();
    assert_eq!(types(&second), ["hello", "resume"]);
    assert_eq!((resume["streamEpoch"].as_str(), resume["afterSequence"].as_u64()), (Some("epoch-1"), Some(11)));
    assert_eq!(c.model.diag.reconnects, 1);
    assert_eq!(c.model.diag.resyncs, 0);
}

#[tokio::test]
async fn a_protocol_mismatch_stops_reconnecting() {
    let dir = socket_dir("mismatch");
    let sock = dir.join("reviewer.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let accepts = Arc::new(Mutex::new(0));
    let counter = accepts.clone();
    let mismatch = read("fixtures/server.error.protocol-mismatch.json").replace('\n', "");
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            *counter.lock().unwrap() += 1;
            let mut w = record(stream, Default::default());
            send_lines(&mut w, std::slice::from_ref(&mismatch)).await;
        }
    });
    let mut c = client(sock);
    c.until("incompatible", |m| matches!(m.link, Link::Incompatible { .. })).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    while let Ok(msg) = c.msgs.try_recv() {
        dispatch(&mut c.model, msg, &c.cmds);
    }
    assert!(matches!(c.model.link, Link::Incompatible { .. }));
    assert_eq!(*accepts.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_socket_in_a_shared_directory_is_refused() {
    let dir = socket_dir("loose");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sock = dir.join("reviewer.sock");
    let _listener = UnixListener::bind(&sock).unwrap();
    let mut c = client(sock);
    c.until("refusal", |m| matches!(m.link, Link::Disconnected { .. })).await;
    let Link::Disconnected { reason, .. } = &c.model.link else { unreachable!() };
    assert_eq!(reason.as_str(), format!("socket directory has mode 755, must be 0700: {}", dir.display()));
}

#[tokio::test]
async fn a_socket_path_longer_than_sun_path_is_refused_without_dialing() {
    let sock = PathBuf::from(format!("/tmp/{}/reviewer.sock", "d".repeat(110)));
    let mut c = client(sock);
    c.until("unusable", |m| matches!(m.link, Link::Unusable { .. })).await;
    let Link::Unusable { message } = &c.model.link else { unreachable!() };
    let limit = net::SOCKET_PATH_MAX;
    assert!(message.as_str().starts_with(&format!("socket path is 129 bytes (max {limit} on ")), "{message}");
    assert!(message.as_str().ends_with("use a shorter --socket"), "{message}");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(c.msgs.try_recv().is_err(), "the network task kept going after refusing the path");
}
