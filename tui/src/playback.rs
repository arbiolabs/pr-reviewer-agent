//! `--playback FILE.ndjson`: plays a scripted server session into the app with no socket. The script owns the stream;
//! requests the script cannot know about (diff, finding detail, timeline) are answered from `protocol/fixtures/`. A
//! timeline is the events already played on top of the history the `server.timeline.page.showcase-*` pages recorded
//! before the session opened. A line with `after: "<gate>"` holds until the client sends a command the gate names,
//! which the playback then accepts: the showcase's approval plays only once the reader approves it, and its start,
//! cancel, retry and account switch play whenever the reader sends them. A start answers as the daemon does: its own
//! key again replays the run it made, and a head with a run is refused. A `snapshot.get` (the `r`
//! key) is answered with the last snapshot played and every event played after it, as one gap-free chain, so the
//! client catches up again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::net::NetCmd;
use crate::protocol::{ClientMessage, Event, EventBody, RunStatus, SequencedEvent, ServerMessage, UsageGroupBy};
use crate::update::{Msg, NetEvent};

pub struct Step {
    pub delay: Duration,
    /// The command this line waits for, if any.
    pub after: Option<String>,
    pub message: ServerMessage,
}

pub struct Script {
    pub steps: Vec<Step>,
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
        let after = v["after"].as_str().map(String::from);
        steps.push(Step { delay, after, message: msg });
    }
    let fixtures = path.parent().unwrap_or(Path::new(".")).join("../fixtures");
    Ok(Script { steps, fixtures })
}

/// The script as it plays: what is left, the commands the reader sent that open a gate, and the messages sent so far.
pub struct Player {
    steps: Vec<Step>,
    pub fixtures: PathBuf,
    gates: Vec<String>,
    sent: Vec<String>,
    /// A gated line plays when the reader acts, out of file order, so a script with gates numbers its events as it
    /// plays them. A script without gates keeps the numbers it was written with, gaps and duplicates included.
    renumber: bool,
    /// Runs the reader cancelled: the script's later ungated lines about them drop once the cancel has played.
    cancelled: Vec<crate::protocol::RunId>,
    /// Each start accepted, by idempotency key, with the operation the daemon would replay for it.
    started: Vec<(String, String)>,
    last: Option<u64>,
    pub played: Vec<ServerMessage>,
}

/// A gate names a command (`review.publish`) or one command on one target, as its idempotency key does
/// (`review.cancel:<runId>`).
fn opens(gate: &str, key: &str) -> bool {
    key == gate || key.strip_prefix(gate).is_some_and(|rest| rest.starts_with(':'))
}

impl Player {
    pub fn new(script: Script) -> Player {
        let gates: Vec<String> = script.steps.iter().filter_map(|s| s.after.clone()).collect();
        Player {
            renumber: !gates.is_empty(),
            steps: script.steps,
            fixtures: script.fixtures,
            gates,
            sent: Vec::new(),
            cancelled: Vec::new(),
            started: Vec::new(),
            last: None,
            played: Vec::new(),
        }
    }

    fn open(&self, s: &Step) -> bool {
        s.after.as_ref().is_none_or(|a| self.sent.contains(a))
    }

    /// The line to play next and its delay: a gated line the reader opened plays first, wherever it sits; else the
    /// next line, unless it waits on a gate. An ungated line about a run the reader cancelled, once it has ended,
    /// drops; every other script plays as written, stale lines included.
    pub fn upcoming(&mut self) -> Option<(usize, Duration)> {
        let ended: Vec<crate::protocol::RunId> = self
            .runs()
            .into_iter()
            .filter(|r| r.status != RunStatus::Running && self.cancelled.contains(&r.id))
            .map(|r| r.id)
            .collect();
        self.steps.retain(|s| match &s.message {
            ServerMessage::Event(Event::Sequenced(e)) if s.after.is_none() => {
                e.run_id.as_ref().is_none_or(|id| !ended.contains(id))
            }
            _ => true,
        });
        let gated = self.steps.iter().position(|s| s.after.is_some() && self.open(s));
        let i = gated.or_else(|| self.steps.first().filter(|s| s.after.is_none()).map(|_| 0))?;
        Some((i, self.steps[i].delay))
    }

