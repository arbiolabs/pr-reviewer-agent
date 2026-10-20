//! Golden frames (insta + ratatui TestBackend) and the all-sizes no-panic sweep.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use pr_review_tui::model::{Look, Model, Screen};
use pr_review_tui::net::NetCmd;
use pr_review_tui::playback;
use pr_review_tui::protocol::{Event, PrId, SequencedEvent, ServerMessage};
use pr_review_tui::text::width;
use pr_review_tui::update::{Effect, Msg, NetEvent, update};
use pr_review_tui::view::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

/// A model driven only through messages, answering its requests the way the playback driver does.
struct Sim {
    m: Model,
    played: Vec<SequencedEvent>,
}

impl Sim {
    fn new(look: Look) -> Sim {
        let mut s = Sim {
            m: Model::new("unix:/run/user/501/pr-review/reviewer.sock", look, Screen::Inbox, None),
            played: vec![],
        };
        s.apply(Msg::Net(NetEvent::Opened));
        s
    }

    fn apply(&mut self, msg: Msg) {
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

    fn server(&mut self, msg: ServerMessage) {
        if let ServerMessage::Event(Event::Sequenced(e)) = &msg {
            self.played.push(e.clone());
        }
        self.apply(Msg::Net(NetEvent::Message(msg)));
    }

    fn play(mut self, name: &str, lines: usize) -> Sim {
        for msg in script(name).into_iter().take(lines) {
            self.server(msg);
        }
        self
    }

    fn key(mut self, code: KeyCode) -> Sim {
        self.apply(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        self
    }

    /// A key whose requests stay unanswered, so the frame shows what the client does while it waits.
    fn key_unanswered(mut self, code: KeyCode) -> Sim {
        update(&mut self.m, Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        self
    }

    fn select(mut self, pr: &str) -> Sim {
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

fn render(m: &mut Model, w: u16, h: u16) -> (String, Terminal<TestBackend>) {
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

const SIZES: [(u16, u16); 4] = [(60, 16), (80, 24), (120, 38), (160, 45)];

fn mid_review(look: Look) -> Sim {
    // welcome, snapshot, tool activity, heartbeat, finding f-1: rev-184-a is in VERIFY.
    Sim::new(look).play("normal", 5)
}

fn states(look: Look) -> Vec<(&'static str, Sim)> {
    vec![
        ("inbox", mid_review(look)),
        ("review", mid_review(look).select("acme/shop#184").key(KeyCode::Enter)),
        ("diff", mid_review(look).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Char('d'))),
        ("timeline", mid_review(look).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Char('4'))),
    ]
}

fn colorless(term: &Terminal<TestBackend>) -> bool {
    term.backend().buffer().content.iter().all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
}

#[test]
fn views_at_every_breakpoint() {
    let ascii = Look { ascii: true, ..Look::default() };
    let mono = Look { color: false, ..Look::default() };
    for ((name, mut s), ((_, mut a), (_, mut n))) in
        states(Look::default()).into_iter().zip(states(ascii).into_iter().zip(states(mono)))
    {
        for (w, h) in SIZES {
            let (text, term) = render(&mut s.m, w, h);
            insta::assert_snapshot!(format!("{name}_{w}x{h}"), text);
            assert!(!colorless(&term), "{name} {w}x{h} has no color in color mode");

            let (ascii_text, _) = render(&mut a.m, w, h);
            insta::assert_snapshot!(format!("{name}_{w}x{h}_ascii"), ascii_text);
            assert!(ascii_text.is_ascii(), "{name} {w}x{h} --ascii frame has non-ASCII glyphs");

            // NO_COLOR keeps every label and only drops color; code spans, chips in color, get their backticks back.
            let (mono_text, term) = render(&mut n.m, w, h);
            let unticked = |t: &str| t.replace('`', "");
            assert_eq!(unticked(&mono_text), unticked(&text), "{name} {w}x{h}: NO_COLOR changed the text");
            assert!(colorless(&term), "{name} {w}x{h}: NO_COLOR frame has colors");
        }
    }
}

/// accounts.ndjson up to its snapshot: two selectable github.com accounts, the daemon offers account.select.
fn accounts_live(look: Look) -> Sim {
    Sim::new(look).play("accounts", 2)
}

/// The fixture daemon state with every reason an account cannot be selected: env token, gh error.
fn accounts_blocked() -> Sim {
    let mut s = Sim::new(Look::default());
    let welcome =
        read("fixtures/server.welcome.json").replace("\"capabilities\": []", "\"capabilities\": [\"account.select\"]");
    s.server(ServerMessage::parse(&welcome).unwrap());
    s.server(ServerMessage::parse(&read("fixtures/server.snapshot.accounts.json")).unwrap());
    s.key(KeyCode::Char('A'))
}

#[test]
fn accounts_view() {
    let ascii = Look { ascii: true, ..Look::default() };
    let mono = Look { color: false, ..Look::default() };
    let mut s = accounts_live(Look::default()).key(KeyCode::Char('A')).key(KeyCode::Char('j'));
    let mut a = accounts_live(ascii).key(KeyCode::Char('A')).key(KeyCode::Char('j'));
    let mut n = accounts_live(mono).key(KeyCode::Char('A')).key(KeyCode::Char('j'));
    for (w, h) in [(80, 24), (120, 38)] {
        let (text, _) = render(&mut s.m, w, h);
        insta::assert_snapshot!(format!("accounts_{w}x{h}"), text);
        let (ascii_text, _) = render(&mut a.m, w, h);
        insta::assert_snapshot!(format!("accounts_{w}x{h}_ascii"), ascii_text);
        assert!(ascii_text.is_ascii(), "accounts {w}x{h} --ascii frame has non-ASCII glyphs");
        let (mono_text, term) = render(&mut n.m, w, h);
        // Settings names the display mode; nothing else may change.
        let mono_text = mono_text.replace("monochrome (NO_COLOR or theme = \"mono\")", "color");
        assert_eq!(mono_text, text, "accounts {w}x{h}: NO_COLOR changed the text");
        assert!(colorless(&term));
    }
}

#[test]
fn special_states() {
    let mut cases: Vec<(&str, Sim, (u16, u16))> = vec![
        ("loading", Sim::new(Look::default()), (80, 24)),
        (
            "empty",
            {
                let mut s = Sim::new(Look::default());
                s.server(ServerMessage::parse(&read("fixtures/server.welcome.json")).unwrap());
                s.server(ServerMessage::parse(&read("fixtures/server.snapshot.empty.json")).unwrap());
                s
            },
            (80, 24),
        ),
        (
            "unavailable",
            {
                let mut s = Sim::new(Look::default());
                s.apply(Msg::Net(NetEvent::Lost { reason: "connection refused".into(), retry_in_ms: Some(2_000) }));
                s
            },
            (80, 24),
        ),
        (
            "disconnected_mid_review",
            {
                let mut s = mid_review(Look::default()).select("acme/shop#184").key(KeyCode::Enter);
                s.apply(Msg::Net(NetEvent::Lost {
                    reason: "no message for 30s (heartbeat timeout)".into(),
                    retry_in_ms: Some(1_000),
                }));
                s
            },
            (120, 38),
        ),
        (
            "stale_review",
            Sim::new(Look::default()).play("superseded", 6).select("acme/shop#184").key(KeyCode::Enter),
            (120, 38),
        ),
        (
            "stale_diff",
            Sim::new(Look::default())
                .play("superseded", 6)
                .select("acme/shop#184")
                .key(KeyCode::Enter)
                .key(KeyCode::Char('j'))
                .key(KeyCode::Char('d')),
            (120, 38),
        ),
        (
            "long_title",
            Sim::new(Look::default()).play("hostile", 5).select("acme/shop#2").key(KeyCode::Enter),
            (80, 24),
        ),
        ("hostile_inbox", Sim::new(Look::default()).play("hostile", 5), (120, 38)),
        (
            "binary_diff",
            mid_review(Look::default())
                .select("acme/shop#184")
                .key(KeyCode::Char('3'))
                .key(KeyCode::Char('h'))
                .key(KeyCode::Char('j')),
            (120, 38),
        ),
        (
            "finding_detail",
            mid_review(Look::default()).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Enter),
            (120, 38),
        ),
        (
            "finding_detail_narrow",
            mid_review(Look::default()).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Enter),
            (60, 16),
        ),
        ("help", mid_review(Look::default()).key(KeyCode::Char('?')), (80, 24)),
        ("settings", mid_review(Look::default()).key(KeyCode::Char('5')), (80, 24)),
        (
            "incompatible",
            {
                let mut s = Sim::new(Look::default());
                s.server(ServerMessage::parse(&read("fixtures/server.error.protocol-mismatch.json")).unwrap());
                s
            },
            (80, 24),
        ),
        ("minimal", mid_review(Look::default()), (59, 15)),
        (
            "confirm",
            {
                let mut s = Sim::new(Look::default());
                let welcome = read("fixtures/server.welcome.json")
                    .replace("\"capabilities\": []", "\"capabilities\": [\"review.start\"]");
                s.server(ServerMessage::parse(&welcome).unwrap());
                s.server(ServerMessage::parse(&read("fixtures/server.snapshot.json")).unwrap());
                s.key(KeyCode::Char('g')).key(KeyCode::Char('s'))
            },
            (80, 24),
        ),
        ("accounts_blocked", accounts_blocked(), (120, 38)),
        (
            "timeline_run_scope",
            mid_review(Look::default())
                .select("acme/shop#184")
                .key(KeyCode::Enter)
                .key(KeyCode::Char('4'))
                .key(KeyCode::Char('f')),
            (120, 38),
        ),
        (
            "timeline_finding_scope",
            mid_review(Look::default())
                .select("acme/shop#184")
                .key(KeyCode::Enter)
                .key(KeyCode::Char('t'))
                .key(KeyCode::Char('f'))
                .key(KeyCode::Char('f')),
            (80, 24),
        ),
        (
            "completed_without_findings",
            {
                let mut s = Sim::new(Look::default());
                s.server(ServerMessage::parse(&read("fixtures/server.welcome.json")).unwrap());
                let mut snap: serde_json::Value = serde_json::from_str(&read("fixtures/server.snapshot.json")).unwrap();
                snap["findings"] = serde_json::json!([]);
                s.server(serde_json::from_value(snap).unwrap());
                s.select("acme/shop#185").key(KeyCode::Enter)
            },
            (80, 24),
        ),
        (
            "accounts_requested",
            accounts_live(Look::default())
                .key(KeyCode::Char('A'))
                .key(KeyCode::Char('j'))
                .key(KeyCode::Enter)
                .key_unanswered(KeyCode::Char('y')),
            (80, 24),
        ),
        (
            "accounts_rejected",
            {
                let mut s = accounts_live(Look::default())
                    .key(KeyCode::Char('A'))
                    .key(KeyCode::Char('j'))
                    .key(KeyCode::Enter)
                    .key_unanswered(KeyCode::Char('y'));
                let rid = s.m.next_request - 1;
                s.server(ServerMessage::parse(&format!(
                    r#"{{"type":"command.result","requestId":"req-{rid}","status":"rejected","code":"CONFLICT","message":"GITHUB_TOKEN is set in the daemon environment; it overrides any selection"}}"#
                )).unwrap());
                s
            },
            (80, 24),
        ),
    ];
    for (name, s, (w, h)) in cases.iter_mut() {
        let (text, _) = render(&mut s.m, *w, *h);
        insta::assert_snapshot!(format!("state_{name}"), text);
    }
}

#[test]
fn a_diff_line_with_a_long_zero_width_run_renders() {
    let mut s = mid_review(Look::default()).select("acme/shop#184").key_unanswered(KeyCode::Char('3'));
    let id = s.m.diff.as_ref().and_then(|d| d.pending.clone()).unwrap();
    let hostile = format!("    abcdefghij{}{}", "\u{200b}".repeat(100_000), "xyz".repeat(40));
    let page = read("fixtures/server.diff.page.json")
        .replace("req-diff", id.as_str())
        .replace("    slot.held = true;", &hostile);
    s.server(ServerMessage::parse(&page).unwrap());
    let start = std::time::Instant::now();
    let (text, _) = render(&mut s.m, 120, 38);
    let took = start.elapsed();
    assert!(took < std::time::Duration::from_millis(500), "frame took {took:?}");
    insta::assert_snapshot!("state_zero_width_diff", text.replace('\u{200b}', ""));
}

#[test]
fn a_connection_error_on_a_long_path_keeps_its_reason_in_view() {
    let long = format!("/private/tmp/claude-501/{}/pr-review/reviewer.sock", "x".repeat(70));
    let reason = pr_review_tui::net::check_socket_dir(std::path::Path::new(&long)).unwrap_err();
    assert_eq!(reason, format!("socket directory does not exist: {}", long.trim_end_matches("/reviewer.sock")));
    for (w, h) in [(80, 24), (120, 38)] {
        let mut s = Sim::new(Look::default());
        s.m.diag.endpoint = format!("unix:{long}");
        s.apply(Msg::Net(NetEvent::Lost { reason: reason.clone(), retry_in_ms: Some(2_000) }));
        let (inbox, _) = render(&mut s.m, w, h);
        insta::assert_snapshot!(format!("state_long_path_error_{w}x{h}"), inbox);
        let s = s.key(KeyCode::Char('5'));
        let mut s = s;
        let (settings, _) = render(&mut s.m, w, h);
        insta::assert_snapshot!(format!("state_long_path_error_settings_{w}x{h}"), settings);
        let shows = |frame: &str, prefix: &str| frame.lines().any(|l| l.contains(prefix));
        assert!(shows(&inbox, "disconnected: socket directory does not exist: /pri"), "banner, {w} cols:\n{inbox}");
        assert!(shows(&inbox, "unavailable: socket directory does not exist:"), "inbox, {w} cols:\n{inbox}");
        assert!(inbox.matches("/private/tmp/claude-501/").count() >= 2, "inbox names the path, {w} cols:\n{inbox}");
        assert!(
            shows(&settings, "disconnected: socket directory does not exist: /"),
            "settings, {w} cols:\n{settings}"
        );
    }
}

#[test]
fn every_view_survives_every_size() {
    let mut sims: Vec<Sim> = states(Look::default()).into_iter().map(|(_, s)| s).collect();
    sims.push(mid_review(Look::default()).key(KeyCode::Char('5')));
    sims.push(accounts_blocked());
    sims.push(mid_review(Look::default()).select("acme/shop#184").key(KeyCode::Enter).key(KeyCode::Enter));
    sims.push(Sim::new(Look::default()).play("hostile", 5).select("acme/shop#2").key(KeyCode::Enter));
    let mut term = Terminal::new(TestBackend::new(0, 0)).unwrap();
    for s in &sims {
        for w in 0..=200u16 {
            for h in 0..=60u16 {
                term.backend_mut().resize(w, h);
                term.resize(ratatui::layout::Rect::new(0, 0, w, h)).unwrap();
                term.draw(|f| view(&s.m, f)).unwrap();
            }
        }
    }
    let _ = NetCmd::Close;
}

#[test]
fn j_reads_the_explanation_before_the_document_moves() {
    let mut s = Sim::new(Look::default()).play("showcase", 2).key(KeyCode::Enter);
    let (before, _) = render(&mut s.m, 120, 38);
    assert!(before.contains("2 more, J scrolls"), "{before}");
    let mut s = s.key(KeyCode::Char('J')).key(KeyCode::Char('J'));
    let (after, _) = render(&mut s.m, 120, 38);
    assert!(after.contains("Orders: self-serve refunds"), "the PR header stays:\n{after}");
    assert!(after.contains("2 more above, K scrolls back"), "{after}");
    assert!(after.contains("refund another customer's order"), "the explanation's last line shows:\n{after}");
}

/// One 160x45 diff frame over 2000 lines of 50k chars each. The view windows the rows before it builds any, and
/// cuts each line to its row budget before it expands or colors it, so the frame costs what the screen shows.
#[test]
fn a_huge_diff_renders_a_frame_in_bounded_time() {
    let mut s = mid_review(Look::default()).select("acme/shop#184").key_unanswered(KeyCode::Char('3'));
    let id = s.m.diff.as_ref().and_then(|d| d.pending.clone()).unwrap();
    let mut page: serde_json::Value =
        serde_json::from_str(&read("fixtures/server.diff.page.json").replace("req-diff", id.as_str())).unwrap();
    let text = "\tconst x = \"a\" // b ".repeat(2_500);
    assert_eq!(text.chars().count(), 50_000);
    let lines: Vec<_> = (1..=2000).map(|n| serde_json::json!({"kind": "add", "new": n, "text": text})).collect();
    page["files"][0]["hunks"][0]["lines"] = serde_json::Value::Array(lines);
    s.server(serde_json::from_value(page).unwrap());
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    update(&mut s.m, Msg::Resize(160, 45));
    term.draw(|f| view(&s.m, f)).unwrap();
    let start = std::time::Instant::now();
    term.draw(|f| view(&s.m, f)).unwrap();
    let took = start.elapsed();
    // About 2 ms in the test profile (opt-level 1) and 1 ms in release; it took 600 ms before the rows were windowed.
    assert!(took < std::time::Duration::from_millis(30), "frame took {took:?}");
}

/// The text of the reverse-video cells on each row that has any: what NO_COLOR shows as selected.
fn reversed(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    let (w, h) = (buf.area.width, buf.area.height);
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| &buf[(x, y)])
                .filter(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
                .map(|c| c.symbol())
                .collect::<String>()
        })
        .filter(|row| !row.is_empty())
        .map(|row| row.trim().to_string())
        .collect()
}

