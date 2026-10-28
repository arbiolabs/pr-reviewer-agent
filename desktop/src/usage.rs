//! Usage: the tokens and estimated cost of reviews, by day, week or month. One series, so one hue: cost per bucket
//! as columns in the accent, on a recessive grid with its axis in USD, and every value in the table under it.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::{UsageBucket, UsageBy, UsageCounts, UsageReport};

use crate::kit::{callout, focusable, heading, overline, tabular, tip};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::{Shell, click};

pub const NOTE: &str = "Estimated at the model's list price. A subscription login is billed differently.";
pub const EMPTY: &str = "Usage is recorded from this version on.";

const PLOT_H: f32 = 180.;
const BAR_MAX: f32 = 24.;
const TICKS: usize = 4;

/// The page's name for a grouping, and the key that picks it.
pub fn label(by: UsageBy) -> (&'static str, &'static str) {
    match by {
        UsageBy::Day => ("Day", "D"),
        UsageBy::Week => ("Week", "W"),
        UsageBy::Month => ("Month", "M"),
    }
}

/// `$1,234.56`; under a cent, `<$0.01`, so a bucket with usage never reads as free.
pub fn money(usd: f64) -> String {
    if usd > 0. && usd < 0.005 {
        return "<$0.01".into();
    }
    let cents = (usd * 100.).round() as u64;
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("${grouped}.{:02}", cents % 100)
}

