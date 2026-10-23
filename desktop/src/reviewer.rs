//! Starting and stopping the reviewer daemon: how this machine runs it (its LaunchAgent, or a process the app
//! spawns), what the app last asked of it, and how that ask settles as the link comes and goes.

use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Proc, Stdio};

use gpui_kit::*;
use pr_review_tui::config::{Desktop, default_socket};
use pr_review_tui::keymap::Action;
use pr_review_tui::model::{ConfirmState, Link, Modal, Model, Notice};
use pr_review_tui::net::NetCmd;
use pr_review_tui::protocol::{Command, ErrorCode, StopParams};

use crate::queue;
use crate::ui::{Overlay, Shell};

/// How long a start may take before the app calls it failed.
pub const START_TIMEOUT_MS: u64 = 20_000;
const NOTICE_MS: u64 = 4_000;

/// How this machine runs the daemon the window dials.
#[derive(Clone, Debug, PartialEq)]
pub enum Launcher {
    /// A LaunchAgent in ~/Library/LaunchAgents that serves the socket this window dials. `relaunches` is its
    /// `KeepAlive: true`: launchd starts it again even after a clean exit. `log` is its StandardErrorPath.
    LaunchAgent { label: String, plist: PathBuf, relaunches: bool, log: Option<PathBuf> },
    /// `pr-review daemon` started by the app, detached. `command` runs pr-review, in shell syntax.
    Spawn { command: String },
}

/// What the app last asked of the daemon, until the link says how it went.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Idle,
    Starting {
        since_ms: u64,
    },
    /// A stop went out; `restart` starts it again once the connection ends.
    Stopping {
        restart: bool,
    },
    /// Stopped because the reader asked: not offline, not crashed.
    Stopped,
    /// The start never brought a daemon up; why, in a sentence.
    StartFailed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Start,
    Stop,
    Restart,
}

impl Ask {
    pub fn title(self) -> &'static str {
        match self {
            Ask::Start => "Start reviewer",
            Ask::Stop => "Stop reviewer",
            Ask::Restart => "Restart reviewer",
        }
    }
}

/// A process the app started for the reviewer: the daemon itself, or a launchctl call.
enum Job {
    Daemon(Child),
    Launchctl(Child),
}

pub struct Reviewer {
    /// The socket the window dials; `None` under playback, where there is no daemon to start.
    pub socket: Option<PathBuf>,
    pub launcher: Result<Launcher, String>,
    pub control: Control,
    desktop: Desktop,
    config_path: PathBuf,
    job: Option<Job>,
}

