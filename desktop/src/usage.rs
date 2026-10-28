//! Usage: the tokens and estimated cost of reviews, by day, week or month. One series, so one hue: cost per bucket
//! as columns in the accent, on a recessive grid with its axis in USD, and every value in the table under it. A
//! breakdown by repository, author or PR shows the same range's cost as horizontal bars, the costliest ten and the
//! rest folded into Other, in the same accent.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::{UsageBucket, UsageBy, UsageCounts, UsageGroup, UsageGroupBy, UsageReport};

use crate::kit::{OnClick, callout, focusable, heading, overline, tabular, tip};
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

/// The page's name for a breakdown, and the key that picks it. None clashes with D, W and M or a global key.
pub fn breakdown_label(group: UsageGroupBy) -> (&'static str, &'static str) {
    match group {
        UsageGroupBy::Time => ("Over time", "T"),
        UsageGroupBy::Repo => ("Repository", "R"),
        UsageGroupBy::Author => ("Author", "A"),
        UsageGroupBy::Pr => ("PR", "P"),
    }
}

/// How many breakdown rows get a bar of their own; the rest fold into Other.
const TOP: usize = 10;

/// The rows past the costliest `TOP`, summed: `count` of them, `share` of the range's cost.
#[derive(Debug, PartialEq)]
pub struct Other {
    pub count: usize,
    pub counts: UsageCounts,
    pub share: f64,
}

/// The bars of a breakdown: every row when one more than `TOP` would only fold a single row into Other, else the
/// costliest `TOP` and the rest as Other. `reviews` in Other is a sum, so a review across two rows counts twice there.
pub fn fold_other(groups: &[UsageGroup]) -> (&[UsageGroup], Option<Other>) {
    if groups.len() <= TOP + 1 {
        return (groups, None);
    }
    let (top, rest) = groups.split_at(TOP);
    let mut counts = UsageCounts::default();
    let mut share = 0.;
    for g in rest {
        let c = &g.counts;
        counts.reviews += c.reviews;
        counts.responses += c.responses;
        counts.input += c.input;
        counts.output += c.output;
        counts.cache_read += c.cache_read;
        counts.cache_write += c.cache_write;
        counts.cost += c.cost;
        share += g.share;
    }
    (top, Some(Other { count: rest.len(), counts, share }))
}

/// A share as a percentage a reader compares at a glance: `24%`, `3.5%`, `<0.1%`.
pub fn percent(share: f64) -> String {
    let p = share * 100.;
    match p {
        0. => "0%".into(),
        p if p < 0.1 => "<0.1%".into(),
        p if p < 10. => format!("{p:.1}%"),
        p => format!("{p:.0}%"),
    }
}

