//! The publication page (TUI_SPEC §5.5): what approving a parked review would post, exactly, and what happened once
//! it was approved. `y` sends only what this page shows; it says why when the preview has gone stale.

use ratatui::text::{Line, Span};

use super::Ctx;
use super::code::rich;
use super::doc::inset;
use crate::model::{ConfirmState, DiffKey, Preview};
use crate::protocol::{PublicationPreview, PublicationStatus, RunId};
use crate::theme::{Role, publication_status};
use crate::update::{publish_block, published_url};

const MARKER: &str = "<!-- durable-review:";

pub fn lines(c: &Ctx, w: usize, run_id: &RunId, preview: &Preview, publish: &ConfirmState) -> Vec<Line<'static>> {
    let g = c.t.g();
    let run = c.m.domain.runs.get(run_id);
    let pr = run.and_then(|r| c.m.domain.prs.get(&r.pr_id));
    let mut out = vec![Line::raw("")];
    let place = pr.map_or(format!("run {}", run_id.short()), |p| format!("{} #{}", p.repository, p.number));
    out.push(Line::from(vec![
        c.span(format!("{place} {} ", g.arrow), Role::Muted),
        c.span("publication preview", Role::Accent),
    ]));
    out.push(Line::raw(""));
    let p = match preview {
        Preview::Loading(_) => {
            out.push(Line::from(c.span(format!("loading the stored review{}", g.ellipsis), Role::Muted)));
            return out;
        }
        Preview::Failed(e) => {
            out.push(Line::from(c.span(c.fit(&format!("preview unavailable: {e}"), w), Role::Fail)));
            out.push(Line::from(c.span("r refreshes and asks again  Esc closes", Role::Muted)));
            return out;
        }
        Preview::Ready(p) => p,
    };
    out.push(Line::from(c.span("Publish this review?", Role::Title)));
    let n = p.comments.len();
    let target = format!(
        "{place} at {} {} as event {} {} {n} inline comment{}",
        p.head_sha.short(),
        g.sep,
        p.event,
        g.sep,
        if n == 1 { "" } else { "s" }
    );
    out.push(Line::from(c.span(c.fit(&target, w), Role::Muted)));
    out.push(Line::raw(""));
    out.extend(state(c, w, run_id, p, publish));
    out.push(Line::raw(""));

    out.push(Line::from(c.span("Summary", Role::Focus)));
    out.extend(body(c, p.body.as_str(), w));
    out.push(Line::raw(""));

    let key = run.map(|r| DiffKey { pr: r.pr_id.clone(), base: r.base_sha.clone(), head: r.head_sha.clone() });
    out.push(Line::from(c.span(format!("Inline comments ({n})"), Role::Focus)));
    if n == 0 {
        out.push(Line::from(c.span("none: the summary is the whole review", Role::Muted)));
    }
    for cm in &p.comments {
        out.push(Line::raw(""));
        out.push(Line::from(vec![
            c.span(c.fit(&format!("{}:{}", cm.path, cm.line), w.saturating_sub(10)), Role::Accent),
            c.span(format!("  {}", cm.side), Role::Muted),
        ]));
        out.extend(rich(c, cm.body.as_str(), w.saturating_sub(2), Role::Normal).into_iter().map(indent));
        out.push(Line::raw(""));
        out.extend(inset(c, key.as_ref(), &cm.path, Some(cm.line.get()), Role::Accent, w).into_iter().map(Line::from));
    }
    out.push(Line::raw(""));
    out.push(Line::from(c.span("Positioning", Role::Focus)));
    out.extend(positioning(c, key.as_ref(), p, w));
    out
}

