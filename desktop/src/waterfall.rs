//! A run's lite waterfall. Adapted from arbiolabs/agents-trace-waterfall's trace-app (`render_card`, `render_row`,
//! `render_bar` and `present::label_at`): one time axis fit to the width, a row per span indented under its parent
//! with a bar from start to end, open spans growing to the now line, and the clicked span's detail under it. Lite:
//! no zoom, pan or import, and no motion beyond what the page already has.

use std::collections::BTreeMap;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::Run;
use pr_review_tui::waterfall::{Span, SpanKind, duration, fit, rows, ticks, tokens};

use crate::kit::{overline, tabular};
use crate::tokens::{Colors, radius, space, text};
use crate::ui::Shell;

const ROW_H: f32 = 26.;
const BAR_H: f32 = 12.;
const AXIS_H: f32 = 22.;
const NAME_W: f32 = 240.;
const INDENT: f32 = 14.;
/// The timeline's width on a page at its reading width; ticks are spaced for it, bars sit at fractions of it.
const TL_W: f64 = 430.;
/// A long review keeps its newest rows: each turn's tool calls fold into it once the next turn starts.
const MAX_ROWS: usize = 28;

fn hue(t: &Colors, k: SpanKind) -> Hsla {
    match k {
        SpanKind::Phase => t.accent,
        SpanKind::Step => t.muted,
        SpanKind::Model => t.syn_keyword,
        SpanKind::Tool => t.syn_type,
    }
}

/// `read_file x2, search_code`: what a span's children did, by tool.
fn called(spans: &[Span], id: &str) -> String {
    let mut n: BTreeMap<&str, usize> = BTreeMap::new();
    for s in spans.iter().filter(|s| s.parent.as_ref().map(|p| p.as_str()) == Some(id)) {
        *n.entry(s.tool.as_str()).or_default() += 1;
    }
    n.into_iter().map(|(k, c)| if c > 1 { format!("{k} x{c}") } else { k.to_string() }).collect::<Vec<_>>().join(", ")
}

