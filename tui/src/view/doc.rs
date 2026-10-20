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
const RAIL_W: u16 = 32;

pub fn spinner(c: &Ctx) -> &'static str {
    const U: [&str; 4] = ["•", "●", "●", "•"];
    const A: [&str; 4] = ["-", "\\", "|", "/"];
    let n = (c.m.now_ms / 125) as usize;
    if c.t.ascii { A[n % 4] } else { U[n % 4] }
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

fn spans_w(spans: &[Span]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// Pads `spans` to exactly `w` columns with `style`.
fn filled(mut spans: Vec<Span<'static>>, w: usize, style: Style) -> Vec<Span<'static>> {
    let used = spans_w(&spans);
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), style));
    }
    spans
}

/// Left text and right text on one `w`-wide row.
fn split_row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
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

fn worst(c: &Ctx, pr: &Pr) -> Option<Severity> {
    current(c, pr).iter().map(|f| f.severity).min()
}

// ---------------------------------------------------------------- layout

/// Review and Inbox share one screen: Inbox is the same document with the rail in focus.
pub fn screen(c: &Ctx, f: &mut Frame, area: Rect) {
    let rail_focus = c.m.ui.screen == Screen::Inbox || c.focused(Pane::Sidebar);
    let wide = area.width >= 100;
    if !wide && rail_focus {
        rail(c, f, area, true);
        return;
    }
    let doc_area = if wide {
        let r = Rect { width: RAIL_W, ..area };
        rail(c, f, r, rail_focus);
        Rect { x: area.x + RAIL_W, width: area.width - RAIL_W, ..area }
    } else {
        area
    };
    let col = column(doc_area);
    let Some(pr) = c.m.selected_pr() else {
        let p = Paragraph::new(vec![Line::raw(""), Line::from(c.span("No tracked PRs yet.", Role::Muted))]);
        f.render_widget(p, col);
        return;
    };
    let (lines, starts) = document(c, pr, col.width as usize, area.height < 30);
    let list = c.m.domain.pr_findings(&pr.id);
    let sel = c.m.ui.selected_finding.as_ref().and_then(|id| list.iter().position(|x| &x.id == id));
    let scroll = match sel {
        Some(i) if i > 0 && !rail_focus => starts.get(i).map_or(0, |s| s.saturating_sub(1)),
        _ => 0,
    };
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), col);
}

/// The reading column: at most `COLUMN` wide, centered, with a margin.
pub fn column(area: Rect) -> Rect {
    let w = area.width.saturating_sub(4).min(COLUMN);
    Rect { x: area.x + (area.width - w) / 2, width: w, ..area }
}

// ---------------------------------------------------------------- rail

fn rail(c: &Ctx, f: &mut Frame, area: Rect, focused: bool) {
    let g = c.t.g();
    let bg = c.t.bg(Bg::Surface);
    f.render_widget(Paragraph::new("").style(bg), area);
    let w = area.width as usize;
    let inner = w.saturating_sub(3);
    let head_role = if focused { Role::Accent } else { Role::Muted };
    let mut lines = vec![
        Line::raw(""),
        Line::from(filled(vec![Span::raw("  "), c.span("PULL REQUESTS", head_role).patch_style(bg)], w, bg)),
        Line::raw(""),
    ];
    for pr in c.m.inbox() {
        let selected = c.m.ui.selected_pr.as_ref() == Some(&pr.id);
        let row_bg = if selected { c.t.bg(Bg::Select) } else { bg };
        let on = |role: Role| c.s(role).patch(row_bg);
        let marker = if selected { if c.t.ascii { ">" } else { "▌" } } else { " " };
        let (glyph, grole) = match worst(c, pr) {
            Some(s) => {
                let (gl, _, r) = c.t.severity(s);
                (gl, r)
            }
            None => (g.idle, Role::Muted),
        };
        let num = format!("#{} ", pr.number);
        let title_w = inner.saturating_sub(width(&num) + 2);
        let first = vec![
            Span::styled(marker.to_string(), on(Role::Accent)),
            Span::styled(" ", on(Role::Normal)),
            Span::styled(format!("{glyph} "), on(grole)),
            Span::styled(num, on(Role::Accent)),
            Span::styled(c.fit(pr.title.as_str(), title_w), on(if selected { Role::Title } else { Role::Normal })),
        ];
        lines.push(Line::from(filled(first, w, row_bg)));
        let repo = pr.repository.as_str().rsplit('/').next().unwrap_or("").to_string();
        let (state, srole) = rail_state(c, pr);
        let second = vec![
            Span::styled(marker.to_string(), on(Role::Accent)),
            Span::styled(format!("   {repo} {} ", g.sep), on(Role::Muted)),
            Span::styled(c.fit(&state, inner.saturating_sub(width(&repo) + 5)), on(srole)),
        ];
        lines.push(Line::from(filled(second, w, row_bg)));
        lines.push(Line::from(filled(vec![], w, bg)));
    }
    f.render_widget(Paragraph::new(lines).style(bg), area);
}

