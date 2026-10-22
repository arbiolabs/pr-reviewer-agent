//! The design system's primitives: buttons, key caps, chips, badges, icons, the progress gauge and the display
//! type, drawn from the tokens. Every control here is a tab stop with a focus ring, and shows hover and pressed.

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::{Phase, Severity};

use crate::tokens::{Colors, radius, severity_mark, space, text};

pub type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

pub fn icon(name: IconName, size: Pixels, color: Hsla) -> Svg {
    svg().path(name.path()).size(size).flex_none().text_color(color)
}

/// The 2-point accent ring every focused control wears.
pub fn ring(t: &Colors) -> BoxShadow {
    BoxShadow {
        color: t.accent,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
        inset: false,
    }
}

/// A tab stop that shows the ring when it has the keyboard.
pub fn focusable<E: InteractiveElement>(el: E, t: &Colors) -> E {
    let ring = ring(t);
    el.tab_index(0).focus(move |s| s.shadow(vec![ring]))
}

/// A tooltip: the name of an icon button, or why a control can't act now.
pub fn tip<E: StatefulInteractiveElement>(el: E, text: impl Into<SharedString>) -> E {
    let text = text.into();
    el.tooltip(move |w, cx| Tooltip::new(text.clone()).build(w, cx))
}

/// A key cap: the shortcut beside the action it triggers.
pub fn keycap(t: &Colors, key: &str, on_fill: bool) -> Div {
    let (fg, bg, border) = if on_fill {
        (t.on_accent, t.on_accent.alpha(0.16), t.on_accent.alpha(0.0))
    } else {
        (t.muted, t.bg, t.border)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .rounded(px(5.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(text::CAPTION)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(key.to_string())
}

/// A button with its shortcut. `on` is `None` for a button that can't act now; it draws dimmed and doesn't press.
pub fn button(
    t: &Colors,
    id: impl Into<ElementId>,
    label: &str,
    key: Option<&str>,
    tone: Tone,
    on: Option<OnClick>,
) -> Stateful<Div> {
    let (bg, fg, hover, press, border) = match tone {
        Tone::Primary => (t.accent, t.on_accent, t.accent_hover, t.accent_press, t.accent),
        Tone::Danger => (t.surface, t.danger, t.danger.alpha(0.10), t.danger.alpha(0.18), t.danger.alpha(0.45)),
        Tone::Secondary => (t.surface, t.text, t.hover, t.press, t.border_strong),
        Tone::Ghost => (t.surface.alpha(0.), t.muted, t.hover, t.press, t.surface.alpha(0.)),
    };
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(px(32.))
        .px(space::S3)
        .rounded(radius::SM)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(text::BODY)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.to_string())
        .children(key.map(|k| keycap(t, k, tone == Tone::Primary)));
    match on {
        Some(on) => {
            focusable(b, t).cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press)).on_click(on)
        }
        None => b.opacity(0.45),
    }
}

/// A square icon button whose name is its tooltip.
pub fn icon_button(
    t: &Colors,
    id: impl Into<ElementId>,
    glyph: IconName,
    name: &str,
    on: Option<OnClick>,
) -> Stateful<Div> {
    let b =
        div().id(id).flex().flex_none().items_center().justify_center().size(px(32.)).rounded(radius::SM).child(icon(
            glyph,
            px(16.),
            t.muted,
        ));
    let b = tip(b, name.to_string());
    match on {
        Some(on) => {
            let (hover, press) = (t.hover, t.press);
            focusable(b, t).cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press)).on_click(on)
        }
        None => b.opacity(0.45),
    }
}

/// A quiet label chip: a category, a status, a SHA.
pub fn chip(t: &Colors, label: impl Into<SharedString>, fg: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(22.))
        .px(space::S2)
        .rounded(radius::SM)
        .bg(fg.alpha(0.10))
        .text_size(text::SMALL)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if fg == t.muted { t.muted } else { fg })
        .child(label.into())
}

/// A count in a circle, in its meaning's hue.
pub fn count_badge(n: usize, hue: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(20.))
        .min_w(px(20.))
        .px(px(6.))
        .rounded(radius::PILL)
        .bg(hue.alpha(0.16))
        .text_size(text::CAPTION)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hue)
        .child(n.to_string())
}

/// Severity as shape, word and hue together.
pub fn severity_badge(t: &Colors, s: Severity) -> Div {
    let (mark, word) = severity_mark(s);
    let hue = t.severity(s);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(22.))
        .px(space::S2)
        .rounded(radius::SM)
        .bg(t.severity_wash(s))
        .text_size(text::SMALL)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hue)
        .child(div().text_size(px(10.)).child(mark))
        .child(word)
}

/// A severity's shape and count: what a file or a PR holds.
pub fn severity_count(t: &Colors, s: Severity, n: usize) -> Div {
    let hue = t.severity(s);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(20.))
        .px(px(6.))
        .rounded(radius::PILL)
        .bg(hue.alpha(0.14))
        .text_size(text::CAPTION)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hue)
        .child(div().text_size(px(9.)).child(severity_mark(s).0))
        .child(n.to_string())
}

/// The small uppercase label over a section.
pub fn overline(t: &Colors, label: impl Into<SharedString>) -> Div {
    div()
        .text_size(text::CAPTION)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(label.into().to_uppercase())
}