/// A token count at a glance: `950`, `12.3K`, `4.1M`, `2B`.
pub fn tokens(n: u64) -> String {
    let f = n as f64;
    let (v, unit) = match n {
        0..=999 => return n.to_string(),
        1_000..=999_999 => (f / 1e3, "K"),
        1_000_000..=999_999_999 => (f / 1e6, "M"),
        _ => (f / 1e9, "B"),
    };
    let s = format!("{v:.1}");
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

/// The top of the axis: the smallest 1, 2, 4, 5, 6 or 8 times a power of ten at or over `max`, each of which splits
/// into `TICKS` steps that read as whole cents.
pub fn axis_top(max: f64) -> f64 {
    if max <= 0. {
        return 1.;
    }
    let pow = 10f64.powf(max.log10().floor());
    [1., 2., 4., 5., 6., 8., 10.].into_iter().map(|k| k * pow).find(|top| *top >= max).unwrap_or(10. * pow)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn ymd(date: &str) -> Option<(&str, usize, u32)> {
    let mut p = date.split('-');
    let (y, m, d) = (p.next()?, p.next()?.parse::<usize>().ok()?, p.next()?.parse::<u32>().ok()?);
    Some((y, m, d)).filter(|(_, m, _)| (1..=12).contains(m))
}

/// A bucket's short name under its column: `Oct 27` for a day or the Monday of a week, `Oct` for a month (with the
/// year in January, so a year's turn reads).
pub fn short(by: UsageBy, b: &UsageBucket) -> String {
    let Some((y, m, d)) = ymd(b.start.as_str()) else { return b.start.to_string() };
    match by {
        UsageBy::Month if m == 1 => format!("Jan {y}"),
        UsageBy::Month => MONTHS[m - 1].into(),
        _ => format!("{} {d}", MONTHS[m - 1]),
    }
}

/// A bucket's full name, for the table and the tooltip.
pub fn long(by: UsageBy, b: &UsageBucket) -> String {
    let Some((y, m, d)) = ymd(b.start.as_str()) else { return b.start.to_string() };
    match by {
        UsageBy::Day => format!("{} {d}, {y}", MONTHS[m - 1]),
        UsageBy::Week => match ymd(b.end.as_str()) {
            Some((_, em, ed)) if em == m => format!("{} {d}–{ed}, {y}", MONTHS[m - 1]),
            Some((ey, em, ed)) => format!("{} {d} – {} {ed}, {ey}", MONTHS[m - 1], MONTHS[em - 1]),
            None => format!("Week of {} {d}, {y}", MONTHS[m - 1]),
        },
        UsageBy::Month => format!("{} {y}", MONTHS[m - 1]),
    }
}

/// Which columns carry a label under them: every `n`th from the newest, so the current bucket is always named and
/// labels never collide in the plot's width.
pub fn labelled(count: usize, width: f32) -> impl Fn(usize) -> bool {
    let room = (width / 56.).floor().max(1.) as usize;
    let every = count.div_ceil(room).max(1);
    move |i| (count - 1 - i).is_multiple_of(every)
}

fn range_words(r: &UsageReport) -> String {
    let n = r.buckets.len();
    let unit = match r.by {
        UsageBy::Day => "day",
        UsageBy::Week => "week",
        UsageBy::Month => "month",
    };
    format!("the last {n} {unit}{}", if n == 1 { "" } else { "s" })
}

pub fn view(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let u = &s.model.usage;
    let by = u.by.unwrap_or(UsageBy::Day);
    let mut body = div().flex().flex_col().gap(space::S6).child(segments(t, by, cx));
    body = match (&u.report, &u.error) {
        (Some(r), _) if r.totals.responses == 0 => {
            body.child(callout(t, t.muted, IconName::Info, "No usage yet", Some(EMPTY.into())))
        }
        (Some(r), _) => body.child(headline(t, r)).child(chart(s, t, r)).child(table(t, r)).child(models(t, r)),
        (None, Some(e)) => {
            body.child(callout(t, t.warn, IconName::TriangleAlert, "Usage did not load", Some(e.to_string())))
        }
        (None, None) => body.child(div().text_size(text::BODY).text_color(t.muted).child("Loading usage…")),
    };
    let zone = u.report.as_ref().map(|r| format!("Dates follow the reviewer's time zone, {}.", r.time_zone));
    let body = body.child(
        div().flex().flex_col().gap(space::S1).text_size(text::SMALL).text_color(t.muted).child(NOTE).children(zone),
    );
    div().flex().flex_1().min_h_0().child(crate::detail::page("usage", body, None, t, false))
}

/// Day, Week, Month: one control, the grouping in use filled.
fn segments(t: &Colors, by: UsageBy, cx: &Context<Shell>) -> Div {
    let mut row = div()
        .flex()
        .flex_none()
        .gap(space::S1)
        .p(px(3.))
        .rounded(radius::SM)
        .bg(t.surface)
        .border_1()
        .border_color(t.border);
    for option in UsageBy::ALL {
        let (name, key) = label(option);
        let on = option == by;
        let face = div()
            .id(SharedString::from(format!("by-{}-face", option.as_str())))
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(26.))
            .px(space::S3)
            .rounded(px(5.))
            .text_size(text::SMALL)
            .font_weight(FontWeight::MEDIUM)
            .text_color(if on { t.text } else { t.muted })
            .when(on, |d| d.bg(t.selected))
            .when(!on, |d| d.hover(move |d| d.bg(t.hover)))
            .child(name)
            .child(crate::kit::keycap(t, key, None, false));
        let el = focusable(
            SharedString::from(format!("by-{}", option.as_str())),
            face,
            t,
            px(5.),
            click(cx, move |s, cx| s.open_usage(Some(option), cx)),
        );
        row = row.child(tip(el, format!("Group by {}", name.to_lowercase())));
    }
    div().flex().child(row)
}

/// The range's cost in the serif, and what it bought.
fn headline(t: &Colors, r: &UsageReport) -> Div {
    let c = &r.totals;
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(overline(t, format!("Estimated cost, {}", range_words(r))))
        .child(tabular(
            div()
                .font_family(text::SERIF)
                .text_size(text::DISPLAY)
                .line_height(text::LINE_DISPLAY)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(money(c.cost)),
        ))
        .child(div().text_size(text::BODY).text_color(t.muted).child(format!(
            "{} review{} · {} model response{} · {} input, {} output, {} cache read, {} cache write tokens",
            c.reviews,
            if c.reviews == 1 { "" } else { "s" },
            c.responses,
            if c.responses == 1 { "" } else { "s" },
            tokens(c.input),
            tokens(c.output),
            tokens(c.cache_read),
            tokens(c.cache_write),
        )))
}

/// Cost per bucket: columns from one baseline, the axis in USD on the left, a hover tooltip on every column and
/// the costliest one labelled at its cap.
fn chart(s: &Shell, t: &Colors, r: &UsageReport) -> Div {
    let top = axis_top(r.buckets.iter().map(|b| b.counts.cost).fold(0., f64::max));
    let peak = r
        .buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| b.counts.cost > 0.)
        .max_by(|a, b| a.1.counts.cost.total_cmp(&b.1.counts.cost))
        .map(|(i, _)| i);
    let axis_w = px(52.);
    // The page column is at most 760 wide less its insets; the plot takes what the axis leaves.
    let width = (f32::from(s.width) - f32::from(crate::tokens::layout::SIDEBAR) - 64.).clamp(240., 696.) - 52.;
    let named = labelled(r.buckets.len(), width);
    let ticks = div().absolute().inset_0().children((0..=TICKS).map(|k| {
        let frac = k as f32 / TICKS as f32;
        div().absolute().left_0().right_0().bottom(relative(frac)).h(px(1.)).bg(if k == 0 {
            t.border_strong
        } else {
            t.border
        })
    }));
    let axis = div().relative().flex_none().w(axis_w).h(px(PLOT_H)).children((0..=TICKS).map(|k| {
        let frac = k as f32 / TICKS as f32;
        tabular(
            div()
                .absolute()
                .right(space::S2)
                .bottom(relative(frac))
                .mb(px(-7.))
                .text_size(text::CAPTION)
                .text_color(t.muted)
                .child(money(top * frac as f64).trim_end_matches(".00").to_string()),
        )
    }));
    let by = r.by;
    let columns = div().absolute().inset_0().flex().children(r.buckets.iter().enumerate().map(|(i, b)| {
        let frac = (b.counts.cost / top).clamp(0., 1.) as f32;
        let words = format!(
            "{}: {} · {} review{} · {} responses",
            long(by, b),
            money(b.counts.cost),
            b.counts.reviews,
            if b.counts.reviews == 1 { "" } else { "s" },
            b.counts.responses
        );
        let bar = div()
            .w(relative(0.62))
            .max_w(px(BAR_MAX))
            .h(relative(frac))
            .when(b.counts.cost > 0., |d| d.min_h(px(2.)))
            .rounded_t(px(4.))
            .bg(t.accent);
        let cap = (Some(i) == peak)
            .then(|| tabular(div().mb(px(2.)).text_size(text::CAPTION).text_color(t.text).child(money(b.counts.cost))));
        tip(
            div()
                .id(("usage-col", i))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .hover(move |d| d.bg(t.hover))
                .children(cap)
                .child(bar),
            words,
        )
    }));
    let labels = div().flex().ml(axis_w).children(r.buckets.iter().enumerate().map(|(i, b)| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .justify_center()
            .text_size(text::CAPTION)
            .text_color(t.muted)
            .whitespace_nowrap()
            .when(named(i), |d| d.child(short(by, b)))
    }));
    let unit = match by {
        UsageBy::Day => "day",
        UsageBy::Week => "week",
        UsageBy::Month => "month",
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(heading(t, format!("Cost per {unit}, USD")))
        .child(
            div()
                .flex()
                .pt(space::S3)
                .child(axis)
                .child(div().relative().flex_1().min_w_0().h(px(PLOT_H)).child(ticks).child(columns)),
        )
        .child(labels)
}

