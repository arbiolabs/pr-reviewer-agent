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
    assert!(frame.contains("quit; reviews keep running"), "{frame}");
    assert!(frame.lines().all(|l| !l.contains('…') || l.contains("more below, j scrolls")), "{frame}");
}

#[test]
fn help_marks_write_keys_the_daemon_does_not_grant() {
    use pr_review_tui::protocol::ServerMessage;
    // The fixture daemon grants nothing.
    let mut s = Sim::new(Look::default());
    s.server(ServerMessage::parse(&read("fixtures/server.welcome.json")).unwrap());
    s.server(ServerMessage::parse(&read("fixtures/server.snapshot.json")).unwrap());
    let mut s = s.key(crossterm::event::KeyCode::Char('?'));
    let (frame, _) = render(&mut s.m, 120, 38);
    for what in ["start a review", "retry the run", "cancel the run", "preview and publish"] {
        assert!(frame.contains(&format!("{what} (not offered)")), "{what}:\n{frame}");
    }
    let mut granted = Sim::new(Look::default());
    let welcome = read("fixtures/server.welcome.json")
        .replace("\"capabilities\": []", "\"capabilities\": [\"review.start\", \"review.cancel\"]");
    granted.server(ServerMessage::parse(&welcome).unwrap());
    granted.server(ServerMessage::parse(&read("fixtures/server.snapshot.json")).unwrap());
    let mut granted = granted.key(crossterm::event::KeyCode::Char('?'));
    let (frame, _) = render(&mut granted.m, 120, 38);
    assert!(frame.contains("start a review") && !frame.contains("start a review (not offered)"), "{frame}");
    assert!(frame.contains("retry the run (not offered)"), "{frame}");
}

#[test]
fn the_showcase_timeline_has_history_for_every_pr() {
    use crossterm::event::KeyCode;
    for (pr, says) in [
        ("1287", "review finished"),
        ("341", "review finished"),
        ("1291", "updated"),
        ("338", "review superseded by a newer head"),
        ("345", "review failed"),
    ] {
        let mut s = showcase(2);
        let id = s.m.domain.prs.values().find(|p| p.number.to_string() == pr).unwrap().id.clone();
        s.m.ui.selected_pr = Some(id);
        let mut s = s.key(KeyCode::Char('t'));
        let (frame, _) = render(&mut s.m, 120, 38);
        assert!(frame.contains(says), "#{pr} timeline lacks {says:?}:\n{frame}");
    }
}

#[test]
fn an_empty_timeline_says_whether_filters_hide_events() {
    use crossterm::event::KeyCode;
    let mut quiet = mid_review(Look::default()).select("acme/shop#186").key(KeyCode::Char('t'));
    let (frame, _) = render(&mut quiet.m, 120, 38);
    assert!(frame.contains("no events recorded yet"), "{frame}");

    // #184's events are a tool call, a finding and a phase change: no run event.
    let mut filtered = mid_review(Look::default()).select("acme/shop#184").key(KeyCode::Char('t'));
    for _ in 0..4 {
        filtered = filtered.key(KeyCode::Char('v'));
    }
    let (frame, _) = render(&mut filtered.m, 120, 38);
    assert!(frame.contains("no events match the filters (f v P /)"), "{frame}");
}

#[test]
fn an_open_finding_event_shows_the_finding_not_the_row_again() {
    use crossterm::event::KeyCode;
    use pr_review_tui::protocol::EventBody;
    let mut s = showcase(2).key(KeyCode::Char('t'));
    let t = s.m.timeline.as_mut().unwrap();
    let (seq, f) = t
        .events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::FindingUpdated(f) if f.severity == pr_review_tui::protocol::Severity::Critical => {
                Some((e.sequence.get(), f.clone()))
            }
            _ => None,
        })
        .unwrap();
    t.selected = Some(seq);
    let mut s = s.key(KeyCode::Enter);
    let (frame, _) = render(&mut s.m, 120, 38);
    let detail = s.m.details.get(&f.id).expect("opening a finding event fetches its detail");
    let first_words: String = detail.explanation.as_str().split(' ').take(6).collect::<Vec<_>>().join(" ");
    assert!(frame.contains(&format!("{}:{}", f.file, f.line.unwrap())), "file:line:\n{frame}");
    assert!(frame.contains(&first_words), "the explanation:\n{frame}");
    assert!(frame.contains("% confident"), "{frame}");
}

