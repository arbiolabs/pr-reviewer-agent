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
