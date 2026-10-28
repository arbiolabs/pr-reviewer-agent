//! The design system's primitives: buttons, key caps, chips, badges, icons, the progress gauge and the display
//! type, drawn from the tokens. Every control here is a tab stop with a focus ring, and shows hover and pressed.

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pr_review_tui::protocol::{Phase, Severity};

use crate::queue::Mark;
use crate::tokens::{Colors, DISABLED_OUTLINE, radius, severity_mark, space, text};
use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Primary,
    Secondary,
    /// Outlined in the danger hue: a cancel on a page.
    Danger,
    /// Filled in the danger hue: the one irreversible act in a sheet.
    Destructive,
    Ghost,
}

pub fn icon(name: IconName, size: Pixels, color: Hsla) -> Svg {
    svg().path(name.path()).size(size).flex_none().text_color(color)
}

/// The ring's offset from the control's edge, and its width.
const RING_GAP: Pixels = px(2.);
const RING: Pixels = px(2.);

/// The keyboard owns focus: ⇥ sets it and a mouse press clears it. GPUI's own focus-visible clears on any pointer
/// move too, and macOS sends one when the window activates, which hid the ring a frame after ⇥. One window, so
/// one flag.
pub static KEYBOARD: AtomicBool = AtomicBool::new(false);

/// The macOS accent color in a live window, which rings focus wherever it holds 3:1 on the theme's window. Unset, the
/// theme's own focus color: a snapshot's pixels must not depend on the machine's settings.
pub static SYSTEM_ACCENT: OnceLock<Hsla> = OnceLock::new();

thread_local! {
    /// While a sheet is open, only the controls built inside it are tab stops, so ⇥ stays in the sheet. The window
    /// renders on one thread, page first and the sheet last, so the flags follow the build.
    static SHEET_OPEN: Cell<bool> = const { Cell::new(false) };
    static IN_SHEET: Cell<bool> = const { Cell::new(false) };
}

/// Marks the frame about to render: whether a sheet will float over the page.
pub fn sheet_open(open: bool) {
    SHEET_OPEN.set(open);
    IN_SHEET.set(false);
}

/// Builds a sheet's controls as the only tab stops of the frame.
pub fn in_sheet<R>(build: impl FnOnce() -> R) -> R {
    IN_SHEET.set(true);
    let out = build();
    IN_SHEET.set(false);
    out
}

/// A tab stop: `inner` inside a frame that is 4 points larger on every side and pulled back by as much, so the
/// layout doesn't move. The frame's border is the focus ring, 2 points wide and 2 points off the control's edge.
/// It shows only while the keyboard holds focus there and clears when focus moves; it never paints the control's
/// own background, so a focused row keeps its fill. The frame takes the click, so ⏎ and space press it too.
/// `radius` is the control's own; the ring's corners follow it.
pub fn focusable(
    id: impl Into<ElementId>,
    inner: impl IntoElement,
    t: &Colors,
    radius: Pixels,
    on: OnClick,
) -> Stateful<Div> {
    let ring = SYSTEM_ACCENT.get().copied().filter(|c| crate::tokens::ring_holds(*c, t)).unwrap_or(t.focus);
    div()
        .id(id)
        .when(!SHEET_OPEN.get() || IN_SHEET.get(), |d| d.tab_index(0))
        .flex()
        .flex_col()
        .m(-(RING_GAP + RING))
        .p(RING_GAP)
        .border(RING)
        .border_color(ring.alpha(0.))
        .rounded(radius + RING_GAP + RING)
        .cursor_pointer()
        .when(KEYBOARD.load(Ordering::Relaxed), |d| d.focus(move |s| s.border_color(ring)))
        .on_click(on)
        .child(inner)
}

/// A hover lift: `base` one step toward the text color, opaque, so a hovered surface brightens in dark and
/// deepens in light without changing its border.
pub fn lift(t: &Colors, base: Hsla) -> Hsla {
    if base.a < 1. {
        return t.hover;
    }
    let (b, f) = (base.to_rgb(), t.text.to_rgb());
    let mix = |x: f32, y: f32| y * 0.05 + x * 0.95;
    Rgba { r: mix(b.r, f.r), g: mix(b.g, f.g), b: mix(b.b, f.b), a: 1. }.into()
}

