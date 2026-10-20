//! Width-aware text fitting. Widths come from `unicode-width`, the same table ratatui lays out with, so CJK and emoji
//! never spill past a cell.

use unicode_width::UnicodeWidthStr;

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// One display line: newlines and tabs become spaces.
pub fn one_line(s: &str) -> String {
    s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect()
}

/// At most `max` columns, ending in `ellipsis` when cut.
pub fn fit(s: &str, max: usize, ellipsis: &str) -> String {
    let s = one_line(s);
    if width(&s) <= max {
        return s;
    }
    let budget = max.saturating_sub(width(ellipsis));
    // Measured as a whole string, like ratatui does, so emoji ZWJ sequences count as one glyph. The loop stops at the
    // budget, so this stays O(columns^2) however long the input is.
    let mut out = String::new();
    for c in s.chars() {
        out.push(c);
        if width(&out) > budget {
            out.pop();
            break;
        }
    }
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

/// Code text: tabs expand to four spaces so columns stay aligned.
pub fn code(s: &str) -> String {
    one_line(&s.replace('\t', "    "))
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
}