const COLS: [&str; 6] = ["Reviews", "Input", "Output", "Cache read", "Cache write", "Cost"];

fn numbers(c: &UsageCounts) -> [String; 6] {
    [
        c.reviews.to_string(),
        tokens(c.input),
        tokens(c.output),
        tokens(c.cache_read),
        tokens(c.cache_write),
        money(c.cost),
    ]
}

fn row(t: &Colors, first: String, cells: [String; 6], strong: bool, rule: bool) -> Div {
    let weight = if strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
    tabular(
        div()
            .flex()
            .items_center()
            .h(px(28.))
            .px(space::S2)
            .when(rule, |d| d.border_t_1().border_color(t.border_strong))
            .text_size(text::SMALL)
            .font_weight(weight)
            .text_color(t.text)
            .child(div().flex_1().min_w_0().truncate().child(first))
            .children(cells.into_iter().map(|c| div().w(px(84.)).flex_none().flex().justify_end().child(c))),
    )
}

fn header(t: &Colors, first: &str) -> Div {
    div()
        .flex()
        .px(space::S2)
        .pb(space::S1)
        .border_b_1()
        .border_color(t.border)
        .text_size(text::CAPTION)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.muted)
        .child(div().flex_1().child(first.to_string()))
        .children(COLS.map(|c| div().w(px(84.)).flex_none().flex().justify_end().child(c)))
}

