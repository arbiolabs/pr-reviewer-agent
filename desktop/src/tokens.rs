//! Design tokens: color for dark and light, the type scale, the 4-point grid, two radii and motion. Every view reads
//! its look from here; DESIGN.md explains the choices.

use std::time::Duration;

use gpui_kit::{Hsla, Pixels, px, rgb};
use pr_review_tui::protocol::Severity;

/// One appearance's palette. Every text token meets WCAG AA (4.5:1) on every surface it sits on; `tests` checks it.
#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_press: Hsla,
    pub on_accent: Hsla,
    pub running: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
    pub critical: Hsla,
    pub high: Hsla,
    pub medium: Hsla,
    pub low: Hsla,
    pub add_bg: Hsla,
    pub del_bg: Hsla,
    pub add_fg: Hsla,
    pub del_fg: Hsla,
    pub code_bg: Hsla,
    pub syn_keyword: Hsla,
    pub syn_string: Hsla,
    pub syn_number: Hsla,
    pub syn_comment: Hsla,
    pub syn_fn: Hsla,
    pub syn_type: Hsla,
    /// Text on a danger fill: the destructive primary of a sheet.
    pub on_danger: Hsla,
    /// Text of a control or a palette row that can't act now: the text color dimmed to about half, as one opaque
    /// color so it still meets AA on every surface.
    pub disabled: Hsla,
    /// The alpha of a chip's or a badge's wash under its hue.
    pub tint: f32,
    pub scrim: Hsla,
    pub shadow: Hsla,
    /// The hover wash over any surface: 4 % of the text color.
    pub hover: Hsla,
    /// The pressed wash: 8 %.
    pub press: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    c(hex).alpha(alpha)
}

/// The air-traffic board at night: blue-graphite, quiet, with color reserved for meaning.
pub fn dark() -> Colors {
    Colors {
        bg: c(0x0F1217),
        sidebar: c(0x0B0D11),
        surface: c(0x161A21),
        raised: c(0x13171D),
        selected: c(0x1E2431),
        border: c(0x242A34),
        border_strong: c(0x343C49),
        text: c(0xE9EDF2),
        muted: c(0xA0A9B6),
        faint: c(0x8590A0),
        accent: c(0x8E95FF),
        accent_hover: c(0xA2A8FF),
        accent_press: c(0x7980EE),
        on_accent: c(0x0B0D14),
        running: c(0x52CBDD),
        ok: c(0x4FC98F),
        warn: c(0xE6BA4E),
        danger: c(0xF9757C),
        critical: c(0xFF6B86),
        high: c(0xFFA05A),
        medium: c(0xE8C95A),
        low: c(0x98AAC4),
        add_bg: ca(0x3FB97A, 0.13),
        del_bg: ca(0xF9757C, 0.13),
        add_fg: c(0x63D69C),
        del_fg: c(0xFF8A8F),
        code_bg: c(0x0C0F13),
        syn_keyword: c(0xC5A3FF),
        syn_string: c(0x9BD88C),
        syn_number: c(0xF5B57A),
        syn_comment: c(0x7F8A99),
        syn_fn: c(0x82C8FF),
        syn_type: c(0x6FD6E4),
        on_danger: c(0x0B0D14),
        disabled: c(0x8A8E94),
        tint: 0.14,
        scrim: ca(0x05070A, 0.5),
        shadow: ca(0x000000, 0.45),
        hover: ca(0xE9EDF2, 0.04),
        press: ca(0xE9EDF2, 0.08),
    }
}

/// The same board on warm paper: ink-dark text, the severity hues deepened to keep AA on white.
pub fn light() -> Colors {
    Colors {
        bg: c(0xF5F4F0),
        sidebar: c(0xECEAE4),
        surface: c(0xFFFFFF),
        raised: c(0xFBFAF7),
        selected: c(0xEEF0FF),
        border: c(0xE1DED6),
        border_strong: c(0xCCC7BC),
        text: c(0x1A1E24),
        muted: c(0x4F5866),
        faint: c(0x566070),
        accent: c(0x4247C8),
        accent_hover: c(0x383DB8),
        accent_press: c(0x2F33A2),
        on_accent: c(0xFFFFFF),
        running: c(0x07606F),
        ok: c(0x14603B),
        warn: c(0x7A5500),
        danger: c(0xB0202C),
        critical: c(0xB01640),
        high: c(0x8F4007),
        medium: c(0x735500),
        low: c(0x4A5870),
        add_bg: ca(0x4F8A62, 0.14),
        del_bg: ca(0xCF222E, 0.10),
        add_fg: c(0x16743A),
        del_fg: c(0xB3242E),
        code_bg: c(0xFAF9F6),
        syn_keyword: c(0x7A3DC8),
        syn_string: c(0x2B7A24),
        syn_number: c(0xA14F00),
        syn_comment: c(0x667080),
        syn_fn: c(0x0B5FAD),
        syn_type: c(0x0B6E82),
        on_danger: c(0xFFFFFF),
        disabled: c(0x63666A),
        tint: 0.12,
        scrim: ca(0x1A1E24, 0.4),
        shadow: ca(0x1A1E24, 0.16),
        hover: ca(0x1A1E24, 0.04),
        press: ca(0x1A1E24, 0.08),
    }
}

