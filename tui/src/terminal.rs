//! Raw mode and the alternate screen, restored on drop and from the panic hook, so a crash never leaves the user's
//! shell unusable.

use std::io::{self, IsTerminal, Stdout, Write};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::theme::{Palette, relative_luminance};
use crate::update::Msg;

pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen, Show);
}

impl TerminalGuard {
    pub fn enter(mouse: bool) -> io::Result<TerminalGuard> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        enable_raw_mode()?;
        let guard = TerminalGuard { terminal: Terminal::new(CrosstermBackend::new(io::stdout()))? };
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        if mouse {
            execute!(io::stdout(), EnableMouseCapture)?;
        }
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// The palette for the terminal's background: its OSC 11 answer, else `COLORFGBG`, else dark. The query runs only
/// when stdin and stdout are both a terminal.
pub fn detect_palette() -> Palette {
    let asked = (io::stdin().is_terminal() && io::stdout().is_terminal())
        .then(|| query_background(Duration::from_millis(100)))
        .flatten();
    let from_env = || std::env::var("COLORFGBG").ok().map(|v| colorfgbg(&v));
    asked.and_then(|reply| parse_osc11(&reply)).map(palette_for).or_else(from_env).unwrap_or(Palette::Dark)
}

/// Keys typed while the query read the terminal's reply, handed to the input path once it starts.
static TYPED: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
/// The query gave up inside a reply: the rest of it is on its way to the key reader.
static REPLY_CUT: std::sync::Mutex<Option<Tail>> = std::sync::Mutex::new(None);

/// Which reply a cut left its tail in: the OSC 11 one, DA1, or none yet after a lone ESC, which a person may have
/// typed as Esc. That ESC lets through anything but a plain `]` or `[`, the only ways a reply cut there continues.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tail {
    Osc,
    Da1,
    Esc,
}

/// Sends OSC 11 and then DA1, which every terminal answers, so the DA1 reply marks the end of whatever the terminal
/// had to say. A terminal that answers neither costs `timeout`. It reads one byte at a time and stops at the DA1
/// answer, so input after it stays queued for the key reader; what the user typed before it is kept in `TYPED`.
/// A reply later than `timeout` reaches the key reader, which `LateReply` drops.
fn query_background(timeout: Duration) -> Option<Vec<u8>> {
    enable_raw_mode().ok()?;
    let reply = (|| {
        let mut out = io::stdout();
        out.write_all(b"\x1b]11;?\x07\x1b[c").ok()?;
        out.flush().ok()?;
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !ends_with_da1(&buf) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !readable(left) {
                break;
            }
            // Unbuffered: std's stdin would pull the whole reply into its own buffer, where select cannot see it.
            // SAFETY: reads at most one byte into `byte`.
            if unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) } != 1 {
                break;
            }
            buf.push(byte[0]);
        }
        Some(buf)
    })();
    let _ = disable_raw_mode();
    let reply = reply?;
    if let Ok(mut c) = REPLY_CUT.lock() {
        *c = cut(&reply);
    }
    let (answers, typed) = split_replies(&reply);
    if let Ok(mut t) = TYPED.lock() {
        *t = typed;
    }
    Some(answers)
}

/// The keys typed before the terminal answered, as key events: printable ASCII, Enter, Tab, Backspace and a bare Esc
/// at the end. An escape sequence (an arrow key's `ESC [ A`, Alt+x) drops whole, never as the plain keys inside it.
pub fn typed_before_start() -> Vec<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let typed = TYPED.lock().map(|mut t| std::mem::take(&mut *t)).unwrap_or_default();
    let mut plain = Vec::new();
    let mut bytes = typed.into_iter().peekable();
    while let Some(b) = bytes.next() {
        if b != 0x1b || bytes.peek().is_none() {
            plain.push(b);
            continue;
        }
        // A second ESC (Alt+arrow is `ESC ESC [ A`) stays queued and starts the sequence again.
        match bytes.next_if(|&b| b != 0x1b) {
            // CSI: parameter and intermediate bytes, then one final byte.
            Some(b'[') => {
                while bytes.next_if(|b| (0x20..0x40).contains(b)).is_some() {}
                bytes.next();
            }
            Some(b'O') => {
                bytes.next();
            }
            _ => {}
        }
    }
    plain
        .into_iter()
        .filter_map(|b| match b {
            b'\r' | b'\n' => Some(KeyCode::Enter),
            b'\t' => Some(KeyCode::Tab),
            0x7f | 0x08 => Some(KeyCode::Backspace),
            0x1b => Some(KeyCode::Esc),
            b' '..=b'~' => Some(KeyCode::Char(b as char)),
            _ => None,
        })
        .map(|code| KeyEvent::new(code, KeyModifiers::NONE))
        .collect()
}

