//! Glue between the reducer's effects and the network task, shared by the binary and the integration tests.

use tokio::sync::mpsc;

use crate::model::Model;
use crate::net::NetCmd;
use crate::update::{Effect, Msg, update};

/// Applies one message and carries out its effects. Returns true when the user asked to quit.
pub fn dispatch(m: &mut Model, msg: Msg, net: &mpsc::Sender<NetCmd>) -> bool {
    for effect in update(m, msg) {
        match effect {
            Effect::Send(req) => {
                if net.try_send(NetCmd::Send(req)).is_err() {
                    m.diag.error(m.now_ms, "request queue full; request dropped");
                }
            }
            Effect::Close => {
                let _ = net.try_send(NetCmd::Close);
            }
            Effect::OpenUrl(url) => open_url(m, &url),
            Effect::Quit => return true,
        }
    }
    false
}

/// Hands a published review's URL to the desktop's opener, detached and silent so it cannot draw over the TUI.
fn open_url(m: &mut Model, url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let spawned = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        m.diag.error(m.now_ms, format!("could not run {opener}: {e}"));
    }
}
