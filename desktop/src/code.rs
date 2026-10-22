//! A small lexer for cited code and diffs: keywords, strings, numbers, comments and calls. It colors one line at a
//! time, so a diff line never depends on the lines around it.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Str,
    Number,
    Comment,
    Call,
}

const KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "for",
    "from",
    "func",
    "function",
    "go",
    "if",
    "import",
    "in",
    "interface",
    "let",
    "match",
    "new",
    "nil",
    "null",
    "package",
    "pub",
    "range",
    "return",
    "select",
    "self",
    "static",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "undefined",
    "use",
    "var",
    "while",
    "fn",
    "impl",
    "mut",
    "class",
    "def",
    "of",
    "SELECT",
    "FROM",
    "WHERE",
    "CREATE",
    "TABLE",
    "PARTITION",
    "BY",
    "INSERT",
    "INTO",
    "VALUES",
    "AND",
    "OR",
    "NOT",
];

/// The colored ranges of `line`, in byte offsets. `#` starts a comment only in languages that use it.
pub fn tokens(line: &str, hash_comments: bool) -> Vec<(Range<usize>, Tok)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        let start = i;
        if (ch == b'/' && b.get(i + 1) == Some(&b'/')) || (ch == b'#' && hash_comments) || line[i..].starts_with("--") {
            out.push((i..b.len(), Tok::Comment));
            break;
        }
        if matches!(ch, b'"' | b'\'' | b'`') {
            i += 1;
            while i < b.len() && b[i] != ch {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            out.push((start..i, Tok::Str));
            continue;
        }
        if ch.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push((start..i, Tok::Number));
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == b'_' || ch == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let word = &line[start..i];
            if KEYWORDS.contains(&word) {
                out.push((start..i, Tok::Keyword));
            } else if b.get(i) == Some(&b'(') {
                out.push((start..i, Tok::Call));
            }
            continue;
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// Languages whose comments start with `#`.
pub fn hash_comments(path: &str) -> bool {
    matches!(path.rsplit('.').next(), Some("py" | "rb" | "sh" | "toml" | "yaml" | "yml"))
}

/// Inline markdown as GitHub shows it: `**bold**` and `` `code` `` spans, by byte range of the text with the markers
/// removed. Returns that text and its spans.
pub fn inline_markdown(src: &str) -> (String, Vec<(Range<usize>, Inline)>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        let next = [rest.find("**").map(|i| (i, "**", Inline::Bold)), rest.find('`').map(|i| (i, "`", Inline::Code))]
            .into_iter()
            .flatten()
            .min_by_key(|(i, _, _)| *i);
        let Some((i, marker, kind)) = next else {
            text.push_str(rest);
            break;
        };
        let after = &rest[i + marker.len()..];
        let Some(end) = after.find(marker) else {
            text.push_str(rest);
            break;
        };
        text.push_str(&rest[..i]);
        let s = text.len();
        text.push_str(&after[..end]);
        spans.push((s..text.len(), kind));
        rest = &after[end + marker.len()..];
    }
    (text, spans)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    Bold,
    Code,
}

#[cfg(test)]
mod tests {
    use super::{Inline, Tok, inline_markdown, tokens};

    #[test]
    fn a_line_of_typescript_lexes_into_its_parts() {
        let line = r#"  const order = await db.find("x"); // load"#;
        let got: Vec<(&str, Tok)> = tokens(line, false).into_iter().map(|(r, t)| (&line[r], t)).collect();
        assert_eq!(
            got,
            [
                ("const", Tok::Keyword),
                ("await", Tok::Keyword),
                ("find", Tok::Call),
                ("\"x\"", Tok::Str),
                ("// load", Tok::Comment)
            ]
        );
    }

    #[test]
    fn inline_markdown_drops_markers_and_keeps_spans() {
        let (text, spans) = inline_markdown("**High: x** in `a.ts:18` now");
        assert_eq!(text, "High: x in a.ts:18 now");
        assert_eq!(spans, [(0..7, Inline::Bold), (11..18, Inline::Code)]);
    }
}
