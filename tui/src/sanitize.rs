//! Terminal-safe text, matching `src/protocol/sanitize.ts` exactly (protocol/README.md, "Text safety").

const ESC: char = '\u{1b}';
const BEL: char = '\u{07}';
const ST: char = '\u{9c}';

fn is_string_introducer_c1(c: char) -> bool {
    matches!(c, '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}')
}

fn is_control(c: char) -> bool {
    matches!(c, '\u{00}'..='\u{08}' | '\u{0b}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Removes whole escape sequences (7-bit and 8-bit forms), then every C0/C1 control and DEL except `\n` and `\t`.
pub fn sanitize(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if (c == ESC && next == Some('[')) || c == '\u{9b}' {
            i += if c == ESC { 2 } else { 1 };
            while i < chars.len() && ('\u{30}'..='\u{3f}').contains(&chars[i]) {
                i += 1;
            }
            while i < chars.len() && ('\u{20}'..='\u{2f}').contains(&chars[i]) {
                i += 1;
            }
            if i < chars.len() && ('\u{40}'..='\u{7e}').contains(&chars[i]) {
                i += 1;
            }
        } else if (c == ESC && matches!(next, Some(']' | 'P' | 'X' | '^' | '_'))) || is_string_introducer_c1(c) {
            i += if c == ESC { 2 } else { 1 };
            while i < chars.len() && !matches!(chars[i], BEL | ESC | ST) {
                i += 1;
            }
            match (chars.get(i), chars.get(i + 1)) {
                (Some(&BEL | &ST), _) => i += 1,
                (Some(&ESC), Some('\\')) => i += 2,
                _ => {}
            }
        } else if c == ESC {
            i += 1;
            while i < chars.len() && ('\u{20}'..='\u{2f}').contains(&chars[i]) {
                i += 1;
            }
            if i < chars.len() && ('\u{30}'..='\u{7e}').contains(&chars[i]) {
                i += 1;
            }
        } else {
            if !is_control(c) {
                out.push(c);
            }
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    // The same cases as tests/protocol.test.ts, so both sides agree byte for byte.
    #[test]
    fn matches_the_typescript_reference() {
        assert_eq!(sanitize("\u{1b}[1;31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize("\u{1b}]8;;https://x\u{7}link\u{1b}]8;;\u{1b}\\"), "link");
        assert_eq!(sanitize("\u{1b}]52;c;ZXZpbA==\u{7}ok"), "ok");
        assert_eq!(sanitize("a\u{9b}2Jb\u{9d}0;title\u{9c}c"), "abc");
        assert_eq!(sanitize("x\u{90}q\u{9c}y\u{1b}cz\u{85}"), "xyz");
        assert_eq!(sanitize("tab\there\nnext\rline\u{0}\u{7f}"), "tab\there\nnextline");
        assert_eq!(sanitize("修复 🔥 👩‍💻"), "修复 🔥 👩‍💻");
    }

    #[test]
    fn unterminated_string_sequence_ends_at_next_escape() {
        assert_eq!(sanitize("a\u{1b}]0;title\u{1b}[1mb"), "ab");
        assert_eq!(sanitize("a\u{1b}]0;never ends"), "a");
    }
}