fn indent(spans: Vec<Span<'static>>) -> Line<'static> {
    let mut s = vec![Span::raw("  ")];
    s.extend(spans);
    Line::from(s)
}

/// The summary exactly as it will be posted, line by line: headings in the title style, code spans as chips, and the
/// hidden marker, which GitHub does not render, dimmed and named.
fn body(c: &Ctx, text: &str, w: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in text.trim_end_matches('\n').split('\n') {
        if line.trim_start().starts_with(MARKER) {
            out.push(Line::from(vec![
                c.span("  hidden marker  ", Role::Muted),
                c.span(c.fit(line.trim(), w.saturating_sub(19)), Role::Muted),
            ]));
            continue;
        }
        let role = if line.starts_with('#') { Role::Title } else { Role::Normal };
        if line.is_empty() {
            out.push(Line::raw(""));
        }
        out.extend(rich(c, line, w.saturating_sub(2), role).into_iter().filter(|l| !l.is_empty()).map(indent));
    }
    out
}

/// What `y` does now, or did: the key while it would publish, why it would not, then the request and the
/// publisher's own statuses. Only `published` reads as done.
fn state(c: &Ctx, w: usize, run_id: &RunId, p: &PublicationPreview, publish: &ConfirmState) -> Vec<Line<'static>> {
    let g = c.t.g();
    let line = |text: String, role| Line::from(c.span(c.fit(&text, w), role));
    let status = c.m.domain.publications.get(run_id).filter(|x| x.status != PublicationStatus::AwaitingApproval);
    if matches!(publish, ConfirmState::Asking) && status.is_none() {
        return vec![match publish_block(c.m, p) {
            None => Line::from(vec![
                c.span("y", Role::Accent),
                c.span(" publishes exactly this to GitHub   ", Role::Title),
                c.span("Esc", Role::Accent),
                c.span(" closes without publishing", Role::Muted),
            ]),
            Some(why) => line(format!("{} y is off: {why}", g.pending), Role::Pending),
        }];
    }
    let mut out = Vec::new();
    match (status.map(|s| s.status), publish) {
        (Some(PublicationStatus::Published), _) => {
            let id = status.and_then(|s| s.review_id).map_or(String::new(), |id| format!(" as review {id}"));
            out.push(line(format!("{} published on GitHub{id}", g.ok), Role::Ok));
            if let Some(url) = status.and_then(|s| s.url.as_ref()) {
                out.push(line(format!("  {url}"), Role::Accent));
            }
            if published_url(c.m, run_id).is_some() {
                out.push(line("  o opens it in the browser".into(), Role::Muted));
            }
        }
        (Some(PublicationStatus::Publishing), _) => {
            out.push(line(format!("{} publishing{}", super::doc::spinner(c), g.ellipsis), Role::Active))
        }
        (Some(s @ (PublicationStatus::Uncertain | PublicationStatus::Failed | PublicationStatus::Unknown)), _) => {
            let (word, role) = publication_status(s);
            out.push(line(format!("{} {word}", if role == Role::Fail { g.fail } else { g.pending }), role));
            if let Some(reason) = c.m.domain.runs.get(run_id).and_then(|r| r.failure_reason.as_ref()) {
                out.push(line(format!("  {reason}"), Role::Normal));
            }
        }
        (_, ConfirmState::Requested(_)) => {
            out.push(line(format!("requested{} waiting for the daemon", g.ellipsis), Role::Pending))
        }
        (_, ConfirmState::Accepted(_)) => out.push(line(
            format!("approval recorded; publishing{} (not on GitHub until it says published)", g.ellipsis),
            Role::Pending,
        )),
        (_, ConfirmState::Failed(e)) => out.push(line(format!("{} rejected: {e}", g.fail), Role::Fail)),
        (_, ConfirmState::TimedOut) => {
            out.push(line("request timed out; y sends the same approval again".into(), Role::Fail))
        }
        (_, ConfirmState::Asking) => {}
    }
    out
}

/// GitHub places an inline comment only on a line of the diff: say which comments land and which cannot.
fn positioning(c: &Ctx, key: Option<&DiffKey>, p: &PublicationPreview, w: usize) -> Vec<Line<'static>> {
    let line = |text: String, role| Line::from(c.span(c.fit(&text, w), role));
    let Some(d) = c.m.diff.as_ref().filter(|d| Some(&d.key) == key) else {
        return vec![line(format!("checking against the diff{}", c.t.g().ellipsis), Role::Muted)];
    };
    if d.error.is_some() || d.pending.is_some() {
        return vec![line("the diff is not loaded, so placement is unchecked".into(), Role::Pending)];
    }
    let mut out = vec![line("comments go on the head side (RIGHT) of the diff, at the line shown".into(), Role::Muted)];
    let outside: Vec<String> = p
        .comments
        .iter()
        .filter(|cm| {
            let file = d.files.iter().find(|f| f.path == cm.path);
            !file.is_some_and(|f| f.hunks.iter().flat_map(|h| &h.lines).any(|l| l.new == Some(cm.line.get())))
        })
        .map(|cm| format!("{}:{}", cm.path, cm.line))
        .collect();
    if outside.is_empty() {
        out.push(line("every comment sits on a line of the diff".into(), Role::Ok));
    }
    for loc in outside {
        out.push(line(format!("{loc} is not on a line of the diff; GitHub cannot anchor it there"), Role::Pending));
    }
    out
}
