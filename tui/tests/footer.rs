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
use pr_review_tui::protocol::Phase;
use pr_review_tui::update::{Effect, Msg, NetEvent, update};
use pr_review_tui::view::view;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const SIZES: [(u16, u16); 3] = [(60, 16), (80, 24), (120, 38)];

/// Keys every footer may leave out: they act the same on every screen, help lists them, and the footer offers them
/// where it has room. Text entry is left out too: in a search field every printable key types.
const GLOBAL: [&str; 11] = ["q", "C-c", "?", "r", "A", "a", "1", "2", "3", "4", "5"];

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
    hints_of(&row)
}

/// A footer row's hints, without the views list at its right: names (`1 inbox  2 review …`) or numbers (`1 2 3 4 5`).
fn hints_of(row: &str) -> Vec<String> {
    let row = row.trim_end();
    let hints = row.split(" 1 inbox").next().unwrap_or("");
    let hints = hints.strip_suffix("1 2 3 4 5").unwrap_or(hints);
    hints.split("  ").map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The footer row of `m` drawn at its own size.
fn footer_row(m: &Model) -> String {
    let mut m = m.clone();
    m.notice = None;
    let (w, h) = m.size;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view(&m, f)).unwrap();
    let buf = term.backend().buffer();
    (0..w).map(|x| buf[(x, h - 1)].symbol().to_string()).collect()
}

fn hint_rank(keys: &str, label: &str) -> usize {
    let keys = if keys == "s" && label == "start another" { "s/S" } else { keys };
    pr_review_tui::offer::RANK.iter().position(|k| *k == keys).unwrap()
}