    pub fn play(&mut self, i: usize) -> ServerMessage {
        let mut msg = self.steps.remove(i).message;
        match &mut msg {
            ServerMessage::Snapshot(s) => self.last = Some(s.sequence),
            ServerMessage::Event(Event::Sequenced(e)) => {
                if let Some(last) = self.last.filter(|_| self.renumber) {
                    e.prev_sequence = last;
                    e.sequence = std::num::NonZeroU64::new(last + 1).unwrap_or(e.sequence);
                }
                self.last = Some(e.sequence.get());
            }
            _ => {}
        }
        self.played.push(msg.clone());
        msg
    }

    /// The replies to a client request: a command a gate names is accepted and opens it; any other is rejected,
    /// saying where the script plays that command.
    pub fn answer(&mut self, req: &ClientMessage) -> Vec<ServerMessage> {
        use crate::protocol::{CommandOutcome, CommandResult, Text};
        let ClientMessage::Command(c) = req else { return answer(&self.fixtures, &self.played, req) };
        if let crate::protocol::Command::Add(p) = &c.command {
            return vec![ServerMessage::CommandResult(self.add(c.request_id.clone(), &p.url))];
        }
        if let crate::protocol::Command::Archive(p) | crate::protocol::Command::Unarchive(p) = &c.command {
            let archive = matches!(c.command, crate::protocol::Command::Archive(_));
            return vec![ServerMessage::CommandResult(self.archive(c, &p.pr_id, archive))];
        }
        let key = c.idempotency_key.clone();
        let opened: Vec<String> = self.gates.iter().filter(|g| opens(g, &key)).cloned().collect();
        let replay = self.started.iter().find(|(k, _)| *k == key).map(|(_, op)| op.clone());
        let known = match &c.command {
            crate::protocol::Command::Start(p) => self.known_head(p),
            _ => None,
        };
        let outcome = if let Some(operation) = replay {
            // As the daemon answers its own key again: the run that start made, however it has gone since.
            CommandOutcome::Accepted { operation_id: Text::new(&operation) }
        } else if let Some(outcome) = known {
            outcome
        } else if opened.is_empty() {
            let name = c.command.name();
            let message = match self.gates.iter().find(|g| opens(name, g)).and_then(|g| self.target(g)) {
                Some(target) => format!("the showcase plays {name} on {target} only"),
                None => format!("the showcase does not play {name}"),
            };
            CommandOutcome::Rejected {
                code: crate::protocol::ErrorCode::CapabilityDenied,
                message: Text::new(&message),
            }
        } else {
            if let Some(run) = key.strip_prefix("review.cancel:") {
                self.cancelled.push(crate::protocol::RunId::new(run));
            }
            let operation = match &c.command {
                crate::protocol::Command::Publish(p) => format!("review.publish:{}", p.run_id),
                crate::protocol::Command::Start(_) => {
                    let run =
                        self.steps.iter().filter(|s| s.after.as_ref().is_some_and(|a| opened.contains(a))).find_map(
                            |s| match &s.message {
                                ServerMessage::Event(Event::Sequenced(e)) => match &e.body {
                                    EventBody::Run(_, r) => Some(r.id.to_string()),
                                    _ => None,
                                },
                                _ => None,
                            },
                        );
                    let operation = run.map_or(key.clone(), |r| format!("review.start:{r}"));
                    self.started.push((key, operation.clone()));
                    operation
                }
                _ => key,
            };
            self.sent.extend(opened);
            CommandOutcome::Accepted { operation_id: Text::new(&operation) }
        };
        vec![ServerMessage::CommandResult(CommandResult {
            request_id: c.request_id.clone(),
            outcome,
            pr_id: None,
            run_id: None,
        })]
    }

