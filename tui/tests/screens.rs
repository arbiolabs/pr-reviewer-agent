//! What the screens say in the showcase session, checked on rendered frames.

mod common;

use common::*;
use pr_review_tui::model::Look;

/// The showcase session after its first `lines` script lines.
fn showcase(lines: usize) -> Sim {
    Sim::new(Look::default()).play("showcase", lines)
}

fn first_line(frame: &str) -> &str {
    frame.lines().next().unwrap_or("")
}

#[test]
fn the_status_line_does_not_repeat_the_phase_it_already_names() {
    // welcome, snapshot, run.started, two tool calls, then #1291 enters TRIAGE.
    let mut s = showcase(6);
    let (frame, _) = render(&mut s.m, 120, 38);
    let top = first_line(&frame);
    assert!(top.contains("reviewing #1291 triage"), "{top}");
    assert!(!top.contains("entered"), "{top}");
}

#[test]
fn the_spinner_never_looks_like_the_separator_or_a_done_dot() {
    let mut s = showcase(6);
    let mut frames = std::collections::BTreeSet::new();
    for now in (0..3_000).step_by(125) {
        s.apply(pr_review_tui::update::Msg::Tick(now));
        let (frame, _) = render(&mut s.m, 120, 38);
        let glyph = first_line(&frame).trim_start().strip_prefix("pr-review · ").unwrap().chars().next().unwrap();
        frames.insert(glyph);
    }
    assert!(frames.len() > 2, "the spinner moves: {frames:?}");
    for dot in ['·', '•', '●', '○', '.', '∙'] {
        assert!(!frames.contains(&dot), "the spinner shows {dot:?}: {frames:?}");
    }
}

fn lose(s: &mut Sim, retry_in_ms: Option<u64>) {
    let reason = "connection refused".into();
    s.apply(pr_review_tui::update::Msg::Net(pr_review_tui::update::NetEvent::Lost { reason, retry_in_ms }));
}

#[test]
fn disconnected_the_status_line_says_so_instead_of_counting_prs() {
    let mut never = Sim::new(Look::default());
    lose(&mut never, Some(7_000));
    let (frame, _) = render(&mut never.m, 120, 38);
    let top = first_line(&frame);
    assert!(top.contains("not connected"), "{top}");
    assert!(!top.contains("tracked") && !top.contains("nothing running"), "{top}");

    let mut seen = showcase(2);
    s_tick(&mut seen, 12_000);
    lose(&mut seen, Some(7_000));
    let (frame, _) = render(&mut seen.m, 120, 38);
    let top = first_line(&frame);
    assert!(top.contains("last known state, 12s old"), "{top}");
    assert!(!top.contains("reviewing"), "a stale snapshot does not claim a review is running: {top}");
}

fn s_tick(s: &mut Sim, now: u64) {
    s.apply(pr_review_tui::update::Msg::Tick(now));
}

#[test]
fn the_disconnect_banner_fits_at_120_columns() {
    let mut seen = showcase(2);
    lose(&mut seen, Some(7_000));
    let (frame, _) = render(&mut seen.m, 120, 38);
    let banner = frame.lines().nth(2).unwrap().trim();
    assert_eq!(banner, "✗ disconnected · retry in 7s · last known state");

    let mut never = Sim::new(Look::default());
    lose(&mut never, Some(2_500));
    let (frame, _) = render(&mut never.m, 80, 24);
    assert_eq!(frame.lines().nth(2).unwrap().trim(), "✗ disconnected · retry in 3s");
}

#[test]
fn the_open_card_counts_its_evidence_in_plain_words() {
    let mut s = showcase(2).key(crossterm::event::KeyCode::Enter);
    let (frame, _) = render(&mut s.m, 120, 38);
    let hint =
        frame.lines().find(|l| l.contains("Enter ") && l.contains("the diff")).unwrap_or_else(|| panic!("{frame}"));
    assert!(hint.contains("Enter  3 evidence items · the fix · the diff"), "{hint}");
}

#[test]
fn a_burst_rate_is_clamped_to_fit_its_cell() {
    let mut s = Sim::new(Look::default()).play("burst", usize::MAX);
    let (frame, _) = render(&mut s.m, 120, 38);
    let rail: Vec<&str> =
        frame.lines().map(|l| l.get(..l.char_indices().nth(34).map_or(l.len(), |(i, _)| i)).unwrap()).collect();
    assert!(rail.iter().any(|l| l.trim_end().ends_with("99+/s")), "{}", rail.join("\n"));
    let mut s = s.select("acme/shop#184");
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(frame.contains("99+ calls/s"), "{frame}");
}

