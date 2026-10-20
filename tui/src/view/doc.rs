//! Editorial direction: a slim PR rail and one centered reading column. The PR reads like a document: title,
//! metadata, a one-line phase timeline, then each finding as a card with a severity bar and its cited code.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::Ctx;
use super::code::{highlight, rich};
use crate::model::{ActivityKind, Pane, Screen};
use crate::protocol::{Evidence, EvidenceKind, Finding, FindingDetail, FindingId, Phase, Pr, Run, RunStatus, Severity};
use crate::text::width;
use crate::theme::{Bg, Role, finding_status, phase_name};

pub const COLUMN: u16 = 100;

/// A braille spinner: no frame reads as the `·` separator or the `●` of a finished phase.
pub fn spinner(c: &Ctx) -> &'static str {
    const U: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const A: &[&str] = &["-", "\\", "|", "/"];
    let set = if c.t.ascii { A } else { U };
    set[(c.m.now_ms / 125) as usize % set.len()]
}

fn secs(at: &str) -> Option<i64> {
    let t = at.get(11..19)?;
    let p: Vec<i64> = t.split(':').filter_map(|x| x.parse().ok()).collect();
    (p.len() == 3).then(|| p[0] * 3600 + p[1] * 60 + p[2])
}

pub fn sev_word(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "CRITICAL",
        Severity::High => "HIGH",
        Severity::Medium => "MEDIUM",
        Severity::Low => "LOW",
        Severity::Unknown => "UNKNOWN",
    }
}

pub fn phase_label(p: Phase) -> &'static str {
    match p {
        Phase::CollectContext => "collect",
        Phase::Triage => "triage",
        Phase::Analyze => "analyze",
        Phase::Verify => "verify",
        Phase::PreparePublication => "prepare",
        Phase::Publish => "publish",
        Phase::Reconcile => "reconcile",
        Phase::Completed => "done",
        Phase::Failed => "failed",
        Phase::Superseded => "superseded",
        Phase::Unknown => "?",
    }
}

pub fn time_of(at: &str) -> &str {
    at.get(11..19).unwrap_or(at)
}

pub(crate) fn spans_w(spans: &[Span]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// Pads `spans` to exactly `w` columns with `style`.
pub(crate) fn filled(mut spans: Vec<Span<'static>>, w: usize, style: Style) -> Vec<Span<'static>> {
    let used = spans_w(&spans);
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), style));
    }
    spans
}

/// Left text and right text on one `w`-wide row.
pub(crate) fn split_row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let gap = w.saturating_sub(spans_w(&left) + spans_w(&right));
    let mut out = left;
    if gap > 0 {
        out.push(Span::raw(" ".repeat(gap)));
        out.extend(right);
    }
    out
}

pub fn current(c: &Ctx, pr: &Pr) -> Vec<Finding> {
    c.m.domain.pr_findings(&pr.id).into_iter().filter(|f| !f.stale).cloned().collect()
}

// ---------------------------------------------------------------- layout

/// Review and Inbox share one screen: Inbox is the same document with the rail in focus.
pub fn screen(c: &Ctx, f: &mut Frame, area: Rect) {
    let rail_focus = c.m.ui.screen == Screen::Inbox || c.focused(Pane::Sidebar);
    let (rail, doc_area) = split(area, rail_focus);
    if let Some(r) = rail {
        super::rail::rail(c, f, r, rail_focus);
    }
    let Some(doc_area) = doc_area else { return };
    let col = column(doc_area);
    let Some(pr) = c.m.selected_pr() else {
        let empty = super::rail::empty_rail(c, c.m.domain.prs.len());
        let lines = empty.into_iter().map(|(t, role)| Line::from(c.span(t.trim_start(), role)));
        let p = Paragraph::new(std::iter::once(Line::raw("")).chain(lines).collect::<Vec<_>>())
            .wrap(ratatui::widgets::Wrap { trim: false });
        f.render_widget(p, col);
        return;
    };
    let doc = document(c, pr, col.width as usize, col.height as usize);
    let scroll = doc.scroll(c, col.height as usize);
    f.render_widget(Paragraph::new(doc.lines).scroll((scroll, 0)), col);
    // Once the repository line and the title have scrolled off, a one-line sticky header stands in for them.
    if scroll >= 2 {
        f.render_widget(Paragraph::new(sticky(c, pr, col.width as usize)), Rect { height: 1, ..col });
    }
}

/// Below this many rows the document drops its spacer lines and the open card makes room for the sticky header.
const COMPACT_H: u16 = 28;

/// `#1287 Orders: self-serve…   ●─●─●─●─●─●  ◆1 ▲1 ●1 ○1`: the PR, its run's dot stepper and its counts.
fn sticky(c: &Ctx, pr: &Pr, w: usize) -> Line<'static> {
    let base = c.t.bg(Bg::Base);
    let mut right: Vec<Span<'static>> = Vec::new();
    if let Some(r) = c.m.domain.latest_run(&pr.id) {
        right.extend(super::live::stepper(c, r));
        right.push(Span::raw("  "));
    }
    right.extend(super::rail::counts(c, &current(c, pr), base));
    let num = format!("#{} ", pr.number);
    let room = w.saturating_sub(spans_w(&right) + width(&num) + 2);
    let left = vec![c.span(num, Role::Accent), c.span(c.fit(pr.title.as_str(), room), Role::Title)];
    Line::from(filled(split_row(left, right, w), w, base))
}

