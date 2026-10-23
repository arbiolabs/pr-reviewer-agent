//! CLI flags, `config.toml` (docs/TUI_SPEC.md section 14) and default paths. Flags override the file. Everything is
//! validated here, before the terminal enters raw mode.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::model::{Look, PrRef, Screen};
use crate::theme::Palette;

pub const USAGE: &str =
    "usage: pr-review-tui [--socket PATH] [--pr URL] [--ascii] [--theme NAME] [--playback FILE.ndjson] [--config FILE]
       pr-review-tui --version

  --socket PATH       reviewer control socket (default: config, then the platform runtime dir)
  --pr URL            open this PR, e.g. https://github.com/owner/repo/pull/184 or owner/repo#184
  --ascii             ASCII glyphs and borders only
  --theme NAME        auto, dark, light or mono (default: config, then auto, which asks the terminal)
  --playback FILE     play a scripted server session (protocol/playback/*.ndjson) instead of connecting
  --config FILE       config file (default: <config dir>/pr-review/config.toml)
  --version           print the version and exit

NO_COLOR set to anything non-empty turns colors off. q quits the client; reviews keep running in the daemon.";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub socket: Option<PathBuf>,
    pub pr: Option<String>,
    pub ascii: bool,
    pub theme: Option<String>,
    pub playback: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// What the command line asks for: run the client, or print `--help` or `--version` text and exit.
#[derive(Debug, PartialEq)]
pub enum Cli {
    Run(Args),
    Print(String),
}

pub fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut a = Args::default();
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--socket" => a.socket = Some(value("--socket")?.into()),
            "--pr" => a.pr = Some(value("--pr")?),
            "--theme" => a.theme = Some(value("--theme")?),
            "--playback" => a.playback = Some(value("--playback")?.into()),
            "--config" => a.config = Some(value("--config")?.into()),
            "--ascii" => a.ascii = true,
            "-h" | "--help" => return Ok(Cli::Print(USAGE.into())),
            "-V" | "--version" => return Ok(Cli::Print(format!("pr-review-tui {}", env!("CARGO_PKG_VERSION")))),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Cli::Run(a))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct File {
    connection: ConnectionSection,
    ui: UiSection,
    desktop: Desktop,
}

/// `[desktop]`: how pr-review-desktop starts the daemon when nothing answers. The terminal client ignores it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Desktop {
    /// The command that runs `pr-review`, as a shell would read it, when it is not on PATH:
    /// `node /path/to/bin/pr-review.mjs`. The app adds `daemon --allow-commands --socket <socket>`.
    pub daemon_command: Option<String>,
    /// The LaunchAgent the app starts and stops through launchctl when its plist is in ~/Library/LaunchAgents.
    pub launch_agent: String,
    /// The app's theme, as its Settings page writes it. The app reads the id; this file only carries it.
    pub theme: Option<String>,
}

impl Default for Desktop {
    fn default() -> Self {
        Desktop { daemon_command: None, launch_agent: "io.github.diego-arbio-dev.pr-review.daemon".into(), theme: None }
    }
}

/// Writes `theme = "<id>"` into the `[desktop]` section of the config file at `path`, adding the section or the
/// file when it is missing. Every other line stays as it was, comments included. Nothing is written unless the new
/// text parses, as a config file, to the old values plus the theme.
pub fn set_desktop_theme(path: &Path, id: &str) -> Result<(), String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let old = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(at(&e)),
    };
    let new = with_desktop_theme(&old, id);
    // The file's values with the theme taken out, and an emptied [desktop] with it.
    let without_theme = |text: &str| -> Result<(toml::Table, Option<toml::Value>), String> {
        let mut t: toml::Table = toml::from_str(text).map_err(|e| at(&e))?;
        let desktop = t.get_mut("desktop").and_then(|d| d.as_table_mut());
        let theme = desktop.and_then(|d| d.remove("theme"));
        if t.get("desktop").and_then(|d| d.as_table()).is_some_and(|d| d.is_empty()) {
            t.remove("desktop");
        }
        Ok((t, theme))
    };
    let (before, _) = without_theme(&old)?;
    let (after, theme) = without_theme(&new)?;
    toml::from_str::<File>(&new).map_err(|e| at(&e))?;
    if before != after || theme.as_ref().and_then(|v| v.as_str()) != Some(id) {
        return Err(at(&"can't set [desktop] theme without changing other keys; edit it by hand"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| at(&e))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, new).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| at(&e))
}

/// `text` with its `[desktop]` section's `theme` line replaced, or added under the section's header, or a new
/// section at the end.
fn with_desktop_theme(text: &str, id: &str) -> String {
    let line = format!("theme = {}", toml::Value::from(id));
    let is_desktop = |l: &str| {
        l.trim().strip_prefix("[desktop]").is_some_and(|rest| rest.trim().is_empty() || rest.trim().starts_with('#'))
    };
    let mut lines: Vec<&str> = text.lines().collect();
    let Some(header) = lines.iter().position(|l| is_desktop(l)) else {
        let mut out = text.to_string();
        if !out.is_empty() {
            out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
        }
        return format!("{out}[desktop]\n{line}\n");
    };
    let end = lines[header + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| header + 1 + i);
    let theme = (header + 1..end)
        .find(|&i| lines[i].trim_start().strip_prefix("theme").is_some_and(|rest| rest.trim_start().starts_with('=')));
    match theme {
        Some(i) => lines[i] = &line,
        None => lines.insert(header + 1, &line),
    }
    lines.join("\n") + "\n"
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ConnectionSection {
    socket: String,
    reconnect: bool,
    heartbeat_timeout_secs: u64,
    request_timeout_secs: u64,
}

impl Default for ConnectionSection {
    fn default() -> Self {
        ConnectionSection {
            socket: "auto".into(),
            reconnect: true,
            heartbeat_timeout_secs: 30,
            request_timeout_secs: 15,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct UiSection {
    theme: String,
    start_view: String,
    show_activity: bool,
    ascii: bool,
    background: String,
    mouse: bool,
}

impl Default for UiSection {
    fn default() -> Self {
        UiSection {
            theme: "auto".into(),
            start_view: "review".into(),
            show_activity: true,
            ascii: false,
            background: "theme".into(),
            mouse: true,
        }
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
    pub request_timeout: Duration,
    pub look: Look,
    pub start: Screen,
    pub wanted_pr: Option<PrRef>,
    pub config_path: PathBuf,
    /// Mouse capture, which turns the terminal's own drag-to-select into a modifier-drag.
    pub mouse: bool,
    pub desktop: Desktop,
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

/// `https://github.com/owner/repo/pull/184` or `owner/repo#184`.
pub fn parse_pr(s: &str) -> Result<PrRef, String> {
    PrRef::parse(s).ok_or_else(|| format!("--pr {s}: expected https://github.com/owner/repo/pull/N or owner/repo#N"))
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

/// `detect` asks the terminal for its background, and runs only for `theme = "auto"` with color on.
pub fn resolve(args: Args, no_color: bool, detect: impl FnOnce() -> Palette) -> Result<Settings, String> {
    let config_path = args.config.clone().unwrap_or_else(|| config_dir().join("config.toml"));
    let file: File = match std::fs::read_to_string(&config_path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", config_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && args.config.is_none() => File::default(),
        Err(e) => return Err(format!("{}: {e}", config_path.display())),
    };
    let (key, theme) = match &args.theme {
        Some(t) => ("--theme", t.as_str()),
        None => ("ui.theme", file.ui.theme.as_str()),
    };
    let (color, palette) = match theme {
        "auto" if no_color => (false, Palette::Dark),
        "auto" => (true, detect()),
        "dark" => (!no_color, Palette::Dark),
        "light" => (!no_color, Palette::Light),
        "mono" => (false, Palette::Dark),
        other => return Err(format!("{key}: {other:?} is not supported (auto, dark, light, mono)")),
    };
    let terminal_bg = match file.ui.background.as_str() {
        "theme" => false,
        "terminal" => true,
        other => return Err(format!("ui.background: {other:?} is not supported (theme, terminal)")),
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
    if file.connection.request_timeout_secs == 0 {
        return Err("connection.request_timeout_secs must be at least 1".into());
    }
    Ok(Settings {
        source: match args.playback {
            Some(p) => Source::Playback(p),
            None => Source::Socket(socket),
        },
        reconnect: file.connection.reconnect,
        heartbeat_timeout: Duration::from_secs(file.connection.heartbeat_timeout_secs),
        request_timeout: Duration::from_secs(file.connection.request_timeout_secs),
        look: Look {
            ascii: args.ascii || file.ui.ascii,
            color,
            show_activity: file.ui.show_activity,
            terminal_bg,
            palette,
        },
        start,
        wanted_pr,
        config_path,
        mouse: file.ui.mouse,
        desktop: file.desktop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Cli, String> {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags_parse_and_unknown_ones_are_rejected() {
        let Cli::Run(a) = args(&["--socket", "/s.sock", "--ascii", "--pr", "o/r#1"]).unwrap() else { panic!() };
        assert_eq!((a.socket, a.ascii, a.pr.as_deref()), (Some(PathBuf::from("/s.sock")), true, Some("o/r#1")));
        assert_eq!(args(&["--help"]).unwrap(), Cli::Print(USAGE.into()));
        assert_eq!(args(&["--sockt"]).unwrap_err(), "unknown argument --sockt");
        assert_eq!(args(&["--socket"]).unwrap_err(), "--socket needs a value");
    }

    #[test]
    fn pr_urls_become_repository_and_number() {
        let pr = |s| parse_pr(s).unwrap().to_string();
        assert_eq!(pr("https://github.com/owner/repo/pull/184"), "owner/repo#184");
        assert_eq!(pr("https://github.com/owner/repo/pull/184/files"), "owner/repo#184");
        assert_eq!(pr("acme/shop#7"), "acme/shop#7");
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
        let s = resolve(a, true, || Palette::Dark).unwrap();
        assert_eq!(s.source, Source::Socket("/from/file.sock".into()));
        assert_eq!((s.look.ascii, s.look.color, s.start), (true, false, Screen::Timeline));
        let a = Args { config: Some(path.clone()), socket: Some("/flag.sock".into()), ..Args::default() };
        assert_eq!(resolve(a, false, || Palette::Dark).unwrap().source, Source::Socket("/flag.sock".into()));

        assert!(!s.look.terminal_bg);
        assert!(s.mouse, "mouse capture is on by default");
        std::fs::write(&path, "[ui]\nbackground = \"terminal\"\n").unwrap();
        assert!(
            resolve(Args { config: Some(path.clone()), ..Args::default() }, false, || Palette::Dark)
                .unwrap()
                .look
                .terminal_bg
        );
        std::fs::write(&path, "[ui]\nbackground = \"clear\"\n").unwrap();
        let err = resolve(Args { config: Some(path.clone()), ..Args::default() }, false, || Palette::Dark).unwrap_err();
        assert!(err.contains("ui.background"), "{err}");

        assert_eq!(s.desktop, Desktop::default());
        std::fs::write(&path, "[desktop]\ndaemon_command = \"node /opt/pr review/bin/pr-review.mjs\"\n").unwrap();
        let desktop =
            resolve(Args { config: Some(path.clone()), ..Args::default() }, false, || Palette::Dark).unwrap().desktop;
        assert_eq!(desktop.daemon_command.as_deref(), Some("node /opt/pr review/bin/pr-review.mjs"));
        assert_eq!(desktop.launch_agent, "io.github.diego-arbio-dev.pr-review.daemon");

        std::fs::write(&path, "[ui]\nascci = true\n").unwrap();
        let err = resolve(Args { config: Some(path), ..Args::default() }, false, || Palette::Dark).unwrap_err();
        assert!(err.contains("unknown field `ascci`"), "{err}");
    }

    #[test]
    fn the_desktop_theme_is_written_beside_the_keys_already_there() {
        let dir = std::env::temp_dir().join(format!("pr-review-tui-desktop-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/config.toml");
        let read = || std::fs::read_to_string(&path).unwrap();

        set_desktop_theme(&path, "nord").unwrap();
        assert_eq!(read(), "[desktop]\ntheme = \"nord\"\n");

        let user = "# mine\n[ui]\nascii = true # keep\n\n[desktop] # the app\nlaunch_agent = \"x.y\"\n\n[connection]\nreconnect = false\n";
        std::fs::write(&path, user).unwrap();
        set_desktop_theme(&path, "solarized-dark").unwrap();
        assert_eq!(
            read(),
            "# mine\n[ui]\nascii = true # keep\n\n[desktop] # the app\ntheme = \"solarized-dark\"\nlaunch_agent = \"x.y\"\n\n[connection]\nreconnect = false\n"
        );
        set_desktop_theme(&path, "light").unwrap();
        assert_eq!(read(), user.replace("[desktop] # the app\n", "[desktop] # the app\ntheme = \"light\"\n"));
        let desktop =
            resolve(Args { config: Some(path.clone()), ..Args::default() }, false, || Palette::Dark).unwrap().desktop;
        assert_eq!((desktop.theme.as_deref(), desktop.launch_agent.as_str()), (Some("light"), "x.y"));

        std::fs::write(&path, "[ui]\nascii = true").unwrap();
        set_desktop_theme(&path, "dark").unwrap();
        assert_eq!(read(), "[ui]\nascii = true\n\n[desktop]\ntheme = \"dark\"\n");

        // A [desktop] table the line edit can't see into is left alone rather than rewritten.
        let inline = "desktop = { launch_agent = \"x.y\" }\n";
        std::fs::write(&path, inline).unwrap();
        assert!(set_desktop_theme(&path, "nord").is_err());
        assert_eq!(read(), inline);
    }

    #[test]
    fn auto_asks_the_terminal_and_the_flag_overrides_the_file() {
        let dir = std::env::temp_dir().join(format!("pr-review-tui-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let look = |file: &str, flag: Option<&str>, no_color: bool, detected: Palette| {
            std::fs::write(&path, file).unwrap();
            let a = Args { config: Some(path.clone()), theme: flag.map(String::from), ..Args::default() };
            resolve(a, no_color, || detected).map(|s| (s.look.color, s.look.palette))
        };
        assert_eq!(look("", None, false, Palette::Light), Ok((true, Palette::Light)), "auto is the default");
        assert_eq!(look("", None, true, Palette::Light), Ok((false, Palette::Dark)), "NO_COLOR skips detection");
        let light = "[ui]\ntheme = \"light\"\n";
        assert_eq!(look(light, None, false, Palette::Dark), Ok((true, Palette::Light)));
        assert_eq!(look(light, Some("dark"), false, Palette::Light), Ok((true, Palette::Dark)));
        assert_eq!(look(light, Some("mono"), false, Palette::Light), Ok((false, Palette::Dark)));
        assert_eq!(look("", Some("auto"), false, Palette::Light), Ok((true, Palette::Light)));
        let err = look("", Some("sepia"), false, Palette::Dark).unwrap_err();
        assert_eq!(err, "--theme: \"sepia\" is not supported (auto, dark, light, mono)");
        let err = look("[ui]\ntheme = \"sepia\"\n", None, false, Palette::Dark).unwrap_err();
        assert!(err.starts_with("ui.theme:"), "{err}");
        let Ok(Cli::Run(a)) = args(&["--theme", "light"]) else { panic!() };
        assert_eq!(a.theme.as_deref(), Some("light"));
    }
}