fn rail_state(c: &Ctx, pr: &Pr) -> (String, Role) {
    let g = c.t.g();
    let n = current(c, pr).len();
    let found = match n {
        0 => "clean".to_string(),
        1 => "1 finding".to_string(),
        n => format!("{n} findings"),
    };
    match c.m.domain.latest_run(&pr.id) {
        None => ("not reviewed".into(), Role::Muted),
        Some(r) if r.status == RunStatus::Running => {
            (format!("{} {}", spinner(c), phase_name(r.phase).to_lowercase()), Role::Accent)
        }
        Some(r) if r.status == RunStatus::Failed => (format!("{} review failed", g.fail), Role::Fail),
        Some(_) if n == 0 => (format!("{} clean", g.ok), Role::Ok),
        Some(_) => (found, Role::Muted),
    }
}

// ---------------------------------------------------------------- document

/// The PR as a document, and the line where each finding card starts.
fn document(c: &Ctx, pr: &Pr, w: usize, compact: bool) -> (Vec<Line<'static>>, Vec<usize>) {
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
    if running {
        live(c, pr, w, &mut lines);
    }
    if findings.is_empty() && run.is_some_and(|r| r.status == RunStatus::Completed) {
        lines.push(Line::from(c.span(format!("{} No actionable issues found in the reviewed scope.", g.ok), Role::Ok)));
    }
    let sel = c.m.ui.selected_finding.as_ref();
    let mut starts = Vec::new();
    for f in &findings {
        starts.push(lines.len());
        let selected = sel == Some(&f.id) && c.m.ui.screen == Screen::Review && c.m.ui.focus != Pane::Sidebar;
        lines.extend(card(c, f, c.m.details.get(&f.id), w, selected, compact));
        lines.push(Line::raw(""));
    }
    (lines, starts)
}

fn rule(c: &Ctx, w: usize) -> String {
    (if c.t.ascii { "-" } else { "─" }).repeat(w)
}

fn summary_line(c: &Ctx, pr: &Pr, findings: &[&Finding], w: usize) -> Line<'static> {
    let g = c.t.g();
    let live: Vec<_> = findings.iter().filter(|f| !f.stale).collect();
    let words = |compact: bool| {
        let mut left = vec![c.span(
            match live.len() {
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
            let took = run.completed_at.as_ref().and_then(|e| Some(secs(e.as_str())? - start?));
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

/// The running review: the newest tool calls for this PR, newest last, with the spinner on the newest.
fn live(c: &Ctx, pr: &Pr, w: usize, lines: &mut Vec<Line<'static>>) {
    let g = c.t.g();
    lines
        .push(Line::from(vec![c.span(format!("{} ", spinner(c)), Role::Accent), c.span("Reviewing now", Role::Title)]));
    lines.push(Line::raw(""));
    let items: Vec<_> = c.m.domain.activity.iter().filter(|a| a.pr_id.as_ref() == Some(&pr.id)).collect();
    let take = 12;
    let shown = &items[items.len().saturating_sub(take)..];
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

fn card(
    c: &Ctx,
    f: &Finding,
    d: Option<&FindingDetail>,
    w: usize,
    selected: bool,
    compact: bool,
) -> Vec<Line<'static>> {
    let iw = w.saturating_sub(2);
    let mut body = card_head(c, f, iw, selected);
    body.push(vec![]);
    match d {
        None => body.push(vec![c.span(loading(c, &f.id), Role::Muted)]),
        Some(d) => {
            let mut prose = rich(c, d.explanation.as_str(), iw, Role::Normal);
            if compact && prose.len() > 2 {
                prose.truncate(2);
                if let Some(last) = prose.last_mut() {
                    last.push(c.span(format!(" {}", c.t.g().ellipsis), Role::Muted));
                }
            }
            body.extend(prose);
            if let Some(ev) = d.evidence.iter().find(|e| e.kind == EvidenceKind::CodeRef) {
                body.push(vec![]);
                body.extend(code_block(c, ev, f, iw, 11));
            }
            if selected {
                let fix = if d.suggested_fix.is_some() { " and the fix" } else { "" };
                body.push(vec![]);
                body.push(vec![
                    c.span("Enter", Role::Accent),
                    c.span(format!(" all {} evidence{fix}    ", d.evidence.len()), Role::Muted),
                    c.span("d", Role::Accent),
                    c.span(" diff at this line", Role::Muted),
                ]);
            }
        }
    }
    barred(c, f.severity, selected, body)
}

fn loading(c: &Ctx, id: &FindingId) -> String {
    match c.m.detail_errors.get(id) {
        Some(e) => format!("evidence unavailable: {e}"),
        None => "loading evidence…".into(),
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
                    let tag = match ev.kind {
                        EvidenceKind::ToolSummary => "tool",
                        EvidenceKind::TestResult => "test",
                        EvidenceKind::Scenario => "scenario",
                        EvidenceKind::Link => "link",
                        _ => "note",
                    };
                    for (k, l) in rich(c, ev.text.as_str(), iw.saturating_sub(14), Role::Normal).into_iter().enumerate()
                    {
                        let mut s =
                            vec![c.span(if k == 0 { format!("{tag:<14}") } else { " ".repeat(14) }, Role::Muted)];
                        s.extend(l);
                        body.push(s);
                    }
                }
                body.push(vec![]);
            }
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
                    let surface = c.t.bg(Bg::Surface);
                    for l in code.split('\n') {
                        let mut s = vec![Span::styled("  ", surface)];
                        s.extend(highlight(c, l, iw.saturating_sub(3), Bg::Surface));
                        body.push(filled(s, iw, surface));
                    }
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