/// A word as a shell reads it: as is when it is plain, else in single quotes, with each embedded quote as `'\''`.
pub fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if plain { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

/// The log a spawned daemon writes to.
pub fn spawn_log() -> PathBuf {
    home().join("Library/Logs/pr-review/daemon.log")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The LaunchAgent `label`'s plist, read as launchd would: the socket it serves (its `--socket`, else the default),
/// whether it comes back after a clean exit, and its log.
pub fn parse_agent(label: &str, plist: PathBuf, json: &serde_json::Value, default: &Path) -> (Launcher, PathBuf) {
    let args: Vec<&str> =
        json["ProgramArguments"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
    let socket = args.iter().position(|a| *a == "--socket").and_then(|i| args.get(i + 1));
    let relaunches = json["KeepAlive"] == serde_json::Value::Bool(true);
    let log = json["StandardErrorPath"].as_str().map(PathBuf::from);
    let socket = socket.map_or_else(|| default.to_path_buf(), PathBuf::from);
    (Launcher::LaunchAgent { label: label.into(), plist, relaunches, log }, socket)
}

/// Which way to start the daemon on `socket`: its LaunchAgent when one serves that socket, else the configured
/// command, else `pr-review` on PATH; an error that names what to set when there is none.
pub fn choose(
    agent: Option<(Launcher, PathBuf)>,
    socket: &Path,
    daemon_command: Option<&str>,
    on_path: Option<&Path>,
    config: &Path,
) -> Result<Launcher, String> {
    match (agent, daemon_command, on_path) {
        (Some((agent, serves)), ..) if serves == socket => Ok(agent),
        (_, Some(command), _) if !command.trim().is_empty() => Ok(Launcher::Spawn { command: command.trim().into() }),
        (_, _, Some(path)) => Ok(Launcher::Spawn { command: shell_quote(&path.to_string_lossy()) }),
        _ => Err(format!(
            "pr-review is not on this app's PATH. Set daemon_command under [desktop] in {}, such as \
             daemon_command = \"node /path/to/pr-reviewer/bin/pr-review.mjs\".",
            config.display()
        )),
    }
}

impl Launcher {
    /// The command that starts the daemon. A loaded LaunchAgent is kicked, an unloaded one bootstrapped; a spawn
    /// passes the socket as its own argument, so no path is ever re-read by the shell.
    pub fn start_argv(&self, uid: u32, loaded: bool, socket: &Path) -> Vec<String> {
        match self {
            Launcher::LaunchAgent { label, .. } if loaded => {
                vec!["launchctl".into(), "kickstart".into(), format!("gui/{uid}/{label}")]
            }
            Launcher::LaunchAgent { plist, .. } => {
                vec!["launchctl".into(), "bootstrap".into(), format!("gui/{uid}"), plist.display().to_string()]
            }
            Launcher::Spawn { command } => vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("exec {command} \"$@\""),
                "sh".into(),
                "daemon".into(),
                "--allow-commands".into(),
                "--socket".into(),
                socket.display().to_string(),
            ],
        }
    }

    /// Unloads a LaunchAgent, so launchd stops it (SIGTERM, the daemon's clean shutdown) and does not start it again.
    pub fn bootout_argv(&self, uid: u32) -> Option<Vec<String>> {
        match self {
            Launcher::LaunchAgent { label, .. } => {
                Some(vec!["launchctl".into(), "bootout".into(), format!("gui/{uid}/{label}")])
            }
            Launcher::Spawn { .. } => None,
        }
    }

    /// Where its output goes, if anywhere the app knows.
    pub fn log(&self) -> Option<PathBuf> {
        match self {
            Launcher::LaunchAgent { log, .. } => log.clone(),
            Launcher::Spawn { .. } => Some(spawn_log()),
        }
    }

    pub fn words(&self) -> String {
        match self {
            Launcher::LaunchAgent { label, relaunches: true, .. } => {
                format!("LaunchAgent {label} (KeepAlive true: launchd restarts it after any exit)")
            }
            Launcher::LaunchAgent { label, .. } => format!("LaunchAgent {label}"),
            Launcher::Spawn { command } => format!("{command} daemon, started by this app"),
        }
    }

    pub fn relaunches(&self) -> bool {
        matches!(self, Launcher::LaunchAgent { relaunches: true, .. })
    }
}

/// What the ask becomes given the link now: a start settles when the daemon welcomes the window or the timeout
/// passes, a stop when the connection ends, and a stopped or failed daemon that answers again is simply up.
pub fn settle(control: &Control, link: &Link, now_ms: u64) -> Option<Control> {
    match control {
        Control::Starting { .. } | Control::Stopped | Control::StartFailed(_) if link.welcomed() => Some(Control::Idle),
        Control::Starting { since_ms } if now_ms.saturating_sub(*since_ms) >= START_TIMEOUT_MS => {
            Some(Control::StartFailed(format!("It did not answer within {} s.", START_TIMEOUT_MS / 1000)))
        }
        Control::Stopping { .. } if matches!(link, Link::Disconnected { .. }) => Some(Control::Stopped),
        _ => None,
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

fn uid() -> u32 {
    std::fs::metadata(home()).map_or(0, |m| m.uid())
}

fn spawn(argv: &[String], log: Option<&Path>) -> Result<Child, String> {
    let mut cmd = Proc::new(&argv[0]);
    // Its own process group: the daemon outlives the app and a terminal's hangup.
    cmd.args(&argv[1..]).stdin(Stdio::null()).process_group(0);
    match log {
        Some(path) => {
            let dir = path.parent().unwrap_or(Path::new("."));
            std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("could not open {}: {e}", path.display()))?;
            let err = file.try_clone().map_err(|e| e.to_string())?;
            cmd.stdout(file).stderr(err);
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        }
    }
    cmd.spawn().map_err(|e| format!("could not run {}: {e}", argv[0]))
}

impl Reviewer {
    /// `socket` is `None` under playback. A snapshot never looks at the machine, so its pixels can't depend on it.
    pub fn new(socket: Option<PathBuf>, desktop: Desktop, config_path: PathBuf, snapshot: bool) -> Reviewer {
        let mut r = Reviewer {
            socket,
            launcher: Ok(Launcher::Spawn { command: "pr-review".into() }),
            control: Control::Idle,
            desktop,
            config_path,
            job: None,
        };
        if !snapshot {
            r.detect();
        }
        r
    }

    /// Reads the LaunchAgent's plist again, so an edit to it counts from the next ask on.
    fn detect(&mut self) {
        let Some(socket) = &self.socket else { return };
        let label = &self.desktop.launch_agent;
        let plist = home().join("Library/LaunchAgents").join(format!("{label}.plist"));
        let agent = plist.is_file().then(|| {
            let out = Proc::new("plutil").args(["-convert", "json", "-o", "-"]).arg(&plist).output();
            let json = out.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default();
            parse_agent(label, plist.clone(), &json, &default_socket())
        });
        let on_path = which("pr-review");
        self.launcher =
            choose(agent, socket, self.desktop.daemon_command.as_deref(), on_path.as_deref(), &self.config_path);
    }

    fn loaded(label: &str) -> bool {
        Proc::new("launchctl")
            .args(["print", &format!("gui/{}/{label}", uid())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Why `ask` can't run now, or `None` when it can.
    pub fn block(&self, m: &Model, ask: Ask) -> Option<String> {
        if self.socket.is_none() {
            return Some("this window plays a script".into());
        }
        match (&self.control, ask) {
            (Control::Starting { .. }, _) => return Some("it is starting".into()),
            (Control::Stopping { .. }, _) => return Some("it is stopping".into()),
            _ => {}
        }
        let up = m.link.welcomed();
        match ask {
            Ask::Start => match &m.link {
                Link::Disconnected { .. } => self.launcher.as_ref().err().cloned(),
                Link::Unusable { .. } => Some("this socket path can't work".into()),
                Link::Incompatible { .. } => Some("a daemon that speaks another protocol is listening".into()),
                _ if up => Some("it is running".into()),
                _ => Some("connecting".into()),
            },
            Ask::Stop | Ask::Restart if !up => Some("it is not running".into()),
            _ if queue::read_only(m) => Some("read-only daemon".into()),
            _ if !m.may_write("daemon.stop") => Some("this daemon can't be stopped from here".into()),
            Ask::Restart => self.launcher.as_ref().err().map(|_| "nothing here can start it again".into()),
            Ask::Stop => None,
        }
    }

    /// Runs the start, or says why it did not.
    fn start(&mut self, now_ms: u64) {
        self.detect();
        let (Some(socket), Ok(launcher)) = (&self.socket, &self.launcher) else {
            if let Err(e) = &self.launcher {
                self.control = Control::StartFailed(e.clone());
            }
            return;
        };
        let loaded = match launcher {
            Launcher::LaunchAgent { label, .. } => Self::loaded(label),
            Launcher::Spawn { .. } => false,
        };
        let argv = launcher.start_argv(uid(), loaded, socket);
        let job = match launcher {
            Launcher::Spawn { .. } => spawn(&argv, Some(&spawn_log())).map(Job::Daemon),
            Launcher::LaunchAgent { .. } => spawn(&argv, None).map(Job::Launchctl),
        };
        match job {
            Ok(job) => {
                self.job = Some(job);
                self.control = Control::Starting { since_ms: now_ms };
            }
            Err(e) => self.control = Control::StartFailed(e),
        }
    }

    /// The sentence for a failed start: why, then where its log is.
    pub fn failure(&self, why: &str) -> String {
        match self.launcher.as_ref().ok().and_then(Launcher::log) {
            Some(log) => format!("{} The log is {}.", crate::kit::sentence(why), log.display()),
            None => crate::kit::sentence(why),
        }
    }

    /// A process the app started that has ended: a launchctl that failed, or a daemon that exited before it listened.
    fn reap(&mut self) -> Option<String> {
        let (child, daemon) = match self.job.as_mut()? {
            Job::Daemon(c) => (c, true),
            Job::Launchctl(c) => (c, false),
        };
        let status = child.try_wait().ok()??;
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        self.job = None;
        match (daemon, status.success()) {
            (true, _) if matches!(self.control, Control::Starting { .. }) => {
                Some(format!("pr-review daemon exited ({status}) before it listened."))
            }
            (false, false) => Some(format!("launchctl failed ({status}): {}", stderr.trim())),
            _ => None,
        }
    }
}

impl Shell {
    pub fn ask(&mut self, ask: Ask, cx: &mut Context<Self>) {
        if !self.snapshot {
            self.reviewer.detect();
        }
        if let Some(why) = self.reviewer.block(&self.model, ask) {
            self.model.notice = Some(Notice {
                text: format!("Can't {}: {why}", ask.title().to_lowercase()),
                until_ms: self.model.now_ms + NOTICE_MS,
            });
            return cx.notify();
        }
        match ask {
            Ask::Start => self.reviewer.start(self.model.now_ms),
            Ask::Stop | Ask::Restart => {
                let restart = ask == Ask::Restart;
                let running = queue::running(&self.model).len();
                let relaunches = self.reviewer.launcher.as_ref().is_ok_and(Launcher::relaunches);
                if running > 0 || relaunches && !restart {
                    self.overlay = Some(Overlay::ConfirmStop { restart, running });
                } else {
                    self.stop(restart, running > 0, cx);
                }
            }
        }
        cx.notify();
    }

    /// ⌘R: starts the reviewer when nothing is listening, else asks the daemon for a fresh snapshot.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if matches!(self.model.link, Link::Disconnected { .. })
            && self.reviewer.block(&self.model, Ask::Start).is_none()
        {
            self.ask(Ask::Start, cx)
        } else {
            self.act(Action::Refresh, cx)
        }
    }

    /// The stop sheet's confirm. A LaunchAgent that relaunches is unloaded instead, so it stays stopped.
    pub fn confirm_stop(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::ConfirmStop { restart, running }) = self.overlay.take() else { return };
        let bootout = self.reviewer.launcher.as_ref().ok().filter(|l| !restart && l.relaunches());
        match bootout.and_then(|l| l.bootout_argv(uid())) {
            Some(argv) => match spawn(&argv, None) {
                Ok(child) => {
                    self.reviewer.job = Some(Job::Launchctl(child));
                    self.reviewer.control = Control::Stopping { restart: false };
                }
                Err(e) => self.model.notice = Some(Notice { text: e, until_ms: self.model.now_ms + NOTICE_MS }),
            },
            None => self.stop(restart, running > 0, cx),
        }
        cx.notify();
    }

    /// `daemon.stop` through the reducer's own confirmation, answered at once: the daemon's reply settles it.
    fn stop(&mut self, restart: bool, pause_reviews: bool, cx: &mut Context<Self>) {
        let command = Command::StopDaemon(StopParams { pause_reviews });
        pr_review_tui::update::confirm(&mut self.model, "Stop the reviewer?".into(), command);
        self.act(Action::Confirm, cx);
        self.reviewer.control = Control::Stopping { restart };
    }

    /// After every message: settles the reviewer's ask from the link, the daemon's reply and the processes it ran.
    pub fn observe_reviewer(&mut self, cx: &mut Context<Self>) {
        let before = self.reviewer.control.clone();
        let r = &mut self.reviewer;
        if let Some(why) = r.reap() {
            if matches!(r.control, Control::Starting { .. }) {
                r.control = Control::StartFailed(r.failure(&why));
            } else {
                r.control = Control::Idle;
                self.model.notice =
                    Some(Notice { text: crate::kit::sentence(&why), until_ms: self.model.now_ms + NOTICE_MS });
            }
        }
        if let Some(Modal::Confirm { command: Command::StopDaemon(p), state, .. }) = self.model.top_modal() {
            let restart = matches!(self.reviewer.control, Control::Stopping { restart: true });
            match state {
                ConfirmState::Accepted { .. } => self.act(Action::Back, cx),
                // A review began after the app looked: ask, as the stop would have.
                ConfirmState::Refused { code: Some(ErrorCode::Conflict), .. } if !p.pause_reviews => {
                    self.act(Action::Back, cx);
                    let running = queue::running(&self.model).len().max(1);
                    self.overlay = Some(Overlay::ConfirmStop { restart, running });
                    self.reviewer.control = Control::Idle;
                }
                ConfirmState::Refused { .. } | ConfirmState::TimedOut => self.reviewer.control = Control::Idle,
                _ => {}
            }
        }
        let r = &mut self.reviewer;
        let restart = matches!(r.control, Control::Stopping { restart: true });
        match settle(&r.control, &self.model.link, self.model.now_ms) {
            // A daemon this app spawned starts again once the old one has exited and let go of the data dir.
            Some(Control::Stopped) if restart && matches!(r.job, Some(Job::Daemon(_))) => {}
            Some(Control::Stopped) if restart => r.start(self.model.now_ms),
            Some(Control::StartFailed(why)) => r.control = Control::StartFailed(r.failure(&why)),
            Some(next) => r.control = next,
            None => {}
        }
        // The socket is up: dial now rather than at the end of the backoff.
        let listening =
            r.socket.as_ref().is_some_and(|s| std::fs::symlink_metadata(s).is_ok_and(|m| m.file_type().is_socket()));
        if matches!(r.control, Control::Starting { .. })
            && matches!(self.model.link, Link::Disconnected { .. })
            && listening
        {
            let _ = self.cmds.try_send(NetCmd::Redial);
        }
        if self.reviewer.control != before {
            cx.notify();
        }
    }
}

/// A snapshot's reviewer state, staged on the model a playback left: the connection a stop or a failed start ends
/// with, and what the app would show for it.
pub fn stage(s: &mut Shell, state: crate::script::Staged) {
    use crate::script::Staged;
    use pr_review_tui::update::{Msg, NetEvent};
    let lost = |s: &mut Shell, reason: &str| {
        let msg = Msg::Net(NetEvent::Lost { reason: reason.into(), retry_in_ms: Some(1_000) });
        pr_review_tui::app::dispatch(&mut s.model, msg, &s.cmds);
    };
    // A playback has no socket; the staged reviewer dials the one 08-disconnected does.
    s.reviewer.socket = Some("/private/tmp/prdc/s.sock".into());
    match state {
        Staged::Stopped => {
            lost(s, "server closed the connection");
            s.reviewer.control = Control::Stopped;
        }
        Staged::Starting => {
            lost(s, "No such file or directory");
            s.reviewer.control = Control::Starting { since_ms: s.model.now_ms };
        }
        Staged::StartFailed => {
            lost(s, "No such file or directory");
            let why = s.reviewer.failure("pr-review daemon exited (exit status: 1) before it listened.");
            s.reviewer.control = Control::StartFailed(why);
        }
        Staged::StopConfirm => {
            s.overlay = Some(Overlay::ConfirmStop { restart: false, running: queue::running(&s.model).len() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Control, Launcher, START_TIMEOUT_MS, choose, parse_agent, settle, shell_quote};
    use pr_review_tui::model::Link;
    use pr_review_tui::protocol::Text;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_word_with_spaces_or_quotes_is_single_quoted() {
        assert_eq!(shell_quote("/private/tmp/prdc/s.sock"), "/private/tmp/prdc/s.sock");
        assert_eq!(
            shell_quote("/Users/d/Library/Application Support/pr-review/reviewer.sock"),
            "'/Users/d/Library/Application Support/pr-review/reviewer.sock'"
        );
        assert_eq!(shell_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
        assert_eq!(shell_quote(""), "''");
    }

    fn agent(json: &str) -> (Launcher, PathBuf) {
        let json = serde_json::from_str(json).unwrap();
        parse_agent("io.test.daemon", "/L/io.test.daemon.plist".into(), &json, Path::new("/default.sock"))
    }

    #[test]
    fn a_plist_says_which_socket_it_serves_and_whether_launchd_brings_it_back() {
        let (l, socket) = agent(
            r#"{"ProgramArguments":["/bin/pr-review","daemon","--allow-commands"],"KeepAlive":true,
                "StandardErrorPath":"/logs/d.log"}"#,
        );
        assert_eq!(socket, PathBuf::from("/default.sock"));
        assert_eq!(
            l,
            Launcher::LaunchAgent {
                label: "io.test.daemon".into(),
                plist: "/L/io.test.daemon.plist".into(),
                relaunches: true,
                log: Some("/logs/d.log".into())
            }
        );
        let (l, socket) = agent(
            r#"{"ProgramArguments":["pr-review","daemon","--socket","/p d/s.sock"],"KeepAlive":{"SuccessfulExit":false}}"#,
        );
        assert_eq!(socket, PathBuf::from("/p d/s.sock"));
        assert!(!l.relaunches(), "SuccessfulExit false keeps a clean exit stopped");
    }

    #[test]
    fn the_launch_agent_wins_only_for_its_own_socket_then_the_config_then_path() {
        let cfg = Path::new("/cfg/config.toml");
        let socket = Path::new("/s.sock");
        let mine = || Some(agent(r#"{"ProgramArguments":["pr-review","daemon","--socket","/s.sock"]}"#));
        let other = || Some(agent(r#"{"ProgramArguments":["pr-review","daemon"]}"#));
        assert!(matches!(choose(mine(), socket, Some("node x.mjs"), None, cfg), Ok(Launcher::LaunchAgent { .. })));
        let spawn = |c: &str| Ok(Launcher::Spawn { command: c.into() });
        assert_eq!(
            choose(other(), socket, Some(" node '/a b/pr-review.mjs' "), None, cfg),
            spawn("node '/a b/pr-review.mjs'")
        );
        let on_path = Path::new("/My Tools/pr-review");
        assert_eq!(choose(other(), socket, None, Some(on_path), cfg), spawn("'/My Tools/pr-review'"));
        assert_eq!(choose(None, socket, Some("node x.mjs"), Some(on_path), cfg), spawn("node x.mjs"));
        let err = choose(None, socket, Some("  "), None, cfg).unwrap_err();
        assert!(err.contains("Set daemon_command under [desktop] in /cfg/config.toml"), "{err}");
    }

    #[test]
    fn each_launcher_builds_its_start_with_paths_kept_whole() {
        let socket = Path::new("/Users/d/Library/Application Support/pr-review/reviewer.sock");
        let agent = Launcher::LaunchAgent {
            label: "io.test.daemon".into(),
            plist: "/Users/d/Library/LaunchAgents/io.test.daemon.plist".into(),
            relaunches: false,
            log: None,
        };
        assert_eq!(agent.start_argv(501, true, socket), ["launchctl", "kickstart", "gui/501/io.test.daemon"]);
        assert_eq!(
            agent.start_argv(501, false, socket),
            ["launchctl", "bootstrap", "gui/501", "/Users/d/Library/LaunchAgents/io.test.daemon.plist"]
        );
        assert_eq!(agent.bootout_argv(501).unwrap(), ["launchctl", "bootout", "gui/501/io.test.daemon"]);
        let spawn = Launcher::Spawn { command: "'/My Tools/pr-review'".into() };
        let argv = spawn.start_argv(501, false, socket);
        assert_eq!(
            argv,
            [
                "/bin/sh",
                "-c",
                "exec '/My Tools/pr-review' \"$@\"",
                "sh",
                "daemon",
                "--allow-commands",
                "--socket",
                "/Users/d/Library/Application Support/pr-review/reviewer.sock"
            ]
        );
        // What the shell makes of it: the quoted program and the socket as single words.
        let out = std::process::Command::new(&argv[0])
            .args(["-c", "printf '%s|' \"$@\"", "sh"])
            .args(&argv[4..])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "daemon|--allow-commands|--socket|/Users/d/Library/Application Support/pr-review/reviewer.sock|"
        );
        assert!(spawn.bootout_argv(501).is_none());
    }

    #[test]
    fn every_ask_says_why_it_cannot_run_on_a_live_read_only_or_older_daemon() {
        use super::{Ask, Reviewer};
        use pr_review_tui::model::{Look, Model, Screen};
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/playback/showcase.ndjson");
        let mut m = Model::new("unix:/s.sock", Look::default(), Screen::Review, None);
        let steps = crate::script::parse("wait:19000").unwrap();
        crate::script::play(pr_review_tui::playback::load(&path).unwrap(), &mut m, &steps, 20_000).unwrap();
        let r = Reviewer::new(Some("/s.sock".into()), Default::default(), "config.toml".into(), true);
        let why = |m: &Model, ask| r.block(m, ask);
        assert_eq!(why(&m, Ask::Start).as_deref(), Some("it is running"));
        assert_eq!(why(&m, Ask::Stop).as_deref(), Some("this daemon can't be stopped from here"));
        let caps = &mut m.server.as_mut().unwrap().capabilities;
        caps.push(Text::new("daemon.stop"));
        assert_eq!((why(&m, Ask::Stop), why(&m, Ask::Restart)), (None, None));
        m.server.as_mut().unwrap().capabilities.retain(|c| c.as_str() == "daemon.stop");
        assert_eq!(why(&m, Ask::Stop).as_deref(), Some("read-only daemon"));
        assert_eq!(why(&m, Ask::Restart).as_deref(), Some("read-only daemon"));
        let playback = Reviewer::new(None, Default::default(), "config.toml".into(), true);
        assert_eq!(playback.block(&m, Ask::Start).as_deref(), Some("this window plays a script"));
    }

    #[test]
    fn an_ask_settles_on_the_link() {
        let down = Link::Disconnected { reason: Text::new("closed"), retry_in_ms: Some(1000) };
        let starting = Control::Starting { since_ms: 1_000 };
        assert_eq!(settle(&starting, &down, 2_000), None);
        assert_eq!(settle(&starting, &Link::Syncing, 2_000), Some(Control::Idle));
        assert!(matches!(settle(&starting, &down, 1_000 + START_TIMEOUT_MS), Some(Control::StartFailed(_))));
        assert_eq!(settle(&Control::Stopping { restart: false }, &Link::Live, 0), None);
        assert_eq!(settle(&Control::Stopping { restart: true }, &down, 0), Some(Control::Stopped));
        assert_eq!(settle(&Control::Stopped, &down, 0), None);
        assert_eq!(settle(&Control::Stopped, &Link::Live, 0), Some(Control::Idle), "started from elsewhere");
        assert_eq!(settle(&Control::Idle, &down, 0), None);
    }
}