    /// `review.add` as a daemon answers it with no GitHub behind it: a PR the session tracks is refused naming it, and
    /// any other is one GitHub would not serve.
    fn add(&self, request_id: crate::protocol::RequestId, url: &str) -> crate::protocol::CommandResult {
        use crate::protocol::{CommandOutcome, CommandResult, ErrorCode, Text};
        let target = crate::model::PrRef::parse(url);
        let tracked = target.as_ref().and_then(|t| {
            self.played.iter().rev().find_map(|m| match m {
                ServerMessage::Snapshot(s) => s.prs.iter().find(|p| t.matches(p)).cloned(),
                ServerMessage::Event(Event::Sequenced(e)) => match &e.body {
                    EventBody::PrUpdated(p) if t.matches(p) => Some(p.clone()),
                    _ => None,
                },
                _ => None,
            })
        });
        let (code, message, pr_id) = match (&target, tracked) {
            (Some(t), Some(p)) => (ErrorCode::Conflict, format!("{t} is already tracked"), Some(p.id)),
            (Some(t), None) => (
                ErrorCode::NotFound,
                format!(
                    "cannot read {t}: it does not exist, or github.com/reviewer-bot has no access to it (HTTP 404)"
                ),
                None,
            ),
            (None, _) => (ErrorCode::InvalidRequest, format!("{url} is not a GitHub pull request"), None),
        };
        CommandResult {
            request_id,
            outcome: CommandOutcome::Rejected { code, message: Text::new(&message) },
            pr_id,
            run_id: None,
        }
    }

    /// The PR as played so far: the newest `pr.updated` for it, else the latest snapshot's.
    fn pr(&self, id: &crate::protocol::PrId) -> Option<crate::protocol::Pr> {
        self.played.iter().rev().find_map(|m| match m {
            ServerMessage::Snapshot(s) => s.prs.iter().find(|p| &p.id == id).cloned(),
            ServerMessage::Event(Event::Sequenced(e)) => match &e.body {
                EventBody::PrUpdated(p) if &p.id == id => Some(p.clone()),
                _ => None,
            },
            _ => None,
        })
    }

    /// `pr.archive` and `pr.unarchive` as the daemon answers them: accepted, then, when the state changes, the PR as
    /// `pr.updated` with or without `archivedAt`, played next. A script without gates numbers its events from here
    /// on, as one with gates always does, so the event slots into the chain.
    fn archive(
        &mut self,
        c: &crate::protocol::CommandRequest,
        id: &crate::protocol::PrId,
        archive: bool,
    ) -> crate::protocol::CommandResult {
        use crate::protocol::{CommandOutcome, CommandResult, ErrorCode, Text};
        let outcome = match self.pr(id) {
            None => CommandOutcome::Rejected { code: ErrorCode::NotFound, message: Text::new(&format!("no PR {id}")) },
            Some(mut pr) => {
                let last = self.played.iter().rev().find_map(|m| match m {
                    ServerMessage::Event(Event::Sequenced(e)) => Some(e.occurred_at.clone()),
                    _ => None,
                });
                let epoch = self.played.iter().rev().find_map(|m| match m {
                    ServerMessage::Snapshot(s) => Some(s.stream_epoch.clone()),
                    _ => None,
                });
                if let Some(epoch) = epoch.filter(|_| pr.archived() != archive) {
                    let at = last.unwrap_or_else(|| Text::new("2026-01-01T00:00:00Z"));
                    pr.archived_at = archive.then(|| at.clone());
                    self.renumber = true;
                    let e = crate::protocol::SequencedEvent {
                        stream_epoch: epoch,
                        sequence: std::num::NonZeroU64::MIN,
                        prev_sequence: 0,
                        occurred_at: at,
                        pr_id: Some(pr.id.clone()),
                        run_id: None,
                        body: EventBody::PrUpdated(pr),
                    };
                    let message = ServerMessage::Event(Event::Sequenced(e));
                    self.steps.insert(0, Step { delay: Duration::ZERO, after: None, message });
                }
                CommandOutcome::Accepted { operation_id: Text::new(&c.idempotency_key) }
            }
        };
        CommandResult { request_id: c.request_id.clone(), outcome, pr_id: None, run_id: None }
    }

