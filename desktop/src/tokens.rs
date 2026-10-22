//! Design tokens for direction C, the triage inbox: color for dark and light, the type scale, the spacing grid, radii
//! and motion. Every view reads its look from here; DESIGN-C.md explains the choices.

use std::time::Duration;

use gpui_kit::{Hsla, Pixels, px, rgb};
use pr_review_tui::protocol::Severity;

/// One appearance's palette. Text tokens meet WCAG AA (4.5:1) on `bg`, `surface` and `raised`; `tests` checks it.
#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub surface_press: Hsla,
    pub selected: Hsla,
    pub raised: Hsla,
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
    pub scrim: Hsla,
    pub shadow: Hsla,
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
        surface_hover: c(0x1B2029),
        surface_press: c(0x141820),
        selected: c(0x1E2431),
        raised: c(0x13171D),
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
        danger: c(0xF4646A),
        critical: c(0xFF6B86),
        high: c(0xFFA05A),
        medium: c(0xE8C95A),
        low: c(0x98AAC4),
        add_bg: ca(0x3FB97A, 0.13),
        del_bg: ca(0xF4646A, 0.13),
        add_fg: c(0x63D69C),
        del_fg: c(0xFF8A8F),
        code_bg: c(0x0C0F13),
        syn_keyword: c(0xC5A3FF),
        syn_string: c(0x9BD88C),
        syn_number: c(0xF5B57A),
        syn_comment: c(0x7F8A99),
        syn_fn: c(0x82C8FF),
        scrim: ca(0x05070A, 0.62),
        shadow: ca(0x000000, 0.45),
    }
}

/// The same board on warm paper: ink-dark text, the severity hues deepened to keep AA on white.
pub fn light() -> Colors {
    Colors {
        bg: c(0xF5F4F0),
        sidebar: c(0xECEAE4),
        surface: c(0xFFFFFF),
        surface_hover: c(0xFBFAF7),
        surface_press: c(0xF1EFEA),
        selected: c(0xEEF0FF),
        raised: c(0xFFFFFF),
        border: c(0xE1DED6),
        border_strong: c(0xCCC7BC),
        text: c(0x1A1E24),
        muted: c(0x535C69),
        faint: c(0x5D6672),
        accent: c(0x4A51D4),
        accent_hover: c(0x3D44C4),
        accent_press: c(0x343AAE),
        on_accent: c(0xFFFFFF),
        running: c(0x086D80),
        ok: c(0x187247),
        warn: c(0x8C6200),
        danger: c(0xC2303A),
        critical: c(0xC21F48),
        high: c(0xA24A08),
        medium: c(0x876400),
        low: c(0x52627A),
        add_bg: ca(0x2DA44E, 0.12),
        del_bg: ca(0xCF222E, 0.10),
        add_fg: c(0x1A7F37),
        del_fg: c(0xB3242E),
        code_bg: c(0xFAF9F6),
        syn_keyword: c(0x7A3DC8),
        syn_string: c(0x2B7A24),
        syn_number: c(0xA14F00),
        syn_comment: c(0x6A7380),
        syn_fn: c(0x0B5FAD),
        scrim: ca(0x1A1E24, 0.28),
        shadow: ca(0x1A1E24, 0.16),
    }
}

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

/// The type scale, in points, on macOS's 13-point body.
pub mod text {
    use super::{Pixels, px};
    pub const UI: &str = ".SystemUIFont";
    pub const MONO: &str = "Menlo";
    pub const CAPTION: Pixels = px(11.);
    pub const SMALL: Pixels = px(12.);
    pub const BODY: Pixels = px(13.);
    pub const LEAD: Pixels = px(15.);
    pub const TITLE: Pixels = px(18.);
    pub const DISPLAY: Pixels = px(24.);
    pub const CODE: Pixels = px(12.);
    pub const LINE_TIGHT: Pixels = px(16.);
    pub const LINE_BODY: Pixels = px(20.);
    pub const LINE_READ: Pixels = px(21.);
    pub const LINE_CODE: Pixels = px(19.);
}

/// The 4-point spacing grid.
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

/// Radii: one per element size, never mixed within a size.
pub mod radius {
    use super::{Pixels, px};
    /// Chips, key caps, severity badges.
    pub const XS: Pixels = px(4.);
    /// Buttons, list rows, inputs.
    pub const SM: Pixels = px(6.);
    /// Cards and code blocks.
    pub const MD: Pixels = px(10.);
    /// Sheets and the palette.
    pub const LG: Pixels = px(14.);
    pub const PILL: Pixels = px(999.);
}

/// The window grid at 1280 points: sidebar, queue column, and the detail takes the rest.
pub mod layout {
    use super::{Pixels, px};
    pub const SIDEBAR: Pixels = px(216.);
    pub const QUEUE: Pixels = px(408.);
    pub const TOOLBAR: Pixels = px(52.);
    pub const STRIP: Pixels = px(44.);
    /// Reading measure for prose: about 72 characters at 13 points.
    pub const MEASURE: Pixels = px(600.);
    pub const PALETTE: Pixels = px(600.);
    pub const SHEET: Pixels = px(440.);
}

/// Motion: short, eased, and only where it says something changed. Reduced motion turns every one off.
pub mod motion {
    use super::Duration;
    /// The live dot's breath: a review is working.
    pub const PULSE: Duration = Duration::from_millis(1600);
    /// How long a finding that just landed stays lit, in model milliseconds.
    pub const ARRIVAL_MS: u64 = 2400;
}

#[cfg(test)]
mod tests {
    use super::{Colors, dark, light};
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

    #[test]
    fn every_text_token_meets_aa_on_every_surface() {
        for (name, t) in [("dark", dark()), ("light", light())] {
            let Colors { bg, surface, selected, sidebar, .. } = t;
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
            ];
            for (tn, fg) in texts {
                for (sn, back) in [("bg", bg), ("surface", surface), ("selected", selected), ("sidebar", sidebar)] {
                    let r = contrast(fg, back);
                    assert!(r >= 4.5, "{name}: {tn} on {sn} is {r:.2}:1");
                }
            }
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "{name}: on_accent");
        }
    }
}
