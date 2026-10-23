//! Design tokens: the themes, the type scale, the 4-point grid, two radii and motion. Every view reads its look
//! from here; DESIGN.md explains the choices.

use std::time::Duration;

use gpui_kit::component::ThemeColor;
use gpui_kit::{Hsla, Pixels, px, rgb};
use pr_review_tui::protocol::Severity;

/// A theme is data: the id the config stores, the name the picker shows, whether gpui-kit draws it dark, and its
/// palette. Adding one is a row in `THEMES`; the picker, the menu, the palette and the contrast test all read it.
#[derive(Debug)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub colors: fn() -> Colors,
}

pub static THEMES: [Theme; 5] = [
    Theme { id: "light", name: "Light", dark: false, colors: light },
    Theme { id: "dark", name: "Dark", dark: true, colors: dark },
    Theme { id: "solarized-light", name: "Solarized Light", dark: false, colors: solarized_light },
    Theme { id: "solarized-dark", name: "Solarized Dark", dark: true, colors: solarized_dark },
    Theme { id: "nord", name: "Nord", dark: true, colors: nord },
];

/// What the reader picked: a theme, or System, which is Light or Dark as macOS is.
#[derive(Clone, Copy, Debug)]
pub enum Choice {
    System,
    Fixed(&'static Theme),
}

impl PartialEq for Choice {
    fn eq(&self, other: &Choice) -> bool {
        self.id() == other.id()
    }
}

impl Choice {
    /// Every choice in the picker's order: System first, then the themes.
    pub fn all() -> impl Iterator<Item = Choice> {
        std::iter::once(Choice::System).chain(THEMES.iter().map(Choice::Fixed))
    }

    pub fn parse(id: &str) -> Option<Choice> {
        Choice::all().find(|c| c.id() == id)
    }

    pub fn id(self) -> &'static str {
        match self {
            Choice::System => "system",
            Choice::Fixed(t) => t.id,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Choice::System => "System",
            Choice::Fixed(t) => t.name,
        }
    }

    /// The theme it paints with, given whether macOS is dark now.
    pub fn theme(self, system_dark: bool) -> &'static Theme {
        match self {
            Choice::Fixed(t) => t,
            Choice::System => &THEMES[usize::from(system_dark)],
        }
    }
}

/// One theme's palette. Every text token meets WCAG AA (4.5:1) on every surface it sits on; `tests` checks it.
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
    /// A sheet's or the palette's outline: stronger than `border_strong`, so it holds over the scrim.
    pub sheet_edge: Hsla,
    /// The hover wash over any surface: 4 % of the text color.
    pub hover: Hsla,
    /// The pressed wash: 8 %.
    pub press: Hsla,
    /// The keyboard focus ring when the macOS accent can't be used: 3:1 on every surface.
    pub focus: Hsla,
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
        faint: c(0x929DAD),
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
        shadow: ca(0x000000, 0.6),
        sheet_edge: c(0x4A5363),
        hover: ca(0xE9EDF2, 0.04),
        press: ca(0xE9EDF2, 0.08),
        focus: c(0x8E95FF),
    }
}

/// The same board on warm paper: ink-dark text, the severity hues deepened to keep AA on white.
pub fn light() -> Colors {
    Colors {
        bg: c(0xF5F4F0),
        sidebar: c(0xECEAE4),
        surface: c(0xFFFFFF),
        raised: c(0xFBFAF7),
        selected: c(0xE9E7E0),
        border: c(0xE1DED6),
        border_strong: c(0xCCC7BC),
        text: c(0x1A1E24),
        muted: c(0x4F5866),
        faint: c(0x525B6A),
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
        sheet_edge: c(0xB9B3A6),
        hover: ca(0x1A1E24, 0.04),
        press: ca(0x1A1E24, 0.08),
        focus: c(0x4247C8),
    }
}

// Solarized (Ethan Schoonover): base03 #002B36, base02 #073642, base01 #586E75, base00 #657B83, base0 #839496,
// base1 #93A1A1, base2 #EEE8D5, base3 #FDF6E3; yellow #B58900, orange #CB4B16, red #DC322F, magenta #D33682,
// violet #6C71C4, blue #268BD2, cyan #2AA198, green #859900. A token marked "from X" is X with its Lab lightness
// moved, hue and chroma kept, until it passes the contrast test; DESIGN.md lists each one.