/// The selection rail: a 3-point accent bar inside a selected row's left edge. The row must be `relative`.
pub fn rail(t: &Colors) -> Div {
    div().absolute().left(px(2.)).top(space::S2).bottom(space::S2).w(px(3.)).rounded(px(2.)).bg(t.accent)
}

/// Digits that line up: one width per figure, so a column of numbers reads as a column.
pub fn tabular<E: Styled>(mut el: E) -> E {
    el.text_style().font_features = Some(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])));
    el
}

/// A tooltip: the name of an icon button, or why a control can't act now.
pub fn tip<E: StatefulInteractiveElement>(el: E, text: impl Into<SharedString>) -> E {
    let text = text.into();
    el.tooltip(move |w, cx| Tooltip::new(text.clone()).build(w, cx))
}

/// A key cap: the shortcut beside the action it triggers. `on_fill` is the fill's text color on a filled button;
/// `dim` is a cap on a control that can't act now.
pub fn keycap(t: &Colors, key: &str, on_fill: Option<Hsla>, dim: bool) -> Div {
    let (fg, bg, border) = match on_fill {
        Some(fg) => (fg, fg.alpha(0.16), fg.alpha(0.0)),
        None if dim => (t.disabled, t.surface.alpha(0.), t.disabled.alpha(DISABLED_OUTLINE)),
        None => (t.muted, t.bg, t.border),
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
    button_with(t, id, label, key, tone, on, None)
}

/// A button whose label ends in an icon, such as a menu's chevron: drawn as an icon centred on the label, since a
/// glyph like ⌄ sits on the text's baseline and reads as dropped below the word.
pub fn button_with(
    t: &Colors,
    id: impl Into<ElementId>,
    label: &str,
    key: Option<&str>,
    tone: Tone,
    on: Option<OnClick>,
    trailing: Option<IconName>,
) -> Stateful<Div> {
    let (bg, fg, hover, press, border) = match (tone, on.is_some()) {
        // A control that can't act now is outlined grey, whatever its tone: 3:1 for the outline, AA for its words.
        (_, false) => {
            let none = t.surface.alpha(0.);
            (none, t.disabled, none, none, t.disabled.alpha(DISABLED_OUTLINE))
        }
        (Tone::Primary, _) => (t.accent, t.on_accent, t.accent_hover, t.accent_press, t.accent),
        (Tone::Destructive, _) => (t.danger, t.on_danger, t.danger.alpha(0.88), t.danger.alpha(0.76), t.danger),
        (Tone::Danger, _) => (t.surface, t.danger, t.danger.alpha(0.10), t.danger.alpha(0.18), t.danger.alpha(0.45)),
        (Tone::Secondary, _) => (t.surface, t.text, lift(t, t.surface), t.press, t.border_strong),
        (Tone::Ghost, _) => (t.surface.alpha(0.), t.muted, t.hover, t.press, t.surface.alpha(0.)),
    };
    let filled = matches!(tone, Tone::Primary | Tone::Destructive) && on.is_some();
    let id = id.into();
    let face = ElementId::Name(format!("{id}-face").into());
    let b = div()
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
        .children(trailing.map(|name| icon(name, px(12.), fg)))
        .children(key.map(|k| keycap(t, k, if filled { Some(fg) } else { None }, on.is_none())));
    match on {
        Some(on) => {
            focusable(id, b.id(face).hover(move |s| s.bg(hover)).active(move |s| s.bg(press)), t, radius::SM, on)
        }
        None => b.id(id),
    }
}

/// A command a read-only daemon refuses: outlined grey like any control that can't act, with a lock where its
/// shortcut would be, so it reads as off for a reason and not as a quiet secondary.
pub fn locked_button(t: &Colors, id: impl Into<ElementId>, label: &str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(px(32.))
        .px(space::S3)
        .rounded(radius::SM)
        .border_1()
        .border_dashed()
        .border_color(t.disabled.alpha(DISABLED_OUTLINE))
        .text_size(text::BODY)
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.disabled)
        .child(icon(IconName::Lock, px(13.), t.disabled))
        .child(label.to_string())
}

