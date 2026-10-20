//! The binary's own flags, run as a user runs them.

use std::process::Command;

#[test]
fn version_prints_the_crate_version_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_pr-review-tui")).arg("--version").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("pr-review-tui {}\n", env!("CARGO_PKG_VERSION")));
}
