//! The footer offers a key exactly when pressing it acts. Every key is pressed in every state below, at three sizes,
//! and checked against what the footer says: an offered key acts, a key that acts is offered unless the width ranking
//! dropped it, the ranking keeps the hints that matter, and hints keep one order everywhere.
//!
//! "Acts" means the key sends something, quits or opens a URL, changes the modals, or changes what the body shows. A
//! toast, or a refused panel that only says why, explains; an offered key must do more than explain.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::keymap;
use pr_review_tui::model::{ConfirmState, Look, Modal, Model, Pane, Screen};
use pr_review_tui::playback::{Player, load};
use pr_review_tui::update::{Effect, Msg, NetEvent, update};
use pr_review_tui::view::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const SIZES: [(u16, u16); 3] = [(60, 16), (80, 24), (120, 38)];

/// Keys every footer may leave out: they act the same on every screen, help lists them, and the footer offers them
/// where it has room. Text entry is left out too: in a search field every printable key types.
const GLOBAL: [&str; 10] = ["q", "C-c", "?", "r", "A", "1", "2", "3", "4", "5"];

/// Where room runs out, the first of each pair outlasts the second.
const OUTLASTS: [(&str, &str); 2] = [("d", "h"), ("q", "h")];

#[derive(Debug, PartialEq)]
enum Outcome {
    Acts,
    Explains,
    Nothing,
}

fn keys() -> Vec<KeyEvent> {
    let mut v: Vec<KeyEvent> = (' '..='~')
        .map(|c| {
            KeyEvent::new(
                KeyCode::Char(c),
                if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE },
            )
        })
        .collect();
    v.extend(['c', 'd', 'u'].map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
    v.push(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    for code in [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
    ] {
        v.push(KeyEvent::new(code, KeyModifiers::NONE));
    }
    v
}

fn name(k: &KeyEvent) -> String {
    match k.code {
        KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) => format!("C-{c}"),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    }
}

/// The footer keys that would cover pressing `key`: the arrows and paging keys move as j and k do, and on the Review
/// page's findings n and N move as j and k do.
fn covering<'a>(key: &'a str, m: &Model) -> Vec<&'a str> {
    let findings = m.ui.screen == Screen::Review && m.ui.focus == Pane::Main && m.top_modal().is_none();
    match key {
        "n" | "N" if findings => vec!["j", "k"],
        "Down" | "Up" | "g" | "G" | "Home" | "End" | "PageDown" | "PageUp" | "C-d" | "C-u" => vec!["j", "k"],
        "Left" => vec!["h"],
        "Right" => vec!["l"],
        "Tab" | "BackTab" => vec!["h", "l", "Tab"],
        other => vec![other],
    }
}

/// The keys a footer hint names: `j/k move` names j and k, `n/Esc cancel` n and Esc, `Enter or l code` Enter and l.
fn hint_keys(hint: &str) -> Vec<String> {
    let words: Vec<&str> = hint.split(' ').collect();
    let mut out: Vec<String> = match words[0] {
        "/" => vec!["/".into()],
        "1-5" => (1..=5).map(|d| d.to_string()).collect(),
        first => first.split('/').map(String::from).collect(),
    };
    if words.get(1) == Some(&"or") {
        out.extend(words.get(2).map(|w| w.to_string()));
    }
    out
}

/// The footer's hints, drawn `cols` wide at the model's own size, without any toast.
fn footer(m: &Model, cols: u16) -> Vec<String> {
    let mut m = m.clone();
    m.notice = None;
    let h = m.size.1;
    let mut term = Terminal::new(TestBackend::new(cols, h)).unwrap();
    term.draw(|f| view(&m, f)).unwrap();
    let buf = term.backend().buffer();
    let row: String = (0..cols).map(|x| buf[(x, h - 1)].symbol().to_string()).collect();
    let hints = row.split(" 1 inbox").next().unwrap_or("");
    hints.split("  ").map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The whole frame without any toast, styles and footer included: a cursor is often only a background, and a key that
/// changes only the footer's hints changes the mode. The Timeline's line that restates its filters is left out: a
/// filter that changes only that line narrows nothing.
fn body(m: &Model) -> ratatui::buffer::Buffer {
    let mut m = m.clone();
    m.notice = None;
    let (w, h) = m.size;
    let mut buf = render(&mut m, w, h).1.backend().buffer().clone();
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let first = row.split_whitespace().next().unwrap_or("");
        if m.ui.screen == Screen::Timeline && first.parse::<u64>().is_ok() && row.contains(" event") {
            for x in 0..w {
                buf[(x, y)].reset();
            }
        }
    }
    buf
}