    /// Every run as played so far: the snapshot's, then each run event's newer copy and each phase change.
    fn runs(&self) -> Vec<crate::protocol::Run> {
        let mut runs: Vec<crate::protocol::Run> = Vec::new();
        for m in &self.played {
            let more = match m {
                ServerMessage::Snapshot(s) => s.runs.clone(),
                ServerMessage::Event(Event::Sequenced(e)) => match &e.body {
                    EventBody::Run(_, r) => vec![r.clone()],
                    EventBody::PhaseChanged(c) => {
                        runs.iter_mut().filter(|r| r.id == c.run_id).for_each(|r| r.phase = c.to);
                        vec![]
                    }
                    _ => vec![],
                },
                _ => vec![],
            };
            for r in more {
                runs.retain(|x| x.id != r.id);
                runs.push(r);
            }
        }
        runs
    }

    /// As the daemon answers a start under a new key for a head it has a run for: a running run takes the start
    /// in, and any other is refused (`#1287 at 647fb3b is already reviewed`).
    fn known_head(&self, p: &crate::protocol::StartParams) -> Option<crate::protocol::CommandOutcome> {
        use crate::protocol::{CommandOutcome, ErrorCode, Text};
        let runs = self.runs();
        let latest = runs.iter().rev().find(|r| r.pr_id == p.pr_id && r.head_sha == p.expected_head_sha)?;
        let pr = self.played.iter().rev().find_map(|m| match m {
            ServerMessage::Snapshot(s) => s.prs.iter().find(|x| x.id == p.pr_id).map(|x| x.number),
            ServerMessage::Event(Event::Sequenced(e)) => match &e.body {
                EventBody::PrUpdated(x) if x.id == p.pr_id => Some(x.number),
                _ => None,
            },
            _ => None,
        });
        let pr = pr.map_or(p.pr_id.to_string(), |n| format!("#{n}"));
        let why = match latest.status {
            RunStatus::Running if latest.parked() => "is waiting for your approval",
            RunStatus::Running => {
                return Some(CommandOutcome::Accepted {
                    operation_id: Text::new(&format!("review.start:{}", latest.id)),
                });
            }
            RunStatus::Failed if latest.cancelled() => "was cancelled; review it again instead",
            RunStatus::Failed => "failed its review; retry it instead",
            RunStatus::Superseded => "is superseded by a newer head",
            _ => "is already reviewed",
        };
        let message = format!("{pr} at {} {why}", p.expected_head_sha.short());
        Some(CommandOutcome::Rejected { code: ErrorCode::Conflict, message: Text::new(&message) })
    }

    /// What a gate's target reads as to the reader: `#338` for a PR or a run, the account for an account.
    fn target(&self, gate: &str) -> Option<String> {
        let id = gate.split(':').nth(1)?;
        let snapshot = self.played.iter().find_map(|m| match m {
            ServerMessage::Snapshot(s) => Some(s),
            _ => None,
        })?;
        let pr = snapshot.runs.iter().find(|r| r.id.as_str() == id).map_or(id, |r| r.pr_id.as_str());
        Some(snapshot.prs.iter().find(|p| p.id.as_str() == pr).map_or(id.to_string(), |p| format!("#{}", p.number)))
    }
}