/// The rail and the document areas. Below 100 columns only one shows: the rail while it has focus.
pub fn split(area: Rect, rail_focus: bool) -> (Option<Rect>, Option<Rect>) {
    let rw = super::rail::RAIL_W;
    match area.width {
        100.. => (Some(Rect { width: rw, ..area }), Some(Rect { x: area.x + rw, width: area.width - rw, ..area })),
        _ if rail_focus => (Some(area), None),
        _ => (None, Some(area)),
    }
}

/// The reading column: at most `COLUMN` wide, centered, with a margin.
pub fn column(area: Rect) -> Rect {
    let w = area.width.saturating_sub(4).min(COLUMN);
    Rect { x: area.x + (area.width - w) / 2, width: w, ..area }
}

/// What a click at (`x`, `y`) on the Review or Inbox screen in `area` lands on.
pub fn hit(c: &Ctx, area: Rect, x: u16, y: u16) -> Option<super::Hit> {
    let rail_focus = c.m.ui.screen == Screen::Inbox || c.focused(Pane::Sidebar);
    let (rail, doc_area) = split(area, rail_focus);
    if let Some(r) = rail.filter(|r| x < r.x + r.width) {
        return super::rail::pr_at(c, r, y).map(super::Hit::Pr);
    }
    let col = column(doc_area?);
    let pr = c.m.selected_pr()?;
    let doc = document(c, pr, col.width as usize, col.height as usize);
    let line = (y.checked_sub(col.y)? + doc.scroll(c, col.height as usize)) as usize;
    doc.owners.iter().rev().find(|(start, _)| *start <= line).map(|(_, id)| super::Hit::Finding(id.clone()))
}

/// How far the reader's own offset can usefully go on the Review or Inbox screen in `area`.
pub fn scroll_limit(c: &Ctx, area: Rect) -> Option<usize> {
    let rail_focus = c.m.ui.screen == Screen::Inbox || c.focused(Pane::Sidebar);
    let col = column(split(area, rail_focus).1?);
    let doc = document(c, c.m.selected_pr()?, col.width as usize, col.height as usize);
    Some(doc.scroll_limit(col.height as usize))
}

// ---------------------------------------------------------------- document

/// The PR rendered as one scrollable document, and where the selected card sits in it.
pub struct Doc {
    pub lines: Vec<Line<'static>>,
    /// Lines `[start, end)` of the expanded card, plus the head of the card after it.
    pub card: Option<(usize, usize)>,
    /// The first line of each finding's card, in order, so a click resolves to the card under it.
    pub owners: Vec<(usize, FindingId)>,
    /// Rows the open card's explanation scrolls through before J moves the document itself.
    pub prose_rest: usize,
}

impl Doc {
    /// Scroll so the expanded card shows whole, the PR header kept when it fits too, then the reader's own offset.
    pub fn scroll(&self, c: &Ctx, height: usize) -> u16 {
        let (auto, max) = self.bounds(height);
        let own = (c.m.ui.doc_scroll.max(0) as usize).saturating_sub(self.prose_rest);
        (auto + own).min(max) as u16
    }

    /// The reader's offset past which J changes nothing: the explanation's rows, then the document's.
    pub fn scroll_limit(&self, height: usize) -> usize {
        let (auto, max) = self.bounds(height);
        self.prose_rest + max.saturating_sub(auto)
    }

    /// Where the selection puts the document, and the furthest it scrolls.
    fn bounds(&self, height: usize) -> (usize, usize) {
        let auto = match self.card {
            Some((start, end)) if end > height => end.saturating_sub(height).min(start.saturating_sub(1)),
            _ => 0,
        };
        (auto, self.lines.len().saturating_sub(height.min(self.lines.len()) / 2))
    }
}