/// Keys the screen's own words name, as in "s starts one", "… 6 more, J scrolls" or "n next finding": a key, then a
/// verb.
fn mentions(m: &Model) -> BTreeSet<String> {
    const KEYS: &str = "qjkgGhldtrAsRcpwJKnNfvPyom";
    let mut m = m.clone();
    m.notice = None;
    let (w, h) = m.size;
    let (frame, _) = render(&mut m, w, h);
    let mut out = BTreeSet::new();
    for row in frame.lines().take(h as usize - 1) {
        let words: Vec<&str> = row.split(|c: char| c.is_whitespace() || ",;·|(".contains(c)).collect();
        for pair in words.windows(2) {
            let key = matches!(pair[0], "Esc" | "Enter") || pair[0].len() == 1 && KEYS.contains(pair[0]);
            let lower = pair[1].chars().all(|c| c.is_ascii_lowercase());
            let verb = lower && (pair[1].len() >= 4 && pair[1].ends_with('s') || pair[1] == "next");
            if key && verb {
                out.insert(pair[0].to_string());
            }
        }
    }
    out
}

/// A key that cycles a Timeline filter acts when some press in its cycle changes the rows.
fn outcome(before: &Model, key: KeyEvent) -> Outcome {
    let first = press_once(before, key);
    let cycles = matches!(key.code, KeyCode::Char('v' | 'P'));
    if first != Outcome::Nothing || !cycles || before.ui.screen != Screen::Timeline || before.top_modal().is_some() {
        return first;
    }
    let mut m = before.clone();
    for _ in 0..12 {
        update(&mut m, Msg::Key(key));
        if body(&m) != body(before) {
            return Outcome::Acts;
        }
    }
    first
}

fn press_once(before: &Model, key: KeyEvent) -> Outcome {
    let mut after = before.clone();
    let fx = update(&mut after, Msg::Key(key));
    let modals = |m: &Model| format!("{:?}", m.ui.modals);
    let refused = matches!(after.top_modal(), Some(Modal::Confirm { state: ConfirmState::Refused { .. }, .. }));
    if refused && modals(before) != modals(&after) {
        return Outcome::Explains;
    }
    if fx.iter().any(|e| matches!(e, Effect::Send(_) | Effect::Quit | Effect::OpenUrl(_))) {
        return Outcome::Acts;
    }
    if modals(before) != modals(&after) {
        return Outcome::Acts;
    }
    let state = |m: &Model| format!("{:?}", (&m.ui, &m.diff, &m.timeline));
    if state(before) != state(&after) && body(before) != body(&after) {
        return Outcome::Acts;
    }
    if after.notice != before.notice { Outcome::Explains } else { Outcome::Nothing }
}

fn showcase_player() -> (Player, Model) {
    let mut p = Player::new(load(&protocol_dir().join("playback/showcase.ndjson")).unwrap());
    let mut m = model();
    let fx = update(&mut m, Msg::Net(NetEvent::Opened));
    drive(&mut p, &mut m, fx);
    (p, m)
}

fn drive(p: &mut Player, m: &mut Model, mut fx: Vec<Effect>) {
    loop {
        let mut replies = Vec::new();
        for e in fx.drain(..) {
            if let Effect::Send(req) = e {
                replies.extend(p.answer(&req));
            }
        }
        if replies.is_empty() {
            let Some((i, _)) = p.upcoming() else { return };
            replies.push(p.play(i));
        }
        for r in replies {
            fx.extend(update(m, Msg::Net(NetEvent::Message(r))));
        }
    }
}

fn press(c: char) -> KeyEvent {
    match c {
        '\n' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        '\x1b' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        c if c.is_ascii_uppercase() => KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT),
        c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
    }
}

/// The whole showcase played, PR `number` read in Review, then `keys` pressed with the playback answering.
fn showcase_after(number: u64, keys: &str) -> Model {
    showcase_player_after(number, keys).1
}

fn showcase_player_after(number: u64, keys: &str) -> (Player, Model) {
    let (mut p, mut m) = showcase_player();
    let id = m.domain.prs.values().find(|p| p.number == number).map(|p| p.id.clone());
    update(&mut m, Msg::Key(press('g')));
    while m.ui.selected_pr != id {
        update(&mut m, Msg::Key(press('j')));
    }
    let fx = update(&mut m, Msg::Key(press('\n')));
    drive(&mut p, &mut m, fx);
    for c in keys.chars() {
        let fx = update(&mut m, Msg::Key(press(c)));
        drive(&mut p, &mut m, fx);
    }
    (p, m)
}

fn sim_keys(s: &Sim, keys: &str) -> Sim {
    let mut s = s.clone();
    for c in keys.chars() {
        s.apply(Msg::Key(press(c)));
    }
    s
}

