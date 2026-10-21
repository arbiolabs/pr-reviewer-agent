//! Light-theme golden frames: the text, then which palette colors paint each cell. Its own test binary, so it can
//! force truecolor before the first frame and name every cell's color exactly.

mod common;

use std::collections::HashMap;

use common::*;
use pr_review_tui::model::Look;
use pr_review_tui::theme::{Palette, rgb};
use ratatui::style::Color;

/// The frame's text, a letter per cell for its (foreground, background) pair, and the key to the letters. A blank
/// cell shows only its background.
fn colored(s: &mut Sim, w: u16, h: u16) -> String {
    let (text, term) = render(&mut s.m, w, h);
    let names: HashMap<Color, &str> = Palette::Light.table().iter().map(|e| (rgb(e.1), e.0)).collect();
    let name = |c: Color| names.get(&c).copied().unwrap_or(if c == Color::Reset { "default" } else { "OTHER" });
    let buf = term.backend().buffer();
    let mut pairs: Vec<String> = Vec::new();
    let mut grid = String::new();
    for y in 0..h {
        for x in 0..w {
            let cell = &buf[(x, y)];
            let fg = if cell.symbol().trim().is_empty() { "-" } else { name(cell.fg) };
            let pair = format!("{fg} on {}", name(cell.bg));
            let i = pairs.iter().position(|p| *p == pair).unwrap_or_else(|| {
                pairs.push(pair);
                pairs.len() - 1
            });
            grid.push(grid_letter(i));
        }
        grid.push('\n');
    }
    let key: Vec<String> = pairs.iter().enumerate().map(|(i, p)| format!("{} {p}", grid_letter(i))).collect();
    format!("{text}\n{grid}\n{}\n", key.join("\n"))
}

fn grid_letter(i: usize) -> char {
    char::from(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"[i])
}

#[test]
fn light_review_diff_and_timeline_at_120x38() {
    // SAFETY: this binary's only test sets it before any frame reads it.
    unsafe { std::env::set_var("COLORTERM", "truecolor") };
    let light = Look { palette: Palette::Light, ..Look::default() };
    for (name, mut s) in states(light).into_iter().filter(|(n, _)| *n != "inbox") {
        let frame = colored(&mut s, 120, 38);
        assert!(!frame.contains("OTHER"), "{name}: a color outside the light palette:\n{frame}");
        insta::assert_snapshot!(format!("light_{name}_120x38"), frame);
    }
}
