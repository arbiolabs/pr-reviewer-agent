//! Semantic roles and glyphs. Every status also carries a word or glyph, so `NO_COLOR` loses no meaning: it drops
//! colors and keeps modifiers (reverse video marks the selection, bold marks focus).

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

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
    sev_high: "^",
    sev_med: "*",
    sev_low: "o",
};

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub color: bool,
    pub ascii: bool,
}

impl Theme {
    pub fn style(&self, role: Role) -> Style {
        let modifier = match role {
            Role::Selected => Modifier::REVERSED,
            Role::Title | Role::Focus => Modifier::BOLD,
            _ => Modifier::empty(),
        };
        let base = Style::default().add_modifier(modifier);
        if !self.color {
            return base;
        }
        let fg = match role {
            Role::Muted | Role::Border => Color::DarkGray,
            Role::Ok | Role::Add => Color::Green,
            Role::Pending => Color::Yellow,
            Role::Fail | Role::Del => Color::Red,
            Role::Active => Color::Blue,
            Role::Hunk => Color::Cyan,
            Role::Focus => Color::White,
            Role::Normal | Role::Title | Role::Selected => return base,
        };
        base.fg(fg)
    }

    pub fn g(&self) -> &'static Glyphs {
        if self.ascii { &ASCII } else { &UNICODE }
    }

    pub fn border(&self) -> border::Set<'static> {
        if self.ascii { ASCII_BORDER } else { border::PLAIN }
    }

    pub fn severity(&self, s: Severity) -> (&'static str, &'static str, Role) {
        let g = self.g();
        match s {
            Severity::Critical => (g.sev_high, "CRIT", Role::Fail),
            Severity::High => (g.sev_high, "HIGH", Role::Fail),
            Severity::Medium => (g.sev_med, "MED", Role::Pending),
            Severity::Low => (g.sev_low, "LOW", Role::Muted),
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