/// Every state the property walks, by name.
fn states() -> Vec<(String, Model)> {
    let mut out = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(protocol_dir().join("playback"))
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.strip_suffix(".ndjson").map(String::from))
        .collect();
    names.sort();
    for name in names {
        let lines = script(&name);
        let mut s = Sim::new(Look::default());
        let n = lines.len();
        for (i, msg) in lines.into_iter().enumerate() {
            s.server(msg);
            if name == "burst" && i % 1000 != 0 && i + 1 != n {
                continue;
            }
            for keys in ["", "\n", "d", "t", "3", "4", "5"] {
                out.push((format!("{name}[{}] {keys:?}", i + 1), sim_keys(&s, keys).m));
            }
        }
    }
    let actions = [
        "",
        "\n",
        "h",
        "j",
        "jj",
        "J",
        "d",
        "dh",
        "d/a\n",
        "t",
        "t\n",
        "tvvv",
        "tPPPPPPPP",
        "5",
        "?",
        "s",
        "R",
        "c",
        "p",
        "sy",
        "Ry",
        "cy",
        "py",
        "pyy",
        "cy\x1b",
        "w",
    ];
    for number in [1287, 341, 1291, 338, 345, 1296] {
        for keys in actions {
            out.push((format!("showcase #{number} {keys:?}"), showcase_after(number, keys)));
        }
    }
    // A confirmation waiting on its reply, accepted with nothing after it yet, and timed out.
    for (number, key) in [(1296, "s"), (1291, "p"), (338, "c"), (345, "R")] {
        let (mut p, mut m) = showcase_player_after(number, key);
        let fx = update(&mut m, Msg::Key(press('y')));
        out.push((format!("showcase #{number} {key:?} y requested"), m.clone()));
        let mut accepted = m.clone();
        for e in &fx {
            if let Effect::Send(req) = e {
                for r in p.answer(req) {
                    update(&mut accepted, Msg::Net(NetEvent::Message(r)));
                }
            }
        }
        out.push((format!("showcase #{number} {key:?} y accepted"), accepted));
        let later = m.now_ms + m.request_timeout_ms + 1;
        update(&mut m, Msg::Tick(later));
        out.push((format!("showcase #{number} {key:?} y timed out"), m));
    }
    let mut none = Sim::new(Look::default());
    none.apply(Msg::Net(NetEvent::Lost { reason: "connection refused".into(), retry_in_ms: Some(1000) }));
    for keys in ["", "\n", "2", "d", "t", "4", "5", "?", "s"] {
        out.push((format!("no daemon {keys:?}"), sim_keys(&none, keys).m));
    }
    out
}

#[test]
fn the_footer_offers_a_key_exactly_when_it_acts() {
    let keys = keys();
    let mut census: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut order: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut flag = |kind: &str, what: String, at: &str| {
        census.entry(format!("{kind}: {what}")).or_default().insert(at.to_string());
    };
    for (label, base) in states() {
        if matches!(base.top_modal(), Some(Modal::Input { .. })) {
            continue;
        }
        for (w, h) in SIZES {
            let mut m = base.clone();
            update(&mut m, Msg::Resize(w, h));
            let at = format!("{label} {w}x{h}");
            let full = footer(&m, 400);
            let kept = footer(&m, w);
            let modal = m.top_modal().map(|x| format!("{x:?}").split([' ', '{']).next().unwrap_or("").to_string());
            let screen = format!("{:?}{}", m.ui.screen, modal.map_or(String::new(), |x| format!("+{x}")));
            let acts: BTreeMap<String, Outcome> = keys
                .iter()
                .filter(|k| keymap::resolve(m.ui.screen, m.top_modal(), **k).is_some())
                .map(|k| (name(k), outcome(&m, *k)))
                .collect();
            let acted = |key: &str| acts.get(key) == Some(&Outcome::Acts);
            for key in mentions(&m) {
                if acts.get(&key) != Some(&Outcome::Acts) {
                    flag("named on screen, does nothing", format!("{key} on {screen}"), &at);
                }
            }
            for hint in &full {
                if !hint_keys(hint).iter().any(|k| acted(k)) {
                    flag("offered, does nothing", format!("{hint:?} on {screen}"), &at);
                }
            }
            let offered: BTreeSet<String> = full.iter().flat_map(|h| hint_keys(h)).collect();
            for (key, o) in &acts {
                if *o == Outcome::Acts
                    && !GLOBAL.contains(&key.as_str())
                    && !covering(key, &m).iter().any(|k| offered.contains(*k))
                {
                    flag("acts, not offered", format!("{key} on {screen}"), &at);
                }
            }
            let mut rest = full.iter();
            if !kept.iter().all(|k| rest.any(|f| f == k)) {
                flag("kept is not a subsequence", format!("{kept:?} of {full:?}"), &at);
            }
            let has = |list: &[String], key: &str| list.iter().any(|h| hint_keys(h).iter().any(|k| k == key));
            for (more, less) in OUTLASTS {
                if has(&full, more) && has(&kept, less) && !has(&kept, more) {
                    flag("ranking", format!("{less} kept over {more}: {kept:?}"), &at);
                }
            }
            for (i, a) in full.iter().enumerate() {
                for b in &full[i + 1..] {
                    if let Some(seen) = order.get(&(b.clone(), a.clone())) {
                        flag("order", format!("{a:?} before {b:?}, but the other way in {seen}"), &at);
                    }
                    order.entry((a.clone(), b.clone())).or_insert(at.clone());
                }
            }
        }
    }
    let report: Vec<String> = census
        .iter()
        .map(|(what, at)| format!("{what}  ({} states, e.g. {})", at.len(), at.iter().next().unwrap()))
        .collect();
    assert!(report.is_empty(), "{} disagreements:\n{}", report.len(), report.join("\n"));
}
