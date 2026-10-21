//! Width-aware text fitting. Widths come from `unicode-width` per grapheme, the way ratatui lays text out, so CJK and
//! emoji never spill past a cell.
//!
//! A row never looks at more than `CHARS_PER_COL` chars per column, so a hostile run of zero-width chars costs the same
//! as a short line.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CHARS_PER_COL: usize = 4;

/// `1 tool event`, `0 tool events`, `2 tool events`.
pub fn count(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// At most the first `chars` chars of `s`.
pub fn chars_prefix(s: &str, chars: usize) -> &str {
    s.char_indices().nth(chars).map_or(s, |(i, _)| &s[..i])
}

/// The byte length of the longest prefix of `s` at most `cols` wide, within the per-row char cap.
pub fn fit_len(s: &str, cols: usize) -> usize {
    let head = chars_prefix(s, cols.saturating_mul(CHARS_PER_COL));
    let mut used = 0;
    for (i, g) in head.grapheme_indices(true) {
        used += width(g);
        if used > cols {
            return i;
        }
    }
    head.len()
}

/// Skin-tone modifiers, which some terminals draw as a second wide cell.
fn is_skin_tone(c: char) -> bool {
    ('\u{1f3fb}'..='\u{1f3ff}').contains(&c)
}

/// Emoji sequences cut to their first component: `👩‍💻` shows as `👩`, `👍🏽` as `👍`. Terminals that draw each code
/// point on its own (tmux among them) would draw the whole sequence two or more cells wider than it measures.
pub fn plain_emoji(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.chars().any(|c| c == '\u{200d}' || is_skin_tone(c)) {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    for g in s.graphemes(true) {
        match g.find(|c: char| c == '\u{200d}' || is_skin_tone(c)) {
            Some(i) => out.push_str(&g[..i]),
            None => out.push_str(g),
        }
    }
    out.into()
}

/// One display line: newlines and tabs become spaces, emoji sequences their first component.
pub fn one_line(s: &str) -> String {
    plain_emoji(s).chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect()
}

/// At most `max` columns, ending in `ellipsis` when cut.
pub fn fit(s: &str, max: usize, ellipsis: &str) -> String {
    let head = chars_prefix(s, max.saturating_mul(CHARS_PER_COL));
    let line = one_line(head);
    if head.len() == s.len() && fit_len(&line, max) == line.len() {
        return line;
    }
    let mut out = line[..fit_len(&line, max.saturating_sub(width(ellipsis)))].to_string();
    if width(ellipsis) <= max {
        out.push_str(ellipsis);
    }
    out
}

/// Exactly `cols` columns: fitted, then right-padded.
pub fn pad(s: &str, cols: usize, ellipsis: &str) -> String {
    let mut out = fit(s, cols, ellipsis);
    let w = width(&out);
    out.extend(std::iter::repeat_n(' ', cols.saturating_sub(w)));
    out
}

/// Exactly `cols` columns, right-aligned.
pub fn pad_left(s: &str, cols: usize) -> String {
    let out = fit(s, cols, "");
    let w = width(&out);
    format!("{}{out}", " ".repeat(cols.saturating_sub(w)))
}

/// The most of `s` a `cols`-wide row can show, plus one char so `fit` still sees the cut.
pub fn row_prefix(s: &str, cols: usize) -> &str {
    chars_prefix(s, cols.saturating_mul(CHARS_PER_COL) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_never_overflow_the_budget() {
        assert_eq!(fit("修复竞态条件", 5, "…"), "修复…");
        assert_eq!(width(&fit("修复竞态条件", 5, "…")), 5);
        assert_eq!(pad("修复竞态", 4, "…"), "修… ");
        assert_eq!(fit("👩‍💻 dev", 3, "…"), "👩…");
        assert_eq!(fit("short", 10, "…"), "short");
        assert_eq!(fit("abc", 0, "…"), "");
        assert_eq!(pad_left("42", 5), "   42");
        assert_eq!(fit("a\nb\tc", 10, "…"), "a b c");
    }

    /// Columns as a terminal that draws each code point on its own (tmux, older xterms) lays `s` out.
    fn per_char_width(s: &str) -> usize {
        s.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)).sum()
    }

    #[test]
    fn zwj_sequences_and_skin_tones_fit_in_terminals_that_draw_each_code_point() {
        for s in ["👩‍💻 dev", "👍🏽 ok", "🏳️‍🌈 pride", "🇺🇸 flag", "a👨‍👩‍👧b"] {
            for cols in 1..10 {
                let out = fit(s, cols, "…");
                assert!(per_char_width(&out) <= cols, "{s:?} at {cols}: {out:?}");
                assert!(per_char_width(&out) <= width(&out), "{s:?} at {cols}: {out:?} draws wider than measured");
            }
        }
        assert_eq!(one_line("👩‍💻 👍🏽 🏳️‍🌈 🇺🇸"), "👩 👍 🏳️ 🇺🇸");
        assert_eq!(fit("👩‍💻 dev", 10, "…"), "👩 dev");
    }

    #[test]
    fn a_long_zero_width_run_fits_without_rescanning() {
        let line = format!("abcdefghij{}xyz", "\u{200b}".repeat(100_000));
        assert!(line.len() > 300_000);
        let start = std::time::Instant::now();
        let out = fit(&line, 12, "…");
        let took = start.elapsed();
        assert!(took < std::time::Duration::from_millis(50), "fit took {took:?}");
        assert_eq!(out.replace('\u{200b}', ""), "abcdefghij…");
        assert_eq!(fit(&line, 80, "…").replace('\u{200b}', ""), "abcdefghij…");
    }

    #[test]
    fn a_count_takes_the_singular_only_for_one() {
        let n = |k: u64| count(k, "match", "matches");
        assert_eq!([n(0), n(1), n(2)], ["0 matches", "1 match", "2 matches"]);
    }
}
