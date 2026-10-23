//! A run's lite waterfall as text lines. Adapted from arbiolabs/agents-trace-waterfall's trace-render
//! `WaterfallWidget`: a tree column, a `│`, and one time axis fit to the width, with open spans growing to the `┊`
//! now line. No selection, zoom or pan here: the row of the newest span carries its latest words.

use ratatui::text::Line;

use super::Ctx;
use crate::text::width;
use crate::theme::Role;
use crate::waterfall::{Span, SpanKind, bar_span, duration, fit, rows, ticks, tokens};

fn kind_glyph(c: &Ctx, k: SpanKind) -> &'static str {
    match (k, c.t.ascii) {
        (SpanKind::Phase, false) => "◆",
        (SpanKind::Step, false) => "○",
        (SpanKind::Model, false) => "●",
        (SpanKind::Tool, false) => "■",
        (SpanKind::Phase, true) => "P",
        (SpanKind::Step, true) => "S",
        (SpanKind::Model, true) => "M",
        (SpanKind::Tool, true) => "T",
    }
}

fn kind_role(k: SpanKind) -> Role {
    match k {
        SpanKind::Phase => Role::Accent,
        SpanKind::Step => Role::Muted,
        SpanKind::Model => Role::Keyword,
        SpanKind::Tool => Role::Type,
    }
}

/// One header line of tick labels, then the newest `max_rows` spans: tree column, `│`, the shared timeline.
pub fn lines(c: &Ctx, spans: &[Span], now: i64, w: usize, max_rows: usize) -> Vec<Line<'static>> {
    let tw = (w * 2 / 5).clamp(18, 44).min(w.saturating_sub(12));
    let tl_w = w.saturating_sub(tw + 1);
    let Some(vp) = fit(spans, now, tl_w as f64) else { return Vec::new() };
    let (sep, now_mark, done_fill, open_fill) =
        if c.t.ascii { ("|", ":", "#", "=") } else { ("│", "┊", "█", "▓") };
    let now_x = vp.col_of(now).floor() as usize;

    let mut axis = vec![' '; tl_w];
    let mut free = 0;
    for (t, label) in ticks(&vp, tl_w as f64, 10.0) {
        let x = vp.col_of(t).floor() as usize;
        if x >= free && x + label.len() <= tl_w {
            axis.splice(x..x + label.len(), label.chars());
            free = x + label.len() + 1;
        }
    }
    let mut out = vec![Line::from(vec![
        c.span(c.pad("  span", tw), Role::Muted),
        c.span(sep, Role::Border),
        c.span(axis.into_iter().collect::<String>(), Role::Muted),
    ])];

    let all = rows(spans);
    let newest = spans.iter().enumerate().max_by_key(|(i, s)| (s.started_ms, *i)).map(|(i, _)| i);
    for row in &all[all.len().saturating_sub(max_rows)..] {
        let s = &spans[row.ix];
        let open = s.ended_ms.is_none();
        let status = match (open, c.t.ascii) {
            (true, false) => "▶",
            (true, true) => ">",
            (false, _) => c.t.g().ok,
        };
        let mut meta = duration(s.end_ms(now) - s.started_ms);
        if let Some(n) = s.output_tokens {
            meta = format!("{} tok {meta}", tokens(n));
        }
        let indent = " ".repeat(usize::from(row.depth) * 2 + 1);
        let head = format!("{indent}{}{status} ", kind_glyph(c, s.kind));
        let name_w = tw.saturating_sub(width(&head) + width(&meta) + 1);
        let mut tree = vec![
            c.span(head, kind_role(s.kind)),
            c.span(c.pad(&s.name(), name_w), if open { Role::Title } else { Role::Normal }),
            c.span(format!(" {meta}"), Role::Muted),
        ];
        // The newest span says where the reviewer is now, in its own words, where its bar leaves room.
        let mut cells: Vec<(char, Role)> = vec![(' ', Role::Normal); tl_w];
        if now_x < tl_w {
            cells[now_x] = (now_mark.chars().next().unwrap_or(':'), Role::Pending);
        }
        if let Some((sx, ex)) = bar_span(s.started_ms, s.end_ms(now), &vp, tl_w as u16) {
            let fill = if open { open_fill } else { done_fill }.chars().next().unwrap_or('#');
            for cell in &mut cells[usize::from(sx)..usize::from(ex)] {
                *cell = (fill, kind_role(s.kind));
            }
            if Some(row.ix) == newest && s.kind != SpanKind::Phase {
                let words: Vec<char> =
                    c.fit(s.summary.as_str(), tl_w.saturating_sub(usize::from(ex) + 1)).chars().collect();
                for (k, ch) in words.into_iter().enumerate() {
                    cells[usize::from(ex) + 1 + k] = (ch, Role::Muted);
                }
            }
        }
        tree.push(c.span(sep, Role::Border));
        let mut run = String::new();
        let mut role = cells.first().map_or(Role::Normal, |x| x.1);
        for (ch, r) in cells {
            if r != role && !run.is_empty() {
                tree.push(c.span(std::mem::take(&mut run), role));
            }
            role = r;
            run.push(ch);
        }
        tree.push(c.span(run, role));
        out.push(Line::from(tree));
    }
    out
}