#[test]
fn without_color_the_diff_cursor_and_a_selected_phase_heading_show_in_reverse_video() {
    let mono = Look { color: false, ..Look::default() };
    let (_, mut diff) = states(mono).remove(2);
    let (_, term) = render(&mut diff.m, 120, 38);
    assert_eq!(
        reversed(&term),
        ["▌ ▲ booking.ts", "▌   src                     +2 -1", "▲        42 +     slot.held = true;"]
    );

    let mut s = Sim::new(mono).play("showcase", 12).key(KeyCode::Char('w')).key(KeyCode::Char('t'));
    let phase = |s: &Sim| {
        let t = s.m.timeline.as_ref().unwrap();
        let e = t.events.iter().find(|e| Some(e.sequence.get()) == t.selected).unwrap();
        matches!(e.body, pr_review_tui::protocol::EventBody::PhaseChanged(_))
    };
    for _ in 0..20 {
        if phase(&s) {
            break;
        }
        s = s.key(KeyCode::Char('k'));
    }
    assert!(phase(&s), "a phase change is selected");
    let (frame, term) = render(&mut s.m, 120, 38);
    let rows = reversed(&term);
    assert_eq!(rows.len(), 1, "{rows:?}\n{frame}");
    assert!(rows[0].ends_with(|c: char| c.is_ascii_digit()) && rows[0].contains("───"), "{rows:?}");
}