#[test]
fn settings_describes_the_display_in_plain_words() {
    let row = |frame: &str, key: &str| -> String {
        let line = frame
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{key} ")))
            .unwrap_or_else(|| panic!("{key}:\n{frame}"));
        line.trim_start()[key.len()..].trim().to_string()
    };
    let mut s = showcase(2).key(crossterm::event::KeyCode::Char('5'));
    let (frame, _) = render(&mut s.m, 120, 38);
    assert_eq!(row(&frame, "glyphs"), "unicode");
    assert_eq!(row(&frame, "color"), "on");
    assert_eq!(row(&frame, "background"), "painted (theme)");

    let look = Look { ascii: true, terminal_bg: true, color: false, ..Look::default() };
    let mut s = Sim::new(look).play("showcase", 2).key(crossterm::event::KeyCode::Char('5'));
    let (frame, _) = render(&mut s.m, 120, 38);
    assert_eq!(row(&frame, "glyphs"), "ascii");
    assert_eq!(row(&frame, "color"), "off");
    assert_eq!(row(&frame, "background"), "terminal");
}

#[test]
fn the_footer_lists_settings_among_the_views_when_it_fits() {
    let mut s = showcase(2);
    let (frame, _) = render(&mut s.m, 120, 38);
    let footer = frame.lines().last().unwrap();
    assert!(footer.ends_with("4 timeline  5 settings"), "{footer}");
}

#[test]
fn at_80_columns_the_footer_drops_minor_hints_before_navigation() {
    use crossterm::event::KeyCode;
    let mut review = showcase(2).key(KeyCode::Enter);
    let (frame, _) = render(&mut review.m, 80, 24);
    let footer = frame.lines().last().unwrap();
    for hint in ["j/k finding", "Enter read", "h pull requests", "? help"] {
        assert!(footer.contains(hint), "{hint}: {footer}");
    }
    let mut diff = review.key(KeyCode::Char('d'));
    let (frame, _) = render(&mut diff.m, 80, 24);
    let footer = frame.lines().last().unwrap();
    for hint in ["j/k line", "h files", "Esc back", "? help"] {
        assert!(footer.contains(hint), "{hint}: {footer}");
    }
}

#[test]
fn n_and_shift_n_walk_the_findings_inside_the_reader() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pr_review_tui::model::Modal;
    let mut s = showcase(2).key(KeyCode::Enter).key(KeyCode::Enter);
    let reader = |s: &mut Sim| {
        let (frame, _) = render(&mut s.m, 120, 38);
        frame.lines().find(|l| l.contains("finding ") && l.contains(" of ")).map(str::trim).unwrap_or("").to_string()
    };
    assert!(reader(&mut s).ends_with("finding 1 of 4"), "{}", reader(&mut s));
    s = s.key(KeyCode::Char('n'));
    assert!(reader(&mut s).ends_with("finding 2 of 4"), "{}", reader(&mut s));
    let Some(Modal::Finding { id, .. }) = s.m.top_modal() else { panic!("the reader closed") };
    assert!(s.m.details.contains_key(id), "the next finding's detail was fetched");
    assert_eq!(s.m.ui.selected_finding.as_ref(), Some(id), "the document follows the reader");
    s.apply(pr_review_tui::update::Msg::Key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)));
    s.apply(pr_review_tui::update::Msg::Key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)));
    assert!(reader(&mut s).ends_with("finding 1 of 4"), "N stops at the first: {}", reader(&mut s));
}

#[test]
fn help_at_120_columns_splits_between_sections_without_cutting_text() {
    let mut s = showcase(2).key(crossterm::event::KeyCode::Char('?'));
    let (frame, _) = render(&mut s.m, 120, 38);
    let row = |h: &str| frame.lines().position(|l| l.contains(h)).unwrap_or_else(|| panic!("{h}:\n{frame}"));
    assert!(row("Timeline") < row("Everywhere") + 22, "two columns:\n{frame}");
    assert!(frame.contains("quit; reviews keep running") && !frame.contains('…'), "{frame}");
}

#[test]
fn help_marks_write_keys_the_daemon_does_not_grant() {
    use pr_review_tui::protocol::ServerMessage;
    // The showcase daemon grants account.select only.
    let mut s = showcase(2).key(crossterm::event::KeyCode::Char('?'));
    let (frame, _) = render(&mut s.m, 120, 38);
    for what in ["start a review", "retry the run", "cancel the run", "publish the findings"] {
        assert!(frame.contains(&format!("{what} (not offered)")), "{what}:\n{frame}");
    }
    let mut granted = Sim::new(Look::default());
    let welcome = read("fixtures/server.welcome.json")
        .replace("\"capabilities\": []", "\"capabilities\": [\"review.start\", \"review.cancel\"]");
    granted.server(ServerMessage::parse(&welcome).unwrap());
    granted.server(ServerMessage::parse(&read("fixtures/server.snapshot.json")).unwrap());
    let mut granted = granted.key(crossterm::event::KeyCode::Char('?'));
    let (frame, _) = render(&mut granted.m, 120, 38);
    assert!(frame.lines().any(|l| l.trim_end().ends_with("start a review")), "{frame}");
    assert!(frame.contains("retry the run (not offered)"), "{frame}");
}
