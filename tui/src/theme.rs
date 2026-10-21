//! Semantic roles and glyphs. Every status also carries a word or glyph, so `NO_COLOR` loses no meaning: it drops
//! colors and keeps modifiers (reverse video marks the selection, bold marks focus).

use ratatui::style::{Color, Modifier, Style};

use crate::protocol::{FindingStatus, Phase, PublicationStatus, RunStatus, Severity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Normal,
    Muted,
    Ok,
    Pending,
    Fail,
    Active,
    Title,
    Selected,
    Border,
    Focus,
    Add,
    Del,
    Hunk,
    Accent,
    Crit,
    High,
    Med,
    Low,
    Keyword,
    Str,
    Num,
    Type,
}

/// Backgrounds the views paint. `Base` fills the whole frame, so every pair in the palette test is a pair on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bg {
    Base,
    Surface,
    Anchor,
    Select,
    Add,
    Del,
}

type Table = &'static [(&'static str, u32, bool)];

/// "Ink": the editorial palette. Every foreground reaches 4.5:1 on every background (see the test below).
const INK: Table = &[
    // backgrounds
    ("base", 0x171717, true),
    ("surface", 0x1b1e25, true),
    ("anchor", 0x262b36, true),
    ("select", 0x232a3a, true),
    ("add", 0x16261d, true),
    ("del", 0x2a1a1f, true),
    // foregrounds
    ("text", 0xdfe3ea, false),
    ("muted", 0x9aa2b2, false),
    ("accent", 0x82aaff, false),
    ("ok", 0x8fd18a, false),
    ("warn", 0xe6b673, false),
    ("fail", 0xff7a8a, false),
    ("crit", 0xff6b7f, false),
    ("high", 0xffa066, false),
    ("med", 0xe6c36a, false),
    ("low", 0x7fc8e8, false),
    ("keyword", 0xc5a3ff, false),
    ("string", 0xa8d68a, false),
    ("number", 0xf2a97a, false),
    ("type", 0x7fd6c8, false),
];

/// "Paper": warm off-white with ink-dark text. Code sits on a darker stock, the selection is a pale blue wash, and
/// every hue is deepened until it reaches 4.5:1 on the darkest of those grounds.
const PAPER: Table = &[
    // backgrounds
    ("base", 0xfaf7f0, true),
    ("surface", 0xf2eee4, true),
    ("anchor", 0xe9e3d6, true),
    ("select", 0xe2e8f3, true),
    ("add", 0xe5f0df, true),
    ("del", 0xf7e3e0, true),
    // foregrounds
    ("text", 0x24221e, false),
    ("muted", 0x5c574e, false),
    ("accent", 0x2952a8, false),
    ("ok", 0x2a6a2c, false),
    ("warn", 0x7f5300, false),
    ("fail", 0xae2234, false),
    ("crit", 0x9c1028, false),
    ("high", 0x9e430a, false),
    ("med", 0x6f5800, false),
    ("low", 0x1b6283, false),
    ("keyword", 0x6c3aae, false),
    ("string", 0x3a6519, false),
    ("number", 0x98430e, false),
    ("type", 0x0d665e, false),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Palette {
    #[default]
    Dark,
    Light,
}

impl Palette {
    pub fn table(self) -> Table {
        match self {
            Palette::Dark => INK,
            Palette::Light => PAPER,
        }
    }

    pub fn hex(self, name: &str) -> u32 {
        self.table().iter().find(|p| p.0 == name).map_or(0xffffff, |p| p.1)
    }
}

/// WCAG relative luminance of an sRGB color with channels in 0..=1.
pub fn relative_luminance([r, g, b]: [f64; 3]) -> f64 {
    let ch = |c: f64| if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

fn truecolor() -> bool {
    static TC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TC.get_or_init(|| std::env::var("COLORTERM").is_ok_and(|v| v.contains("truecolor") || v.contains("24bit")))
}

/// Truecolor when the terminal says so, else the nearest xterm-256 cube or gray entry.
pub fn rgb(h: u32) -> Color {
    let (r, g, b) = ((h >> 16) as u8, (h >> 8) as u8, h as u8);
    if truecolor() {
        return Color::Rgb(r, g, b);
    }
    let q = |v: u8| if v < 48 { 0 } else if v < 115 { 1 } else { (v as u16 - 35) / 40 } as u8;
    let lvl = |i: u8| if i == 0 { 0 } else { 55 + 40 * i as i32 };
    let (qr, qg, qb) = (q(r), q(g), q(b));
    let dist = |a: (i32, i32, i32)| (a.0 - r as i32).pow(2) + (a.1 - g as i32).pow(2) + (a.2 - b as i32).pow(2);
    let cube = (lvl(qr), lvl(qg), lvl(qb));
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gi = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + 10 * gi;
    if dist((gv, gv, gv)) < dist(cube) {
        Color::Indexed(232 + gi as u8)
    } else {
        Color::Indexed(16 + 36 * qr + 6 * qg + qb)
    }
}

pub struct Glyphs {
    pub ok: &'static str,
    pub pending: &'static str,
    pub fail: &'static str,
    pub active: &'static str,
    pub idle: &'static str,
    pub cursor: &'static str,
    pub sep: &'static str,
    pub ellipsis: &'static str,
    pub arrow: &'static str,
    pub vbar: &'static str,
    pub sev_crit: &'static str,
    pub sev_high: &'static str,
    pub sev_med: &'static str,
    pub sev_low: &'static str,
}

pub fn glyphs(ascii: bool) -> &'static Glyphs {
    if ascii { &ASCII } else { &UNICODE }
}

const UNICODE: Glyphs = Glyphs {
    ok: "✓",
    pending: "◔",
    fail: "✗",
    active: "●",
    idle: "·",
    cursor: "›",
    sep: "·",
    ellipsis: "…",
    arrow: "→",
    vbar: "│",
    sev_crit: "◆",
    sev_high: "▲",
    sev_med: "●",
    sev_low: "○",
};

const ASCII: Glyphs = Glyphs {
    ok: "+",
    pending: "~",
    fail: "x",
    active: "*",
    idle: ".",
    cursor: ">",
    sep: "|",
    ellipsis: "~",
    arrow: "->",
    vbar: "|",
    sev_crit: "!",
    sev_high: "^",
    sev_med: "*",
    sev_low: "o",
};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub color: bool,
    pub ascii: bool,
    pub terminal_bg: bool,
    pub palette: Palette,
}

