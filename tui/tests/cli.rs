//! The binary's own flags, run as a user runs them.

use std::process::Command;

#[test]
fn version_prints_the_crate_version_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_pr-review-tui")).arg("--version").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("pr-review-tui {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn the_readme_starts_the_client_on_a_playback_that_loads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let start = &readme[readme.find("## Get started").expect("a Get started section")..];
    let first_block = start.split("```bash\n").nth(1).and_then(|b| b.split("```").next()).unwrap();
    let script = first_block.split("--playback ").nth(1).expect("the first command plays a session").trim();
    let loaded = pr_review_tui::playback::load(&root.join(script)).unwrap();
    assert!(!loaded.steps.is_empty());
}
