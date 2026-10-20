//! The network task: Unix socket, NDJSON framing with a 1 MiB line cap, jittered reconnect, heartbeat timeout, and a
//! bounded queue into the app that never blocks on events. Protocol decisions belong to the reducer, not here.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::{Instant, sleep, sleep_until};

use crate::protocol::{ClientMessage, Event, EventBody, MAX_LINE_BYTES, ServerMessage};
use crate::update::{ActivityBatch, Msg, NetEvent};

pub const QUEUE: usize = 1024;
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(10);
const FLUSH_EVERY: Duration = Duration::from_millis(16);
/// sun_path holds 104 bytes on macOS and 108 on Linux, the terminating NUL included. A longer path never connects.
pub const SOCKET_PATH_MAX: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

#[derive(Debug)]
pub enum NetCmd {
    Send(ClientMessage),
    Close,
}

#[derive(Clone, Debug)]
pub struct NetConfig {
    pub socket: PathBuf,
    pub heartbeat_timeout: Duration,
    pub reconnect: bool,
}

enum End {
    Closed,
    Lost(String),
}

/// Runs until the app goes away or sends `NetCmd::Close`.
pub async fn run(cfg: NetConfig, tx: mpsc::Sender<Msg>, mut rx: mpsc::Receiver<NetCmd>) {
    if let Err(reason) = check_socket_path(&cfg.socket) {
        if tx.send(Msg::Net(NetEvent::Unusable(reason))).await.is_ok() {
            while !matches!(rx.recv().await, None | Some(NetCmd::Close)) {}
        }
        return;
    }
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if tx.send(Msg::Net(NetEvent::Dialing { attempt })).await.is_err() {
            return;
        }
        let reason = match connect(&cfg.socket).await {
            Ok(stream) => {
                attempt = 0;
                if tx.send(Msg::Net(NetEvent::Opened)).await.is_err() {
                    return;
                }
                match session(stream, &cfg, &tx, &mut rx).await {
                    End::Closed => return,
                    End::Lost(reason) => reason,
                }
            }
            Err(e) => e,
        };
        let retry = cfg.reconnect.then(|| backoff(attempt.max(1)));
        let lost = NetEvent::Lost { reason, retry_in_ms: retry.map(|d| d.as_millis() as u64) };
        if tx.send(Msg::Net(lost)).await.is_err() {
            return;
        }
        let wait = async {
            match retry {
                Some(d) => sleep(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(wait);
        // Requests made while offline are dropped; the reducer marked its pending ones failed when the link was lost.
        loop {
            tokio::select! {
                _ = &mut wait => break,
                cmd = rx.recv() => if matches!(cmd, None | Some(NetCmd::Close)) { return },
            }
        }
    }
}

/// Jittered exponential backoff: a uniform pick in [d/2, d], with d doubling from 250 ms to 10 s.
pub fn backoff(attempt: u32) -> Duration {
    let d = BACKOFF_MIN.saturating_mul(1 << attempt.saturating_sub(1).min(16)).min(BACKOFF_MAX);
    let jitter = RandomState::new().hash_one(attempt) % (d.as_millis() as u64 / 2 + 1);
    d / 2 + Duration::from_millis(jitter)
}

pub fn check_socket_path(socket: &Path) -> Result<(), String> {
    let len = socket.as_os_str().len();
    if len <= SOCKET_PATH_MAX {
        return Ok(());
    }
    Err(format!(
        "socket path is {len} bytes (max {SOCKET_PATH_MAX} on {}); use a shorter --socket",
        std::env::consts::OS
    ))
}

/// Refuses a socket whose directory another user could tamper with. Messages put the reason first and the path last,
/// so a truncated banner still says what is wrong.
pub fn check_socket_dir(socket: &Path) -> Result<(), String> {
    let dir = socket.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let meta = std::fs::symlink_metadata(dir).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => format!("socket directory does not exist: {}", dir.display()),
        _ => format!("socket directory unreadable ({}): {}", io_reason(&e), dir.display()),
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!("socket directory is a symlink: {}", dir.display()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!("socket directory has mode {mode:o}, must be 0700: {}", dir.display()));
    }
    match std::fs::symlink_metadata(socket) {
        Ok(m) if !m.file_type().is_socket() => Err(format!("not a socket: {}", socket.display())),
        _ => Ok(()),
    }
}

/// "Connection refused", without the " (os error 61)" that only costs banner width.
fn io_reason(e: &io::Error) -> String {
    let text = e.to_string();
    match text.rfind(" (os error ") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

async fn connect(socket: &Path) -> Result<UnixStream, String> {
    check_socket_dir(socket)?;
    // The endpoint is on screen already; the path would only push the reason out of view.
    UnixStream::connect(socket).await.map_err(|e| io_reason(&e))
}

async fn session(stream: UnixStream, cfg: &NetConfig, tx: &mpsc::Sender<Msg>, rx: &mut mpsc::Receiver<NetCmd>) -> End {
    let (read, mut write) = stream.into_split();
    let mut lines = LineReader::new(read);
    let mut outbox = Outbox::default();
    let mut flush = tokio::time::interval(FLUSH_EVERY);
    let mut deadline = Instant::now() + cfg.heartbeat_timeout;
    loop {
        let step = tokio::select! {
            line = lines.next_line() => {
                deadline = Instant::now() + cfg.heartbeat_timeout;
                match line {
                    Ok(Some(Line::Text(text))) => match ServerMessage::parse(&text) {
                        Ok(msg) => outbox.deliver(tx, msg).await,
                        Err(e) => tx.send(Msg::Net(NetEvent::Garbled(e.to_string()))).await.map_err(|_| AppGone),
                    },
                    Ok(Some(Line::TooLong)) => outbox.deliver_net(tx, NetEvent::LineTooLong).await,
                    Ok(None) => return End::Lost("server closed the connection".into()),
                    Err(e) => return End::Lost(io_reason(&e)),
                }
            }
            cmd = rx.recv() => match cmd {
                Some(NetCmd::Send(msg)) => {
                    let mut bytes = serde_json::to_vec(&msg).unwrap_or_default();
                    bytes.push(b'\n');
                    if let Err(e) = write.write_all(&bytes).await {
                        return End::Lost(io_reason(&e));
                    }
                    Ok(())
                }
                Some(NetCmd::Close) | None => return End::Closed,
            },
            _ = sleep_until(deadline) => {
                return End::Lost(format!("no message for {}s (heartbeat timeout)", cfg.heartbeat_timeout.as_secs()));
            }
            _ = flush.tick() => outbox.flush(tx),
        };
        if step.is_err() {
            return End::Closed;
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Line {
    Text(String),
    TooLong,
}

/// Newline framing with a hard cap. An oversized line is skipped to its newline and reported, never buffered.
pub struct LineReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
    discarding: bool,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(r: R) -> Self {
        LineReader { inner: BufReader::new(r), buf: Vec::new(), discarding: false }
    }

    /// Cancel-safe: progress lives in `self` before every await point.
    pub async fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                return Ok(None);
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let chunk = &available[..newline.unwrap_or(available.len())];
            if !self.discarding && self.buf.len() + chunk.len() > MAX_LINE_BYTES {
                self.discarding = true;
                self.buf = Vec::new();
            }
            if !self.discarding {
                self.buf.extend_from_slice(chunk);
            }
            let used = chunk.len() + usize::from(newline.is_some());
            self.inner.consume(used);
            if newline.is_none() {
                continue;
            }
            if std::mem::take(&mut self.discarding) {
                return Ok(Some(Line::TooLong));
            }
            let line = std::mem::take(&mut self.buf);
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Ok(Some(Line::Text(String::from_utf8_lossy(&line).into_owned())));
        }
    }
}

/// Delivery into the bounded app queue. When the queue is full, consecutive `tool.activity` events merge into one
/// batch; a domain event that does not fit is dropped and replaced by an `Overflow` marker, which makes the reducer
/// resync. Responses (snapshots, pages) wait for room: they are rare and the user asked for them.
#[derive(Default)]
pub struct Outbox {
    pending: Option<ActivityBatch>,
    overflow: bool,
}

/// The app side of the queue is gone; the network task should stop.
#[derive(Debug)]
pub struct AppGone;

impl Outbox {
    pub fn flush(&mut self, tx: &mpsc::Sender<Msg>) -> Result<(), AppGone> {
        if self.overflow {
            match tx.try_send(Msg::Net(NetEvent::Overflow)) {
                Ok(()) => {
                    self.overflow = false;
                    self.pending = None;
                }
                Err(TrySendError::Full(_)) => return Ok(()),
                Err(TrySendError::Closed(_)) => return Err(AppGone),
            }
        }
        if let Some(batch) = self.pending.take() {
            match tx.try_send(Msg::Net(NetEvent::Activities(batch))) {
                Ok(()) => {}
                Err(TrySendError::Full(Msg::Net(NetEvent::Activities(b)))) => self.pending = Some(b),
                Err(_) => return Err(AppGone),
            }
        }
        Ok(())
    }

    pub async fn deliver(&mut self, tx: &mpsc::Sender<Msg>, msg: ServerMessage) -> Result<(), AppGone> {
        self.flush(tx)?;
        let backed_up = self.overflow || self.pending.is_some();
        match msg {
            ServerMessage::Event(Event::Heartbeat { .. }) if backed_up => Ok(()),
            ServerMessage::Event(Event::Heartbeat { .. }) => match tx.try_send(Msg::Net(NetEvent::Message(msg))) {
                Err(TrySendError::Closed(_)) => Err(AppGone),
                _ => Ok(()),
            },
            ServerMessage::Event(Event::Sequenced(e)) => {
                if self.overflow {
                    return Ok(());
                }
                if let Some(batch) = &mut self.pending {
                    if batch.absorb(e).is_err() {
                        tracing::warn!("app queue full on a domain event; forcing a resync");
                        self.overflow = true;
                        self.pending = None;
                    }
                    return Ok(());
                }
                match tx.try_send(Msg::Net(NetEvent::Message(ServerMessage::Event(Event::Sequenced(e))))) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(Msg::Net(NetEvent::Message(ServerMessage::Event(Event::Sequenced(e)))))) => {
                        if matches!(e.body, EventBody::ToolActivity(_)) {
                            self.pending = Some(ActivityBatch::start(e));
                        } else {
                            tracing::warn!("app queue full on a domain event; forcing a resync");
                            self.overflow = true;
                        }
                        Ok(())
                    }
                    Err(_) => Err(AppGone),
                }
            }
            other => self.deliver_net(tx, NetEvent::Message(other)).await,
        }
    }

    /// Waits for room, after first handing over anything held back, so order is kept.
    pub async fn deliver_net(&mut self, tx: &mpsc::Sender<Msg>, e: NetEvent) -> Result<(), AppGone> {
        if std::mem::take(&mut self.overflow) {
            self.pending = None;
            tx.send(Msg::Net(NetEvent::Overflow)).await.map_err(|_| AppGone)?;
        }
        if let Some(batch) = self.pending.take() {
            tx.send(Msg::Net(NetEvent::Activities(batch))).await.map_err(|_| AppGone)?;
        }
        tx.send(Msg::Net(e)).await.map_err(|_| AppGone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Look, Model, Screen};
    use crate::update::update;

    fn activity(seq: u64) -> ServerMessage {
        ServerMessage::parse(&format!(
            r#"{{"type":"event","streamEpoch":"e","sequence":{seq},"prevSequence":{},"occurredAt":"t","prId":"p","runId":"r","eventType":"tool.activity","data":{{"tool":"read_file","summary":"f{seq}"}}}}"#,
            seq - 1
        ))
        .unwrap()
    }

    fn finding_event(seq: u64) -> ServerMessage {
        ServerMessage::parse(&format!(
            r#"{{"type":"event","streamEpoch":"e","sequence":{seq},"prevSequence":{},"occurredAt":"t","prId":"p","runId":"r","eventType":"finding.updated","data":{{"id":"f","prId":"p","runId":"r","fingerprint":"x","title":"t","category":"general","severity":"low","status":"DETECTED","confidence":0.5,"file":"a","line":null,"evidenceCount":0,"stale":false}}}}"#,
            seq - 1
        ))
        .unwrap()
    }

    fn live_model() -> Model {
        let mut m = Model::new("t", Look::default(), Screen::Inbox, None);
        update(&mut m, Msg::Net(NetEvent::Opened));
        let snap = r#"{"type":"snapshot","streamEpoch":"e","sequence":10,"prs":[],"runs":[],"findings":[]}"#;
        update(&mut m, Msg::Net(NetEvent::Message(ServerMessage::parse(snap).unwrap())));
        m
    }

    #[tokio::test]
    async fn a_full_queue_coalesces_activity_without_breaking_the_sequence_chain() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut out = Outbox::default();
        for seq in 11..=15 {
            out.deliver(&tx, activity(seq)).await.unwrap();
        }
        let mut m = live_model();
        let first = rx.try_recv().unwrap();
        assert!(matches!(&first, Msg::Net(NetEvent::Message(_))));
        update(&mut m, first);
        out.flush(&tx).unwrap();
        let Msg::Net(NetEvent::Activities(b)) = rx.try_recv().unwrap() else { panic!() };
        assert_eq!((b.first_sequence, b.first_prev, b.last_sequence, b.count), (12, 11, 15, 4));
        update(&mut m, Msg::Net(NetEvent::Activities(b)));
        assert_eq!((m.stream.last_applied, m.domain.tool_activity_count, m.diag.resyncs), (15, 5, 0));
        assert_eq!(m.domain.activity.len(), 5);
    }

    #[tokio::test]
    async fn a_domain_event_that_does_not_fit_forces_a_resync_instead_of_blocking() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut out = Outbox::default();
        out.deliver(&tx, activity(11)).await.unwrap();
        out.deliver(&tx, activity(12)).await.unwrap();
        out.deliver(&tx, finding_event(13)).await.unwrap();
        out.deliver(&tx, activity(14)).await.unwrap();
        let mut m = live_model();
        update(&mut m, rx.try_recv().unwrap());
        out.flush(&tx).unwrap();
        let overflow = rx.try_recv().unwrap();
        assert!(matches!(overflow, Msg::Net(NetEvent::Overflow)));
        update(&mut m, overflow);
        assert_eq!(m.diag.resyncs, 1);
        assert!(m.domain.findings.is_empty());
    }

    #[tokio::test]
    async fn an_oversized_line_is_reported_and_the_next_line_still_parses() {
        let mut data = vec![b'x'; MAX_LINE_BYTES + 10];
        data.extend_from_slice(b"\n\n{\"ok\":1}\n");
        let mut r = LineReader::new(&data[..]);
        assert_eq!(r.next_line().await.unwrap(), Some(Line::TooLong));
        assert_eq!(r.next_line().await.unwrap(), Some(Line::Text("{\"ok\":1}".into())));
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[test]
    fn a_socket_path_one_byte_over_the_limit_is_refused() {
        let at = |bytes: usize| PathBuf::from(format!("/{}", "s".repeat(bytes - 1)));
        assert_eq!(check_socket_path(&at(SOCKET_PATH_MAX)), Ok(()));
        let over = SOCKET_PATH_MAX + 1;
        assert_eq!(
            check_socket_path(&at(over)),
            Err(format!(
                "socket path is {over} bytes (max {SOCKET_PATH_MAX} on {}); use a shorter --socket",
                std::env::consts::OS
            ))
        );
    }

    #[test]
    fn backoff_doubles_from_250ms_to_10s_with_jitter() {
        for (attempt, max_ms) in [(1, 250), (2, 500), (3, 1_000), (7, 10_000), (40, 10_000)] {
            for _ in 0..50 {
                let d = backoff(attempt).as_millis() as u64;
                assert!((max_ms / 2..=max_ms).contains(&d), "attempt {attempt}: {d}ms");
            }
        }
    }
}
