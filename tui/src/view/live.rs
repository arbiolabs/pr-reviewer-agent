//! What a running review is doing now: tool-call rates, the sparkline, the phase bar and the compact dot stepper.

use ratatui::text::Span;

use super::Ctx;
use super::doc::spinner;
use crate::model::ActivityKind;
use crate::protocol::{Phase, PrId, Run, RunStatus};
use crate::theme::Role;

/// Tool calls per second over the last `n` seconds, oldest first; `pr` narrows to one PR.
pub fn rate(c: &Ctx, pr: Option<&PrId>, n: usize) -> Vec<u64> {
    let mut v = vec![0u64; n];
    for a in c.m.domain.activity.iter().rev() {
        let age = (c.m.now_ms.saturating_sub(a.recv_ms) / 1000) as usize;
        if age >= n {
            break;
        }
        if matches!(a.kind, ActivityKind::Tool { .. }) && pr.is_none_or(|p| a.pr_id.as_ref() == Some(p)) {
            v[n - 1 - age] += 1;
        }
    }
    v
}

/// Calls per second, averaged over the last five seconds.
pub fn per_sec(c: &Ctx, pr: Option<&PrId>) -> f64 {
    rate(c, pr, 5).iter().sum::<u64>() as f64 / 5.0
}

/// One block glyph per bucket, scaled to the busiest bucket (at least 3 calls tall).
pub fn sparkline(c: &Ctx, data: &[u64]) -> String {
    let levels: &[&str] =
        if c.t.ascii { &[" ", ".", ":", "|", "#"] } else { &["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"] };
    let max = data.iter().copied().max().unwrap_or(0).max(3);
    data.iter().map(|n| levels[(*n as usize * (levels.len() - 1)).div_ceil(max as usize)]).collect()
}

/// A rate in at most four columns: `0.2`, `42`, then `99+` for a burst.
pub fn rate_label(per_sec: f64) -> String {
    match per_sec {
        r if r < 9.95 => format!("{r:.1}"),
        r if r < 99.5 => format!("{r:.0}"),
        _ => "99+".into(),
    }
}

/// One cell for a rate: how busy, at a glance.
pub fn rate_cell(c: &Ctx, per_sec: f64) -> &'static str {
    let levels: &[&str] = if c.t.ascii { &[".", ":", "|", "#"] } else { &["▁", "▃", "▅", "▇"] };
    levels[((per_sec * 2.0) as usize).min(levels.len() - 1)]
}

/// The six review phases in order; `None` for a run that did not end normally.
pub fn phase_index(p: Phase) -> Option<usize> {
    Some(match p {
        Phase::CollectContext => 0,
        Phase::Triage => 1,
        Phase::Analyze => 2,
        Phase::Verify => 3,
        Phase::PreparePublication => 4,
        Phase::Publish | Phase::Reconcile => 5,
        Phase::Completed => 6,
        Phase::Failed | Phase::Superseded | Phase::Unknown => return None,
    })
}

/// `●─●─◐─○─○─○`: eleven cells, the current phase pulsing while the run is live.
pub fn stepper(c: &Ctx, run: &Run) -> Vec<Span<'static>> {
    let cur = phase_index(run.phase);
    let (done, todo, link) = if c.t.ascii { ("+", ".", "-") } else { ("●", "○", "─") };
    let mut out = Vec::new();
    for i in 0..6 {
        if i > 0 {
            let role = if cur.is_some_and(|k| i <= k) { Role::Ok } else { Role::Muted };
            out.push(c.span(link, role));
        }
        let (glyph, role) = match cur {
            Some(k) if i < k => (done, Role::Ok),
            Some(k) if i == k && run.status == RunStatus::Running => (spinner(c), Role::Accent),
            _ => (todo, Role::Muted),
        };
        out.push(c.span(glyph, role));
    }
    out
}

/// Six `seg`-wide segments: done ones solid, the current one with a highlight that walks while the run is live.
pub fn phase_bar(c: &Ctx, run: &Run, seg: usize) -> Vec<Span<'static>> {
    let (full, rail) = if c.t.ascii { ("=", "-") } else { ("━", "─") };
    let cur = phase_index(run.phase);
    let mut out = Vec::new();
    for i in 0..6 {
        if i > 0 {
            out.push(Span::raw(" "));
        }
        match cur {
            Some(k) if i < k => out.push(c.span(full.repeat(seg), Role::Ok)),
            Some(k) if i == k && run.status == RunStatus::Running => {
                let hot = (c.m.now_ms / 250) as usize % seg.max(1);
                for j in 0..seg {
                    out.push(c.span(full, if j == hot { Role::Title } else { Role::Accent }));
                }
            }
            None if run.status == RunStatus::Failed => out.push(c.span(rail.repeat(seg), Role::Fail)),
            _ => out.push(c.span(rail.repeat(seg), Role::Muted)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Look, Model, Screen};
    use crate::theme::Theme;

    #[test]
    fn the_sparkline_scales_to_the_busiest_second_with_a_floor_of_three() {
        let m = Model::new("x", Look::default(), Screen::Review, None);
        let c = Ctx {
            m: &m,
            t: Theme { color: false, ascii: false, terminal_bg: false, palette: Default::default() },
            width: super::super::Width::Wide,
        };
        assert_eq!(sparkline(&c, &[0, 1, 3, 6]), "▁▃▅█");
        assert_eq!(sparkline(&c, &[0, 1, 2, 3]), "▁▄▆█");
        assert_eq!(rate_cell(&c, 0.0), "▁");
        assert_eq!(rate_cell(&c, 1.2), "▅");
        assert_eq!([rate_label(0.2), rate_label(42.4), rate_label(400.0)], ["0.2", "42", "99+"]);
    }
}
