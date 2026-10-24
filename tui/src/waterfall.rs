//! The lite waterfall both clients draw: the spans a run's `tool.activity` lines carry, their tree, and one shared
//! time axis. Adapted from arbiolabs/agents-trace-waterfall (trace-core's span model, trace-view's `flatten`,
//! `Viewport`, `bar_span`, `tick_times` and `format_duration`), in milliseconds and without folds, zoom or pan.

use crate::protocol::Text;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpanKind {
    Phase,
    Step,
    Model,
    Tool,
}

impl SpanKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "phase" => SpanKind::Phase,
            "step" => SpanKind::Step,
            "model" => SpanKind::Model,
            "tool" => SpanKind::Tool,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            SpanKind::Phase => "phase",
            SpanKind::Step => "step",
            SpanKind::Model => "model",
            SpanKind::Tool => "tool",
        }
    }
}

/// One timed piece of a run, built from every line that named it: the first opened it, the latest says where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub id: Text,
    pub parent: Option<Text>,
    pub kind: SpanKind,
    /// The source word of its lines: `phase`, `git`, `model`, `read_file`...
    pub tool: Text,
    /// The line that opened it, and the latest one.
    pub opened: Text,
    pub summary: Text,
    /// Journal time, in Unix milliseconds.
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub output_tokens: Option<u64>,
}

impl Span {
    /// What its row is called: the phase, `turn N`, the tool, or a step's opening words.
    pub fn name(&self) -> String {
        let suffix = |p: &str| self.id.as_str().strip_prefix(p).map(|s| s.split(':').next().unwrap_or(s).to_string());
        match self.kind {
            SpanKind::Phase => suffix("phase:").unwrap_or_else(|| self.tool.to_string()),
            SpanKind::Model => suffix("turn:").map_or_else(|| "model".into(), |n| format!("turn {n}")),
            SpanKind::Tool => self.tool.to_string(),
            SpanKind::Step => self.opened.to_string(),
        }
    }

    /// Where its bar ends: its close, or `now` while it is open.
    pub fn end_ms(&self, now: i64) -> i64 {
        self.ended_ms.unwrap_or(now).max(self.started_ms)
    }
}

/// Apply one line's span: a new id opens a span, a known one takes the latest summary, tokens and end.
pub fn upsert(spans: &mut Vec<Span>, s: Span) {
    match spans.iter_mut().find(|x| x.id == s.id) {
        Some(x) => {
            x.summary = s.summary;
            x.ended_ms = s.ended_ms.or(x.ended_ms);
            x.output_tokens = s.output_tokens.or(x.output_tokens);
        }
        None => spans.push(s),
    }
}

/// One visible row: the span's index in the run's list and its depth in the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub ix: usize,
    pub depth: u16,
}

/// The spans in pre-order, children by start under their parent; a span whose parent never arrived is a root.
pub fn rows(spans: &[Span]) -> Vec<Row> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| (spans[i].started_ms, i));
    let known = |p: &Text| spans.iter().any(|s| &s.id == p);
    let mut out = Vec::with_capacity(spans.len());
    let mut stack: Vec<(usize, u16)> =
        order.iter().rev().filter(|&&i| spans[i].parent.as_ref().is_none_or(|p| !known(p))).map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        out.push(Row { ix: i, depth });
        let kids = order.iter().rev().filter(|&&k| k != i && spans[k].parent.as_ref() == Some(&spans[i].id));
        stack.extend(kids.map(|&k| (k, depth.saturating_add(1))));
    }
    out
}

/// The visible time window, fit to the width: no zoom, no pan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub start_ms: i64,
    pub ms_per_cell: f64,
}

impl Viewport {
    /// `start..end` across `width` cells (terminal columns or pixels).
    pub fn fit(start: i64, end: i64, width: f64) -> Self {
        Self { start_ms: start, ms_per_cell: (end - start).max(1) as f64 / width.max(1.0) }
    }

    pub fn col_of(&self, t: i64) -> f64 {
        (t - self.start_ms) as f64 / self.ms_per_cell
    }
}

/// The run's whole extent, from its first span to now or its last close, as a viewport `width` cells wide. A fifth
/// of the width stays free past the end, so the now line shows and the newest bar has room for its words.
pub fn fit(spans: &[Span], now: i64, width: f64) -> Option<Viewport> {
    let start = spans.iter().map(|s| s.started_ms).min()?;
    let end = spans.iter().map(|s| s.end_ms(now)).max()?.max(start + 1000);
    Some(Viewport::fit(start, end + (end - start) / 4, width))
}

/// Half-open cell range `[start_x, end_x)` of a bar, clipped to `[0, width)`; any span gets at least one cell.
pub fn bar_span(start: i64, end: i64, vp: &Viewport, width: u16) -> Option<(u16, u16)> {
    if width == 0 {
        return None;
    }
    let w = f64::from(width);
    let sx = vp.col_of(start).floor().clamp(0.0, w - 1.0) as u16;
    let ex = vp.col_of(end.max(start)).ceil().clamp(f64::from(sx) + 1.0, w) as u16;
    Some((sx, ex))
}