pub fn document(c: &Ctx, pr: &Pr, w: usize, height: usize) -> Doc {
    let compact = height < COMPACT_H as usize;
    let g = c.t.g();
    let sep = format!(" {} ", g.sep);
    let run = c.m.domain.latest_run(&pr.id);
    let mut lines = vec![Line::raw("")];
    lines.push(Line::from(vec![
        c.span(pr.repository.to_string(), Role::Muted),
        c.span(format!("  #{}", pr.number), Role::Accent),
    ]));
    lines.push(Line::from(c.span(c.fit(pr.title.as_str(), w), Role::Title)));
    let mut meta = Vec::new();
    if let Some(a) = &pr.author {
        meta.push(format!("by {a}"));
    }
    meta.push(format!("{} {} {}", pr.base_sha.short(), g.arrow, pr.head_sha.short()));
    if let Some(r) = run {
        meta.push(r.model.to_string());
        meta.push(format!("run {}", r.id.short()));
    }
    lines.push(Line::from(c.span(c.fit(&meta.join(&sep), w), Role::Muted)));
    if !compact {
        lines.push(Line::raw(""));
    }
    lines.push(phase_line(c, run, w));
    if let Some(r) = run.filter(|r| r.head_sha != pr.head_sha) {
        let t =
            format!("{} stale: reviewed {}, the PR head is now {}", g.pending, r.head_sha.short(), pr.head_sha.short());
        lines.push(Line::from(c.span(c.fit(&t, w), Role::Pending)));
    }
    lines.push(Line::raw(""));

    let findings = c.m.domain.pr_findings(&pr.id);
    let running = run.is_some_and(|r| r.status == RunStatus::Running);
    lines.push(summary_line(c, pr, &findings, w));
    lines.push(Line::from(c.span(rule(c, w), Role::Muted)));
    if !compact {
        lines.push(Line::raw(""));
    }

    if let Some(r) = run.filter(|r| r.status == RunStatus::Failed) {
        lines.push(Line::from(c.span(format!("{} The review did not finish", g.fail), Role::Fail)));
        if let Some(reason) = &r.failure_reason {
            for l in rich(c, reason.as_str(), w.saturating_sub(2), Role::Normal) {
                let mut s = vec![Span::raw("  ")];
                s.extend(l);
                lines.push(Line::from(s));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(c.span("  R retries the run once the cause is fixed.", Role::Muted)));
        lines.push(Line::raw(""));
    }
    if running && c.m.look.show_activity {
        let log = if findings.is_empty() { height.saturating_sub(lines.len() + 6).max(5) } else { 4 };
        live(c, pr, w, log, &mut lines);
    }
    if findings.is_empty() && run.is_some_and(|r| r.status == RunStatus::Completed) {
        lines.push(Line::from(c.span(format!("{} No actionable issues found in the reviewed scope.", g.ok), Role::Ok)));
    }
    // The open card fits under the PR header (the sticky line when compact) with the next card's head still in
    // view, so at least two cards read at once when the screen has room for both.
    let sel = c.m.ui.selected_finding.as_ref();
    let focused = c.m.ui.screen == Screen::Review && c.m.ui.focus != Pane::Sidebar;
    let above = if compact { 1 } else { lines.len() };
    let mut card_at = None;
    let mut owners = Vec::new();
    let mut prose_rest = 0;
    for (i, f) in findings.iter().enumerate() {
        let start = lines.len();
        owners.push((start, f.id.clone()));
        let open = sel == Some(&f.id);
        let next = if i + 1 < findings.len() && !compact { NEXT_HEAD } else { 0 };
        let budget = if open { height.saturating_sub(above + next).max(MIN_CARD) } else { 0 };
        let (body, rest) = card(c, f, c.m.details.get(&f.id), w, open && focused, budget);
        lines.extend(body);
        prose_rest += rest;
        lines.push(Line::raw(""));
        if open {
            card_at = Some((start, lines.len() + next.saturating_sub(1)));
        }
    }
    let card = card_at.map(|(s, e)| (s, e.min(lines.len())));
    Doc { lines, card, owners, prose_rest }
}

/// Rows the next card's head needs to stay in view under the open card: its severity, title and location.
const NEXT_HEAD: usize = 4;
/// The smallest open card: head, three lines of explanation and five of code.
const MIN_CARD: usize = 16;

fn rule(c: &Ctx, w: usize) -> String {
    (if c.t.ascii { "-" } else { "─" }).repeat(w)
}

fn summary_line(c: &Ctx, pr: &Pr, findings: &[&Finding], w: usize) -> Line<'static> {
    let g = c.t.g();
    let live: Vec<_> = findings.iter().filter(|f| !f.stale).collect();
    let words = |compact: bool| {
        let mut left = vec![c.span(
            match live.len() {
                0 if c.m.domain.latest_run(&pr.id).is_some_and(|r| r.status == RunStatus::Completed) => {
                    "No findings".to_string()
                }
                0 => "No findings yet".to_string(),
                1 => "1 finding".to_string(),
                n => format!("{n} findings"),
            },
            Role::Title,
        )];
        for s in [Severity::Critical, Severity::High, Severity::Medium, Severity::Low] {
            let n = live.iter().filter(|f| f.severity == s).count();
            if n > 0 {
                let (glyph, _, role) = c.t.severity(s);
                let word = if compact { String::new() } else { format!(" {}", sev_word(s).to_lowercase()) };
                left.push(c.span(format!("{}{glyph} {n}{word}", if compact { "  " } else { "   " }), role));
            }
        }
        let stale = findings.len() - live.len();
        if stale > 0 {
            left.push(c.span(format!("   {stale} from an older head"), Role::Pending));
        }
        left
    };
    let left = words(false);
    let published = live.iter().filter(|f| f.status == crate::protocol::FindingStatus::Published).count();
    let right = if live.is_empty() {
        vec![]
    } else if published == 0 {
        vec![c.span(format!("{} none on GitHub yet", g.idle), Role::Muted)]
    } else {
        vec![c.span(format!("{} {published} of {} on GitHub", g.ok, live.len()), Role::Ok)]
    };
    let _ = pr;
    let left = if spans_w(&left) + spans_w(&right) + 2 > w { words(true) } else { left };
    Line::from(split_row(left, right, w))
}