/// Every view, waiting on its replies and answered, at each step of the showcase playback: under `--ascii` no cell
/// holds anything but ASCII.
#[test]
fn ascii_frames_stay_ascii_across_the_showcase() {
    use KeyCode::{Char, Enter};
    let ascii = Look { ascii: true, ..Look::default() };
    let routes: [&[KeyCode]; 9] = [
        &[],
        &[Enter],
        &[Enter, Enter],
        &[Enter, Char('d')],
        &[Char('w'), Char('t')],
        &[Char('w'), Char('t'), Enter],
        &[Char('?')],
        &[Char('5')],
        &[Char('A'), Char('j'), Enter, Char('y')],
    ];
    let msgs = script("showcase");
    for keys in routes {
        for answered in [true, false] {
            let mut s = Sim::new(ascii).play("showcase", 2);
            for &k in keys {
                s = if answered { s.key(k) } else { s.key_unanswered(k) };
            }
            for (i, msg) in msgs.iter().enumerate().skip(2) {
                s.server(msg.clone());
                for (w, h) in [(80, 24), (120, 38)] {
                    let (frame, _) = render(&mut s.m, w, h);
                    let bad: String = frame.chars().filter(|c| !c.is_ascii()).collect();
                    assert!(bad.is_empty(), "{keys:?} answered={answered} message {i} {w}x{h}: {bad:?}\n{frame}");
                }
            }
        }
    }
}