/// Splits what stdin held into the terminal's answers (the OSC reply up to BEL or ST, and the DA1 reply) and
/// everything else, which the user typed.
pub fn split_replies(buf: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut answers, mut typed) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < buf.len() {
        let rest = &buf[i..];
        let len = if rest.starts_with(b"\x1b]") {
            let end = (2..rest.len()).find(|&k| rest[k] == 0x07 || (rest[k] == b'\\' && rest[k - 1] == 0x1b));
            end.map_or(rest.len(), |k| k + 1)
        } else if rest.starts_with(b"\x1b[?") {
            rest.iter().position(|&b| b == b'c').map_or(rest.len(), |k| k + 1)
        } else {
            0
        };
        if len == 0 {
            typed.push(buf[i]);
            i += 1;
        } else {
            answers.extend_from_slice(&rest[..len]);
            i += len;
        }
    }
    (answers, typed)
}

/// Drops a terminal reply that arrives after the query gave up: crossterm reads `ESC ]` as Alt+`]` and the reply's
/// text as keys, ended by BEL (Ctrl+G) or ST (Alt+`\\`). Only in the first `WINDOW` after startup, and only keys
/// that can belong to `11;rgb:RRRR/GGGG/BBBB` arriving within `BURST` of the Alt+`]`; any other key ends it and acts.
/// A tail the query's deadline cut off (no ESC of its own) drops the same way, DA1's `[?62;22c` included.
#[derive(Default)]
pub struct LateReply {
    /// When the reply's Alt+`]` came, how many keys followed it, and which reply they are in.
    open: Option<(Duration, usize, Tail)>,
}

impl LateReply {
    pub const WINDOW: Duration = Duration::from_secs(2);
    /// A reply arrives in one burst; a key this long after the Alt+`]` was typed by a person.
    const BURST: Duration = Duration::from_millis(200);
    const MAX: usize = 64;

    /// The filter for the key reader that starts after the query: already open when the query gave up inside a
    /// reply, so the tail that follows, which has no ESC of its own, is swallowed too.
    pub fn after_query() -> LateReply {
        let cut = REPLY_CUT.lock().ok().and_then(|mut c| c.take());
        LateReply { open: cut.map(|tail| (Duration::ZERO, 0, tail)) }
    }

    /// True when `key`, `since_start` into the session, is part of a late reply and must not act.
    pub fn swallow(&mut self, key: &crossterm::event::KeyEvent, since_start: Duration) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let alt = key.modifiers == KeyModifiers::ALT;
        // An expired filter is a closed one, so an Alt+] after it can still start a reply.
        let open =
            self.open.take().filter(|(began, n, _)| since_start.saturating_sub(*began) < Self::BURST && *n < Self::MAX);
        let Some((began, n, tail)) = open else {
            let starts = since_start < Self::WINDOW && alt && key.code == KeyCode::Char(']');
            self.open = starts.then_some((since_start, 0, Tail::Osc));
            return starts;
        };
        let KeyCode::Char(c) = key.code else { return false };
        let control = key.modifiers == KeyModifiers::CONTROL;
        if tail == Tail::Osc && (control && c == 'g' || alt && c == '\\') {
            return true;
        }
        // A whole reply after a cut that was only the user's Esc.
        if alt && c == ']' {
            self.open = Some((since_start, 0, Tail::Osc));
            return true;
        }
        if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
            return false;
        }
        // `ESC [ ? 62;22c`: DA1 ends at its `c`, and a plain `[` after a cut ESC starts it.
        let then = match (tail, c) {
            (Tail::Osc, '\\') | (Tail::Da1, 'c') => None,
            (Tail::Osc | Tail::Esc, '[') => Some(Tail::Da1),
            (Tail::Esc, ']') => Some(Tail::Osc),
            (Tail::Osc, c) if c.is_ascii_hexdigit() || "]rgb:/;".contains(c) => Some(Tail::Osc),
            (Tail::Da1, c) if c.is_ascii_digit() || "?;".contains(c) => Some(Tail::Da1),
            _ => return false,
        };
        self.open = then.map(|t| (began, n + 1, t));
        true
    }
}