/// At every width from 60 to 200 the footer drops hints least important first, drops or shrinks the views list
/// before any hint, and a wider terminal never shows fewer hints.
#[test]
fn the_footer_gives_its_hints_room_before_the_views_list_and_never_loses_one_to_width() {
    let mut cases = vec![
        ("inbox".to_string(), showcase_player().1),
        ("settings".to_string(), {
            let mut m = showcase_player().1;
            update(&mut m, Msg::Key(press('5')));
            m
        }),
    ];
    for (number, keys) in [(1291, ""), (1291, "h"), (338, ""), (338, "h"), (345, ""), (1287, "t"), (1296, "")] {
        cases.push((format!("#{number} {keys:?}"), showcase_after(number, keys)));
    }
    let mut bad = Vec::new();
    for (name, base) in cases {
        let mut prev: Option<(u16, BTreeSet<String>)> = None;
        for w in 60..=200u16 {
            let mut m = base.clone();
            update(&mut m, Msg::Resize(w, 38));
            let row = footer_row(&m);
            let shown = hints_of(&row);
            let keys: BTreeSet<String> = shown.iter().map(|h| h.split(' ').next().unwrap().to_string()).collect();
            let offered = pr_review_tui::offer::offered(&m);
            let missing: Vec<_> = offered.iter().filter(|h| !keys.contains(h.keys)).collect();
            let worst_shown =
                offered.iter().filter(|h| keys.contains(h.keys)).map(|h| hint_rank(h.keys, h.label)).max();
            if let (Some(best_missing), Some(worst)) =
                (missing.iter().map(|h| hint_rank(h.keys, h.label)).min(), worst_shown)
                && best_missing < worst
            {
                bad.push(format!("{name} {w}: dropped a hint ranked above one it shows: {row:?}"));
            }
            let unstripped: Vec<String> =
                pr_review_tui::offer::fit(offered.clone(), w as usize - 2).iter().map(|h| h.text()).collect();
            if shown != unstripped {
                bad.push(format!("{name} {w}: the views list costs hints {unstripped:?}: {row:?}"));
            }
            // A hint the layout offers at one width only (J/K where the page wraps longer) is not the footer's to keep.
            let still: BTreeSet<String> = offered.iter().map(|h| h.keys.to_string()).collect();
            if let Some((pw, before)) = &prev
                && !before.intersection(&still).all(|k| keys.contains(k))
            {
                bad.push(format!("{name}: {pw} columns show {before:?}, {w} show {keys:?}"));
            }
            prev = Some((w, keys));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The cases the jury met, by name: the README's own 120x38 and the 60x16 minimum.
#[test]
fn the_footer_keeps_quit_and_the_main_action_where_the_jury_lost_them() {
    let at = |mut m: Model, w: u16, h: u16| {
        update(&mut m, Msg::Resize(w, h));
        hints_of(&footer_row(&m)).iter().map(|h| h.split(' ').next().unwrap().to_string()).collect::<Vec<_>>()
    };
    let has = |got: &[String], want: &[&str]| want.iter().all(|k| got.iter().any(|g| g == k));
    let parked = at(showcase_after(1291, ""), 120, 38);
    assert!(has(&parked, &["p", "c", "d", "q"]), "{parked:?}");
    let timeline = at(showcase_after(1287, "t"), 120, 38);
    assert!(has(&timeline, &["f", "v", "P", "q"]), "{timeline:?}");
    let mut settings = showcase_player().1;
    update(&mut settings, Msg::Key(press('5')));
    let settings = at(settings, 60, 16);
    assert!(has(&settings, &["j/k", "Enter", "Esc", "q"]), "{settings:?}");
    let rail = at(showcase_after(1291, "h"), 60, 16);
    assert!(has(&rail, &["p", "q"]), "{rail:?}");
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
    // A daemon restarted in dry_run grants no review.publish: a start on the parked head goes to it.
    let mut dry = Sim::new(Look::default());
    for line in read("playback/approval.ndjson").lines() {
        let line = line.replace(r#","review.publish""#, "");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        dry.server(serde_json::from_value(v["message"].clone()).unwrap());
        for keys in ["", "\n", "h"] {
            out.push((format!("dry_run approval {keys:?}"), sim_keys(&dry, keys).m));
        }
    }
    // Pages that hold one entry with more behind them: j on the one file, k on the one event, loads the next.
    let parked = Sim::new(Look::default()).play("approval", 5);
    let mut one_file = sim_keys(&parked, "\n3h").m;
    let d = one_file.diff.as_mut().unwrap();
    (d.files, d.next_cursor, d.pending) =
        (d.files[..1].to_vec(), Some(pr_review_tui::protocol::Text::new("c-2")), None);
    d.selected_file = Some(d.files[0].path.clone());
    out.push(("one-file diff page, more to load".into(), one_file));
    let mut one_event = sim_keys(&parked, "\n4").m;
    let t = one_event.timeline.as_mut().unwrap();
    let last = t.events.iter().rposition(|e| !pr_review_tui::update::is_heading(e)).unwrap();
    (t.events, t.next_before, t.pending) = (vec![t.events[last].clone()], Some(1), None);
    t.selected = Some(t.events[0].sequence.get());
    out.push(("one-event timeline page, older to load".into(), one_event));
    // A preview whose PR moved on: y is off.
    let mut stale = sim_keys(&parked, "\np").m;
    let pr = stale.domain.prs.values_mut().next().unwrap();
    pr.head_sha = pr_review_tui::protocol::Sha::new("9999999999999999999999999999999999999999");
    out.push(("stale preview".into(), stale));
    let mut none = Sim::new(Look::default());
    none.apply(Msg::Net(NetEvent::Lost { reason: "connection refused".into(), retry_in_ms: Some(1000) }));
    for keys in ["", "\n", "2", "d", "t", "4", "5", "?", "s"] {
        out.push((format!("no daemon {keys:?}"), sim_keys(&none, keys).m));
    }
    out
}

/// Whether j/k has somewhere to go, from the model's data alone and not through the reducer's guard: another PR,
/// another stop on the Review page, another file or a page of them to load, another event or older ones to load,
/// another account. It answers only where the data says so for certain.
fn room_to_move(m: &Model) -> bool {
    if m.top_modal().is_some() {
        return false;
    }
    match (m.ui.screen, m.ui.focus) {
        (Screen::Inbox, _) | (Screen::Review, Pane::Sidebar) => m.inbox().len() > 1,
        (Screen::Review, Pane::Main) => m
            .selected_pr()
            .is_some_and(|p| m.domain.pr_findings(&p.id).len() + usize::from(m.parked_run().is_some()) > 1),
        (Screen::Diff, Pane::Sidebar) => {
            m.diff.as_ref().is_some_and(|d| d.files.len() > 1 || d.next_cursor.is_some() && d.pending.is_none())
        }
        (Screen::Diff, Pane::Main) => false,
        (Screen::Timeline, _) => m.timeline.as_ref().is_some_and(|t| t.next_before.is_some() && t.pending.is_none()),
        (Screen::Settings, _) => m.domain.accounts.as_ref().is_some_and(|a| a.len() > 1),
    }
}

/// Whether the daemon takes a start of the selected PR, from what it granted and what the client knows: it refuses
/// for certain only a head parked under the same policy, and a daemon without review.publish runs another policy.
/// It refuses a head whose latest run ended there whatever the policy, so the client does not offer it.
fn daemon_takes_start(m: &Model) -> bool {
    let Some(p) = m.selected_pr().filter(|_| m.top_modal().is_none()) else { return false };
    let parked = m.domain.runs.values().any(|r| {
        r.pr_id == p.id && r.head_sha == p.head_sha && r.base_sha == p.base_sha && r.phase == Phase::AwaitingApproval
    });
    let ended = m
        .domain
        .latest_run(&p.id)
        .is_some_and(|r| r.head_sha == p.head_sha && r.status != pr_review_tui::protocol::RunStatus::Running);
    matches!(m.ui.screen, Screen::Inbox | Screen::Review)
        && !ended
        && m.link == pr_review_tui::model::Link::Live
        && m.may_write("review.start")
        && !(parked && m.may_write("review.publish"))
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
            if room_to_move(&m) && !offered.contains("j") {
                flag("oracle: somewhere to move, j/k not offered", screen.clone(), &at);
            }
            if daemon_takes_start(&m) && !offered.contains("s") {
                flag("oracle: the daemon takes a start, s not offered", screen.clone(), &at);
            }
            for (key, o) in &acts {
                if *o == Outcome::Acts
                    && !GLOBAL.contains(&key.as_str())
                    && !covering(key, &m).iter().any(|k| offered.contains(*k))
                {
                    flag("acts, not offered", format!("{key} on {screen}"), &at);
                }
            }
            let mut rest = full.iter();
            let key = |h: &String| h.split(' ').next().unwrap_or("").to_string();
            if !kept.iter().all(|k| rest.any(|f| key(f) == key(k))) {
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

/// On a parked run j and k also stop on the "Ready to publish" line, which is not a finding, so the hint says move.
#[test]
fn j_and_k_on_a_parked_review_are_offered_as_moves_not_findings() {
    let label = |number: u64| {
        let mut m = showcase_after(number, "");
        update(&mut m, Msg::Resize(120, 38));
        hints_of(&footer_row(&m)).into_iter().find(|h| h.starts_with("j/k")).unwrap()
    };
    assert_eq!(label(1291), "j/k move");
    assert_eq!(label(1287), "j/k finding");
}

/// The failed PR's Diff names its next step, and the key it names acts there.
#[test]
fn the_failed_prs_diff_says_r_retries_and_r_does() {
    let mut m = showcase_after(345, "d");
    let (frame, _) = render(&mut m, 120, 38);
    assert!(frame.contains("The review failed before it read the diff. R retries the review."), "{frame}");
    assert!(hints_of(&footer_row(&m)).contains(&"R retry".to_string()), "{frame}");
    update(&mut m, Msg::Key(press('R')));
    assert!(
        matches!(m.top_modal(), Some(Modal::Confirm { prompt, .. }) if prompt.starts_with("Retry the failed review of #345"))
    );
}
