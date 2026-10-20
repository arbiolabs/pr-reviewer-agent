//! Real pty runs: the binary panics after its first frame, or gets a signal, and the shell behind it must get its
//! terminal back.

use std::process::{Command, Stdio};

/// Runs `sh -c inner` in a pty, with `$BIN` and `$SCRIPT` set, and returns everything the pty printed.
fn pty(inner: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_pr-review-tui");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../protocol/playback/normal.ndjson");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", "sh", "-c", inner]);
    } else {
        cmd.args(["-qec", inner, "/dev/null"]);
    }
    let out = cmd.env("BIN", bin).env("SCRIPT", script).stdin(Stdio::null()).output().expect("script(1) runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The pty was cooked before the client ran and is cooked again after it exited.
fn assert_restored(text: &str) {
    let enter = text.find("\u{1b}[?1049h").expect("entered the alternate screen");
    let leave = text.find("\u{1b}[?1049l").expect("left the alternate screen");
    assert!(enter < leave);
    let mouse_on = text.find("\u{1b}[?1000h").expect("turned mouse reporting on");
    let mouse_off = text.rfind("\u{1b}[?1000l").expect("turned mouse reporting off");
    assert!(mouse_on < mouse_off, "mouse reporting is off again after exit");
    let after = &text[text.find("EXIT=").unwrap()..];
    let before = &text[..enter];
    for flag in [" icanon", " echo "] {
        assert!(before.contains(flag), "precondition: the pty starts cooked ({flag})");
        assert!(after.contains(flag), "{flag} not restored: {after}");
    }
}

#[test]
fn raw_mode_and_the_alternate_screen_are_restored_after_a_panic() {
    let text = pty(r#"stty -a; PR_REVIEW_TUI_TEST_PANIC=1 "$BIN" --playback "$SCRIPT"; echo EXIT=$?; stty -a"#);
    assert_restored(&text);
    let leave = text.find("\u{1b}[?1049l").unwrap();
    let panicked = text.find("PR_REVIEW_TUI_TEST_PANIC").expect("panic message printed");
    assert!(leave < panicked, "the screen is restored before the panic message prints");
    assert!(text.contains("EXIT=101"), "{text}");
}

#[test]
fn sigterm_sighup_and_sigint_exit_through_the_restore_path() {
    for signal in ["TERM", "HUP", "INT"] {
        let text = pty(&format!(
            r#"stty -a; "$BIN" --playback "$SCRIPT" </dev/tty & pid=$!; sleep 1; kill -{signal} $pid; wait $pid; echo EXIT=$?; stty -a"#
        ));
        assert!(text.contains("EXIT=0"), "SIG{signal}: {text}");
        assert_restored(&text);
    }
}

#[test]
fn mouse_false_leaves_the_terminals_own_drag_to_select_alone() {
    let dir = std::env::temp_dir().join(format!("pr-review-tui-mouse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "[ui]\nmouse = false\n").unwrap();
    let text = pty(&format!(
        r#"PR_REVIEW_TUI_TEST_PANIC=1 "$BIN" --playback "$SCRIPT" --config {}; echo EXIT=$?"#,
        config.display()
    ));
    assert!(text.contains("\u{1b}[?1049h"), "the client drew: {text}");
    assert!(!text.contains("\u{1b}[?1000h"), "mouse reporting stays off");
}
