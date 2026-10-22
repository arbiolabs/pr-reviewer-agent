//! The design system's primitives: buttons, key caps, chips, badges and icons, drawn from the tokens.

use gpui_kit::assets::IconName;
use gpui_kit::*;
use pr_review_tui::protocol::Severity;

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

/// A key cap: the shortcut beside the action it triggers.
pub fn keycap(t: &Colors, key: &str, on_fill: bool) -> Div {
    let (fg, bg, border) = if on_fill {
        (t.on_accent, t.on_accent.alpha(0.16), t.on_accent.alpha(0.0))
    } else {
        (t.muted, t.bg, t.border)
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .rounded(radius::XS)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(text::CAPTION)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(key.to_string())
}

/// A button with its shortcut. `on` is `None` for a button that can't act now; it draws, but doesn't press.
pub fn button(
    t: &Colors,
    id: impl Into<ElementId>,
    label: &str,
    key: Option<&str>,
    tone: Tone,
    on: Option<OnClick>,
) -> Stateful<Div> {
    let enabled = on.is_some();
    let (bg, fg, hover, press, border) = match tone {
        Tone::Primary => (t.accent, t.on_accent, t.accent_hover, t.accent_press, t.accent),
        Tone::Danger => (t.surface, t.danger, t.danger.alpha(0.10), t.danger.alpha(0.18), t.danger.alpha(0.45)),
        Tone::Secondary => (t.surface, t.text, t.surface_hover, t.surface_press, t.border_strong),
        Tone::Ghost => (t.surface.alpha(0.), t.muted, t.surface_hover, t.surface_press, t.surface.alpha(0.)),
    };
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(px(28.))
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
    if let Some(on) = on {
        b = b.cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press)).on_click(on);
    }
    if !enabled {
        b = b.opacity(0.45);
    }
    b
}

/// A quiet label chip: a category, a status, a SHA.
pub fn chip(t: &Colors, label: impl Into<SharedString>, fg: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(space::S2)
        .rounded(radius::XS)
        .bg(fg.alpha(0.10))
        .text_size(text::SMALL)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if fg == t.muted { t.muted } else { fg })
        .child(label.into())
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
        .h(px(20.))
        .px(space::S2)
        .rounded(radius::XS)
        .bg(t.severity_wash(s))
        .text_size(text::SMALL)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hue)
        .child(div().text_size(px(10.)).child(mark))
        .child(word)
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

/// A callout band: the state of something, in its color, with an icon.
pub fn callout(t: &Colors, hue: Hsla, glyph: IconName, title: impl Into<SharedString>, body: Option<String>) -> Div {
    div()
        .flex()
        .gap(space::S3)
        .p(space::S3)
        .rounded(radius::MD)
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