/// The run's waterfall, or `None` when its daemon sent no spans.
pub fn waterfall(s: &Shell, t: &Colors, run: &Run, cx: &Context<Shell>) -> Option<Div> {
    let m = &s.model;
    let spans = m.domain.spans.get(&run.id).filter(|v| !v.is_empty())?;
    let live = run.active();
    // A run that is not working stops at its last line: a span it never closed does not grow.
    let last = spans.iter().map(|x| x.ended_ms.unwrap_or(x.started_ms)).max()?;
    let now = if live { m.domain.journal_now(m.now_ms)? } else { last };
    let vp = fit(spans, now, TL_W)?;
    let frac = |ms: i64| (vp.col_of(ms) / TL_W).clamp(0., 1.) as f32;
    let selected = |sp: &Span| s.span.as_deref().is_some_and(|k| k == sp.id.as_str() || k == sp.name());

    // A phase that is over, and a turn that is over but the newest, fold what ran inside them into a `+N` chip,
    // unless the reader opened them.
    let newest_turn = spans.iter().filter(|x| x.kind == SpanKind::Model).max_by_key(|x| x.started_ms).map(|x| &x.id);
    let folded = |p: &Span| {
        p.ended_ms.is_some()
            && !selected(p)
            && (p.kind == SpanKind::Phase || (p.kind == SpanKind::Model && Some(&p.id) != newest_turn))
    };
    // Pre-order: a row deeper than a folded row before it is inside it.
    let mut fold_at: Option<u16> = None;
    let shown: Vec<_> = rows(spans)
        .into_iter()
        .filter(|r| {
            if fold_at.is_some_and(|d| r.depth > d) {
                return false;
            }
            fold_at = folded(&spans[r.ix]).then_some(r.depth);
            true
        })
        .collect();
    let shown = &shown[shown.len().saturating_sub(MAX_ROWS)..];

    let tick_row =
        div().relative().h(px(AXIS_H)).flex_1().children(ticks(&vp, TL_W, 70.).into_iter().map(|(at, l)| {
            tabular(div())
                .absolute()
                .left(relative(frac(at)))
                .top(px(4.))
                .text_size(text::CAPTION)
                .text_color(t.muted)
                .child(l)
        }));
    let axis = div()
        .flex()
        .border_b_1()
        .border_color(t.border)
        .child(div().w(px(NAME_W)).flex_none().pl(space::S2).pt(px(4.)).child(overline(t, "Waterfall")))
        .child(tick_row);

    let body = div().flex().flex_col().children(shown.iter().flat_map(|r| {
        let sp = &spans[r.ix];
        let open = sp.ended_ms.is_none() && live;
        let c = hue(t, sp.kind);
        let on = selected(sp);
        let (x0, x1) = (frac(sp.started_ms), frac(sp.end_ms(now)));
        let mut label = duration(sp.end_ms(now) - sp.started_ms);
        if let Some(n) = sp.output_tokens {
            label = format!("{label} · {} tok", tokens(n));
        }
        let hidden = if folded(sp) { spans.iter().filter(|k| k.parent.as_ref() == Some(&sp.id)).count() } else { 0 };
        let name = div()
            .w(px(NAME_W))
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(8. + f32::from(r.depth) * INDENT))
            .pr(space::S2)
            .child(div().size(px(7.)).flex_none().rounded(radius::PILL).bg(if open { t.running } else { c }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(text::SMALL)
                    .font_family(if sp.kind == SpanKind::Tool { text::MONO } else { text::UI })
                    .when(sp.kind == SpanKind::Phase, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(if open || on { t.text } else { t.muted })
                    .child(sp.name()),
            )
            .when(hidden > 0, |d| {
                d.child(
                    tabular(div())
                        .flex_none()
                        .px(px(6.))
                        .rounded(radius::PILL)
                        .bg(t.hover)
                        .text_size(text::CAPTION)
                        .text_color(t.muted)
                        .child(format!("+{hidden}")),
                )
            });
        // The label sits after the bar while it fits, else before it (present::label_at, by fractions).
        let after = x1 < 0.78;
        let bar = div()
            .absolute()
            .left(relative(x0))
            .w(relative((x1 - x0).max(0.004)))
            .min_w(px(3.))
            .top(px((ROW_H - BAR_H) / 2.))
            .h(px(BAR_H))
            .rounded(px(4.))
            .border_1()
            .border_color(c.alpha(if open { 0.9 } else { 0.55 }))
            .bg(c.alpha(if open { 0.14 } else { 0.28 }));
        let tag = tabular(div())
            .absolute()
            .top_0()
            .h_full()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .text_size(text::CAPTION)
            .text_color(t.muted)
            .map(|d| if after { d.left(relative(x1)).ml(px(6.)) } else { d.right(relative(1. - x0)).mr(px(6.)) })
            .child(label);
        let id = sp.id.to_string();
        let row = div()
            .id(SharedString::from(format!("span-{}", sp.id.as_str())))
            .flex()
            .h(px(ROW_H))
            .rounded(radius::SM)
            .cursor_pointer()
            .hover(|st| st.bg(t.hover))
            .when(on, |d| d.bg(t.selected))
            .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.span = if s.span.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                cx.notify();
            }))
            .child(name)
            .child(div().relative().flex_1().h_full().overflow_hidden().child(bar).child(tag))
            .into_any_element();
        // The opened span's detail sits under its row, as trace-app's popover sits under its bar.
        let opened = on.then(|| detail(t, spans, sp, now, vp.start_ms).mx(space::S2).my(space::S1).into_any_element());
        std::iter::once(row).chain(opened)
    }));
    // The now line crosses every row while the run is live.
    let now_line = live.then(|| {
        div()
            .absolute()
            .top(px(AXIS_H))
            .bottom_0()
            .left(px(NAME_W))
            .right_0()
            .child(div().absolute().top_0().bottom_0().left(relative(frac(now))).w(px(1.)).bg(t.running.alpha(0.7)))
    });
    let card = div()
        .relative()
        .flex()
        .flex_col()
        .p(space::S2)
        .rounded(radius::LG)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .child(axis)
        .children(now_line)
        .child(body);
    Some(card)
}

/// The clicked span: its kind, name and duration, when it started, its tokens and tools, and its latest words.
fn detail(t: &Colors, spans: &[Span], sp: &Span, now: i64, origin: i64) -> Div {
    let line = |k: &str, v: String| {
        div()
            .flex()
            .gap(space::S3)
            .child(div().w(px(84.)).flex_none().text_size(text::SMALL).text_color(t.muted).child(k.to_string()))
            .child(tabular(div()).flex_1().min_w_0().text_size(text::SMALL).text_color(t.text).child(v))
    };
    let state = if sp.ended_ms.is_some() { "done" } else { "running" };
    let tools = called(spans, sp.id.as_str());
    div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .p(space::S3)
        .rounded(radius::LG)
        .border_1()
        .border_color(t.border)
        .bg(t.raised)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(space::S2)
                .child(
                    div().text_size(text::BODY).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(sp.name()),
                )
                .child(div().font_family(text::MONO).text_size(text::CAPTION).text_color(hue(t, sp.kind)).child(
                    match sp.kind.word() == sp.tool.as_str() {
                        true => sp.tool.to_string(),
                        false => format!("{} · {}", sp.kind.word(), sp.tool),
                    },
                )),
        )
        .child(line("Duration", format!("{} · {state}", duration(sp.end_ms(now) - sp.started_ms))))
        .child(line("Started", format!("+{} into the run", duration(sp.started_ms - origin))))
        .children(sp.output_tokens.map(|n| line("Output", format!("{} tokens", tokens(n)))))
        .when(!tools.is_empty(), |d| d.child(line("Called", tools)))
        .child(line("Latest", sp.summary.to_string()))
}
