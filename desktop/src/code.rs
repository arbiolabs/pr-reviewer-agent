//! A small lexer for cited code and diffs: keywords, strings, numbers, comments, calls and types. It colors one line
//! at a time but carries what the line left open, so a block comment or a template string that spans lines colors
//! every line it covers.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Str,
    Number,
    Comment,
    Call,
    Type,
}

/// What an earlier line left open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Open {
    #[default]
    Nothing,
    BlockComment,
    Template,
    /// A Python triple quote, by its quote character.
    Triple(u8),
}

/// Lexes the lines of one file in order.
#[derive(Clone, Copy, Debug)]
pub struct Lexer {
    /// `#` starts a comment.
    hash: bool,
    /// `--` starts a comment.
    dashes: bool,
    open: Open,
}

const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "chan",
    "class",
    "const",
    "continue",
    "def",
    "default",
    "defer",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "go",
    "if",
    "impl",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "match",
    "mod",
    "mut",
    "new",
    "nil",
    "null",
    "of",
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
    "typeof",
    "undefined",
    "use",
    "var",
    "where",
    "while",
    "None",
    "True",
    "False",
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

impl Lexer {
    pub fn for_path(path: &str) -> Lexer {
        let ext = path.rsplit('.').next().unwrap_or("");
        Lexer {
            hash: matches!(ext, "py" | "rb" | "sh" | "toml" | "yaml" | "yml"),
            dashes: matches!(ext, "sql" | "lua" | "hs"),
            open: Open::Nothing,
        }
    }