/// `● Collect ── ● Triage ── ⠹ Analyze ── ○ Verify ...`, then how the run ended.
fn phase_line(c: &Ctx, run: Option<&Run>, w: usize) -> Line<'static> {
    let g = c.t.g();
    let Some(run) = run else { return Line::from(c.span("Not reviewed yet. s starts a review.", Role::Muted)) };
    const NAMES: [&str; 6] = ["Collect", "Triage", "Analyze", "Verify", "Prepare", "Publish"];
    let cur = match run.phase {
        Phase::CollectContext => 0,
        Phase::Triage => 1,
        Phase::Analyze => 2,
        Phase::Verify => 3,
        Phase::PreparePublication => 4,
        Phase::Publish | Phase::Reconcile => 5,
        Phase::Completed => 6,
        _ => 7,
    };
    let (done, todo, link) = if c.t.ascii { ("+", ".", " - ") } else { ("●", "○", " ── ") };
    let mut spans = Vec::new();
    for (i, name) in NAMES.iter().enumerate() {
        if i > 0 {
            spans.push(c.span(link, Role::Muted));
        }
        let (glyph, role) = match () {
            _ if cur == 7 => (todo, Role::Muted),
            _ if i < cur => (done, Role::Ok),
            _ if i == cur && run.status == RunStatus::Running => (spinner(c), Role::Accent),
            _ if i == cur => (g.pending, Role::Pending),
            _ => (todo, Role::Muted),
        };
        let label_role = if i == cur && cur < 6 {
            Role::Focus
        } else if i < cur {
            Role::Normal
        } else {
            Role::Muted
        };
        spans.push(c.span(format!("{glyph} "), role));
        spans.push(c.span(*name, label_role));
    }
    let start = secs(run.started_at.as_str());
    let tail = match run.status {
        RunStatus::Completed => {
            // A clock that ran backwards gives no duration rather than a negative one.
            let took = run.completed_at.as_ref().and_then(|e| Some(secs(e.as_str())? - start?)).filter(|s| *s >= 0);
            let took = took.map_or(String::new(), |s| format!(" in {}m {:02}s", s / 60, s % 60));
            (format!("{} done{took}", g.ok), Role::Ok)
        }
        RunStatus::Running => (format!("since {}", time_of(run.started_at.as_str())), Role::Muted),
        RunStatus::Failed => (format!("{} failed", g.fail), Role::Fail),
        RunStatus::Superseded => (format!("{} superseded", g.idle), Role::Muted),
        RunStatus::Unknown => ("?".into(), Role::Muted),
    };
    let right = vec![c.span(tail.0, tail.1)];
    if spans_w(&spans) + spans_w(&right) + 2 > w {
        // Narrow: drop the connectors.
        spans.retain(|s| s.content != link);
        for s in spans.iter_mut().filter(|s| s.content.ends_with(' ')) {
            s.content = format!(" {}", s.content).into();
        }
    }
    Line::from(split_row(spans, right, w))
}

/// The running review: a sparkline of tool calls, a phase bar per live run (this PR first), and this PR's newest
/// `log` events, newest last with the pulse on it.
fn live(c: &Ctx, pr: &Pr, w: usize, log: usize, lines: &mut Vec<Line<'static>>) {
    use super::live::{per_sec, phase_bar, rate, sparkline};
    let g = c.t.g();
    let spark_w = (w / 4).clamp(8, 30);
    let rate_now = per_sec(c, Some(&pr.id));
    let left = vec![c.span(format!("{} ", spinner(c)), Role::Accent), c.span("Reviewing now", Role::Title)];
    let right = vec![
        c.span(sparkline(c, &rate(c, Some(&pr.id), spark_w)), Role::Accent),
        c.span(format!("  {} calls/s", super::live::rate_label(rate_now)), Role::Muted),
    ];
    lines.push(Line::from(split_row(left, right, w)));
    lines.push(Line::raw(""));
    let mut runs: Vec<_> = c.m.domain.runs.values().filter(|r| r.status == RunStatus::Running).collect();
    runs.sort_by_key(|r| (r.pr_id != pr.id, c.m.domain.prs.get(&r.pr_id).map(|p| p.number)));
    for r in runs {
        let Some(p) = c.m.domain.prs.get(&r.pr_id) else { continue };
        let calls = c.m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&p.id)).count();
        let this = p.id == pr.id;
        let label = format!("{:<7}", format!("#{}", p.number));
        let tail = format!("  {:<8} {:>3} calls", super::doc::phase_label(r.phase), calls);
        let seg = w.saturating_sub(width(&label) + width(&tail) + 6) / 6;
        let mut s = vec![c.span(label, if this { Role::Title } else { Role::Muted })];
        s.extend(phase_bar(c, r, seg.clamp(2, 10)));
        s.push(c.span(tail, if this { Role::Accent } else { Role::Muted }));
        lines.push(Line::from(s));
    }
    lines.push(Line::raw(""));
    let items: Vec<_> = c.m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&pr.id)).collect();
    let shown = &items[items.len().saturating_sub(log)..];
    for (i, a) in shown.iter().enumerate() {
        let newest = i + 1 == shown.len();
        let at = time_of(a.at.as_str()).to_string();
        let (tool, text, role) = match &a.kind {
            ActivityKind::Tool { tool, summary } => (tool.to_string(), summary.to_string(), Role::Normal),
            ActivityKind::Phase { to, .. } => {
                ("phase".into(), format!("{} {}", g.arrow, phase_name(*to)), Role::Accent)
            }
            ActivityKind::Finding { title, severity, .. } => {
                let (glyph, sev, role) = c.t.severity(*severity);
                ("finding".into(), format!("{glyph} {sev} {title}"), role)
            }
            ActivityKind::Run { .. } => ("run".into(), "started".into(), Role::Muted),
        };
        let mark = if newest { spinner(c) } else { " " };
        let head = vec![
            c.span(format!("  {mark} "), Role::Accent),
            c.span(format!("{at}  "), Role::Muted),
            c.span(format!("{:<12}", c.fit(&tool, 12)), if newest { Role::Focus } else { Role::Accent }),
        ];
        let rest = w.saturating_sub(spans_w(&head));
        let mut s = head;
        s.push(c.span(c.fit(&text, rest), if newest { Role::Title } else { role }));
        lines.push(Line::from(s));
    }
    if items.is_empty() {
        lines.push(Line::from(c.span("  waiting for the first tool call", Role::Muted)));
    }
    lines.push(Line::raw(""));
}