/// Solarized's dark mode: base03 paper, base02 for the selection, the accents lifted to read on it.
pub fn solarized_dark() -> Colors {
    Colors {
        bg: c(0x002B36),
        sidebar: c(0x00232D),
        surface: c(0x042F3A),
        raised: c(0x022D38),
        selected: c(0x073642),
        border: c(0x0B3D4A),
        border_strong: c(0x1A4F5C),
        text: c(0xEEE8D5),
        muted: c(0x9EACAC),  // from base1
        faint: c(0x9BACAE),  // from base0
        accent: c(0x73B2F9), // from blue
        accent_hover: c(0x8CC0FA),
        accent_press: c(0x5FA2EC),
        on_accent: c(0x002B36),
        running: c(0x5ABEB5),  // from cyan
        ok: c(0xA2B636),       // from green
        warn: c(0xD0A432),     // from yellow
        danger: c(0xFE8571),   // from red
        critical: c(0xFF7FA4), // from magenta
        high: c(0xFE875A),     // from orange
        medium: c(0xD0A432),   // from yellow
        low: c(0xA1A6F9),      // from violet
        add_bg: ca(0x859900, 0.13),
        del_bg: ca(0xDC322F, 0.13),
        add_fg: c(0x90A41B), // from green
        del_fg: c(0xFF6754), // from red
        code_bg: c(0x002B36),
        syn_keyword: c(0x859900),
        syn_string: c(0x2AA198),
        syn_number: c(0xF15391),  // from magenta
        syn_comment: c(0x7B9198), // from base01
        syn_fn: c(0x3793DA),      // from blue
        syn_type: c(0xB58900),
        on_danger: c(0x002B36),
        disabled: c(0x8B9C9E), // from base0
        tint: 0.14,
        scrim: ca(0x00141A, 0.5),
        shadow: ca(0x000000, 0.6),
        sheet_edge: c(0x586E75),
        hover: ca(0xEEE8D5, 0.04),
        press: ca(0xEEE8D5, 0.08),
        focus: c(0x268BD2),
    }
}

/// Solarized's light mode: base3 paper, base2 for the selection, base02 ink, the accents deepened to read on it.
pub fn solarized_light() -> Colors {
    Colors {
        bg: c(0xFDF6E3),
        sidebar: c(0xF5EEDA),
        surface: c(0xFFFCF4),
        raised: c(0xFEF9EC),
        selected: c(0xEEE8D5),
        border: c(0xE6DFCA),
        border_strong: c(0xD3CBB2),
        text: c(0x073642),
        muted: c(0x495F66),  // from base01
        faint: c(0x495F67),  // from base00
        accent: c(0x026391), // from blue
        accent_hover: c(0x02557D),
        accent_press: c(0x01476A),
        on_accent: c(0xFDF6E3),
        running: c(0x026960),  // from cyan
        ok: c(0x516501),       // from green
        warn: c(0x745A00),     // from yellow
        danger: c(0xB8081D),   // from red
        critical: c(0xB1026A), // from magenta
        high: c(0xA53801),     // from orange
        medium: c(0x745A00),   // from yellow
        low: c(0x5055A8),      // from violet
        add_bg: ca(0x859900, 0.14),
        del_bg: ca(0xDC322F, 0.10),
        add_fg: c(0x5C7001), // from green
        del_fg: c(0xCA2026), // from red
        code_bg: c(0xFDF6E3),
        syn_keyword: c(0x657900), // from green
        syn_string: c(0x007F76),  // from cyan
        syn_number: c(0xCD307F),  // from magenta
        syn_comment: c(0x667474), // from base1
        syn_fn: c(0x0677B5),      // from blue
        syn_type: c(0x886E02),    // from yellow
        on_danger: c(0xFDF6E3),
        disabled: c(0x566C74), // from base00
        tint: 0.12,
        scrim: ca(0x073642, 0.4),
        shadow: ca(0x073642, 0.16),
        sheet_edge: c(0x93A1A1),
        hover: ca(0x073642, 0.04),
        press: ca(0x073642, 0.08),
        focus: c(0x268BD2),
    }
}

// Nord (Arctic Ice Studio): Polar Night nord0 #2E3440, nord1 #3B4252, nord2 #434C5E, nord3 #4C566A; Snow Storm
// nord4 #D8DEE9, nord5 #E5E9F0, nord6 #ECEFF4; Frost nord7 #8FBCBB, nord8 #88C0D0, nord9 #81A1C1, nord10 #5E81AC;
// Aurora nord11 #BF616A, nord12 #D08770, nord13 #EBCB8B, nord14 #A3BE8C, nord15 #B48EAD.