#[test]
fn an_open_tool_event_shows_its_whole_summary() {
    use crossterm::event::KeyCode;
    use pr_review_tui::protocol::EventBody;
    let mut s = showcase(2).key(KeyCode::Char('t'));
    let t = s.m.timeline.as_mut().unwrap();
    let (seq, summary) = t
        .events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ToolActivity(a) => Some((e.sequence.get(), a.summary.to_string())),
            _ => None,
        })
        .unwrap();
    t.selected = Some(seq);
    let mut s = s.key(KeyCode::Enter);
    let (frame, _) = render(&mut s.m, 80, 24);
    let shown: String = frame.split_whitespace().collect::<Vec<_>>().join(" ");
    let tail: String =
        summary.split(' ').rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
    assert!(shown.contains(&tail), "the summary's end {tail:?} shows at 80 columns:\n{frame}");
}

#[test]
fn a_long_hunk_header_wraps_like_the_code_under_it() {
    use crossterm::event::KeyCode;
    let mut s = showcase(2).key(KeyCode::Enter).key(KeyCode::Char('d')).key(KeyCode::Char('g'));
    let (frame, _) = render(&mut s.m, 80, 24);
    let joined: String = frame.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("@@ -84,8 +85,26 @@"), "{frame}");
    assert!(joined.contains("export function ordersRouter(db: Db, payments: Payments) {"), "{frame}");
}

#[test]
fn at_200_columns_the_rail_and_the_column_use_the_width() {
    let mut s = showcase(2);
    let (frame, _) = render(&mut s.m, 200, 50);
    assert!(frame.contains("#345 Partition ledger_entries by month"), "the wider rail shows whole titles:\n{frame}");
    let right = frame.lines().skip(3).map(|l| l.chars().count()).max().unwrap();
    assert!(right >= 200 - 24, "content ends at column {right}, a dead band on the right:\n{frame}");
    // Below 140 columns the rail keeps its 34.
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(frame.contains("#345 Partition ledger_entrie…"), "{frame}");
}

fn typed(mut s: Sim, text: &str) -> Sim {
    for ch in text.chars() {
        s = s.key(crossterm::event::KeyCode::Char(ch));
    }
    s
}

#[test]
fn slash_on_timeline_filters_its_events_by_text() {
    use crossterm::event::KeyCode;
    let s = showcase(2).key(KeyCode::Char('t')).key(KeyCode::Char('/'));
    let mut s = typed(s, "refund").key(KeyCode::Enter);
    assert_eq!(s.m.ui.pr_filter, "", "the PR list is not what / filters here");
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(frame.contains("Any signed-in customer can refund any order"), "{frame}");
    assert!(!frame.contains("review finished"), "{frame}");
    assert!(frame.contains("\"refund\" (/)"), "the header names the filter:\n{frame}");
}

#[test]
fn slash_on_diff_searches_its_text_and_n_walks_the_matches() {
    use crossterm::event::KeyCode;
    use pr_review_tui::model::{DiffRow, file_rows};
    let line_at_cursor = |s: &Sim| {
        let d = s.m.diff.as_ref().unwrap();
        match file_rows(d.file().unwrap()).get(d.cursor) {
            Some(DiffRow::Line(l)) => l.text.to_string(),
            _ => String::new(),
        }
    };
    let s = showcase(2).key(KeyCode::Enter).key(KeyCode::Char('d')).key(KeyCode::Char('g')).key(KeyCode::Char('/'));
    let mut s = typed(s, "refundedcents").key(KeyCode::Enter);
    let first = line_at_cursor(&s);
    assert!(first.to_lowercase().contains("refundedcents"), "{first:?}");
    s = s.key(KeyCode::Char('n'));
    let second = line_at_cursor(&s);
    assert!(second.to_lowercase().contains("refundedcents") && second != first, "{second:?}");
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(frame.contains("match 2 of"), "{frame}");
    let s = s.key(KeyCode::Char(':'));
    assert!(matches!(
        s.m.top_modal(),
        Some(pr_review_tui::model::Modal::Input { purpose: pr_review_tui::model::InputPurpose::JumpTo, .. })
    ));
}