const TICK_STEPS_MS: [i64; 16] = [
    100, 200, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000,
    3_600_000,
];

/// Tick times and their labels relative to the axis start, at least `min_gap` cells apart.
pub fn ticks(vp: &Viewport, width: f64, min_gap: f64) -> Vec<(i64, String)> {
    let Some(step) = TICK_STEPS_MS.into_iter().find(|&s| s as f64 / vp.ms_per_cell >= min_gap) else {
        return Vec::new();
    };
    (0..)
        .map(|k| vp.start_ms + k * step)
        .take_while(|&t| vp.col_of(t) < width)
        .map(|t| (t, duration(t - vp.start_ms)))
        .collect()
}

fn one_decimal(v: f64) -> String {
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

/// `0s`, `250ms`, `1.5s`, `2m`, `2m 30s`, `1h 5m`.
pub fn duration(ms: i64) -> String {
    let ms = ms.max(0);
    match ms {
        0 => "0s".into(),
        1..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{}s", one_decimal(ms as f64 / 1e3)),
        _ => {
            let s = ms / 1000;
            let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
            match (h, s) {
                (0, 0) => format!("{m}m"),
                (0, _) => format!("{m}m {s}s"),
                _ if m == 0 => format!("{h}h"),
                _ => format!("{h}h {m}m"),
            }
        }
    }
}

/// `999`, `12.3k`, `1.5M`.
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", one_decimal(n as f64 / 1e3)),
        _ => format!("{}M", one_decimal(n as f64 / 1e6)),
    }
}

/// Unix milliseconds of an RFC 3339 UTC timestamp (`2026-10-20T13:00:01.420Z`); `None` for anything else.
pub fn epoch_ms(at: &str) -> Option<i64> {
    let b = at.as_bytes();
    let num = |r: std::ops::Range<usize>| at.get(r)?.parse::<i64>().ok();
    if b.len() < 20 || b[4] != b'-' || b[10] != b'T' || !at.ends_with('Z') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let frac = match at.get(19..at.len() - 1)? {
        "" => 0,
        f => {
            let digits = f.strip_prefix('.')?;
            format!("{digits:0<3}").get(..3)?.parse::<i64>().ok()?
        }
    };
    // Howard Hinnant's days_from_civil.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + s * 1000 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, parent: Option<&str>, kind: SpanKind, start: i64, end: Option<i64>) -> Span {
        Span {
            id: Text::new(id),
            parent: parent.map(Text::new),
            kind,
            tool: Text::new("t"),
            opened: Text::new("o"),
            summary: Text::new("s"),
            started_ms: start,
            ended_ms: end,
            output_tokens: None,
        }
    }

    #[test]
    fn timestamps_read_as_unix_milliseconds() {
        assert_eq!(epoch_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_ms("2026-10-20T13:00:01.42Z"), Some(1_792_501_201_420));
        assert_eq!(epoch_ms("2026-10-20T13:00:01.420Z"), Some(1_792_501_201_420));
        assert_eq!(epoch_ms("yesterday"), None);
    }

    #[test]
    fn rows_nest_children_under_their_parent_by_start_and_orphans_become_roots() {
        let mut v = vec![
            span("phase:analyze", None, SpanKind::Phase, 10, None),
            span("phase:collect", None, SpanKind::Phase, 0, Some(10)),
            span("turn:1:a", Some("phase:analyze"), SpanKind::Model, 11, Some(20)),
            span("tool:1", Some("turn:1:a"), SpanKind::Tool, 19, Some(20)),
            span("tool:2", Some("turn:gone"), SpanKind::Tool, 30, Some(31)),
        ];
        let ids = |v: &[Span]| {
            rows(v).iter().map(|r| format!("{}{}", " ".repeat(r.depth.into()), v[r.ix].id.as_str())).collect::<Vec<_>>()
        };
        assert_eq!(ids(&v), ["phase:collect", "phase:analyze", " turn:1:a", "  tool:1", "tool:2"]);
        upsert(
            &mut v,
            Span { summary: Text::new("done"), ..span("phase:analyze", None, SpanKind::Phase, 99, Some(40)) },
        );
        assert_eq!((v[0].started_ms, v[0].ended_ms, v[0].summary.as_str()), (10, Some(40), "done"));
        assert_eq!([v[2].name(), v[0].name()], ["turn 1", "analyze"]);
    }

    #[test]
    fn bars_and_ticks_fit_the_width() {
        let vp = Viewport::fit(0, 10_000, 100.0);
        assert_eq!(bar_span(0, 10_000, &vp, 100), Some((0, 100)));
        assert_eq!(bar_span(5_000, 5_000, &vp, 100), Some((50, 51)));
        assert_eq!(
            ticks(&vp, 100.0, 10.0).iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
            ["0s", "1s", "2s", "3s", "4s", "5s", "6s", "7s", "8s", "9s"]
        );
        assert_eq!(
            [duration(250), duration(1_500), duration(134_000), tokens(2114)],
            ["250ms", "1.5s", "2m 14s", "2.1k"]
        );
    }
}