/// Nord: Polar Night surfaces, Snow Storm ink, Frost for the accent and Aurora for meaning. The selection is nord1,
/// not nord2: on nord2 no red keeps AA without turning pink.
pub fn nord() -> Colors {
    Colors {
        bg: c(0x2E3440),
        sidebar: c(0x292E39),
        surface: c(0x353B49),
        raised: c(0x323845),
        selected: c(0x3B4252),
        border: c(0x434C5E),
        border_strong: c(0x4C566A),
        text: c(0xECEFF4),
        muted: c(0xD8DEE9),
        faint: c(0xB9C1CF),
        accent: c(0x90C8D8), // from nord8
        accent_hover: c(0xA3D4E2),
        accent_press: c(0x82B6C6),
        on_accent: c(0x2E3440),
        running: c(0x9BC8C7), // from nord7
        ok: c(0xADC896),      // from nord14
        warn: c(0xEBCB8B),
        danger: c(0xFFA6AF),   // from nord11
        critical: c(0xFFA5BF), // from nord11, turned toward nord15 so it stays apart from high
        high: c(0xF6AD96),     // from nord12
        medium: c(0xEBCB8B),
        low: c(0xA3C3E3), // from nord9
        add_bg: ca(0xA3BE8C, 0.13),
        del_bg: ca(0xBF616A, 0.13),
        add_fg: c(0xA3BE8C),
        del_fg: c(0xF1939C), // from nord11
        code_bg: c(0x2E3440),
        syn_keyword: c(0x81A1C1),
        syn_string: c(0xA3BE8C),
        syn_number: c(0xB690AF),  // from nord15
        syn_comment: c(0x8F9CB6), // from nord3
        syn_fn: c(0x88C0D0),
        syn_type: c(0x8FBCBB),
        on_danger: c(0x2E3440),
        disabled: c(0xA5AEBE),
        tint: 0.10,
        scrim: ca(0x1E222A, 0.5),
        shadow: ca(0x000000, 0.5),
        sheet_edge: c(0x616E88),
        hover: ca(0xECEFF4, 0.04),
        press: ca(0xECEFF4, 0.08),
        focus: c(0x88C0D0),
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

    /// gpui-kit's own component colors from this palette, so its scrollbars, tooltips and inputs match the page.
    pub fn paint_kit(&self, k: &mut ThemeColor) {
        k.background = self.bg;
        k.foreground = self.text;
        k.border = self.border;
        k.input = self.border_strong;
        k.caret = self.text;
        k.ring = self.focus;
        k.selection = self.accent.alpha(0.3);
        k.accent = self.selected;
        k.accent_foreground = self.text;
        k.muted = self.raised;
        k.muted_foreground = self.muted;
        k.popover = self.raised;
        k.popover_foreground = self.text;
        k.primary = self.accent;
        k.primary_hover = self.accent_hover;
        k.primary_active = self.accent_press;
        k.primary_foreground = self.on_accent;
        k.secondary = self.surface;
        k.secondary_foreground = self.text;
        k.danger = self.danger;
        k.danger_foreground = self.on_danger;
        k.success = self.ok;
        k.warning = self.warn;
        k.info = self.running;
        k.link = self.accent;
        k.list_active = self.selected;
        k.list_hover = self.hover;
        k.sidebar = self.sidebar;
        k.sidebar_foreground = self.text;
        k.sidebar_border = self.border;
        k.title_bar = self.sidebar;
        k.title_bar_border = self.border;
        k.skeleton = self.raised;
        k.scrollbar = self.bg.alpha(0.);
        k.scrollbar_thumb = self.faint.alpha(0.5);
        k.scrollbar_thumb_hover = self.faint.alpha(0.75);
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
    /// A finding's argument column: 360 points (55 characters), so the evidence beside it gets about 60 % of the
    /// page and the lines that fit the diff mostly fit there too.
    pub const ARGUMENT: Pixels = px(360.);
    /// Code never clips and never scrolls sideways: every block soft-wraps at these columns, the most Menlo 12
    /// (7.22 points a character) fits beside the gutter on each surface at 1280 points, less a few for safety.
    /// ponytail: fixed counts instead of measuring the pane, so a narrower window wraps late; measure the pane if
    /// the window gets resizable below 1280.
    pub const COLS_EVIDENCE: usize = 71;
    /// The finding's hunk, beside its sign column.
    pub const COLS_HUNK: usize = 68;
    /// A code block inside the reading column.
    pub const COLS_PROSE: usize = 38;
    pub const COLS_DIFF: usize = 84;
    /// A proof comment's context.
    pub const COLS_WIDE: usize = 80;
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

/// WCAG relative luminance.
fn luminance(c: Hsla) -> f64 {
    let rgba = c.to_rgb();
    let lin = |v: f32| {
        let v = v as f64;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(rgba.r) + 0.7152 * lin(rgba.g) + 0.0722 * lin(rgba.b)
}

/// The WCAG contrast ratio of two opaque colors.
pub fn contrast(a: Hsla, b: Hsla) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// A focus ring color holds 3:1 against the theme's window, as WCAG asks of a focus indicator.
pub fn ring_holds(c: Hsla, t: &Colors) -> bool {
    contrast(c, t.bg) >= 3.
}

#[cfg(test)]
mod tests {
    use super::{Choice, Colors, DISABLED_OUTLINE, THEMES, contrast};
    use gpui_kit::Hsla;

    /// `fg` at `alpha` over `bg`, as the screen shows it.
    fn over(fg: Hsla, alpha: f32, bg: Hsla) -> Hsla {
        let (f, b) = (fg.to_rgb(), bg.to_rgb());
        let mix = |x: f32, y: f32| x * alpha + y * (1. - alpha);
        gpui_kit::Rgba { r: mix(f.r, b.r), g: mix(f.g, b.g), b: mix(f.b, b.b), a: 1. }.into()
    }

    #[test]
    fn a_system_accent_rings_focus_only_where_it_holds_three_to_one() {
        let rgb = |r, g, b| gpui_kit::Rgba { r, g, b, a: 1. }.into();
        let (light, dark) = (super::light(), super::dark());
        assert!(super::ring_holds(rgb(0.0, 0.48, 1.0), &light), "macOS blue on the light window");
        assert!(super::ring_holds(rgb(0.0, 0.48, 1.0), &dark), "macOS blue on the dark window");
        assert!(!super::ring_holds(rgb(1.0, 0.78, 0.0), &light), "yellow fades into the light window");
    }

    #[test]
    fn a_choice_reads_back_from_the_id_the_config_stores() {
        let ids: Vec<&str> = Choice::all().map(Choice::id).collect();
        assert_eq!(ids, ["system", "light", "dark", "solarized-light", "solarized-dark", "nord"]);
        for c in Choice::all() {
            assert_eq!(Choice::parse(c.id()), Some(c));
        }
        assert_eq!(Choice::parse("solarized"), None);
        assert_eq!(Choice::System.theme(true).id, "dark");
        assert_eq!(Choice::System.theme(false).id, "light");
    }

    /// CIE L*a*b* of an opaque color, D65.
    fn lab(c: Hsla) -> [f64; 3] {
        let c = c.to_rgb();
        let lin = |v: f32| {
            let v = v as f64;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16. / 116. };
        [116. * f(y) - 16., 500. * (f(x) - f(y)), 200. * (f(y) - f(z))]
    }

    /// Severity is carried by shape and word too, but its hues must still read apart: every pair at least 20 apart
    /// in CIE76 ΔE, about where two swatches stop looking like one color with a shade of difference.
    #[test]
    fn every_theme_keeps_the_four_severities_apart() {
        let mut bad = Vec::new();
        for th in &THEMES {
            let t = (th.colors)();
            let sev = [("critical", t.critical), ("high", t.high), ("medium", t.medium), ("low", t.low)];
            for (i, (an, a)) in sev.iter().enumerate() {
                for (bn, b) in &sev[i + 1..] {
                    let (x, y) = (lab(*a), lab(*b));
                    let de = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt();
                    if de < 20. {
                        bad.push(format!("{}: {an} and {bn} are {de:.1} apart", th.id));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn every_text_token_meets_aa_on_every_surface() {
        let mut bad: Vec<String> = Vec::new();
        for th in &THEMES {
            let (name, t) = (th.id, (th.colors)());
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
            // Secondary text is set at 11 and 12 points, where antialiasing leaves only a stroke's core at the
            // token's own color: measured on the renders, the stroke reads at about 0.85 of the core's ratio. So
            // the core keeps 5.5:1, and the text the eye reads keeps 4.5:1.
            for (tn, fg) in [("muted", t.muted), ("faint", t.faint)] {
                for (sn, back) in surfaces {
                    let r = contrast(fg, back);
                    if r < 5.5 {
                        bad.push(format!("{name}: small {tn} on {sn} is {r:.2}:1, under 5.5"));
                    }
                }
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
                let r = contrast(t.focus, back);
                if r < 3.0 {
                    bad.push(format!("{name}: focus ring on {sn} is {r:.2}:1"));
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