#[test]
fn the_pr_title_stays_in_view_while_the_live_log_pushes_the_open_card_down() {
    for (w, h) in [100u16, 120, 130, 140, 160].into_iter().flat_map(|w| (24..=50u16).map(move |h| (w, h))) {
        for events in 30..=43 {
            let mut s = Sim::new(Look::default()).play("showcase", 2 + events).key(KeyCode::Char('w'));
            let (frame, _) = render(&mut s.m, w, h);
            // The reading column's top three rows, right of the PR rail.
            let top: Vec<String> = frame.lines().skip(2).take(3).map(|l| l.chars().skip(34).collect()).collect();
            let title: String = s.m.selected_pr().unwrap().title.as_str().chars().take(16).collect();
            assert!(top.iter().any(|l| l.contains(&title)), "{w}x{h} after {events}:\n{frame}");
        }
    }
}

#[test]
fn the_help_page_shows_every_key_at_80x24() {
    let mut s = mid_review(Look::default()).key(KeyCode::Char('?'));
    let (frame, _) = render(&mut s.m, 80, 24);
    use pr_review_tui::keymap::Context as C;
    let rows = pr_review_tui::keymap::help_rows();
    // The confirmation panel names its own y and n.
    for (_, keys, what) in rows.iter().filter(|r| matches!(r.0, C::Global | C::Review | C::Diff | C::Timeline)) {
        // A long description may end in an ellipsis; its start must show beside its keys.
        let row = format!("{keys:<11}{}", what.chars().take(12).collect::<String>());
        assert!(frame.contains(&row), "{row} is missing:\n{frame}");
    }
}

