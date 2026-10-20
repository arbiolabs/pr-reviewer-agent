//! CLI flags, `config.toml` (TUI_SPEC.md section 14) and default paths. Flags override the file. Everything is
//! validated here, before the terminal enters raw mode.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::model::{Look, Screen};
use crate::protocol::PrId;

pub const USAGE: &str =
    "usage: pr-review-tui [--socket PATH] [--pr URL] [--ascii] [--playback FILE.ndjson] [--config FILE]

  --socket PATH       reviewer control socket (default: config, then the platform runtime dir)
  --pr URL            open this PR, e.g. https://github.com/owner/repo/pull/184 or owner/repo#184
  --ascii             ASCII glyphs and borders only
  --playback FILE     play a scripted server session (protocol/playback/*.ndjson) instead of connecting
  --config FILE       config file (default: <config dir>/pr-review/config.toml)

NO_COLOR set to anything non-empty turns colors off. q quits the client; reviews keep running in the daemon.";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub socket: Option<PathBuf>,
    pub pr: Option<String>,
    pub ascii: bool,
    pub playback: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

pub fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args::default();
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--socket" => a.socket = Some(value("--socket")?.into()),
            "--pr" => a.pr = Some(value("--pr")?),
            "--playback" => a.playback = Some(value("--playback")?.into()),
            "--config" => a.config = Some(value("--config")?.into()),
            "--ascii" => a.ascii = true,
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Some(a))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct File {
    connection: ConnectionSection,
    ui: UiSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ConnectionSection {
    socket: String,
    reconnect: bool,
    heartbeat_timeout_secs: u64,
}

impl Default for ConnectionSection {
    fn default() -> Self {
        ConnectionSection { socket: "auto".into(), reconnect: true, heartbeat_timeout_secs: 30 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct UiSection {
    theme: String,
    start_view: String,
    show_activity: bool,
    ascii: bool,
}

impl Default for UiSection {
    fn default() -> Self {
        UiSection { theme: "dark".into(), start_view: "inbox".into(), show_activity: true, ascii: false }
    }
}

#[derive(Debug, PartialEq)]
pub enum Source {
    Socket(PathBuf),
    Playback(PathBuf),
}

#[derive(Debug, PartialEq)]
pub struct Settings {
    pub source: Source,
    pub reconnect: bool,
    pub heartbeat_timeout: Duration,
    pub look: Look,
    pub start: Screen,
    pub wanted_pr: Option<PrId>,
    pub config_path: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn app_support() -> PathBuf {
    home().join("Library/Application Support/pr-review")
}

/// `$XDG_RUNTIME_DIR/pr-review/reviewer.sock`, else the macOS application-support dir, else `~/.local/state`.
pub fn default_socket() -> PathBuf {
    match env_dir("XDG_RUNTIME_DIR") {
        Some(d) => d.join("pr-review/reviewer.sock"),
        None if cfg!(target_os = "macos") => app_support().join("reviewer.sock"),
        None => home().join(".local/state/pr-review/reviewer.sock"),
    }
}

pub fn config_dir() -> PathBuf {
    match env_dir("XDG_CONFIG_HOME") {
        Some(d) => d.join("pr-review"),
        None if cfg!(target_os = "macos") => app_support(),
        None => home().join(".config/pr-review"),
    }
}

pub fn state_dir() -> PathBuf {
    match env_dir("XDG_STATE_HOME") {
        Some(d) => d.join("pr-review"),
        None if cfg!(target_os = "macos") => app_support(),
        None => home().join(".local/state/pr-review"),
    }
}

/// `https://github.com/owner/repo/pull/184` or `owner/repo#184`, as the protocol's PR id `owner/repo#184`.
pub fn parse_pr(s: &str) -> Result<PrId, String> {
    let bad = || format!("--pr {s}: expected https://github.com/owner/repo/pull/N or owner/repo#N");
    let (repo, number) = if let Some((repo, n)) = s.split_once('#') {
        (repo.to_string(), n.to_string())
    } else {
        let path = s.split("://").nth(1).ok_or_else(bad)?;
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        match parts[..] {
            [_host, owner, repo, "pull", n, ..] => (format!("{owner}/{repo}"), n.to_string()),
            _ => return Err(bad()),
        }
    };
    let n: u64 = number.parse().map_err(|_| bad())?;
    if repo.split('/').count() != 2 || repo.split('/').any(str::is_empty) {
        return Err(bad());
    }
    Ok(PrId::new(&format!("{repo}#{n}")))
}

fn screen(name: &str) -> Result<Screen, String> {
    Ok(match name {
        "inbox" => Screen::Inbox,
        "review" => Screen::Review,
        "diff" => Screen::Diff,
        "timeline" => Screen::Timeline,
        "settings" => Screen::Settings,
        other => return Err(format!("ui.start_view: unknown view {other:?}")),
    })
}

pub fn resolve(args: Args, no_color: bool) -> Result<Settings, String> {
    let config_path = args.config.clone().unwrap_or_else(|| config_dir().join("config.toml"));
    let file: File = match std::fs::read_to_string(&config_path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", config_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && args.config.is_none() => File::default(),
        Err(e) => return Err(format!("{}: {e}", config_path.display())),
    };
    let color = match file.ui.theme.as_str() {
        "dark" => !no_color,
        "mono" => false,
        other => return Err(format!("ui.theme: {other:?} is not supported (dark, mono)")),
    };
    let socket = match (args.socket, file.connection.socket.as_str()) {
        (Some(p), _) => p,
        (None, "auto") => default_socket(),
        (None, path) => Path::new(path).to_path_buf(),
    };
    let wanted_pr = args.pr.as_deref().map(parse_pr).transpose()?;
    let start = if wanted_pr.is_some() { Screen::Review } else { screen(&file.ui.start_view)? };
    if file.connection.heartbeat_timeout_secs == 0 {
        return Err("connection.heartbeat_timeout_secs must be at least 1".into());
    }
    Ok(Settings {
        source: match args.playback {
            Some(p) => Source::Playback(p),
            None => Source::Socket(socket),
        },
        reconnect: file.connection.reconnect,
        heartbeat_timeout: Duration::from_secs(file.connection.heartbeat_timeout_secs),
        look: Look { ascii: args.ascii || file.ui.ascii, color, show_activity: file.ui.show_activity },
        start,
        wanted_pr,
        config_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Option<Args>, String> {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags_parse_and_unknown_ones_are_rejected() {
        let a = args(&["--socket", "/s.sock", "--ascii", "--pr", "o/r#1"]).unwrap().unwrap();
        assert_eq!((a.socket, a.ascii, a.pr.as_deref()), (Some(PathBuf::from("/s.sock")), true, Some("o/r#1")));
        assert_eq!(args(&["--help"]).unwrap(), None);
        assert_eq!(args(&["--sockt"]).unwrap_err(), "unknown argument --sockt");
        assert_eq!(args(&["--socket"]).unwrap_err(), "--socket needs a value");
    }

    #[test]
    fn pr_urls_become_protocol_ids() {
        assert_eq!(parse_pr("https://github.com/owner/repo/pull/184").unwrap(), PrId::new("owner/repo#184"));
        assert_eq!(parse_pr("https://github.com/owner/repo/pull/184/files").unwrap(), PrId::new("owner/repo#184"));
        assert_eq!(parse_pr("acme/shop#7").unwrap(), PrId::new("acme/shop#7"));
        assert!(parse_pr("https://github.com/owner/repo/issues/1").is_err());
        assert!(parse_pr("shop#x").is_err());
    }

    #[test]
    fn the_config_file_is_validated_and_flags_win() {
        let dir = std::env::temp_dir().join(format!("pr-review-tui-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[connection]\nsocket = \"/from/file.sock\"\n[ui]\nascii = true\nstart_view = \"timeline\"\n",
        )
        .unwrap();
        let a = Args { config: Some(path.clone()), ..Args::default() };
        let s = resolve(a, true).unwrap();
        assert_eq!(s.source, Source::Socket("/from/file.sock".into()));
        assert_eq!((s.look.ascii, s.look.color, s.start), (true, false, Screen::Timeline));
        let a = Args { config: Some(path.clone()), socket: Some("/flag.sock".into()), ..Args::default() };
        assert_eq!(resolve(a, false).unwrap().source, Source::Socket("/flag.sock".into()));

        std::fs::write(&path, "[ui]\nascci = true\n").unwrap();
        let err = resolve(Args { config: Some(path), ..Args::default() }, false).unwrap_err();
        assert!(err.contains("unknown field `ascci`"), "{err}");
    }
}