// ---------------------------------------------------------------- cards

fn bar(c: &Ctx, sev: Severity, selected: bool) -> Span<'static> {
    let (_, _, role) = c.t.severity(sev);
    let glyph = match (c.t.ascii, selected) {
        (true, true) => "# ",
        (true, false) => "| ",
        (false, true) => "┃ ",
        (false, false) => "│ ",
    };
    c.span(glyph, role)
}

fn barred(c: &Ctx, sev: Severity, selected: bool, body: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
    body.into_iter()
        .map(|spans| {
            let mut s = vec![bar(c, sev, selected)];
            s.extend(spans);
            Line::from(s)
        })
        .collect()
}

fn card_head(c: &Ctx, f: &Finding, w: usize, selected: bool) -> Vec<Vec<Span<'static>>> {
    let g = c.t.g();
    let (glyph, _, role) = c.t.severity(f.severity);
    let (status, srole) = finding_status(f.status);
    let cat = format!("{:?}", f.category).to_lowercase();
    let left =
        vec![c.span(format!("{glyph} {}", sev_word(f.severity)), role), c.span(format!("   {cat}"), Role::Muted)];
    let right =
        vec![c.span(status, srole), c.span(format!(" {} {:.0}% confident", g.sep, f.confidence * 100.0), Role::Muted)];
    let title_role = if selected { Role::Title } else { Role::Normal };
    let loc = match f.line {
        Some(l) => format!("{}:{l}", f.file),
        None => format!("{} (whole file)", f.file),
    };
    let mut out = vec![
        split_row(left, right, w),
        vec![c.span(c.fit(f.title.as_str(), w), title_role)],
        vec![c.span(c.fit(&loc, w), Role::Accent)],
    ];
    if f.stale {
        out.push(vec![c.span("found on an older head; shown as found, not remapped", Role::Pending)]);
    }
    out
}

/// A finding card. Folded (`budget` 0): the head and the first line of the explanation. Open: the explanation and
/// the cited code, fitted to `budget` rows by the fold rule. The explanation gives way first, down to 3 lines, then
/// the code, down to 5 lines around its anchor, then the explanation again, down to 1. The explanation scrolls inside
/// its window with J/K, so the code under it never moves; each cut edge says how much it hides.
/// The card's lines, and how many rows its explanation can scroll.
fn card(
    c: &Ctx,
    f: &Finding,
    d: Option<&FindingDetail>,
    w: usize,
    focused: bool,
    budget: usize,
) -> (Vec<Line<'static>>, usize) {
    let iw = w.saturating_sub(2);
    let mut body = card_head(c, f, iw, focused || budget > 0);
    let Some(d) = d else {
        body.push(vec![c.span(loading(c, &f.id), Role::Muted)]);
        return (barred(c, f.severity, focused, body), 0);
    };
    let prose = rich(c, d.explanation.as_str(), iw, Role::Normal);
    if budget == 0 {
        let mut prose = prose;
        if prose.len() > 1 {
            prose.truncate(1);
            prose[0] = fold(c, std::mem::take(&mut prose[0]), iw, "");
        }
        body.extend(prose);
        return (barred(c, f.severity, focused, body), 0);
    }
    let ev = d.evidence.iter().find(|e| e.kind == EvidenceKind::CodeRef);
    let src_len = ev.map_or(0, |e| e.text.as_str().split('\n').count());
    let room = budget.saturating_sub(body.len() + 4);
    let (prose_rows, code_lines) = fold_rule(room, prose.len(), src_len.min(11), ev.is_some());
    let rest = prose.len().saturating_sub(prose_rows);
    body.push(vec![]);
    body.extend(prose_window(c, prose, prose_rows, c.m.ui.doc_scroll.max(0) as usize, iw));
    if let Some(ev) = ev {
        body.push(vec![]);
        body.extend(code_block(c, ev, f, iw, code_lines));
    }
    body.push(vec![]);
    let mut hint = vec![
        c.span("Enter", Role::Accent),
        c.span(format!("  {}    ", opens(d.evidence.len(), d.suggested_fix.is_some(), c.t.g().sep)), Role::Muted),
        c.span("d", Role::Accent),
        c.span(" diff at this line", Role::Muted),
    ];
    if spans_w(&hint) > iw {
        hint.truncate(2);
    }
    body.push(hint);
    (barred(c, f.severity, focused, body), rest)
}

