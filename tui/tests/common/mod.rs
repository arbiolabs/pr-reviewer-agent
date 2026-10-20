#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::model::{Look, Model, Screen};
use pr_review_tui::playback;
use pr_review_tui::protocol::{Event, PrId, SequencedEvent, ServerMessage};
use pr_review_tui::text::width;
use pr_review_tui::update::{Effect, Msg, NetEvent, update};
use pr_review_tui::view::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
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
    if !m.domain.publications.is_empty() {
        v["publications"] =
            m.domain.publications.values().map(|p| json!({"runId": p.run_id.as_str(), "status": p.status})).collect();
    }
    v
}

/// A model driven only through messages, answering its requests the way the playback driver does.
pub struct Sim {
    pub m: Model,
    pub played: Vec<SequencedEvent>,
}

impl Sim {
    pub fn new(look: Look) -> Sim {
        let mut s = Sim {
            m: Model::new("unix:/run/user/501/pr-review/reviewer.sock", look, Screen::Inbox, None),
            played: vec![],
        };
        s.apply(Msg::Net(NetEvent::Opened));
        s
    }

    pub fn apply(&mut self, msg: Msg) {
        let fx = update(&mut self.m, msg);
        for e in fx {
            if let Effect::Send(req) = e {
                let dir = protocol_dir().join("fixtures");
                if let Some(reply) = playback::answer(&dir, &self.played, &req) {
                    self.apply(Msg::Net(NetEvent::Message(reply)));
                }
            }
        }
    }

    pub fn server(&mut self, msg: ServerMessage) {
        if let ServerMessage::Event(Event::Sequenced(e)) = &msg {
            self.played.push(e.clone());
        }
        self.apply(Msg::Net(NetEvent::Message(msg)));
    }

    pub fn play(mut self, name: &str, lines: usize) -> Sim {
        for msg in script(name).into_iter().take(lines) {
            self.server(msg);
        }
        self
    }

    pub fn key(mut self, code: KeyCode) -> Sim {
        self.apply(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        self
    }

    /// A key whose requests stay unanswered, so the frame shows what the client does while it waits.
    pub fn key_unanswered(mut self, code: KeyCode) -> Sim {
        update(&mut self.m, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        self
    }

    pub fn select(mut self, pr: &str) -> Sim {
        self = self.key(KeyCode::Char('g'));
        for _ in 0..10 {
            if self.m.ui.selected_pr == Some(PrId::new(pr)) {
                return self;
            }
            self = self.key(KeyCode::Char('j'));
        }
        panic!("{pr} not reachable");
    }
}

pub fn render(m: &mut Model, w: u16, h: u16) -> (String, Terminal<TestBackend>) {
    update(m, Msg::Resize(w, h));
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view(m, f)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..w {
            let sym = buf[(x, y)].symbol();
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = width(sym).saturating_sub(1);
            line.push_str(sym);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    (out, term)
}

pub fn mid_review(look: Look) -> Sim {
    // welcome, snapshot, tool activity, heartbeat, finding f-1: rev-184-a is in VERIFY.
    Sim::new(look).play("normal", 5)
}

pub fn states(look: Look) -> Vec<(&'static str, Sim)> {
    vec![
        ("inbox", mid_review(look)),
        ("review", mid_review(look).select("acme/shop#184").key(KeyCode::Enter)),
        ("diff", mid_review(look).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Char('d'))),
        ("timeline", mid_review(look).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Char('4'))),
        ("parked", parked(look)),
        ("publication", parked(look).key(KeyCode::Char('p'))),
        ("published", {
            let mut s = parked(look).key(KeyCode::Char('p'));
            for msg in script("approval").into_iter().skip(7).take(3) {
                s.server(msg);
            }
            s
        }),
    ]
}

/// approval.ndjson up to the run parking: #184 is AWAITING_APPROVAL on an engine daemon, read in Review.
pub fn parked(look: Look) -> Sim {
    Sim::new(look).play("approval", 5).select("acme/shop#184").key(KeyCode::Enter)
}