/// The alpha of `disabled` that outlines a control that can't act now: 3:1 on every surface, so the control reads
/// as a control.
pub const DISABLED_OUTLINE: f32 = 0.8;

impl Colors {
    pub fn severity(&self, s: Severity) -> Hsla {
        match s {
            Severity::Critical => self.critical,
            Severity::High => self.high,
            Severity::Medium => self.medium,
            Severity::Low | Severity::Unknown => self.low,
        }
    }

    /// A severity's hue washed into the surface: the card's tint and the anchored diff line.
    pub fn severity_wash(&self, s: Severity) -> Hsla {
        self.severity(s).alpha(0.12)
    }
}

/// A severity's shape and word, so color is never the only carrier: ◆ critical, ▲ high, ● medium, ○ low.
pub fn severity_mark(s: Severity) -> (&'static str, &'static str) {
    match s {
        Severity::Critical => ("◆", "Critical"),
        Severity::High => ("▲", "High"),
        Severity::Medium => ("●", "Medium"),
        Severity::Low => ("○", "Low"),
        Severity::Unknown => ("◇", "Unrated"),
    }
}

/// The type scale, in points, on macOS's 13-point body. The interface is set in the system sans; what is said (page
/// and PR titles, the quoted finding) in the system serif; paths, ids and code in Menlo.
pub mod text {
    use super::{Pixels, px};
    pub const UI: &str = ".SystemUIFont";
    pub const SERIF: &str = ".AppleSystemUIFontSerif";
    pub const MONO: &str = "Menlo";
    pub const CAPTION: Pixels = px(11.);
    pub const SMALL: Pixels = px(12.);
    pub const BODY: Pixels = px(13.);
    pub const LEAD: Pixels = px(15.);
    pub const TITLE: Pixels = px(18.);
    /// Serif: a PR or finding title in the detail.
    pub const DISPLAY: Pixels = px(24.);
    pub const CODE: Pixels = px(12.);
    pub const LINE_TIGHT: Pixels = px(16.);
    pub const LINE_BODY: Pixels = px(20.);
    pub const LINE_READ: Pixels = px(21.);
    pub const LINE_CODE: Pixels = px(19.);
    pub const LINE_DISPLAY: Pixels = px(30.);
}

/// The 4-point spacing grid: 16 between cards, 24 between detail sections.
pub mod space {
    use super::{Pixels, px};
    pub const S1: Pixels = px(4.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S5: Pixels = px(20.);
    pub const S6: Pixels = px(24.);
    pub const S8: Pixels = px(32.);
}

/// Two radii: 8 for controls, chips and rows; 12 for cards, code, sheets and the palette. Dots and count badges are
/// circles.
pub mod radius {
    use super::{Pixels, px};
    pub const SM: Pixels = px(8.);
    pub const LG: Pixels = px(12.);
    pub const PILL: Pixels = px(999.);
}

/// The window grid at 1280 points: title bar across the top, sidebar, queue column, and the detail takes the rest.
pub mod layout {
    use super::{Pixels, px};
    pub const SIDEBAR: Pixels = px(232.);
    pub const QUEUE: Pixels = px(400.);
    pub const TOOLBAR: Pixels = px(52.);
    pub const STRIP: Pixels = px(44.);
    /// The pinned action bar under a page.
    pub const BAR: Pixels = px(56.);
    /// Reading measure for prose: 68 characters at 13 points.
    pub const MEASURE: Pixels = px(440.);
    pub const PALETTE: Pixels = px(600.);
    pub const SHEET: Pixels = px(460.);
    /// Code never clips. A code block scrolls sideways, and a line longer than this many characters earns the
    /// fade at its right edge; the diff soft-wraps at a word or operator boundary instead, so a finding's card can
    /// sit between its rows. These are the columns each surface fits at 1280 points. ponytail: fixed counts
    /// instead of measuring the pane, so a narrower window fades late; measure the pane if the window gets
    /// resizable.
    pub const COLS_EVIDENCE: usize = 60;
    /// A code block inside the reading column.
    pub const COLS_PROSE: usize = 48;
    pub const COLS_DIFF: usize = 84;
    pub const COLS_WIDE: usize = 108;
    /// The inset after the traffic lights, which end 70 points in: the title bar's content starts 20 points later.
    pub const TRAFFIC_INSET: Pixels = px(90.);
}

/// Motion: short, eased, and only where it says something changed. Reduce Motion turns every one off.
pub mod motion {
    use super::Duration;
    /// The live dot's breath: a review is working.
    pub const PULSE: Duration = Duration::from_millis(1600);
    /// A row that just landed fades in over this.
    pub const FADE: Duration = Duration::from_millis(150);
    /// How long a finding that just landed stays lit, in model milliseconds.
    pub const ARRIVAL_MS: u64 = 2400;
}

#[cfg(test)]
mod tests {
    use super::{Colors, DISABLED_OUTLINE, dark, light};
    use gpui_kit::Hsla;