impl Theme {
    fn rgb(&self, name: &str) -> Color {
        rgb(self.palette.hex(name))
    }

    pub fn style(&self, role: Role) -> Style {
        let modifier = match role {
            Role::Selected if !self.color => Modifier::REVERSED,
            Role::Selected | Role::Title | Role::Focus | Role::Crit => Modifier::BOLD,
            _ => Modifier::empty(),
        };
        let base = Style::default().add_modifier(modifier);
        if !self.color {
            return base;
        }
        let name = match role {
            Role::Muted | Role::Border => "muted",
            Role::Ok | Role::Add => "ok",
            Role::Pending => "warn",
            Role::Fail | Role::Del => "fail",
            Role::Active | Role::Accent | Role::Focus => "accent",
            Role::Hunk | Role::Type => "type",
            Role::Crit => "crit",
            Role::High => "high",
            Role::Med => "med",
            Role::Low => "low",
            Role::Keyword => "keyword",
            Role::Str => "string",
            Role::Num => "number",
            Role::Normal | Role::Title => "text",
            Role::Selected => return base.fg(self.rgb("text")).bg(self.rgb("select")),
        };
        base.fg(self.rgb(name))
    }

    /// A background fill. Without color only the selection shows, in reverse video. With `terminal_bg`, `Base` is the
    /// terminal's own background.
    pub fn bg(&self, bg: Bg) -> Style {
        if !self.color {
            let reversed = if bg == Bg::Select { Modifier::REVERSED } else { Modifier::empty() };
            return Style::default().add_modifier(reversed);
        }
        if self.terminal_bg && bg == Bg::Base {
            return Style::default().bg(Color::Reset);
        }
        let name = match bg {
            Bg::Base => "base",
            Bg::Surface => "surface",
            Bg::Anchor => "anchor",
            Bg::Select => "select",
            Bg::Add => "add",
            Bg::Del => "del",
        };
        Style::default().bg(self.rgb(name))
    }

    pub fn g(&self) -> &'static Glyphs {
        glyphs(self.ascii)
    }

    pub fn severity(&self, s: Severity) -> (&'static str, &'static str, Role) {
        let g = self.g();
        match s {
            Severity::Critical => (g.sev_crit, "CRIT", Role::Crit),
            Severity::High => (g.sev_high, "HIGH", Role::High),
            Severity::Medium => (g.sev_med, "MED", Role::Med),
            Severity::Low => (g.sev_low, "LOW", Role::Low),
            Severity::Unknown => ("?", "UNKN", Role::Muted),
        }
    }

    pub fn run_status(&self, s: RunStatus) -> (&'static str, &'static str, Role) {
        let g = self.g();
        match s {
            RunStatus::Running => (g.active, "running", Role::Active),
            RunStatus::Completed => (g.ok, "completed", Role::Ok),
            RunStatus::Failed => (g.fail, "failed", Role::Fail),
            RunStatus::Superseded => (g.idle, "superseded", Role::Muted),
            RunStatus::Unknown => ("?", "unknown", Role::Muted),
        }
    }
}

pub fn finding_status(s: FindingStatus) -> (&'static str, Role) {
    match s {
        FindingStatus::Detected => ("detected", Role::Pending),
        FindingStatus::Published => ("published", Role::Active),
        FindingStatus::StillPresent => ("still present", Role::Fail),
        FindingStatus::PotentiallyResolved => ("maybe resolved", Role::Pending),
        FindingStatus::Outdated => ("outdated", Role::Muted),
        FindingStatus::Resolved => ("resolved", Role::Ok),
        FindingStatus::Uncertain => ("uncertain", Role::Pending),
        FindingStatus::Unknown => ("unknown", Role::Muted),
    }
}