    /// The colored ranges of `line`, in byte offsets.
    pub fn line(&mut self, line: &str) -> Vec<(Range<usize>, Tok)> {
        let b = line.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        // The opener sits at `i` when this line opened the state, so the close search starts after it.
        let mut fresh = false;
        loop {
            // Close what an earlier line (or this one) opened, or color to the end and stay open.
            match self.open {
                Open::Nothing => {}
                Open::BlockComment => {
                    let start = i;
                    let skip = if fresh { 2 } else { 0 };
                    fresh = false;
                    match line[i + skip..].find("*/") {
                        Some(j) => {
                            i += skip + j + 2;
                            out.push((start..i, Tok::Comment));
                            self.open = Open::Nothing;
                        }
                        None => {
                            out.push((start..b.len(), Tok::Comment));
                            return out;
                        }
                    }
                }
                Open::Template => {
                    let start = i;
                    let mut j = i + usize::from(fresh);
                    fresh = false;
                    while j < b.len() && b[j] != b'`' {
                        j += if b[j] == b'\\' { 2 } else { 1 };
                    }
                    if j < b.len() {
                        i = j + 1;
                        out.push((start..i, Tok::Str));
                        self.open = Open::Nothing;
                    } else {
                        out.push((start..b.len(), Tok::Str));
                        return out;
                    }
                }
                Open::Triple(q) => {
                    let start = i;
                    let fence = [q; 3];
                    let fence = std::str::from_utf8(&fence).unwrap_or("\"\"\"");
                    let skip = if fresh { 3 } else { 0 };
                    fresh = false;
                    match line[i + skip..].find(fence) {
                        Some(j) => {
                            i += skip + j + 3;
                            out.push((start..i, Tok::Str));
                            self.open = Open::Nothing;
                        }
                        None => {
                            out.push((start..b.len(), Tok::Str));
                            return out;
                        }
                    }
                }
            }
            if i >= b.len() {
                return out;
            }
            let ch = b[i];
            let start = i;
            let next = b.get(i + 1).copied();
            if (ch == b'/' && next == Some(b'/'))
                || (ch == b'#' && self.hash)
                || (self.dashes && line[i..].starts_with("--"))
            {
                out.push((i..b.len(), Tok::Comment));
                return out;
            }
            if ch == b'/' && next == Some(b'*') {
                self.open = Open::BlockComment;
                fresh = true;
                continue;
            }
            if ch == b'`' {
                self.open = Open::Template;
                fresh = true;
                continue;
            }
            if self.hash && matches!(ch, b'"' | b'\'') && next == Some(ch) && b.get(i + 2) == Some(&ch) {
                self.open = Open::Triple(ch);
                fresh = true;
                continue;
            }
            if matches!(ch, b'"' | b'\'') {
                i += 1;
                while i < b.len() && b[i] != ch {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.push((start..i, Tok::Str));
                continue;
            }
            let word = |j: usize| j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'$');
            if ch.is_ascii_digit() && (i == 0 || !word(i - 1)) {
                while word(i) || (i < b.len() && b[i] == b'.') {
                    i += 1;
                }
                out.push((start..i, Tok::Number));
                continue;
            }
            if ch.is_ascii_alphabetic() || ch == b'_' || ch == b'$' {
                while word(i) {
                    i += 1;
                }
                let w = &line[start..i];
                if KEYWORDS.contains(&w) {
                    out.push((start..i, Tok::Keyword));
                } else if b.get(i) == Some(&b'(') {
                    out.push((start..i, Tok::Call));
                } else if w.starts_with(|c: char| c.is_ascii_uppercase()) && w.len() > 1 {
                    out.push((start..i, Tok::Type));
                }
                continue;
            }
            // Multi-byte characters advance whole, so every range stays on a char boundary.
            i += line[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

/// `line` cut into runs of at most `cols` characters, as byte ranges. The first run is the line; the rest continue
/// it. An empty line is one empty run.
pub fn wrap(line: &str, cols: usize) -> Vec<Range<usize>> {
    let cols = cols.max(8);
    let mut runs = Vec::new();
    let mut start = 0;
    let mut n = 0;
    for (i, _) in line.char_indices() {
        if n == cols {
            runs.push(start..i);
            start = i;
            n = 0;
        }
        n += 1;
    }
    runs.push(start..line.len());
    runs
}

/// The tokens of a whole line clipped to one of its runs, re-based on the run's start.
pub fn clip(tokens: &[(Range<usize>, Tok)], run: &Range<usize>) -> Vec<(Range<usize>, Tok)> {
    tokens
        .iter()
        .filter_map(|(r, t)| {
            let a = r.start.max(run.start);
            let b = r.end.min(run.end);
            (a < b).then(|| (a - run.start..b - run.start, *t))
        })
        .collect()
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
    use super::{Inline, Lexer, Tok, clip, inline_markdown, wrap};

    fn kinds<'a>(lx: &mut Lexer, line: &'a str) -> Vec<(&'a str, Tok)> {
        lx.line(line).into_iter().map(|(r, t)| (&line[r], t)).collect()
    }

    #[test]
    fn a_line_of_typescript_lexes_into_its_parts() {
        let mut lx = Lexer::for_path("a.ts");
        assert_eq!(
            kinds(&mut lx, r#"  const order = await db.find("x", 42); // load"#),
            [
                ("const", Tok::Keyword),
                ("await", Tok::Keyword),
                ("find", Tok::Call),
                ("\"x\"", Tok::Str),
                ("42", Tok::Number),
                ("// load", Tok::Comment)
            ]
        );
        assert_eq!(kinds(&mut lx, "x2 = RefundBody.parse(y)"), [("RefundBody", Tok::Type), ("parse", Tok::Call)]);
    }

    #[test]
    fn block_comments_and_template_strings_carry_across_lines() {
        let mut lx = Lexer::for_path("a.ts");
        assert_eq!(
            kinds(&mut lx, "const a = 1; /* start"),
            [("const", Tok::Keyword), ("1", Tok::Number), ("/* start", Tok::Comment)]
        );
        assert_eq!(kinds(&mut lx, "  still a comment"), [("  still a comment", Tok::Comment)]);
        assert_eq!(
            kinds(&mut lx, "end */ let b = `multi"),
            [("end */", Tok::Comment), ("let", Tok::Keyword), ("`multi", Tok::Str)]
        );
        assert_eq!(kinds(&mut lx, "  line ${x}` + f(y)"), [("  line ${x}`", Tok::Str), ("f", Tok::Call)]);
        assert_eq!(kinds(&mut lx, "f(1)"), [("f", Tok::Call), ("1", Tok::Number)]);
        assert_eq!(kinds(&mut lx, "const s = `open"), [("const", Tok::Keyword), ("`open", Tok::Str)]);
        assert_eq!(kinds(&mut lx, "`; done()"), [("`", Tok::Str), ("done", Tok::Call)]);
    }

    #[test]
    fn python_triple_quotes_and_hash_comments() {
        let mut lx = Lexer::for_path("a.py");
        assert_eq!(kinds(&mut lx, r#"x = """doc"#), [("\"\"\"doc", Tok::Str)]);
        assert_eq!(kinds(&mut lx, r#"more""" # note"#), [("more\"\"\"", Tok::Str), ("# note", Tok::Comment)]);
    }

    #[test]
    fn wrap_cuts_by_characters_and_clip_rebases_tokens() {
        let line = "abcdefghij klmnopqrstuvwxyz";
        assert_eq!(wrap(line, 10), [0..10, 10..20, 20..27]);
        assert_eq!(wrap("", 10), vec![0..0]);
        let toks = vec![(5..15, Tok::Str)];
        assert_eq!(clip(&toks, &(10..20)), [(0..5, Tok::Str)]);
        assert_eq!(clip(&toks, &(20..27)), []);
    }

    #[test]
    fn inline_markdown_drops_markers_and_keeps_spans() {
        let (text, spans) = inline_markdown("**High: x** in `a.ts:18` now");
        assert_eq!(text, "High: x in a.ts:18 now");
        assert_eq!(spans, [(0..7, Inline::Bold), (11..18, Inline::Code)]);
    }
}