fn click(s: &mut Sim, column: u16, row: u16) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let kind = MouseEventKind::Down(MouseButton::Left);
    s.apply(pr_review_tui::update::Msg::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }));
}

/// The screen cell where `text` starts in `frame`.
fn cell_of(frame: &str, text: &str) -> (u16, u16) {
    frame
        .lines()
        .enumerate()
        .find_map(|(y, l)| l.find(text).map(|i| (l[..i].chars().count() as u16, y as u16)))
        .unwrap_or_else(|| panic!("{text:?} is not on screen:\n{frame}"))
}

#[test]
fn clicks_select_diff_files_and_lines_timeline_events_and_footer_views() {
    use crossterm::event::KeyCode;
    let mut s = showcase(2).key(KeyCode::Enter).key(KeyCode::Char('d'));
    let (frame, _) = render(&mut s.m, 120, 38);
    let (x, y) = cell_of(&frame, "reserve.ts");
    click(&mut s, x, y);
    assert_eq!(s.m.diff.as_ref().unwrap().selected_file.as_ref().unwrap().as_str(), "src/server/inventory/reserve.ts");

    let (frame, _) = render(&mut s.m, 120, 38);
    let before = s.m.diff.as_ref().unwrap().cursor;
    let line = frame.lines().skip(8).find(|l| l.contains(" + ")).unwrap();
    let (x, y) = cell_of(&frame, line.trim());
    click(&mut s, x + 20, y);
    assert_ne!(s.m.diff.as_ref().unwrap().cursor, before, "a click on a patch line moves the cursor there");

    let mut s = s.key(KeyCode::Char('t'));
    let (frame, _) = render(&mut s.m, 120, 38);
    let (x, y) = cell_of(&frame, "review started");
    click(&mut s, x, y);
    let t = s.m.timeline.as_ref().unwrap();
    use pr_review_tui::protocol::{EventBody, RunChange};
    let started = t.events.iter().find(|e| matches!(e.body, EventBody::Run(RunChange::Started, _))).unwrap();
    let started = Some(started.sequence.get());
    assert_eq!(t.selected, started);
    click(&mut s, x, y);
    assert_eq!(s.m.timeline.as_ref().unwrap().open, started, "a second click opens the event");

    let (frame, _) = render(&mut s.m, 120, 38);
    let (x, y) = cell_of(&frame, "5 settings");
    click(&mut s, x + 3, y);
    assert_eq!(s.m.ui.screen, pr_review_tui::model::Screen::Settings);
}

#[test]
fn a_read_only_daemon_says_so_in_settings_and_on_the_help_page() {
    use pr_review_tui::protocol::ServerMessage;
    let off = "review commands are off; start the daemon with --allow-commands or set control.allow_commands";
    for (reason, note) in [
        ("another pr-review is running on /srv/pr-review/.data", "daemon is read-only"),
        (off, "commands are off (start the daemon with --allow-commands)"),
    ] {
        let mut s = Sim::new(Look::default());
        let welcome = read("fixtures/server.welcome.read-only.json")
            .replace("another pr-review is running on /srv/pr-review/.data", reason);
        s.server(ServerMessage::parse(&welcome).unwrap());
        s.server(ServerMessage::parse(&read("fixtures/server.snapshot.json")).unwrap());
        let mut s = s.key(crossterm::event::KeyCode::Char('5'));
        let (frame, _) = render(&mut s.m, 140, 45);
        assert!(frame.contains(&format!("mode                  read-only: {}", &reason[..40])), "{frame}");
        let mut s = s.key(crossterm::event::KeyCode::Char('?'));
        let (frame, _) = render(&mut s.m, 140, 45);
        let short = note.split(" (").next().unwrap();
        assert!(frame.contains(&format!("start a review ({short})")), "{frame}");
        s.apply(pr_review_tui::update::Msg::Key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(
            'G',
        ))));
        let (frame, _) = render(&mut s.m, 140, 45);
        assert!(frame.contains(&format!("Write keys are off: {note}.")), "{frame}");
    }
}