/// What Enter on an open card opens: `3 evidence items · the fix · the diff`.
fn opens(evidence: usize, fix: bool, sep: &str) -> String {
    let mut parts = match evidence {
        0 => vec!["the reasoning".to_string()],
        1 => vec!["1 evidence item".to_string()],
        n => vec![format!("{n} evidence items")],
    };
    if fix {
        parts.push("the fix".into());
    }
    parts.push("the diff".into());
    parts.join(&format!(" {sep} "))
}

/// How many explanation rows and code lines fit in `room` rows; the code block adds a path row and a cut note.
fn fold_rule(room: usize, prose: usize, code: usize, has_code: bool) -> (usize, usize) {
    let frame = if has_code { 3 } else { 0 };
    let (mut p, mut k) = (prose.max(1), code);
    let fits = |p: usize, k: usize| p + k + frame <= room;
    while !fits(p, k) && p > 3 {
        p -= 1;
    }
    while !fits(p, k) && k > 5 {
        k -= 1;
    }
    while !fits(p, k) && p > 1 {
        p -= 1;
    }
    (p, k)
}

/// `rows` lines of the explanation from `offset`, with the cut edges marked.
fn prose_window(
    c: &Ctx,
    prose: Vec<Vec<Span<'static>>>,
    rows: usize,
    offset: usize,
    w: usize,
) -> Vec<Vec<Span<'static>>> {
    if prose.len() <= rows {
        return prose;
    }
    let g = c.t.g();
    let offset = offset.min(prose.len() - rows);
    let below = prose.len() - rows - offset;
    let mut out: Vec<_> = prose.into_iter().skip(offset).take(rows).collect();
    if offset > 0 && rows > 1 {
        out[0] = vec![c.span(format!("{} {} more above, K scrolls back", g.ellipsis, offset + 1), Role::Muted)];
    }
    if below > 0 {
        let last = out.pop().unwrap_or_default();
        out.push(fold(c, last, w, &format!("{} more, J scrolls", below + 1)));
    }
    out
}

/// Ends a cut line with the fold mark and an optional note, inside `w` columns.
fn fold(c: &Ctx, mut line: Vec<Span<'static>>, w: usize, note: &str) -> Vec<Span<'static>> {
    let mark = if note.is_empty() { format!(" {}", c.t.g().ellipsis) } else { format!(" {} {note}", c.t.g().ellipsis) };
    while !line.is_empty() && spans_w(&line) + width(&mark) > w {
        let last = line.pop().unwrap_or_default();
        let keep = w.saturating_sub(spans_w(&line) + width(&mark));
        if keep > 0 && !last.content.trim().is_empty() {
            let cut = crate::text::fit_len(&last.content, keep);
            let head = &last.content[..cut];
            let word = if cut < last.content.len() { head.rfind(' ').map_or("", |i| &head[..i]) } else { head };
            if !word.trim().is_empty() {
                line.push(Span::styled(word.trim_end().to_string(), last.style));
            }
            break;
        }
    }
    line.push(c.span(mark, Role::Muted));
    line
}

fn loading(c: &Ctx, id: &FindingId) -> String {
    match c.m.detail_errors.get(id) {
        Some(e) => format!("evidence unavailable: {e}"),
        None => format!("loading evidence{}", c.t.g().ellipsis),
    }
}

/// Cited code on the surface background: line numbers, syntax colors, the anchor line lifted and marked.
fn code_block(c: &Ctx, ev: &Evidence, f: &Finding, w: usize, max: usize) -> Vec<Vec<Span<'static>>> {
    let g = c.t.g();
    let surface = c.t.bg(Bg::Surface);
    let start = ev.start.unwrap_or(1);
    let src: Vec<&str> = ev.text.as_str().split('\n').collect();
    let anchor = f.line.map(|l| l.get()).filter(|_| ev.path.as_ref() == Some(&f.file));
    let anchor_i = anchor.and_then(|a| a.checked_sub(start)).map(|i| i as usize).filter(|i| *i < src.len());
    let from = match anchor_i {
        Some(a) if src.len() > max => a.saturating_sub(max / 2).min(src.len() - max),
        _ => 0,
    };
    let to = (from + max).min(src.len());
    let mut out = Vec::new();
    let path = ev.path.as_ref().map_or(String::new(), |p| p.to_string());
    let range = match ev.end {
        Some(e) if e != start => format!("lines {start}-{e}"),
        _ => format!("line {start}"),
    };
    let at = ev.ref_sha.as_ref().map_or(String::new(), |s| format!(" @ {}", s.short()));
    out.push(filled(
        vec![Span::styled(c.fit(&format!("  {path}  {range}{at}"), w), c.s(Role::Muted).patch(surface))],
        w,
        surface,
    ));
    let num_w = (start as usize + src.len()).to_string().len().max(3);
    for (i, text) in src.iter().enumerate().take(to).skip(from) {
        let is_anchor = Some(i) == anchor_i;
        let bg = if is_anchor { Bg::Anchor } else { Bg::Surface };
        let st = c.t.bg(bg);
        let (mark, mrole) = if is_anchor {
            let (_, _, r) = c.t.severity(f.severity);
            (if c.t.ascii { ">" } else { "▶" }, r)
        } else {
            (" ", Role::Muted)
        };
        let num = format!("{:>num_w$}", start as usize + i);
        let mut spans = vec![
            Span::styled(format!(" {mark}"), c.s(mrole).patch(st)),
            Span::styled(num, c.s(if is_anchor { Role::Title } else { Role::Muted }).patch(st)),
            Span::styled(format!(" {} ", if c.t.ascii { "|" } else { "│" }), c.s(Role::Muted).patch(st)),
        ];
        let room = w.saturating_sub(spans_w(&spans) + 1);
        spans.extend(highlight(c, text, room, bg));
        out.push(filled(spans, w, st));
    }
    if to < src.len() || from > 0 {
        let note = format!("  {} {} of {} lines shown", g.ellipsis, to - from, src.len());
        out.push(filled(vec![Span::styled(note, c.s(Role::Muted).patch(surface))], w, surface));
    }
    out
}