/// A square icon button whose name is its tooltip.
pub fn icon_button(
    t: &Colors,
    id: impl Into<ElementId>,
    glyph: IconName,
    name: &str,
    side: Pixels,
    on: Option<OnClick>,
) -> Stateful<Div> {
    let id = id.into();
    let face = ElementId::Name(format!("{id}-face").into());
    let b = div().flex().flex_none().items_center().justify_center().size(side).rounded(radius::SM).child(icon(
        glyph,
        px(16.),
        if on.is_some() { t.muted } else { t.disabled },
    ));
    match on {
        Some(on) => {
            let (hover, press) = (t.hover, t.press);
            tip(
                focusable(id, b.id(face).hover(move |s| s.bg(hover)).active(move |s| s.bg(press)), t, radius::SM, on),
                name.to_string(),
            )
        }
        None => tip(b.id(id), name.to_string()),
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
        .bg(fg.alpha(t.tint))
        .border_1()
        .border_color(fg.alpha(t.tint * 2.))
        .text_size(text::SMALL)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.into())
}

/// A count in a circle, in its meaning's hue.
pub fn count_badge(t: &Colors, n: usize, hue: Hsla) -> Div {
    tabular(div())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(20.))
        .min_w(px(20.))
        .px(px(6.))
        .rounded(radius::PILL)
        .bg(hue.alpha(t.tint))
        .border_1()
        .border_color(hue.alpha(t.tint * 2.))
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
        .bg(hue.alpha(t.tint))
        .border_1()
        .border_color(hue.alpha(t.tint * 2.))
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
        .bg(hue.alpha(t.tint))
        .border_1()
        .border_color(hue.alpha(t.tint * 2.))
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

/// What is said, set in the serif: a page or PR title, in the detail pane's 696-point column.
pub fn display(t: &Colors, title: impl Into<SharedString>) -> Div {
    display_in(t, title, 696.)
}

/// The width a serif title wraps at so its lines come out even: GPUI wraps greedily, which can leave one word
/// alone on the last line, so a title that needs more than one line gets a measure that splits its characters
/// evenly, plus a little slack. `room` is the column's width in points. ponytail: an average glyph width instead
/// of shaping the text; shape it if a title still orphans.
pub fn balanced(title: &str, room: f32) -> Option<f32> {
    const GLYPH: f32 = 13.5;
    let chars = title.chars().count() as f32;
    let per_line = (room / GLYPH).floor();
    if chars <= per_line {
        return None;
    }
    let lines = (chars / per_line).ceil();
    Some((((chars / lines).ceil() + 5.) * GLYPH).min(room))
}

