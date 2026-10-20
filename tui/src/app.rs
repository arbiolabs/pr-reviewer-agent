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
            Effect::Quit => return true,
        }
    }
    false
}