#[test]
fn the_showcase_parks_1291_and_its_preview_lands_every_comment_on_the_diff() {
    use pr_review_tui::protocol::{Event, ServerMessage};
    let msgs = script("showcase");
    let parked = msgs
        .iter()
        .position(|m| matches!(m, ServerMessage::Event(Event::Sequenced(e)) if matches!(&e.body, pr_review_tui::protocol::EventBody::PublicationChanged(_))))
        .unwrap();
    // #338 still runs: the status line names it and counts #1291 as waiting; w visits #338, then #1291.
    let mut s = showcase(parked + 1).key(crossterm::event::KeyCode::Char('w'));
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(
        first_line(&frame).contains("reviewing #338") && first_line(&frame).contains("◔ 1 awaiting you"),
        "{frame}"
    );
    let mut s = s.key(crossterm::event::KeyCode::Char('w'));
    let (frame, _) = render(&mut s.m, 120, 38);
    assert!(frame.contains("◔ Ready to publish · p previews"), "{frame}");
    let mut s = s.key(crossterm::event::KeyCode::Char('p'));
    let (frame, _) = render(&mut s.m, 120, 160);
    for want in [
        "Publish this review?",
        "Found 2 actionable issues.",
        "**High: Sign-in redirects to any URL passed in returnTo**",
        "every comment sits on a line of the diff",
        "y publishes exactly this to GitHub",
    ] {
        assert!(frame.contains(want), "{want}:\n{frame}");
    }
}

/// Rows of the body that show text, below the status line and above the footer.
fn body_rows(frame: &str) -> usize {
    let lines: Vec<&str> = frame.lines().collect();
    lines[2..lines.len() - 1].iter().filter(|l| !l.trim().is_empty()).count()
}

#[test]
fn g_page_down_and_the_wheel_keep_the_reader_and_the_preview_on_screen() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use pr_review_tui::update::Msg;
    let reader = || showcase(2).key(KeyCode::Enter).key(KeyCode::Enter);
    let preview = || parked(Look::default()).key(KeyCode::Char('p'));
    for (name, open) in [("reader", &reader as &dyn Fn() -> Sim), ("preview", &preview)] {
        for (w, h) in [(80, 24), (120, 38)] {
            let mut s = open();
            render(&mut s.m, w, h);
            let mut g = open().key(KeyCode::Char('G'));
            let (end, _) = render(&mut g.m, w, h);
            assert!(body_rows(&end) >= (h as usize - 3) / 2, "{name} {w}x{h} G blanks the page:\n{end}");
            for _ in 0..6 {
                s.apply(Msg::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)));
            }
            let (paged, _) = render(&mut s.m, w, h);
            assert!(body_rows(&paged) >= (h as usize - 3) / 2, "{name} {w}x{h} C-d x6 blanks the page:\n{paged}");
            for _ in 0..40 {
                s.apply(Msg::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 50,
                    row: 10,
                    modifiers: KeyModifiers::NONE,
                }));
            }
            let (wheeled, _) = render(&mut s.m, w, h);
            assert_eq!(wheeled, end, "{name} {w}x{h}: the wheel stops at the same end as G");
            let mut up = g.key(KeyCode::Char('k'));
            assert_ne!(render(&mut up.m, w, h).0, end, "{name} {w}x{h}: one k scrolls back from the end");
        }
    }
}
