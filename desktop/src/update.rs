//! New releases: the install's `scripts/update.sh` says which release tag is newest and installs one, and the app
//! offers it in Settings → General and the title bar, then restarts into it.

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use gpui_kit::*;

use crate::kit::{Tone, button};
use crate::tokens::{Colors, space, text};
use crate::ui::{Shell, on};

/// How often the app asks for a newer release while it runs.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A release as its tag names it, `v1.6.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Release(u64, u64, u64);

impl Release {
    pub fn parse(s: &str) -> Option<Release> {
        let mut n = s.trim().trim_start_matches('v').splitn(3, '.').map(|p| p.parse().ok());
        Some(Release(n.next()??, n.next()??, n.next()??))
    }

    pub fn running() -> Release {
        Release::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is x.y.z")
    }

    fn tag(self) -> String {
        format!("v{self}")
    }
}

impl std::fmt::Display for Release {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Not asked yet, or the app runs with no install to update (a snapshot, a build outside one).
    Idle,
    Checking,
    Current,
    Available(Release),
    Installing {
        to: Release,
        step: String,
    },
    /// Installed; the window still runs the old version until it restarts.
    Installed(Release),
    Failed(String),
}

/// The update script of the install the app was set up from, when there is one.
pub fn script() -> Option<PathBuf> {
    let path = crate::reviewer::home().join("Library/Application Support/pr-review/source/scripts/update.sh");
    path.is_file().then_some(path)
}

/// What `update.sh --check` printed, against the version running.
fn checked(out: &str) -> Update {
    match out.lines().find_map(|l| l.strip_prefix("latest ")).and_then(Release::parse) {
        Some(latest) if latest > Release::running() => Update::Available(latest),
        Some(_) => Update::Current,
        None => Update::Failed(failure(out)),
    }
}

/// The script's last word on why it stopped, as a sentence.
fn failure(out: &str) -> String {
    let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("the update script stopped");
    crate::kit::sentence(last.trim().trim_start_matches("error: "))
}

/// A step `update.sh` reported, or how it ended.
enum Progress {
    Step(String),
    Done(Result<(), String>),
}

impl Shell {
    /// Asks for the newest release, unless an install is under way or done.
    pub fn check_update(&mut self, cx: &mut Context<Self>) {
        let Some(script) = self.update_script.clone() else { return };
        if matches!(self.update, Update::Checking | Update::Installing { .. } | Update::Installed(_)) {
            return;
        }
        self.update = Update::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let run = cx.background_executor().spawn(async move {
                match Command::new("/bin/sh").arg(&script).arg("--check").stdin(Stdio::null()).output() {
                    Ok(o) => checked(&String::from_utf8_lossy(&o.stdout)),
                    Err(e) => Update::Failed(format!("Can't run {}: {e}", script.display())),
                }
            });
            let found = run.await;
            let _ = this.update(cx, |s, cx| {
                if s.update == Update::Checking {
                    s.update = found;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs the update to the release on offer, showing each step the script reports.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let (Some(script), Update::Available(to)) = (self.update_script.clone(), self.update.clone()) else { return };
        self.update = Update::Installing { to, step: "Starting".into() };
        cx.notify();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            let child = Command::new("/bin/sh")
                .arg(&script)
                .arg(to.tag())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Progress::Done(Err(format!("Can't run {}: {e}", script.display()))));
                    return;
                }
            };
            let mut said = String::new();
            for line in std::io::BufReader::new(child.stdout.take().expect("piped")).lines().map_while(Result::ok) {
                let _ = tx.send(Progress::Step(crate::kit::sentence(&line)));
                said.push_str(&line);
                said.push('\n');
            }
            let done = match child.wait() {
                Ok(s) if s.success() => Ok(()),
                _ => Err(failure(&said)),
            };
            let _ = tx.send(Progress::Done(done));
        });
        cx.spawn(async move |this, cx| {
            while let Some(p) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    s.update = match p {
                        Progress::Step(step) => Update::Installing { to, step },
                        Progress::Done(Ok(())) => Update::Installed(to),
                        Progress::Done(Err(why)) => Update::Failed(why),
                    };
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Quits and opens the app again once this process has gone, so the new binary runs.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        let reopen = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            let target = crate::menubar::bundle(&exe).map_or(exe.clone(), PathBuf::from);
            Command::new("/bin/sh")
                .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$0\""])
                .arg(&target)
                .arg(std::process::id().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())
        });
        match reopen {
            Ok(_) => cx.quit(),
            Err(e) => {
                self.update = Update::Failed(format!("Can't restart: {e}. Quit and open the app yourself."));
                cx.notify();
            }
        }
    }
}

/// Settings → General → Updates: the version running, what is on offer, and the one action that moves it on.
pub fn card(s: &Shell, t: &Colors, cx: &Context<Shell>) -> Div {
    let running = Release::running();
    let (words, hue, action): (String, Hsla, Option<(&str, &str, Tone)>) = match &s.update {
        _ if s.update_script.is_none() => (
            format!(
                "Version {running}. Updates install into ~/Library/Application Support/pr-review/source, and this \
                 app wasn't installed from there."
            ),
            t.muted,
            None,
        ),
        Update::Idle | Update::Checking => (format!("Version {running}. Checking for a newer release…"), t.muted, None),
        Update::Current => (
            format!("Version {running} is the newest release."),
            t.muted,
            Some(("update-check", "Check now", Tone::Secondary)),
        ),
        Update::Available(to) => (
            format!(
                "Version {to} is available; this is {running}. Updating builds it from source, which takes a few \
                 minutes, and restarts the reviewer. A failed update leaves this version in place."
            ),
            t.text,
            Some(("update-install", "Update", Tone::Primary)),
        ),
        Update::Installing { to, step } => (format!("Updating to {to}. {step}"), t.text, None),
        Update::Installed(to) => (
            format!("Version {to} is installed. Restart the app to use it."),
            t.text,
            Some(("update-restart", "Restart", Tone::Primary)),
        ),
        Update::Failed(why) => (
            format!("{why} Details are in ~/Library/Logs/pr-review/update.log."),
            t.danger,
            Some(("update-check", "Try again", Tone::Secondary)),
        ),
    };
    let act = action.map(|(id, name, tone)| {
        let run = on(cx, move |s, cx| match s.update {
            Update::Available(_) => s.install_update(cx),
            Update::Installed(_) => s.restart(cx),
            _ => s.check_update(cx),
        });
        div().flex_none().child(button(t, id, name, None, tone, run))
    });
    div()
        .flex()
        .items_center()
        .gap(space::S3)
        .p(space::S4)
        .child(div().flex_1().min_w_0().text_size(text::BODY).text_color(hue).child(words))
        .children(act)
}

#[cfg(test)]
mod tests {
    use super::{Release, Update, checked};

    #[test]
    fn a_newer_tag_is_on_offer_and_anything_else_is_not() {
        let next = Release(Release::running().0 + 1, 0, 0);
        assert_eq!(checked(&format!("latest {}\n", next.tag())), Update::Available(next));
        assert_eq!(checked(&format!("latest {}\n", Release::running().tag())), Update::Current);
        assert_eq!(checked("latest v0.9.0\n"), Update::Current);
        assert_eq!(
            checked("error: can't fetch release tags from origin; see /x/update.log\n"),
            Update::Failed("Can't fetch release tags from origin; see /x/update.log.".into())
        );
        assert!(Release::parse("v1.10.0") > Release::parse("v1.9.3"));
        assert_eq!(Release::parse("v1.6"), None);
    }
}