/// What one terminal event means to the app, or `None` for a late reply's keys, pointer motion, focus and paste.
pub fn input(event: Event, late: &mut LateReply, since_start: Duration) -> Option<Msg> {
    match event {
        Event::Key(k) if late.swallow(&k, since_start) => None,
        Event::Key(k) => Some(Msg::Key(k)),
        // Motion events would wake the app on every cell the pointer crosses.
        Event::Mouse(e) if !matches!(e.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) => Some(Msg::Mouse(e)),
        Event::Resize(w, h) => Some(Msg::Resize(w, h)),
        _ => None,
    }
}

/// select(2), not poll(2): macOS poll reports `/dev/tty` as invalid, so a read after it would block.
fn readable(timeout: Duration) -> bool {
    // SAFETY: a zeroed fd_set is empty; select reads and writes only the set and the timeval passed here.
    unsafe {
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_SET(libc::STDIN_FILENO, &mut set);
        let mut tv = libc::timeval { tv_sec: timeout.as_secs() as _, tv_usec: timeout.subsec_micros() as _ };
        libc::select(libc::STDIN_FILENO + 1, &mut set, std::ptr::null_mut(), std::ptr::null_mut(), &mut tv) > 0
    }
}

/// Where the deadline left the query's buffer: inside the OSC reply, inside DA1 (`ESC [ ? 6`), just past an ESC, or
/// between replies.
fn cut(buf: &[u8]) -> Option<Tail> {
    let in_da1 = (buf.windows(3).rposition(|w| w == b"\x1b[?"))
        .is_some_and(|s| buf[s + 3..].iter().all(|b| b.is_ascii_digit() || *b == b';'));
    if inside_osc(buf) {
        Some(Tail::Osc)
    } else if in_da1 {
        Some(Tail::Da1)
    } else if buf.last() == Some(&0x1b) {
        Some(Tail::Esc)
    } else {
        None
    }
}

/// An `ESC ]` with no BEL or ST after it.
fn inside_osc(buf: &[u8]) -> bool {
    let Some(start) = buf.windows(2).rposition(|w| w == b"\x1b]") else { return false };
    let rest = &buf[start + 2..];
    !rest.contains(&0x07) && !rest.windows(2).any(|w| w == b"\x1b\\")
}

/// `ESC [ ? 6 2 ; 2 2 c`: a DA1 reply is `ESC [ ?`, digits and semicolons, then `c`.
fn ends_with_da1(buf: &[u8]) -> bool {
    let Some(start) = buf.windows(3).rposition(|w| w == b"\x1b[?") else { return false };
    let rest = &buf[start + 3..];
    rest.last() == Some(&b'c') && rest[..rest.len() - 1].iter().all(|b| b.is_ascii_digit() || *b == b';')
}

/// `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ended by BEL or ST, with 1 to 4 hex digits per channel, as channels in 0..=1.
pub fn parse_osc11(reply: &[u8]) -> Option<[f64; 3]> {
    let text = std::str::from_utf8(reply).ok()?;
    let body = &text[text.find("\x1b]11;rgb:")? + 9..];
    let body = &body[..body.find(['\x07', '\x1b'])?];
    let mut channels = body.split('/').map(|c| {
        let ok = (1..=4).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| u32::from_str_radix(c, 16).ok()).flatten().map(|v| v as f64 / ((1u32 << (4 * c.len())) - 1) as f64)
    });
    let rgb = [channels.next()??, channels.next()??, channels.next()??];
    channels.next().is_none().then_some(rgb)
}