#[test]
fn scrolling_the_help_page_to_its_end_shows_every_section_at_every_size() {
    use pr_review_tui::keymap::Context as C;
    let rows = pr_review_tui::keymap::help_rows();
    for (w, h) in [(60, 16), (80, 24), (120, 38)] {
        let mut s = mid_review(Look::default()).key(KeyCode::Char('?'));
        let (first, _) = render(&mut s.m, w, h);
        let mut seen = first.clone();
        let mut last = first.clone();
        for _ in 0..200 {
            s = s.key(KeyCode::Char('j'));
            let (frame, _) = render(&mut s.m, w, h);
            if frame == last {
                break;
            }
            seen.push_str(&frame);
            last = frame;
        }
        for (_, keys, what) in rows.iter().filter(|r| matches!(r.0, C::Global | C::Review | C::Diff | C::Timeline)) {
            let row = format!("{keys:<11}{}", what.chars().take(5).collect::<String>());
            assert!(seen.contains(&row), "{w}x{h}: {row} never shows:\n{seen}");
        }
        for text in [
            "Glyphs",
            "low",
            "failed",
            "findings published on GitHub",
            "Mouse",
            "select it.",
            "The wheel scrolls the page under it.",
            "Write keys need a capability the server grants.",
        ] {
            assert!(last.contains(text), "{w}x{h}: the end of the page lacks {text}:\n{last}");
        }
        let overflows = first != last;
        assert_eq!(first.contains("more below, j scrolls"), overflows, "{w}x{h}: the cue shows only on overflow");
        assert!(!last.contains("more below"), "{w}x{h}: no cue at the end:\n{last}");
        let mut k = s.key(KeyCode::Char('k'));
        assert_eq!(render(&mut k.m, w, h).0 != last, overflows, "{w}x{h}: one k scrolls back from the end");
        let mut paged = mid_review(Look::default()).key(KeyCode::Char('?'));
        render(&mut paged.m, w, h);
        for _ in 0..20 {
            paged = paged.key(KeyCode::PageDown);
        }
        assert_eq!(render(&mut paged.m, w, h).0, last, "{w}x{h}: PgDn reaches the same end");
    }
    let mut s = mid_review(Look::default()).key(KeyCode::Char('?'));
    let (top, _) = render(&mut s.m, 80, 24);
    let wheel = |kind| MouseEvent { kind, column: 10, row: 10, modifiers: KeyModifiers::NONE };
    s.apply(Msg::Mouse(wheel(MouseEventKind::ScrollDown)));
    assert_ne!(render(&mut s.m, 80, 24).0, top, "the wheel scrolls the help page");
    s.apply(Msg::Mouse(wheel(MouseEventKind::ScrollUp)));
    assert_eq!(render(&mut s.m, 80, 24).0, top, "and back");
}