/// A breakdown row's name: a PR reads as its repository, number and title; the rest as the daemon labelled them.
pub fn row_name(g: &UsageGroup) -> String {
    match (&g.repository, g.number, &g.pr_id) {
        (Some(repo), Some(n), Some(_)) => format!("{repo}#{n} {}", g.label),
        _ => g.label.to_string(),
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

fn date(d: &str) -> String {
    match ymd(d) {
        Some((y, m, d)) => format!("{} {d}, {y}", MONTHS[m - 1]),
        None => d.to_string(),
    }
}

/// The range the report covers, by count and by its first and last dates: `the last 12 weeks, Aug 10, 2026 – Nov 1,
/// 2026`.
pub fn range_words(r: &UsageReport) -> String {
    let n = r.buckets.len();
    let unit = match r.by {
        UsageBy::Day => "day",
        UsageBy::Week => "week",
        UsageBy::Month => "month",
    };
    let dates = match (r.buckets.first(), r.buckets.last()) {
        (Some(a), Some(b)) => format!(", {} – {}", date(a.start.as_str()), date(b.end.as_str())),
        _ => String::new(),
    };
    format!("the last {n} {unit}{}{dates}", if n == 1 { "" } else { "s" })
}

pub fn view(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let u = &s.model.usage;
    let by = u.by.unwrap_or(UsageBy::Day);
    let controls = div().flex().flex_wrap().gap(space::S3).child(segments(t, by, cx)).child(breakdowns(t, u.group, cx));
    let mut body = div().flex().flex_col().gap(space::S6).child(controls);
    let warn = |e: &str| callout(t, t.warn, IconName::TriangleAlert, "Usage did not load", Some(e.to_string()));
    body = match (&u.report, &u.error) {
        (Some(r), _) if r.totals.responses == 0 => {
            body.child(callout(t, t.muted, IconName::Info, "No usage yet", Some(EMPTY.into())))
        }
        // The last answer stays until the next one comes, so a breakdown shows only once it is the one on screen.
        (Some(r), e) if r.group.is_some_and(|g| g != UsageGroupBy::Time) => body
            .children(e.as_ref().map(|e| warn(e.as_str())))
            .child(headline(t, r))
            .child(bars(t, r))
            .child(group_table(t, r, cx))
            .child(models(t, r)),
        (Some(r), e) => body
            .children(e.as_ref().map(|e| warn(e.as_str())))
            .child(headline(t, r))
            .child(chart(s, t, r))
            .child(table(t, r))
            .child(models(t, r)),
        (None, Some(e)) => body.child(warn(e.as_str())),
        (None, None) => body.child(div().text_size(text::BODY).text_color(t.muted).child("Loading usage…")),
    };
    let zone = u.report.as_ref().map(|r| format!("Dates follow the reviewer's time zone, {}.", r.time_zone));
    let body = body.child(
        div().flex().flex_col().gap(space::S1).text_size(text::SMALL).text_color(t.muted).child(NOTE).children(zone),
    );
    div().flex().flex_1().min_h_0().child(crate::detail::page("usage", body, None, t, false))
}

/// One segmented control: each option's name and key, the one in use filled.
fn control(
    t: &Colors,
    options: impl IntoIterator<Item = (String, &'static str, &'static str, bool, String, OnClick)>,
) -> Div {
    let mut row = div()
        .flex()
        .flex_none()
        .gap(space::S1)
        .p(px(3.))
        .rounded(radius::SM)
        .bg(t.surface)
        .border_1()
        .border_color(t.border);
    for (id, name, key, on, words, on_click) in options {
        let face = div()
            .id(SharedString::from(format!("{id}-face")))
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
        row = row.child(tip(focusable(SharedString::from(id), face, t, px(5.), on_click), words));
    }
    div().flex().child(row)
}

/// Day, Week, Month: the range and, over time, its buckets.
fn segments(t: &Colors, by: UsageBy, cx: &Context<Shell>) -> Div {
    control(
        t,
        UsageBy::ALL.map(|option| {
            let (name, key) = label(option);
            let words = format!("Group by {}", name.to_lowercase());
            (
                format!("by-{}", option.as_str()),
                name,
                key,
                option == by,
                words,
                click(cx, move |s, cx| s.open_usage(Some(option), cx)),
            )
        }),
    )
}

/// Over time, Repository, Author, PR: how the same range is split.
fn breakdowns(t: &Colors, group: UsageGroupBy, cx: &Context<Shell>) -> Div {
    control(
        t,
        UsageGroupBy::ALL.map(|option| {
            let (name, key) = breakdown_label(option);
            let words = match option {
                UsageGroupBy::Time => "Cost per day, week or month".to_string(),
                _ => format!("Cost by {}", name.to_lowercase()),
            };
            let on_click = click(cx, move |s, cx| s.usage_breakdown(option, cx));
            (format!("group-{}", option.as_str()), name, key, option == group, words, on_click)
        }),
    )
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
const SHARE_COLS: [&str; 7] = ["Reviews", "Input", "Output", "Cache read", "Cache write", "Cost", "Share"];

fn numbers(c: &UsageCounts) -> Vec<String> {
    vec![
        c.reviews.to_string(),
        tokens(c.input),
        tokens(c.output),
        tokens(c.cache_read),
        tokens(c.cache_write),
        money(c.cost),
    ]
}

fn with_share(c: &UsageCounts, share: f64) -> Vec<String> {
    let mut cells = numbers(c);
    cells.push(percent(share));
    cells
}

const LABEL_W: f32 = 200.;
const VALUE_W: f32 = 112.;
const BAR_ROW: f32 = 28.;

/// Cost by repository, author or PR: one horizontal bar per row from one baseline, the costliest ten and Other,
/// the axis in USD under them, each bar's cost and share beside it, and its numbers on hover.
fn bars(t: &Colors, r: &UsageReport) -> Div {
    let (top_rows, other) = fold_other(&r.groups);
    let mut items: Vec<(String, &UsageCounts, f64)> =
        top_rows.iter().map(|g| (row_name(g), &g.counts, g.share)).collect();
    if let Some(o) = &other {
        items.push((format!("Other ({})", o.count), &o.counts, o.share));
    }
    let max = items.iter().map(|(_, c, _)| c.cost).fold(0., f64::max);
    let top = axis_top(max);
    let ticks =
        div().absolute().top_0().bottom_0().left(px(LABEL_W)).right(px(VALUE_W)).children((0..=TICKS).map(|k| {
            let frac = k as f32 / TICKS as f32;
            div().absolute().top_0().bottom_0().left(relative(frac)).w(px(1.)).bg(if k == 0 {
                t.border_strong
            } else {
                t.border
            })
        }));
    let rows = items.iter().enumerate().map(|(i, (name, c, share))| {
        let frac = (c.cost / top).clamp(0., 1.) as f32;
        let words = format!(
            "{name}: {} · {} of the range · {} review{} · {} responses",
            money(c.cost),
            percent(*share),
            c.reviews,
            if c.reviews == 1 { "" } else { "s" },
            c.responses
        );
        let bar =
            div().h(px(16.)).w(relative(frac)).when(c.cost > 0., |d| d.min_w(px(2.))).rounded_r(px(4.)).bg(t.accent);
        tip(
            div()
                .id(("usage-bar", i))
                .flex()
                .items_center()
                .h(px(BAR_ROW))
                .hover(move |d| d.bg(t.hover))
                .child(
                    div()
                        .w(px(LABEL_W))
                        .flex_none()
                        .pr(space::S3)
                        .truncate()
                        .text_size(text::SMALL)
                        .text_color(t.text)
                        .child(name.clone()),
                )
                .child(div().flex_1().min_w_0().flex().items_center().child(bar))
                .child(tabular(
                    div()
                        .w(px(VALUE_W))
                        .flex_none()
                        .pl(space::S2)
                        .text_size(text::SMALL)
                        .text_color(t.text)
                        .child(format!("{} · {}", money(c.cost), percent(*share))),
                )),
            words,
        )
    });
    let axis = div().relative().h(px(16.)).ml(px(LABEL_W)).mr(px(VALUE_W)).children((0..=TICKS).map(|k| {
        let frac = k as f32 / TICKS as f32;
        tabular(
            div()
                .absolute()
                .left(relative(frac))
                .ml(px(-20.))
                .w(px(40.))
                .flex()
                .justify_center()
                .text_size(text::CAPTION)
                .text_color(t.muted)
                .child(money(top * frac as f64).trim_end_matches(".00").to_string()),
        )
    }));
    let name = match r.group.unwrap_or_default() {
        UsageGroupBy::Pr => "PR",
        UsageGroupBy::Author => "author",
        _ => "repository",
    };
    div()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(heading(t, format!("Cost by {name}, USD")))
        .child(div().relative().flex().flex_col().pt(space::S1).child(ticks).children(rows))
        .child(axis)
}

/// Every row of the breakdown with its numbers and share, costliest first, and the range's totals. A PR's row opens
/// that PR, by click or ⏎.
fn group_table(t: &Colors, r: &UsageReport, cx: &Context<Shell>) -> Div {
    let first = breakdown_label(r.group.unwrap_or_default()).0;
    div()
        .flex()
        .flex_col()
        .child(header(t, first, &SHARE_COLS))
        .children(r.groups.iter().enumerate().map(|(i, g)| {
            let cells = row(t, row_name(g), with_share(&g.counts, g.share), false, false);
            match g.pr_id.clone() {
                Some(pr) => tip(
                    focusable(
                        ("usage-pr", i),
                        cells.rounded(px(4.)).hover(move |d| d.bg(t.hover)),
                        t,
                        px(4.),
                        click(cx, move |s, cx| s.select_pr(pr.clone(), cx)),
                    ),
                    "Open this PR",
                )
                .into_any_element(),
                None => cells.into_any_element(),
            }
        }))
        .child(row(t, "Total".into(), with_share(&r.totals, if r.totals.cost > 0. { 1. } else { 0. }), true, true))
}

fn row(t: &Colors, first: String, cells: Vec<String>, strong: bool, rule: bool) -> Div {
    let weight = if strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
    let width = cell_w(cells.len());
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
            .children(cells.into_iter().map(|c| div().w(width).flex_none().flex().justify_end().child(c))),
    )
}

fn header(t: &Colors, first: &str, cols: &[&str]) -> Div {
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
        .children(
            cols.iter().map(|c| div().w(cell_w(cols.len())).flex_none().flex().justify_end().child(c.to_string())),
        )
}

/// Seven columns leave the name too little room at the six-column width.
fn cell_w(cols: usize) -> Pixels {
    px(if cols > 6 { 72. } else { 84. })
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
        .child(header(t, first, &COLS))
        .children(r.buckets.iter().rev().map(|b| row(t, long(r.by, b), numbers(&b.counts), false, false)))
        .child(row(t, "Total".into(), numbers(&r.totals), true, true))
}

/// The range split by model, costliest first.
fn models(t: &Colors, r: &UsageReport) -> Div {
    div().flex().flex_col().gap(space::S2).child(heading(t, "By model")).child(
        div()
            .flex()
            .flex_col()
            .child(header(t, "Model", &COLS))
            .children(r.models.iter().map(|m| row(t, m.model.to_string(), numbers(&m.counts), false, false))),
    )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's glob brings its own `test` attribute.
    use super::{axis_top, fold_other, labelled, long, money, percent, range_words, row_name, short, tokens};
    use pr_review_tui::protocol::{
        PrId, Text, UsageBucket, UsageBy, UsageCounts, UsageGroup, UsageGroupBy, UsageReport,
    };

    fn group(i: usize, cost: f64, share: f64) -> UsageGroup {
        UsageGroup {
            key: Text::new(&format!("k{i}")),
            label: Text::new(&format!("row {i}")),
            repository: None,
            number: None,
            title: None,
            pr_id: None,
            author: None,
            counts: UsageCounts { reviews: 1, responses: 2, input: 10, cost, ..Default::default() },
            share,
        }
    }

    #[test]
    fn the_costliest_ten_get_a_bar_and_the_rest_fold_into_other() {
        let eleven: Vec<_> = (0..11).map(|i| group(i, 1., 1. / 11.)).collect();
        assert_eq!(fold_other(&eleven).0.len(), 11, "one row is never folded alone");
        let fourteen: Vec<_> = (0..14).map(|i| group(i, 14. - i as f64, 0.05)).collect();
        let (top, other) = fold_other(&fourteen);
        assert_eq!(top.len(), 10);
        let other = other.unwrap();
        assert_eq!((other.count, other.counts.cost, other.counts.responses, other.counts.input), (4, 10., 8, 40));
        assert!((other.share - 0.2).abs() < 1e-9);
    }

    #[test]
    fn shares_read_as_percentages_and_a_pr_reads_by_repository_and_number() {
        assert_eq!([0., 0.0004, 0.035, 0.243].map(percent), ["0%", "<0.1%", "3.5%", "24%"]);
        let mut pr = group(0, 1., 1.);
        (pr.repository, pr.number, pr.pr_id) = (Some(Text::new("acme/shop")), Some(12), Some(PrId::new("p")));
        assert_eq!(row_name(&pr), "acme/shop#12 row 0");
        assert_eq!(row_name(&group(1, 1., 1.)), "row 1");
    }

    #[test]
    fn the_range_is_named_by_its_dates() {
        let r = UsageReport {
            request_id: None,
            by: UsageBy::Week,
            time_zone: Text::new("UTC"),
            currency: Text::new("USD"),
            price_source: Text::new("catalog"),
            buckets: vec![bucket("2026-08-10", "2026-08-16"), bucket("2026-10-26", "2026-11-01")],
            totals: UsageCounts::default(),
            models: vec![],
            group: Some(UsageGroupBy::Repo),
            groups: vec![],
        };
        assert_eq!(range_words(&r), "the last 2 weeks, Aug 10, 2026 – Nov 1, 2026");
    }

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