    fn luminance(c: Hsla) -> f64 {
        let rgba = c.to_rgb();
        let lin = |v: f32| {
            let v = v as f64;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(rgba.r) + 0.7152 * lin(rgba.g) + 0.0722 * lin(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `fg` at `alpha` over `bg`, as the screen shows it.
    fn over(fg: Hsla, alpha: f32, bg: Hsla) -> Hsla {
        let (f, b) = (fg.to_rgb(), bg.to_rgb());
        let mix = |x: f32, y: f32| x * alpha + y * (1. - alpha);
        gpui_kit::Rgba { r: mix(f.r, b.r), g: mix(f.g, b.g), b: mix(f.b, b.b), a: 1. }.into()
    }

    #[test]
    fn every_text_token_meets_aa_on_every_surface() {
        let mut bad: Vec<String> = Vec::new();
        for (name, t) in [("dark", dark()), ("light", light())] {
            let Colors { bg, surface, selected, sidebar, raised, code_bg, .. } = t;
            let texts = [
                ("text", t.text),
                ("muted", t.muted),
                ("faint", t.faint),
                ("accent", t.accent),
                ("running", t.running),
                ("ok", t.ok),
                ("warn", t.warn),
                ("danger", t.danger),
                ("critical", t.critical),
                ("high", t.high),
                ("medium", t.medium),
                ("low", t.low),
                ("add_fg", t.add_fg),
                ("del_fg", t.del_fg),
            ];
            let surfaces =
                [("bg", bg), ("surface", surface), ("selected", selected), ("sidebar", sidebar), ("raised", raised)];
            for (tn, fg) in texts {
                for (sn, back) in surfaces {
                    let r = contrast(fg, back);
                    if r < 4.5 {
                        bad.push(format!("{name}: {tn} on {sn} is {r:.2}:1"));
                    }
                }
            }
            let code = [
                ("text", t.text),
                ("faint", t.faint),
                ("keyword", t.syn_keyword),
                ("string", t.syn_string),
                ("number", t.syn_number),
                ("comment", t.syn_comment),
                ("fn", t.syn_fn),
                ("type", t.syn_type),
            ];
            for (tn, fg) in code {
                let r = contrast(fg, code_bg);
                if r < 4.5 {
                    bad.push(format!("{name}: syntax {tn} on code_bg is {r:.2}:1"));
                }
            }
            if contrast(t.on_accent, t.accent) < 4.5 {
                bad.push(format!("{name}: on_accent"));
            }
            if contrast(t.on_danger, t.danger) < 4.5 {
                bad.push(format!("{name}: on_danger"));
            }
            // A dimmed control or palette row keeps AA text; the outline of a disabled control keeps 3:1.
            for (sn, back) in surfaces {
                let r = contrast(t.disabled, back);
                if r < 4.5 {
                    bad.push(format!("{name}: disabled on {sn} is {r:.2}:1"));
                }
                let r = contrast(over(t.disabled, DISABLED_OUTLINE, back), back);
                if r < 3.0 {
                    bad.push(format!("{name}: disabled outline on {sn} is {r:.2}:1"));
                }
            }
            // A chip or a count badge: its hue as text over its own wash, on the surfaces that carry one.
            let hues = [
                ("accent", t.accent),
                ("running", t.running),
                ("ok", t.ok),
                ("warn", t.warn),
                ("danger", t.danger),
                ("critical", t.critical),
                ("high", t.high),
                ("medium", t.medium),
                ("low", t.low),
            ];
            for (hn, hue) in hues {
                for (sn, back) in [("surface", surface), ("sidebar", sidebar), ("selected", selected), ("bg", bg)] {
                    let r = contrast(hue, over(hue, t.tint, back));
                    if r < 4.5 {
                        bad.push(format!("{name}: {hn} chip on {sn} is {r:.2}:1"));
                    }
                }
            }
            // The gutter and the dim rows of a code block.
            if contrast(t.muted, code_bg) < 4.5 {
                bad.push(format!("{name}: muted on code_bg"));
            }
            if contrast(t.add_fg, over(t.add_bg, t.add_bg.a, code_bg)) < 4.5 {
                bad.push(format!("{name}: add_fg on add_bg"));
            }
            if contrast(t.del_fg, over(t.del_bg, t.del_bg.a, code_bg)) < 4.5 {
                bad.push(format!("{name}: del_fg on del_bg"));
            }
            if contrast(t.text, over(t.add_bg, t.add_bg.a, code_bg)) < 4.5 {
                bad.push(format!("{name}: text on add_bg"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
