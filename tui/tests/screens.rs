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
