//! Semantic roles and glyphs. Every status also carries a word or glyph, so `NO_COLOR` loses no meaning: it drops
//! colors and keeps modifiers (reverse video marks the selection, bold marks focus).

use ratatui::style::{Color, Modifier, Style};

use crate::protocol::{FindingStatus, Phase, RunStatus, Severity};

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

/// "Ink": the editorial palette. Every foreground reaches 4.5:1 on every background (see the test below).
pub const PALETTE: &[(&str, u32, bool)] = &[
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

pub fn hex(name: &str) -> u32 {
    PALETTE.iter().find(|p| p.0 == name).map_or(0xffffff, |p| p.1)
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

const UNICODE: Glyphs = Glyphs {
    ok: "✓",
    pending: "◐",
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
}

impl Theme {
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
            Role::Selected => return base.fg(rgb(hex("text"))).bg(rgb(hex("select"))),
        };
        base.fg(rgb(hex(name)))
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
        Style::default().bg(rgb(hex(name)))
    }

    pub fn g(&self) -> &'static Glyphs {
        if self.ascii { &ASCII } else { &UNICODE }
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

pub fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::CollectContext => "COLLECT_CONTEXT",
        Phase::Triage => "TRIAGE",
        Phase::Analyze => "ANALYZE",
        Phase::Verify => "VERIFY",
        Phase::PreparePublication => "PREPARE_PUBLICATION",
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
        let ch = |s: u32| {
            let c = ((h >> s) & 0xff) as f64 / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    #[test]
    fn every_foreground_reaches_wcag_aa_on_every_background() {
        let mut report = Vec::new();
        let mut worst = f64::MAX;
        for (fname, fg, _) in PALETTE.iter().filter(|p| !p.2) {
            for (bname, bg, _) in PALETTE.iter().filter(|p| p.2) {
                let (a, b) = (luminance(*fg), luminance(*bg));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                worst = worst.min(ratio);
                report.push(format!("{fname:>8} on {bname:<8} {ratio:5.2}"));
                assert!(ratio >= 4.5, "{fname} on {bname}: {ratio:.2} < 4.5");
            }
        }
        println!("{}\nworst pair {worst:.2}:1 over {} pairs", report.join("\n"), report.len());
    }

    /// `background = "terminal"` swaps `base` for the terminal's own background, which the client cannot see. Every
    /// foreground must still reach 4.5:1 on the dark backgrounds terminals ship with. A light terminal is reported,
    /// not asserted: code blocks, the selection and diff lines stay painted dark in both modes, so the text has to be
    /// light, and light text cannot also reach 4.5:1 on white. The docs say `terminal` assumes a dark terminal.
    #[test]
    fn with_the_terminal_background_every_foreground_still_reaches_aa_on_a_dark_terminal() {
        let ratio = |fg: u32, bg: u32| {
            let (a, b) = (luminance(fg), luminance(bg));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        let fgs: Vec<_> = PALETTE.iter().filter(|p| !p.2).collect();
        for (name, bg) in [("black", 0x000000), ("dark gray", 0x1e1e1e), ("one dark", 0x282c34)] {
            let (worst, fname) =
                fgs.iter().map(|f| (ratio(f.1, bg), f.0)).fold((f64::MAX, ""), |a, b| if b.0 < a.0 { b } else { a });
            println!("terminal {name:<9} worst {worst:5.2}:1 ({fname})");
            assert!(worst >= 4.5, "{fname} on a {name} terminal: {worst:.2} < 4.5");
        }
        let (worst, fname) =
            fgs.iter().map(|f| (ratio(f.1, 0xffffff), f.0)).fold((f64::MAX, ""), |a, b| if b.0 < a.0 { b } else { a });
        println!("terminal white     worst {worst:5.2}:1 ({fname}): unsupported, see the doc comment");
        let t = Theme { color: true, ascii: false, terminal_bg: true };
        assert_eq!(t.bg(Bg::Base).bg, Some(Color::Reset));
        assert_eq!(t.bg(Bg::Surface).bg, Some(rgb(hex("surface"))), "surfaces stay painted");
    }

    #[test]
    fn every_severity_has_its_own_glyph_in_both_glyph_sets() {
        for ascii in [false, true] {
            let t = Theme { color: false, ascii, terminal_bg: false };
            let glyphs: Vec<&str> =
                [Severity::Critical, Severity::High, Severity::Medium, Severity::Low].map(|s| t.severity(s).0).to_vec();
            let expected = if ascii { vec!["!", "^", "*", "o"] } else { vec!["◆", "▲", "●", "○"] };
            assert_eq!(glyphs, expected, "ascii={ascii}");
        }
    }
}