/// Light when the background's relative luminance is above 0.5.
pub fn palette_for(rgb: [f64; 3]) -> Palette {
    if relative_luminance(rgb) > 0.5 { Palette::Light } else { Palette::Dark }
}

/// `COLORFGBG` is `fg;bg` or `fg;default;bg`; a background of 7 or 15 is light.
pub fn colorfgbg(value: &str) -> Palette {
    match value.rsplit(';').next() {
        Some("7" | "15") => Palette::Light,
        _ => Palette::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc11_replies_parse_in_both_channel_widths_and_both_terminators() {
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x07"), Some([1.0, 1.0, 1.0]));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:0000/0000/0000\x1b\\"), Some([0.0, 0.0, 0.0]));
        assert_eq!(parse_osc11(b"\x1b]11;rgb:ff/80/00\x1b\\"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:fafa/f7f7/f0f0\x07\x1b[?62;22c"),
            parse_osc11(b"\x1b]11;rgb:fa/f7/f0\x07")
        );
        assert_eq!(parse_osc11(b"noise\x1b]11;rgb:1717/1717/1717\x07"), parse_osc11(b"\x1b]11;rgb:17/17/17\x07"));
        for garbage in [
            &b""[..],
            b"\x1b[?62;22c",
            b"\x1b]11;rgb:ffff/ffff/ffff",
            b"\x1b]11;rgb:ffff/ffff\x07",
            b"\x1b]11;rgb:ffff/ffff/ffff/ffff\x07",
            b"\x1b]11;rgb:fffff/0/0\x07",
            b"\x1b]11;rgb:zz/00/00\x07",
            b"\x1b]11;rgb://\x07",
            b"\x1b]10;rgb:ffff/ffff/ffff\x07",
            b"\xff\xfe",
        ] {
            assert_eq!(parse_osc11(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn a_background_brighter_than_half_luminance_picks_the_light_palette() {
        let pick = |reply: &[u8]| parse_osc11(reply).map(palette_for);
        assert_eq!(pick(b"\x1b]11;rgb:ffff/ffff/ffff\x07"), Some(Palette::Light));
        assert_eq!(pick(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07"), Some(Palette::Light), "solarized light");
        assert_eq!(pick(b"\x1b]11;rgb:c0/c0/c0\x07"), Some(Palette::Light), "luminance 0.53");
        assert_eq!(pick(b"\x1b]11;rgb:b0/b0/b0\x07"), Some(Palette::Dark), "luminance 0.43");
        assert_eq!(pick(b"\x1b]11;rgb:8080/8080/8080\x07"), Some(Palette::Dark), "mid gray is 0.22");
        assert_eq!(pick(b"\x1b]11;rgb:2828/2c2c/3434\x1b\\"), Some(Palette::Dark));
    }

    #[test]
    fn colorfgbg_names_a_light_background_with_7_or_15() {
        assert_eq!(colorfgbg("0;15"), Palette::Light);
        assert_eq!(colorfgbg("0;default;7"), Palette::Light);
        assert_eq!(colorfgbg("15;0"), Palette::Dark);
        assert_eq!(colorfgbg("15"), Palette::Light);
        assert_eq!(colorfgbg(""), Palette::Dark);
    }

    #[test]
    fn the_replies_are_split_from_what_the_user_typed() {
        let (answers, typed) = split_replies(b"j\x1b]11;rgb:1717/1717/1717\x07k\x1b[?62;22c");
        assert_eq!(answers, b"\x1b]11;rgb:1717/1717/1717\x07\x1b[?62;22c");
        assert_eq!(typed, b"jk");
        let (answers, typed) = split_replies(b"\x1b]11;rgb:0/0/0\x1b\\q");
        assert_eq!((answers.as_slice(), typed.as_slice()), (&b"\x1b]11;rgb:0/0/0\x1b\\"[..], &b"q"[..]));
    }

    #[test]
    fn the_da1_reply_ends_the_read() {
        assert!(ends_with_da1(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22c"));
        assert!(ends_with_da1(b"\x1b[?1;2c"));
        assert!(!ends_with_da1(b"\x1b[?62;2"));
        assert!(!ends_with_da1(b"\x1b]11;rgb:0/0/0\x07"));
    }
}