/// A section heading inside the detail pane.
pub fn heading(t: &Colors, label: impl Into<SharedString>) -> Div {
    div().text_size(text::BODY).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(label.into())
}

/// What is said, set in the serif: a page or PR title.
pub fn display(t: &Colors, title: impl Into<SharedString>) -> Div {
    div()
        .font_family(text::SERIF)
        .text_size(text::DISPLAY)
        .line_height(text::LINE_DISPLAY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text)
        .child(title.into())
}

/// The one-sentence state summary under a heading.
pub fn summary(t: &Colors, sentence: impl Into<SharedString>) -> Div {
    div().text_size(text::LEAD).line_height(px(22.)).text_color(t.muted).child(sentence.into())
}

/// A callout band: the state of something, in its color, with an icon.
pub fn callout(t: &Colors, hue: Hsla, glyph: IconName, title: impl Into<SharedString>, body: Option<String>) -> Div {
    div()
        .flex()
        .gap(space::S3)
        .p(space::S3)
        .rounded(radius::LG)
        .bg(hue.alpha(0.08))
        .border_1()
        .border_color(hue.alpha(0.28))
        .child(div().pt(px(2.)).child(icon(glyph, px(16.), hue)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S1)
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(text::BODY)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(title.into()),
                )
                .children(
                    body.map(|b| div().text_size(text::BODY).line_height(text::LINE_BODY).text_color(t.muted).child(b)),
                ),
        )
}

/// The phases a run walks, as the gauge counts them.
pub const STEPS: [(&str, Phase); 6] = [
    ("Collect", Phase::CollectContext),
    ("Triage", Phase::Triage),
    ("Analyze", Phase::Analyze),
    ("Verify", Phase::Verify),
    ("Prepare", Phase::PreparePublication),
    ("Post", Phase::Publish),
];

/// The step a phase is on; 6 once the run is past the last.
pub fn step_of(p: Phase) -> usize {
    match p {
        Phase::CollectContext => 0,
        Phase::Triage => 1,
        Phase::Analyze => 2,
        Phase::Verify => 3,
        Phase::PreparePublication => 4,
        Phase::AwaitingApproval | Phase::Publish | Phase::Reconcile => 5,
        _ => 6,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Gauge {
    /// Six 6×3 ticks: a sidebar row.
    Pip,
    /// Six 10×4 ticks: a live pill.
    Strip,
    /// Full width with the phase names under it: a run on its page.
    Full,
}

/// The one progress gauge: done ticks are solid, the current tick is a ring in the hue, pending ticks are hairlines.
/// A failed run shows where it stopped in the danger hue.
pub fn progress(t: &Colors, at: usize, hue: Hsla, failed: bool, gauge: Gauge) -> Div {
    let (w, h, gap) = match gauge {
        Gauge::Pip => (px(6.), px(3.), px(1.)),
        Gauge::Strip => (px(10.), px(4.), px(2.)),
        Gauge::Full => (px(0.), px(5.), px(4.)),
    };
    let ticks = div().flex().items_center().gap(gap).children((0..STEPS.len()).map(|i| {
        let tick = div()
            .h(h)
            .rounded(px(2.))
            .when(gauge == Gauge::Full, |d| d.flex_1())
            .when(gauge != Gauge::Full, |d| d.w(w));
        if failed {
            tick.bg(if i <= at.min(5) { t.danger.alpha(0.45) } else { t.border_strong.alpha(0.5) })
        } else if i < at {
            tick.bg(hue)
        } else if i == at {
            tick.bg(hue.alpha(0.28)).border_1().border_color(hue)
        } else {
            tick.bg(t.border_strong.alpha(0.6))
        }
    }));
    if gauge != Gauge::Full {
        return ticks.flex_none();
    }
    div().flex().flex_col().gap(space::S2).w_full().child(ticks).child(div().flex().gap(gap).children(
        STEPS.iter().enumerate().map(|(i, (label, _))| {
            let (color, weight) = if failed {
                (t.faint, FontWeight::NORMAL)
            } else if i == at {
                (t.text, FontWeight::SEMIBOLD)
            } else if i < at {
                (t.muted, FontWeight::NORMAL)
            } else {
                (t.faint, FontWeight::NORMAL)
            };
            div().flex_1().text_size(text::SMALL).font_weight(weight).text_color(color).child(*label)
        }),
    ))
}

/// `14:30` from an ISO timestamp, the wall-clock time the daemon reported.
pub fn clock(iso: &str) -> String {
    iso.split('T').nth(1).map(|t| t.chars().take(5).collect()).unwrap_or_default()
}

/// `14:30:05` from an ISO timestamp.
pub fn clock_s(iso: &str) -> String {
    iso.split('T').nth(1).map(|t| t.chars().take(8).collect()).unwrap_or_default()
}

/// The first `n` sentences of `s`: a preview that ends on a full stop instead of a clipped word.
pub fn lead(s: &str, n: usize) -> String {
    let mut end = 0;
    for _ in 0..n {
        match s[end..].find(". ") {
            Some(i) => end += i + 1,
            None => return s.to_string(),
        }
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lead_keeps_whole_sentences() {
        assert_eq!(super::lead("One. Two. Three.", 2), "One. Two.");
        assert_eq!(super::lead("Only one.", 2), "Only one.");
    }
}