pub async fn run(script: Script, tx: mpsc::Sender<Msg>, mut rx: mpsc::Receiver<NetCmd>) {
    let mut player = Player::new(script);
    if tx.send(Msg::Net(NetEvent::Opened)).await.is_err() {
        return;
    }
    loop {
        let next = player.upcoming();
        tokio::select! {
            _ = tokio::time::sleep(next.map_or(Duration::MAX, |n| n.1)), if next.is_some() => {
                let Some((i, _)) = next else { continue };
                let msg = player.play(i);
                if tx.send(Msg::Net(NetEvent::Message(msg))).await.is_err() {
                    return;
                }
            }
            cmd = rx.recv() => match cmd {
                Some(NetCmd::Send(req)) => {
                    for reply in player.answer(&req) {
                        if tx.send(Msg::Net(NetEvent::Message(reply))).await.is_err() {
                            return;
                        }
                    }
                }
                Some(NetCmd::Redial) => {}
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

/// The replies to `req`, given the messages the script has `played` so far.
pub fn answer(fixtures: &Path, played: &[ServerMessage], req: &ClientMessage) -> Vec<ServerMessage> {
    let rid = req.request_id().as_str();
    let events: Vec<SequencedEvent> = played
        .iter()
        .filter_map(|m| match m {
            ServerMessage::Event(Event::Sequenced(e)) => Some(e.clone()),
            _ => None,
        })
        .collect();
    let reply = match req {
        // The script plays the server side of the handshake and the stream.
        ClientMessage::Hello { .. } | ClientMessage::Resume { .. } => return vec![],
        ClientMessage::SnapshotGet { request_id } => {
            let Some(mut s) = played.iter().rev().find_map(|m| match m {
                ServerMessage::Snapshot(s) => Some(s.clone()),
                _ => None,
            }) else {
                return vec![];
            };
            s.request_id = Some(request_id.clone());
            let mut after: Vec<SequencedEvent> = Vec::new();
            for e in events.iter().filter(|e| e.stream_epoch == s.stream_epoch && e.sequence.get() > s.sequence) {
                if after.iter().all(|a| a.sequence != e.sequence) {
                    after.push(e.clone());
                }
            }
            // A script's deliberate gap must not come back in the answer, or the client resyncs forever: the events
            // are renumbered into one chain that ends at the last sequence played, so later lines follow on.
            let last = after.iter().map(|e| e.sequence.get()).max().unwrap_or(s.sequence);
            s.sequence = last - after.len() as u64;
            for (k, e) in after.iter_mut().enumerate() {
                e.prev_sequence = s.sequence + k as u64;
                e.sequence = std::num::NonZeroU64::new(s.sequence + k as u64 + 1).unwrap_or(e.sequence);
            }
            let after = after.into_iter().map(|e| ServerMessage::Event(Event::Sequenced(e)));
            return std::iter::once(ServerMessage::Snapshot(s)).chain(after).collect();
        }
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
        ClientMessage::SettingsGet { .. } => fixtures_named(fixtures, "server.settings", rid)
            .into_iter()
            .find(|(n, _)| n == "server.settings.json")
            .map(|(_, v)| v)
            .unwrap_or_else(|| not_found(rid, "no settings".into())),
        ClientMessage::ReposList { .. } => fixtures_named(fixtures, "server.repos.list.result", rid)
            .into_iter()
            .find(|(n, _)| n == "server.repos.list.result.json")
            .map(|(_, v)| v)
            .unwrap_or_else(|| not_found(rid, "no repositories".into())),
        ClientMessage::UsageGet { by, group, .. } => {
            let name = match group {
                Some(g) if *g != UsageGroupBy::Time => {
                    format!("server.usage.report.{}.{}.json", by.as_str(), g.as_str())
                }
                _ => format!("server.usage.report.{}.json", by.as_str()),
            };
            fixtures_named(fixtures, "server.usage.report", rid)
                .into_iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v)
                .unwrap_or_else(|| not_found(rid, "no usage".into()))
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
                .filter(|e| events.iter().all(|p| p.sequence != e.sequence))
                .collect();
            let mut all: Vec<&SequencedEvent> = events.iter().chain(&history).collect();
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
            return vec![ServerMessage::TimelinePage {
                request_id: Some(req.request_id().clone()),
                events: page.into_iter().cloned().collect(),
                next_before_sequence: next,
            }];
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
    serde_json::from_value(reply).into_iter().collect()
}
