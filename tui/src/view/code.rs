//! Hand-rolled TS/Go colorizer and `code span` aware word wrap for the editorial views.

use ratatui::style::Modifier;
use ratatui::text::Span;

use super::Ctx;
use crate::text::{fit, row_prefix, width};
use crate::theme::{Bg, Role};

const KEYWORDS: &[&str] = &[
    "const",
    "let",
    "var",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "async",
    "await",
    "import",
    "export",
    "from",
    "new",
    "class",
    "extends",
    "interface",
    "type",
    "throw",
    "try",
    "catch",
    "finally",
    "of",
    "in",
    "typeof",
    "instanceof",
    "true",
    "false",
    "null",
    "undefined",
    "this",
    "switch",
    "case",
    "break",
    "continue",
    "default",
    "func",
    "package",
    "range",
    "go",
    "defer",
    "select",
    "chan",
    "map",
    "struct",
    "nil",
];

/// The part of `s` a `cols`-wide row can show, tabs as four spaces; the result is safe to measure.
pub fn expand(s: &str, cols: usize) -> String {
    row_prefix(s, cols).replace('\t', "    ").chars().filter(|c| !c.is_control()).collect()
}

/// Colored spans for one code line, cut to `w` columns, every span on `bg`.
pub fn highlight(c: &Ctx, line: &str, w: usize, bg: Bg) -> Vec<Span<'static>> {
    let line = fit(&expand(line, w), w, c.t.g().ellipsis);
    let on = |role: Role| c.s(role).patch(c.t.bg(bg));
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Span<'static>>| {
        if !plain.is_empty() {
            out.push(Span::styled(std::mem::take(plain), on(Role::Normal)));
        }
    };
    while i < chars.len() {
        let ch = chars[i];
        if ch == '/' && chars.get(i + 1) == Some(&'/') {
            flush(&mut plain, &mut out);
            let rest: String = chars[i..].iter().collect();
            out.push(Span::styled(rest, on(Role::Muted).add_modifier(Modifier::ITALIC)));
            break;
        }
        if ch == '"' || ch == '\'' || ch == '`' {
            flush(&mut plain, &mut out);
            let mut j = i + 1;
            while j < chars.len() && chars[j] != ch {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(chars.len());
            out.push(Span::styled(chars[i..j].iter().collect::<String>(), on(Role::Str)));
            i = j;
            continue;
        }
        if ch.is_ascii_digit() {
            flush(&mut plain, &mut out);
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
                j += 1;
            }
            out.push(Span::styled(chars[i..j].iter().collect::<String>(), on(Role::Num)));
            i = j;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' || ch == '$' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let role = if KEYWORDS.contains(&word.as_str()) {
                Some(Role::Keyword)
            } else if chars.get(j) == Some(&'(') {
                Some(Role::Accent)
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Some(Role::Type)
            } else {
                None
            };
            match role {
                Some(r) => {
                    flush(&mut plain, &mut out);
                    out.push(Span::styled(word, on(r)));
                }
                None => plain.push_str(&word),
            }
            i = j;
            continue;
        }
        plain.push(ch);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

/// Prose with `code spans`, word-wrapped to `w`; code spans become chips on the surface background.
pub fn rich(c: &Ctx, text: &str, w: usize, role: Role) -> Vec<Vec<Span<'static>>> {
    let code_style = c.s(Role::Type).patch(c.t.bg(Bg::Surface));
    let mut lines = Vec::new();
    for para in crate::text::plain_emoji(text).split('\n') {
        // Each token, and whether it touches the one before with no space between: `(` and a code span after it,
        // or a code span and the punctuation after it, stay one word, written exactly as the text has them.
        let mut tokens: Vec<(String, bool, bool)> = Vec::new();
        let mut open = false;
        for (n, seg) in para.split('`').enumerate() {
            if n % 2 == 1 {
                let chip = if c.t.color { seg.to_string() } else { format!("`{seg}`") };
                tokens.push((chip, true, open));
                open = true;
            } else {
                let parts: Vec<&str> = seg.split(' ').collect();
                let last = parts.len() - 1;
                for (k, part) in parts.into_iter().enumerate() {
                    if !part.is_empty() {
                        tokens.push((part.to_string(), false, k == 0 && open));
                    }
                    open = k == last && !part.is_empty();
                }
            }
        }
        let mut words: Vec<Vec<(String, bool)>> = Vec::new();
        for (tok, code, glue) in tokens {
            match words.last_mut() {
                Some(word) if glue => word.push((tok, code)),
                _ => words.push(vec![(tok, code)]),
            }
        }
        let mut line: Vec<Span<'static>> = Vec::new();
        let mut used = 0;
        for word in words {
            let ww: usize = word.iter().map(|(t, _)| width(t)).sum();
            if used > 0 && used + 1 + ww > w {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if used > 0 {
                line.push(Span::styled(" ", c.s(role)));
                used += 1;
            }
            used += ww;
            line.extend(word.into_iter().map(|(t, code)| Span::styled(t, if code { code_style } else { c.s(role) })));
        }
        lines.push(line);
    }
    lines
}

/// Splits one row of spans into rows at most `w` columns wide, keeping each piece's style. A row breaks after the
/// last space or comma that leaves it at least half full, else mid-token.
pub fn wrap_spans(spans: Vec<Span<'static>>, w: usize) -> Vec<Vec<Span<'static>>> {
    use unicode_segmentation::UnicodeSegmentation;
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let mut cuts = Vec::new();
    let (mut used, mut soft) = (0, None);
    for (i, g) in text.grapheme_indices(true) {
        let gw = width(g);
        if used + gw > w && used > 0 {
            let cut = soft.filter(|&(_, u)| u * 2 >= w).map_or(i, |(at, _)| at);
            cuts.push(cut);
            used = width(&text[cut..i]);
            soft = None;
        }
        used += gw;
        if g == " " || g == "," {
            soft = Some((i + g.len(), used));
        }
    }
    let mut rows = vec![Vec::new()];
    let mut at = 0;
    let mut next = cuts.into_iter().peekable();
    for span in spans {
        let mut rest: &str = &span.content;
        while let Some(&cut) = next.peek() {
            if cut >= at + rest.len() {
                break;
            }
            let (head, tail) = rest.split_at(cut - at);
            if !head.is_empty() {
                rows.last_mut().unwrap().push(Span::styled(head.to_string(), span.style));
            }
            rows.push(Vec::new());
            at = cut;
            rest = tail;
            next.next();
        }
        if !rest.is_empty() {
            rows.last_mut().unwrap().push(Span::styled(rest.to_string(), span.style));
        }
        at += rest.len();
    }
    rows
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;

    #[test]
    fn wrapped_rows_keep_every_char_and_never_pass_the_width() {
        let rows = super::wrap_spans(vec![Span::raw("const refund"), Span::raw(" = await pay(x);")], 10);
        let text: Vec<String> = rows.iter().map(|r| r.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(text, ["const ", "refund = ", "await ", "pay(x);"]);
        let rows = super::wrap_spans(vec![Span::raw("abcdefghijkl")], 5);
        assert_eq!(rows.len(), 3);
    }
}