#[test]
fn j_past_the_end_stops_so_one_k_scrolls_back() {
    let mut s = Sim::new(Look::default()).play("showcase", 2).key(KeyCode::Enter);
    render(&mut s.m, 120, 38);
    for _ in 0..100 {
        s = s.key(KeyCode::Char('J'));
    }
    let (end, _) = render(&mut s.m, 120, 38);
    let mut s = s.key(KeyCode::Char('J'));
    assert_eq!(render(&mut s.m, 120, 38).0, end, "the document is at its end");
    let mut s = s.key(KeyCode::Char('K'));
    assert_ne!(render(&mut s.m, 120, 38).0, end, "one K moves back from the end");
}

#[test]
fn show_activity_false_hides_the_live_log_and_the_rates() {
    let frame = |look: Look| {
        let mut s = Sim::new(look).play("showcase", 2 + 20).key(KeyCode::Char('w'));
        render(&mut s.m, 120, 38).0
    };
    let on = frame(Look::default());
    assert!(on.contains("Reviewing now") && on.contains("calls/s") && on.contains("/s  "), "{on}");
    let off = frame(Look { show_activity: false, ..Look::default() });
    assert!(!off.contains("Reviewing now") && !off.contains("/s  "), "{off}");
    assert!(off.contains("Sign-in: send users back"), "the document still shows:\n{off}");
}
