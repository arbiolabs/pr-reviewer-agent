//! A real pty run: the binary panics after its first frame, and the shell behind it must get its terminal back.

use std::process::{Command, Stdio};

#[test]
fn raw_mode_and_the_alternate_screen_are_restored_after_a_panic() {
    let bin = env!("CARGO_BIN_EXE_pr-review-tui");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../protocol/playback/normal.ndjson");
    let inner = format!("stty -a; PR_REVIEW_TUI_TEST_PANIC=1 '{bin}' --playback '{script}'; echo EXIT=$?; stty -a");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", "sh", "-c", &inner]);
    } else {
        cmd.args(["-qec", &inner, "/dev/null"]);
    }
    let out = cmd.stdin(Stdio::null()).output().expect("script(1) runs");
    let text = String::from_utf8_lossy(&out.stdout);

    let enter = text.find("\u{1b}[?1049h").expect("entered the alternate screen");
    let leave = text.find("\u{1b}[?1049l").expect("left the alternate screen");
    let panicked = text.find("PR_REVIEW_TUI_TEST_PANIC").expect("panic message printed");
    assert!(enter < leave && leave < panicked, "the screen is restored before the panic message prints");
    assert!(text.contains("EXIT=101"), "{text}");

    let after = &text[text.find("EXIT=").unwrap()..];
    let before = &text[..enter];
    for flag in [" icanon", " echo "] {
        assert!(before.contains(flag), "precondition: the pty starts cooked ({flag})");
        assert!(after.contains(flag), "{flag} not restored: {after}");
    }
}