/// A publication's status in words. Only `published` reads as done.
pub fn publication_status(s: PublicationStatus) -> (&'static str, Role) {
    match s {
        PublicationStatus::AwaitingApproval => ("waiting for your approval", Role::Pending),
        PublicationStatus::Publishing => ("publishing", Role::Active),
        PublicationStatus::Published => ("published", Role::Ok),
        PublicationStatus::Uncertain => ("uncertain: the daemon is checking GitHub", Role::Pending),
        PublicationStatus::Failed => ("failed", Role::Fail),
        PublicationStatus::Unknown => ("unknown", Role::Muted),
    }
}

pub fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::CollectContext => "COLLECT_CONTEXT",
        Phase::Triage => "TRIAGE",
        Phase::Analyze => "ANALYZE",
        Phase::Verify => "VERIFY",
        Phase::PreparePublication => "PREPARE_PUBLICATION",
        Phase::AwaitingApproval => "AWAITING_APPROVAL",
        Phase::Publish => "PUBLISH",
        Phase::Reconcile => "RECONCILE",
        Phase::Completed => "COMPLETED",
        Phase::Failed => "FAILED",
        Phase::Superseded => "SUPERSEDED",
        Phase::Unknown => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(h: u32) -> f64 {
        relative_luminance([16, 8, 0].map(|s| ((h >> s) & 0xff) as f64 / 255.0))
    }

    fn ratio(fg: u32, bg: u32) -> f64 {
        let (a, b) = (luminance(fg), luminance(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn fgs(p: Palette) -> impl Iterator<Item = &'static (&'static str, u32, bool)> {
        p.table().iter().filter(|e| !e.2)
    }

    /// The lowest-contrast foreground on `bg`.
    fn worst_on(p: Palette, bg: u32) -> (f64, &'static str) {
        fgs(p).map(|f| (ratio(f.1, bg), f.0)).fold((f64::MAX, ""), |a, b| if b.0 < a.0 { b } else { a })
    }

    #[test]
    fn every_foreground_reaches_wcag_aa_on_every_background_in_both_palettes() {
        for p in [Palette::Dark, Palette::Light] {
            assert_eq!(p.table().len(), Palette::Dark.table().len(), "{p:?} has every role");
            let mut report = Vec::new();
            let mut worst = (f64::MAX, String::new());
            for (fname, fg, _) in fgs(p) {
                for (bname, bg, _) in p.table().iter().filter(|e| e.2) {
                    let r = ratio(*fg, *bg);
                    if r < worst.0 {
                        worst = (r, format!("{fname} on {bname}"));
                    }
                    report.push(format!("{fname:>8} on {bname:<8} {r:5.2}"));
                    assert!(r >= 4.5, "{p:?}: {fname} on {bname}: {r:.2} < 4.5");
                }
            }
            println!(
                "{p:?}\n{}\n{p:?} worst pair {:.2}:1 ({}) over {} pairs",
                report.join("\n"),
                worst.0,
                worst.1,
                report.len()
            );
        }
    }

    /// `background = "terminal"` swaps `base` for the terminal's own background, which the client cannot see. Each
    /// palette must still reach 4.5:1 on the terminal backgrounds it is meant for: dark on dark terminals, light on
    /// white and light-grey ones. The mismatched pairs are reported, not asserted; `theme = "auto"` avoids them.
    #[test]
    fn with_the_terminal_background_each_palette_still_reaches_aa_on_its_own_kind_of_terminal() {
        let dark = [("black", 0x000000), ("dark gray", 0x1e1e1e), ("one dark", 0x282c34)];
        let light = [("white", 0xffffff), ("light gray", 0xeeeeee), ("solarized", 0xfdf6e3)];
        for (p, own, other) in [(Palette::Dark, dark, light), (Palette::Light, light, dark)] {
            for (name, bg) in own {
                let (worst, fname) = worst_on(p, bg);
                println!("{p:?} on a {name:<10} terminal worst {worst:5.2}:1 ({fname})");
                assert!(worst >= 4.5, "{p:?}: {fname} on a {name} terminal: {worst:.2} < 4.5");
            }
            let (name, bg) = other[0];
            let (worst, fname) = worst_on(p, bg);
            println!("{p:?} on a {name:<10} terminal worst {worst:5.2}:1 ({fname}): mismatched, not supported");
        }
        let t = Theme { color: true, ascii: false, terminal_bg: true, palette: Palette::Light };
        assert_eq!(t.bg(Bg::Base).bg, Some(Color::Reset));
        assert_eq!(t.bg(Bg::Surface).bg, Some(rgb(0xf2eee4)), "surfaces stay painted");
    }

    #[test]
    fn every_severity_has_its_own_glyph_in_both_glyph_sets() {
        for ascii in [false, true] {
            let t = Theme { color: false, ascii, terminal_bg: false, palette: Palette::Dark };
            let glyphs: Vec<&str> =
                [Severity::Critical, Severity::High, Severity::Medium, Severity::Low].map(|s| t.severity(s).0).to_vec();
            let expected = if ascii { vec!["!", "^", "*", "o"] } else { vec!["◆", "▲", "●", "○"] };
            assert_eq!(glyphs, expected, "ascii={ascii}");
        }
    }
}