/// A serif title balanced for a column `room` points wide.
pub fn display_in(t: &Colors, title: impl Into<SharedString>, room: f32) -> Div {
    let title = title.into();
    div()
        .when_some(balanced(&title, room), |d, w| d.max_w(px(w)))
        .font_family(text::SERIF)
        .text_size(text::DISPLAY)
        .line_height(text::LINE_DISPLAY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text)
        .child(title)
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

/// The one progress gauge, drawn from `marks`: done ticks are solid in `hue`, the current tick is a ring in it,
/// waiting ticks are hairlines, a stop is solid in `hue` with the steps before it in the muted ink, and a skipped
/// step is a dashed outline whose label says "Not posted".
pub fn progress(t: &Colors, marks: [Mark; 6], hue: Hsla, gauge: Gauge) -> Div {
    let (w, h, gap) = match gauge {
        Gauge::Pip => (px(6.), px(3.), px(1.)),
        Gauge::Strip => (px(10.), px(4.), px(2.)),
        Gauge::Full => (px(0.), px(5.), px(4.)),
    };
    let stopped = marks.contains(&Mark::Stopped);
    let ticks = div().flex().items_center().gap(gap).children(marks.iter().map(|mark| {
        let tick = div()
            .h(h)
            .rounded(px(2.))
            .when(gauge == Gauge::Full, |d| d.flex_1())
            .when(gauge != Gauge::Full, |d| d.w(w));
        match mark {
            Mark::Done if stopped => tick.bg(t.muted.alpha(0.6)),
            Mark::Done | Mark::Stopped => tick.bg(hue),
            Mark::Now => tick.bg(hue.alpha(0.28)).border_1().border_color(hue),
            Mark::Waiting => tick.bg(t.border_strong.alpha(0.6)),
            Mark::Skipped => tick.border_1().border_dashed().border_color(t.faint),
        }
    }));
    if gauge != Gauge::Full {
        return ticks.flex_none();
    }
    div().flex().flex_col().gap(space::S2).w_full().child(ticks).child(div().flex().gap(gap).children(
        STEPS.iter().zip(marks).map(|((label, _), mark)| {
            let (color, weight, label) = match mark {
                Mark::Stopped => (hue, FontWeight::SEMIBOLD, *label),
                Mark::Now => (t.text, FontWeight::SEMIBOLD, *label),
                Mark::Done => (t.muted, FontWeight::NORMAL, *label),
                Mark::Waiting => (t.faint, FontWeight::NORMAL, *label),
                Mark::Skipped => (t.faint, FontWeight::NORMAL, "Not posted"),
            };
            div().flex_1().text_size(text::SMALL).font_weight(weight).text_color(color).child(label)
        }),
    ))
}

/// A daemon's reason as a sentence: a capital to start, a full stop to end.
pub fn sentence(s: &str) -> String {
    let s = s.trim().trim_end_matches('.');
    let mut out: String = s.chars().take(1).flat_map(char::to_uppercase).collect();
    out.push_str(s.get(s.chars().next().map_or(0, char::len_utf8)..).unwrap_or(""));
    out.push('.');
    out
}

/// `s` in at most `max` characters: a longer one keeps its head and its tail around " … ", each cut where a
/// list separates (a space) or else where a path does (a slash), so a path is never split inside a name.
pub fn middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let half = (max - 3) / 2;
    let at = |seps: &[char], range: std::ops::Range<usize>, back: bool| {
        let mut r: Vec<usize> = range.filter(|&i| seps.contains(&chars[i])).collect();
        if back {
            r.reverse();
        }
        r.first().copied()
    };
    let head = at(&[' '], half / 2..half + 1, true).or(at(&['/'], half / 2..half + 1, true)).unwrap_or(half);
    let from = chars.len() - half;
    let tail = at(&[' '], from..chars.len() - half / 2, false)
        .or(at(&['/'], from..chars.len() - half / 2, false))
        .unwrap_or(from);
    let head: String = chars[..head].iter().collect();
    let tail: String = chars[tail..].iter().collect();
    format!("{} … {}", head.trim_end_matches([',', ' ']), tail.trim_start_matches([',', ' ']))
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

/// One segmented control: each option's name and key, the one in use filled, and a badge after the name when an
/// option wants attention.
pub fn segmented(
    t: &Colors,
    options: impl IntoIterator<Item = (String, &'static str, &'static str, bool, String, OnClick, Option<Div>)>,
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
    for (id, name, key, on, words, on_click, badge) in options {
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
            .child(keycap(t, key, None, false))
            .children(badge);
        row = row.child(tip(focusable(SharedString::from(id), face, t, px(5.), on_click), words));
    }
    div().flex().child(row)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sentence_capitalizes_and_ends_with_a_full_stop() {
        assert_eq!(
            super::sentence("socket directory does not exist: /tmp/x"),
            "Socket directory does not exist: /tmp/x."
        );
        assert_eq!(super::sentence("Already a sentence."), "Already a sentence.");
    }

    #[test]
    fn a_long_title_wraps_at_an_even_measure() {
        // Greedy wrapping at 696 points left "from" alone on the second line.
        let w = super::balanced("Sign-in: send users back to the page they started from", 696.).unwrap();
        assert!((400. ..500.).contains(&w), "{w}");
        assert_eq!(super::balanced("Gift cards: redeem a balance at checkout", 696.), None);
    }

    #[test]
    fn middle_keeps_whole_path_parts() {
        let s = "3 files changed (+41 -6): src/server/routes/orders.ts, src/server/orders/reserve.ts, src/server/orders/history.ts";
        let m = super::middle(s, 72);
        assert!(m.chars().count() <= 72, "{m}");
        assert_eq!(m, "3 files changed (+41 -6): … src/server/orders/history.ts");
        assert_eq!(super::middle("short", 72), "short");
    }

    #[test]
    fn lead_keeps_whole_sentences() {
        assert_eq!(super::lead("One. Two. Three.", 2), "One. Two.");
        assert_eq!(super::lead("Only one.", 2), "Only one.");
    }
}
