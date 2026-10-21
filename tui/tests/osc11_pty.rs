//! The background query against a real pty on fd 0 and 1. Its own test binary, because it swaps the process's
//! stdin and stdout.

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pr_review_tui::terminal::{LateReply, detect_palette, input, typed_before_start};
use pr_review_tui::theme::Palette;
use pr_review_tui::update::Msg;

fn openpty() -> (RawFd, RawFd) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty writes the two fds; the name, termios and winsize pointers may be null.
    let rc = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    };
    assert_eq!(rc, 0, "openpty");
    (master, slave)
}

/// Runs `detect_palette` with fds 0 and 1 on a fresh pty whose terminal side, once it sees the query, writes
/// `first`, then `rest` after `pause`.
fn detect_on_pty(first: &'static [u8], pause: Duration, rest: &'static [u8]) -> (Palette, Duration) {
    let (master, slave) = openpty();
    // SAFETY: dup and dup2 on fds this process owns; the originals are restored before returning.
    let (stdin, stdout) = unsafe { (libc::dup(0), libc::dup(1)) };
    unsafe {
        libc::dup2(slave, 0);
        libc::dup2(slave, 1);
    }
    let terminal = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        while !seen.windows(3).any(|w| w == b"\x1b[c") {
            // SAFETY: reads into a local buffer of the given length.
            let n = unsafe { libc::read(master, buf.as_mut_ptr().cast(), buf.len()) };
            assert!(n > 0, "the query never came");
            seen.extend_from_slice(&buf[..n as usize]);
        }
        // SAFETY: writes local buffers of the given lengths.
        unsafe { libc::write(master, first.as_ptr().cast(), first.len()) };
        std::thread::sleep(pause);
        unsafe { libc::write(master, rest.as_ptr().cast(), rest.len()) };
    });
    let started = Instant::now();
    let palette = detect_palette();
    let took = started.elapsed();
    unsafe {
        libc::dup2(stdin, 0);
        libc::dup2(stdout, 1);
    }
    terminal.join().unwrap();
    // SAFETY: closes the pty fds this function opened.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    (palette, took)
}

/// One test, because each case swaps the process's fds 0 and 1.
#[test]
fn the_background_query_reads_the_reply_and_hands_on_only_what_was_typed() {
    // SAFETY: the test only removes a variable it reads back through detect_palette.
    unsafe { std::env::remove_var("COLORFGBG") };

    // A one-write light reply, with keys typed before it: an arrow key's escape sequence stays whole and drops.
    let (palette, took) = detect_on_pty(b"j\x1b[Ak\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c", Duration::ZERO, b"");
    let typed: Vec<KeyCode> = typed_before_start().into_iter().map(|k| k.code).collect();
    assert_eq!(palette, Palette::Light);
    assert!(took < Duration::from_millis(60), "took {took:?}");
    assert_eq!(typed, [KeyCode::Char('j'), KeyCode::Char('k')], "no [ or A from the arrow key");

    // Alt+Up is ESC ESC [ A: the second ESC starts the arrow's sequence, which drops whole too.
    detect_on_pty(b"j\x1b\x1b[Ak\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c", Duration::ZERO, b"");
    let typed: Vec<KeyCode> = typed_before_start().into_iter().map(|k| k.code).collect();
    assert_eq!(typed, [KeyCode::Char('j'), KeyCode::Char('k')], "no [ or A from Alt+Up");

    // The deadline falls inside the reply: its tail reaches the key reader without the opening Alt+].
    let (palette, _) = detect_on_pty(b"\x1b]11;rgb:ffff/ff", Duration::from_millis(150), b"ff/ffff\x07\x1b[?62;22c");
    assert_eq!(palette, Palette::Dark);
    assert_eq!(after_query("ff/ffff\x07j"), [KeyCode::Char('j')], "the tail is swallowed; the next key acts");

    // Cut between the reply's ESC and its ]: the tail starts with a plain ].
    detect_on_pty(b"\x1b", Duration::from_millis(150), b"]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c");
    assert_eq!(after_query("]11;rgb:ffff/ffff/ffff\x07j"), [KeyCode::Char('j')], "cut before ]");

    // A bare Esc typed in the first 100 ms on a slow terminal: the Esc acts, and the whole late reply, which
    // crossterm reads as Alt+] and keys, is swallowed.
    let (palette, _) =
        detect_on_pty(b"\x1b", Duration::from_millis(150), b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c");
    assert_eq!(palette, Palette::Dark);
    let typed: Vec<KeyCode> = typed_before_start().into_iter().map(|k| k.code).collect();
    assert_eq!(typed, [KeyCode::Esc], "the early Esc reaches the app");
    let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
    keys.extend("11;rgb:ffff/ffff/ffff".chars().map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    keys.push(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    keys.push(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    let acted = |at: u64| -> Vec<KeyCode> {
        let mut late = LateReply::after_query();
        keys.iter()
            .filter_map(|k| input(Event::Key(*k), &mut late, Duration::from_millis(at)))
            .filter_map(|m| match m {
                Msg::Key(k) => Some(k.code),
                _ => None,
            })
            .collect()
    };
    assert_eq!(acted(60), [KeyCode::Char('j')], "the late reply is swallowed whole; the next key acts");

    // The same reply 300 ms in, after the filter the cut opened has expired: its Alt+] opens a new one.
    detect_on_pty(b"\x1b", Duration::from_millis(150), b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c");
    assert_eq!(acted(300), [KeyCode::Char('j')], "a reply later than the cut's burst is swallowed too");

    // Cut inside the DA1 reply: its digits and final c drop, and a key after the c acts, hex digit or not.
    let (palette, _) = detect_on_pty(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?6", Duration::from_millis(150), b"2;22c");
    assert_eq!(palette, Palette::Light);
    assert_eq!(after_query("2;22c1"), [KeyCode::Char('1')], "cut inside DA1");

    // Cut between DA1's ESC and its [: the tail is a plain [ and the rest of DA1.
    detect_on_pty(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b", Duration::from_millis(150), b"[?62;22c");
    assert_eq!(after_query("[?62;22cf"), [KeyCode::Char('f')], "cut before DA1's [");
}

/// What acts of `tail` (BEL as Ctrl+G, the rest as plain keys) once it reaches the key reader just after the query.
fn after_query(tail: &str) -> Vec<KeyCode> {
    let mut late = LateReply::after_query();
    tail.chars()
        .map(|c| match c {
            '\x07' => KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        })
        .filter_map(|k| input(Event::Key(k), &mut late, Duration::from_millis(60)))
        .filter_map(|m| match m {
            Msg::Key(k) => Some(k.code),
            _ => None,
        })
        .collect()
}