/// Every bucket's numbers, newest first, and the range's totals.
fn table(t: &Colors, r: &UsageReport) -> Div {
    let first = match r.by {
        UsageBy::Day => "Day",
        UsageBy::Week => "Week",
        UsageBy::Month => "Month",
    };
    div()
        .flex()
        .flex_col()
        .child(header(t, first))
        .children(r.buckets.iter().rev().map(|b| row(t, long(r.by, b), numbers(&b.counts), false, false)))
        .child(row(t, "Total".into(), numbers(&r.totals), true, true))
}

/// The range split by model, costliest first.
fn models(t: &Colors, r: &UsageReport) -> Div {
    div().flex().flex_col().gap(space::S2).child(heading(t, "By model")).child(
        div()
            .flex()
            .flex_col()
            .child(header(t, "Model"))
            .children(r.models.iter().map(|m| row(t, m.model.to_string(), numbers(&m.counts), false, false))),
    )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's glob brings its own `test` attribute.
    use super::{axis_top, labelled, long, money, short, tokens};
    use pr_review_tui::protocol::{Text, UsageBucket, UsageBy, UsageCounts};

    #[test]
    fn money_groups_thousands_and_never_reads_a_fraction_of_a_cent_as_free() {
        assert_eq!(money(0.), "$0.00");
        assert_eq!(money(0.004), "<$0.01");
        assert_eq!(money(2.145), "$2.15");
        assert_eq!(money(1234567.891), "$1,234,567.89");
    }

    #[test]
    fn tokens_read_at_a_glance() {
        assert_eq!(
            [0, 999, 1_000, 12_345, 4_100_000, 2_000_000_000].map(tokens),
            ["0", "999", "1K", "12.3K", "4.1M", "2B"]
        );
    }

    #[test]
    fn the_axis_tops_out_at_a_round_number_in_four_even_steps() {
        assert_eq!(axis_top(0.), 1.);
        assert_eq!(axis_top(3.42), 4.);
        assert_eq!(axis_top(13.92), 20.);
        assert_eq!(axis_top(49.44), 50.);
        assert_eq!(axis_top(0.3), 0.4);
        assert_eq!(axis_top(8.1), 10.);
    }

    fn bucket(start: &str, end: &str) -> UsageBucket {
        UsageBucket { start: Text::new(start), end: Text::new(end), counts: UsageCounts::default() }
    }

    #[test]
    fn buckets_are_named_short_under_a_column_and_whole_in_the_table() {
        let w = bucket("2026-10-26", "2026-11-01");
        assert_eq!(
            (short(UsageBy::Week, &w), long(UsageBy::Week, &w)),
            ("Oct 26".into(), "Oct 26 – Nov 1, 2026".into())
        );
        let w = bucket("2026-10-19", "2026-10-25");
        assert_eq!(long(UsageBy::Week, &w), "Oct 19–25, 2026");
        let m = bucket("2026-01-01", "2026-01-31");
        assert_eq!((short(UsageBy::Month, &m), long(UsageBy::Month, &m)), ("Jan 2026".into(), "Jan 2026".into()));
        assert_eq!(long(UsageBy::Day, &bucket("2026-10-28", "2026-10-28")), "Oct 28, 2026");
    }

    #[test]
    fn column_labels_always_name_the_newest_and_thin_out_to_fit() {
        let named = labelled(14, 400.);
        let shown: Vec<usize> = (0..14).filter(|i| named(*i)).collect();
        assert_eq!(shown.last(), Some(&13));
        assert!(shown.len() <= 7, "{shown:?}");
        let all = labelled(12, 696.);
        assert!((0..12).all(&all));
    }
}