// ---------------------------------------------------------------- finding page

/// Enter on a card: the whole finding as an article, every piece of evidence in full.
pub fn finding_page(c: &Ctx, f: &mut Frame, area: Rect, id: &FindingId, scroll: u16) {
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new("").style(c.t.bg(Bg::Base)), area);
    let col = column(area);
    let w = col.width as usize;
    let Some(fd) = c.m.domain.findings.get(id) else {
        f.render_widget(Paragraph::new(c.span("This finding is no longer in the snapshot.", Role::Muted)), col);
        return;
    };
    let iw = w.saturating_sub(2);
    let mut lines = vec![Line::raw("")];
    if let Some(pr) = c.m.domain.prs.get(&fd.pr_id) {
        let list = c.m.domain.pr_findings(&pr.id);
        let n = list.iter().position(|x| x.id == fd.id).map_or(0, |i| i + 1);
        lines.push(Line::from(vec![
            c.span(format!("{} #{} {} ", pr.repository, pr.number, c.t.g().arrow), Role::Muted),
            c.span(format!("finding {n} of {}", list.len()), Role::Accent),
        ]));
        lines.push(Line::raw(""));
    }
    let mut body = card_head(c, fd, iw, true);
    body.push(vec![]);
    let section = |body: &mut Vec<Vec<Span<'static>>>, title: &str| {
        body.push(vec![c.span(title.to_string(), Role::Focus)]);
    };
    match c.m.details.get(id) {
        None => body.push(vec![c.span(loading(c, id), Role::Muted)]),
        Some(d) => {
            section(&mut body, "What is wrong");
            body.extend(rich(c, d.explanation.as_str(), iw, Role::Normal));
            body.push(vec![]);
            section(&mut body, "Evidence");
            for ev in &d.evidence {
                if ev.kind == EvidenceKind::CodeRef {
                    body.extend(code_block(c, ev, fd, iw, 40));
                } else {
                    body.extend(trail(c, ev, iw));
                }
                body.push(vec![]);
            }
            section(&mut body, "Diff at the anchor");
            body.extend(diff_inset(c, fd, iw));
            body.push(vec![]);
            section(&mut body, "How the reviewer checked");
            body.extend(rich(c, d.reasoning_summary.as_str(), iw, Role::Normal));
            body.push(vec![c.span("model-written summary, not a confirmation", Role::Muted)]);
            body.push(vec![]);
            if let Some(fix) = &d.suggested_fix {
                section(&mut body, "Suggested fix");
                let (prose, code) = fix.as_str().split_once("\n\n").unwrap_or((fix.as_str(), ""));
                body.extend(rich(c, prose, iw, Role::Normal));
                if !code.is_empty() {
                    body.push(vec![]);
                    body.extend(fix_block(c, code, iw));
                }
                body.push(vec![]);
            }
            section(&mut body, "History");
            for h in &d.history {
                let (word, role) = finding_status(h.disposition);
                body.push(vec![
                    c.span(format!("{}  run {}  ", time_of(h.at.as_str()), h.run_id.short()), Role::Muted),
                    c.span(word, role),
                ]);
            }
        }
    }
    lines.extend(barred(c, fd.severity, true, body));
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), col);
}

/// A tool, test or scenario line from the evidence: `◎ search_code "x": 4 matches`, wrapped under its text.
fn trail(c: &Ctx, ev: &Evidence, w: usize) -> Vec<Vec<Span<'static>>> {
    let tag = match ev.kind {
        EvidenceKind::ToolSummary => "",
        EvidenceKind::TestResult => "test ",
        EvidenceKind::Scenario => "scenario ",
        EvidenceKind::Link => "link ",
        _ => "note ",
    };
    let icon = if c.t.ascii { "* " } else { "◎ " };
    let mut out = Vec::new();
    for (k, l) in rich(c, &format!("{tag}{}", ev.text), w.saturating_sub(2), Role::Normal).into_iter().enumerate() {
        let mut s = vec![c.span(if k == 0 { icon } else { "  " }, Role::Accent)];
        s.extend(l);
        out.push(s);
    }
    out
}

