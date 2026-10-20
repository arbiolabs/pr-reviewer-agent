//! Raw mode and the alternate screen, restored on drop and from the panic hook, so a crash never leaves the user's
//! shell unusable.

use std::io::{self, IsTerminal, Read, Stdout, Write};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::theme::{Palette, relative_luminance};

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

/// Sends OSC 11 and then DA1, which every terminal answers, so the DA1 reply marks the end of whatever the terminal
/// had to say. A terminal that answers neither costs `timeout`. Input left over is flushed, so no reply reaches the
/// key reader as keys.
fn query_background(timeout: Duration) -> Option<Vec<u8>> {
    enable_raw_mode().ok()?;
    let reply = (|| {
        let mut out = io::stdout();
        out.write_all(b"\x1b]11;?\x07\x1b[c").ok()?;
        out.flush().ok()?;
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        while !ends_with_da1(&buf) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !readable(left) {
                break;
            }
            let n = io::stdin().lock().read(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some(buf)
    })();
    // SAFETY: tcflush on stdin, which is a terminal here.
    unsafe { libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH) };
    let _ = disable_raw_mode();
    reply
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
    fn the_da1_reply_ends_the_read() {
        assert!(ends_with_da1(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22c"));
        assert!(ends_with_da1(b"\x1b[?1;2c"));
        assert!(!ends_with_da1(b"\x1b[?62;2"));
        assert!(!ends_with_da1(b"\x1b]11;rgb:0/0/0\x07"));
    }
}
