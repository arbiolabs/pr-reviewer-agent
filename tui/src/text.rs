//! Width-aware text fitting. Widths come from `unicode-width` per grapheme, the way ratatui lays text out, so CJK and
//! emoji never spill past a cell.
//!
//! A row never looks at more than `CHARS_PER_COL` chars per column, so a hostile run of zero-width chars costs the same
//! as a short line.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CHARS_PER_COL: usize = 4;

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

/// One display line: newlines and tabs become spaces.
pub fn one_line(s: &str) -> String {
    s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect()
}

/// At most `max` columns, ending in `ellipsis` when cut.
pub fn fit(s: &str, max: usize, ellipsis: &str) -> String {
    let line = one_line(chars_prefix(s, max.saturating_mul(CHARS_PER_COL)));
    if line.len() == s.len() && fit_len(&line, max) == line.len() {
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
        assert_eq!(fit("👩‍💻 dev", 3, "…"), "👩‍💻…");
        assert_eq!(fit("short", 10, "…"), "short");
        assert_eq!(fit("abc", 0, "…"), "");
        assert_eq!(pad_left("42", 5), "   42");
        assert_eq!(fit("a\nb\tc", 10, "…"), "a b c");
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
}