/// The suggested change as an inset block on the surface, each line marked as an addition.
fn fix_block(c: &Ctx, code: &str, w: usize) -> Vec<Vec<Span<'static>>> {
    let surface = c.t.bg(Bg::Surface);
    let mut out = vec![filled(vec![Span::styled("  suggested change", c.s(Role::Muted).patch(surface))], w, surface)];
    let bar = if c.t.ascii { "|" } else { "│" };
    for l in code.trim_end().split('\n') {
        let mut s = vec![
            Span::styled("  + ", c.s(Role::Add).patch(surface)),
            Span::styled(format!("{bar} "), c.s(Role::Muted).patch(surface)),
        ];
        s.extend(highlight(c, l, w.saturating_sub(7), Bg::Surface));
        out.push(filled(s, w, surface));
    }
    out
}

/// Rows of the change itself around the finding's anchor, from the diff the reader loads when it opens, so the
/// finding reads against what the PR changed without leaving the page.
fn diff_inset(c: &Ctx, fd: &Finding, w: usize) -> Vec<Vec<Span<'static>>> {
    use crate::model::{DiffRow, file_rows};
    use crate::protocol::LineKind;
    let note = |text: String, role| vec![vec![c.span(text, role)]];
    let key = crate::update::finding_diff_key(c.m, &fd.id);
    let Some(d) = c.m.diff.as_ref().filter(|d| Some(&d.key) == key.as_ref()) else {
        return note(format!("loading the diff{}", c.t.g().ellipsis), Role::Pending);
    };
    if let Some(e) = &d.error {
        return note(format!("diff unavailable: {e}"), Role::Fail);
    }
    let Some(file) = d.files.iter().find(|x| x.path == fd.file) else {
        let text = match d.pending {
            Some(_) => format!("loading the diff{}", c.t.g().ellipsis),
            None => "this file is not in the diff".into(),
        };
        return note(text, Role::Muted);
    };
    let rows = file_rows(file);
    let anchor = fd.line.map(|l| l.get());
    let at = anchor
        .and_then(|a| rows.iter().position(|r| matches!(r, DiffRow::Line(l) if l.new.is_some_and(|n| n >= a))))
        .unwrap_or(0);
    let surface = c.t.bg(Bg::Surface);
    let mut out = vec![filled(
        vec![
            Span::styled(
                format!("  {}", c.fit(file.path.as_str(), w.saturating_sub(30))),
                c.s(Role::Muted).patch(surface),
            ),
            Span::styled(format!("  +{}", file.additions), c.s(Role::Add).patch(surface)),
            Span::styled(format!(" -{}", file.deletions), c.s(Role::Del).patch(surface)),
            Span::styled("   d opens the full diff", c.s(Role::Muted).patch(surface)),
        ],
        w,
        surface,
    )];
    let num_w = 4;
    for row in &rows[super::window(rows.len(), Some(at), 11)] {
        match row {
            DiffRow::Header(h) => {
                let text = c.fit(h.header.as_str(), w.saturating_sub(4));
                out.push(filled(vec![Span::styled(format!("  {text}"), c.s(Role::Hunk).patch(surface))], w, surface));
            }
            DiffRow::Line(l) => {
                let hit = anchor.is_some() && l.new == anchor;
                let (sign, role, bg) = match l.kind {
                    LineKind::Add => ("+", Role::Add, Bg::Add),
                    LineKind::Del => ("-", Role::Del, Bg::Del),
                    _ => (" ", Role::Muted, Bg::Surface),
                };
                let bg = if hit { Bg::Anchor } else { bg };
                let st = c.t.bg(bg);
                let (mark, mrole) = if hit {
                    (if c.t.ascii { ">" } else { "▶" }, c.t.severity(fd.severity).2)
                } else {
                    (" ", Role::Muted)
                };
                let num = l.new.or(l.old).map_or(String::new(), |n| n.to_string());
                let mut s = vec![
                    Span::styled(format!(" {mark}"), c.s(mrole).patch(st)),
                    Span::styled(format!("{num:>num_w$} "), c.s(if hit { Role::Title } else { Role::Muted }).patch(st)),
                    Span::styled(format!("{sign} "), c.s(role).patch(st)),
                ];
                let room = w.saturating_sub(spans_w(&s) + 1);
                s.extend(highlight(c, l.text.as_str(), room, bg));
                out.push(filled(s, w, st));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{fold_rule, opens};

    #[test]
    fn the_card_hint_counts_evidence_in_the_singular_and_plural() {
        assert_eq!(opens(1, false, "·"), "1 evidence item · the diff");
        assert_eq!(opens(3, true, "·"), "3 evidence items · the fix · the diff");
        assert_eq!(opens(0, false, "|"), "the reasoning | the diff");
    }

    #[test]
    fn the_fold_rule_cuts_the_explanation_to_three_then_the_code_to_five_then_the_explanation_to_one() {
        assert_eq!(fold_rule(30, 4, 11, true), (4, 11));
        assert_eq!(fold_rule(14, 4, 11, true), (3, 8));
        assert_eq!(fold_rule(9, 6, 11, true), (1, 5));
        assert_eq!(fold_rule(10, 6, 0, false), (6, 0));
    }
}
