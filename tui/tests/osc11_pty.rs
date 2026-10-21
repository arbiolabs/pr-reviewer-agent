//! The background query against a real pty on fd 0 and 1. Its own test binary, because it swaps the process's
//! stdin and stdout.

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use pr_review_tui::terminal::{detect_palette, typed_before_start};
use pr_review_tui::theme::Palette;

fn openpty() -> (RawFd, RawFd) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty writes the two fds; the name, termios and winsize pointers may be null.
    let rc = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    };
    assert_eq!(rc, 0, "openpty");
    (master, slave)
}

#[test]
fn a_one_write_light_reply_is_read_at_once_and_keys_typed_before_it_are_kept() {
    // SAFETY: the test only removes a variable it reads back through detect_palette.
    unsafe { std::env::remove_var("COLORFGBG") };
    let (master, slave) = openpty();
    // SAFETY: dup and dup2 on fds this process owns; the originals are restored before the asserts.
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
        let reply = b"jk\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c";
        // SAFETY: writes a local buffer of the given length.
        unsafe { libc::write(master, reply.as_ptr().cast(), reply.len()) };
    });
    let started = Instant::now();
    let palette = detect_palette();
    let took = started.elapsed();
    unsafe {
        libc::dup2(stdin, 0);
        libc::dup2(stdout, 1);
    }
    terminal.join().unwrap();
    let typed: Vec<KeyCode> = typed_before_start().into_iter().map(|k| k.code).collect();
    assert_eq!(palette, Palette::Light);
    assert!(took < Duration::from_millis(60), "took {took:?}");
    assert_eq!(typed, [KeyCode::Char('j'), KeyCode::Char('k')]);
}
