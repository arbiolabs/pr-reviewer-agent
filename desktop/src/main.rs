//! The macOS client: the TUI crate's reducer and network task behind a GPUI window.

mod capture;
mod script;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::*;
use pr_review_tui::app::dispatch;
use pr_review_tui::config::{self, Source};
use pr_review_tui::model::Model;
use pr_review_tui::net::{self, NetCmd, NetConfig, QUEUE};
use pr_review_tui::playback;
use pr_review_tui::theme::Palette;
use pr_review_tui::update::Msg;
use tokio::sync::mpsc;

const USAGE: &str = "usage: pr-review-desktop [--socket PATH] [--pr URL] [--playback FILE.ndjson] [--config FILE]
       [--snapshot OUT.png [--snapshot-after-ms N] [--actions STEPS]] [--window WxH] [--appearance dark|light]

  --snapshot OUT.png      capture the window to OUT.png, then quit
  --snapshot-after-ms N   when to capture, in milliseconds from launch (default 1000); with --playback the script
                          plays in virtual time, so the capture is the same however long rendering takes
  --actions STEPS         with --playback and --snapshot: comma-separated steps, such as down,open,goto:diff,wait:300;
                          see desktop/README.md
  --window WxH            window size in points (default 1280x800)
  --appearance MODE       dark or light (default: the system's)

The other flags are pr-review-tui's.";

struct Opts {
    tui: config::Args,
    snapshot: Option<PathBuf>,
    after_ms: u64,
    steps: Vec<script::Step>,
    window: (f32, f32),
    appearance: Option<ThemeMode>,
}

enum Cli {
    Run(Opts),
    Print(String),
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut rest = Vec::new();
    let (mut snapshot, mut after_ms, mut steps, mut window, mut appearance) = (None, 1000, Vec::new(), None, None);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--snapshot" => snapshot = Some(PathBuf::from(value("--snapshot")?)),
            "--snapshot-after-ms" => {
                after_ms =
                    value("--snapshot-after-ms")?.parse().map_err(|_| "--snapshot-after-ms takes milliseconds")?
            }
            "--actions" => steps = script::parse(&value("--actions")?)?,
            "--window" => window = Some(size_arg(&value("--window")?)?),
            "--appearance" => {
                appearance = Some(match value("--appearance")?.as_str() {
                    "dark" => ThemeMode::Dark,
                    "light" => ThemeMode::Light,
                    other => return Err(format!("--appearance: {other:?} is not dark or light")),
                })
            }
            "-h" | "--help" => return Ok(Cli::Print(USAGE.into())),
            "-V" | "--version" => return Ok(Cli::Print(format!("pr-review-desktop {}", env!("CARGO_PKG_VERSION")))),
            _ => rest.push(flag),
        }
    }
    let tui = match config::parse_args(rest.into_iter())? {
        config::Cli::Run(a) => a,
        config::Cli::Print(text) => return Ok(Cli::Print(text)),
    };
    if !steps.is_empty() && (snapshot.is_none() || tui.playback.is_none()) {
        return Err("--actions needs --snapshot and --playback".into());
    }
    Ok(Cli::Run(Opts { tui, snapshot, after_ms, steps, window: window.unwrap_or((1280., 800.)), appearance }))
}

fn size_arg(v: &str) -> Result<(f32, f32), String> {
    let parse = |n: &str| n.parse::<f32>().ok().filter(|n| *n >= 100.);
    match v.split_once('x').map(|(w, h)| (parse(w), parse(h))) {
        Some((Some(w), Some(h))) => Ok((w, h)),
        _ => Err(format!("--window: {v:?} is not WxH, each at least 100")),
    }
}

/// The window's view: the TUI's model, and the channel its effects go out on.
struct Shell {
    model: Model,
    cmds: mpsc::Sender<NetCmd>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.model;
        let theme = cx.theme();
        let inbox = m.inbox();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(".SystemUIFont")
            .child(div().text_color(theme.muted_foreground).child(format!(
                "{:?} · {:?} · {} panels · {}",
                m.link,
                m.ui.screen,
                m.ui.modals.len(),
                m.diag.endpoint
            )))
            .child(div().pb_2().child(format!("{} PRs", inbox.len())))
            .children(inbox.into_iter().map(|p| {
                let selected = m.ui.selected_pr.as_ref() == Some(&p.id);
                div().child(format!("{} #{} {}", if selected { "▸" } else { " " }, p.number, p.title.as_str()))
            }))
    }
}

fn main() -> ExitCode {
    let opts = match parse_args(std::env::args().skip(1)) {
        Ok(Cli::Run(o)) => o,
        Ok(Cli::Print(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("pr-review-desktop: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match start(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pr-review-desktop: {e}");
            ExitCode::from(2)
        }
    }
}

fn start(opts: Opts) -> Result<(), String> {
    // The look is the terminal's; the window takes its colors from the gpui-kit theme.
    let settings = config::resolve(opts.tui, false, || Palette::Dark)?;
    let endpoint = match &settings.source {
        Source::Socket(p) => format!("unix:{}", p.display()),
        Source::Playback(p) => format!("playback:{}", p.display()),
    };
    let mut model = Model::new(endpoint, settings.look, settings.start, settings.wanted_pr);
    model.request_timeout_ms = settings.request_timeout.as_millis() as u64;
    let (cmds, cmd_rx) = mpsc::channel::<NetCmd>(QUEUE);

    // A playback snapshot is played to its moment before the window opens and the window shows it still.
    if let (Source::Playback(p), Some(_)) = (&settings.source, &opts.snapshot) {
        script::play(playback::load(p)?, &mut model, &opts.steps, opts.after_ms)?;
        let snapshot = opts.snapshot.map(|out| (out, 0));
        run_app(Shell { model, cmds }, None, snapshot, opts.window, opts.appearance);
        return Ok(());
    }

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    let (msg_tx, msgs) = mpsc::channel::<Msg>(QUEUE);
    match &settings.source {
        Source::Playback(p) => drop(rt.spawn(playback::run(playback::load(p)?, msg_tx.clone(), cmd_rx))),
        Source::Socket(socket) => {
            let cfg = NetConfig {
                socket: socket.clone(),
                heartbeat_timeout: settings.heartbeat_timeout,
                reconnect: settings.reconnect,
            };
            drop(rt.spawn(net::run(cfg, msg_tx.clone(), cmd_rx)));
        }
    }
    drop(rt.spawn(ticks(msg_tx)));
    let snapshot = opts.snapshot.map(|out| (out, opts.after_ms));
    // `rt` lives until the process exits, since `run_app` returns only after the app quits.
    run_app(Shell { model, cmds }, Some(msgs), snapshot, opts.window, opts.appearance);
    drop(rt);
    Ok(())
}

/// `Msg::Tick` at the TUI's cadence, through the same queue as the network's messages.
async fn ticks(tx: mpsc::Sender<Msg>) {
    let started = Instant::now();
    let mut every = tokio::time::interval(Duration::from_millis(script::TICK_MS));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        every.tick().await;
        if tx.send(Msg::Tick(started.elapsed().as_millis() as u64)).await.is_err() {
            return;
        }
    }
}

/// Opens the window and, with `msgs`, feeds the reducer on the main thread; with `snapshot`, captures the window
/// that many milliseconds after it opens and quits.
fn run_app(
    shell: Shell,
    msgs: Option<mpsc::Receiver<Msg>>,
    snapshot: Option<(PathBuf, u64)>,
    (w, h): (f32, f32),
    appearance: Option<ThemeMode>,
) {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        match appearance {
            Some(mode) => Theme::change(mode, None, cx),
            None => Theme::sync_system_appearance(None, cx),
        }
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("pr-review".into()), ..Default::default() }),
            ..Default::default()
        };
        let (window, shell) = match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| shell)) {
            Ok(opened) => opened,
            Err(e) => fail(format!("could not open a window: {e}")),
        };
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
        if let Some(mut msgs) = msgs {
            cx.spawn(async move |cx| {
                while let Some(msg) = msgs.recv().await {
                    let quit = shell.update(cx, |s, cx| {
                        let quit = dispatch(&mut s.model, msg, &s.cmds);
                        if std::mem::take(&mut s.model.dirty) {
                            cx.notify();
                        }
                        quit
                    });
                    if quit {
                        cx.update(|cx| cx.quit());
                        return;
                    }
                }
            })
            .detach();
        }
        if let Some((out, after_ms)) = snapshot {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(after_ms)).await;
                match capture_settled(window, cx, (w, h)).await {
                    Ok(image) => match image.save(&out) {
                        Ok(()) => cx.update(|cx| cx.quit()),
                        Err(e) => fail(e),
                    },
                    Err(e) => fail(e),
                }
            })
            .detach();
        }
    });
}

/// Captures once the window has settled: at its full size (macOS animates a new window in), painted (a frame before
/// the first paint is one flat color) and showing the same pixels on two captures running.
async fn capture_settled(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    size: (f32, f32),
) -> Result<capture::Image, String> {
    let (mut last, mut expected): (Option<capture::Image>, usize) = (None, 0);
    for _ in 0..50 {
        let (done, presented) = tokio::sync::oneshot::channel();
        let scale = cx
            .update_window(window, |_, window, _| {
                window.refresh();
                window.on_next_frame(move |_, _| {
                    let _ = done.send(());
                });
                window.scale_factor()
            })
            .map_err(|e| e.to_string())?;
        let _ = presented.await;
        let image = cx.update(|_| capture::window(size.1, scale))?;
        expected = (size.0 * scale).round() as usize;
        if image.width == expected && !image.is_uniform() && last.as_ref().is_some_and(|l| l.rgba == image.rgba) {
            return Ok(image);
        }
        last = Some(image);
        cx.background_executor().timer(Duration::from_millis(50)).await;
    }
    Err(match last {
        Some(l) => format!(
            "the window never settled to the same painted frame twice; the last capture was {}x{} (expected width {}), {}",
            l.width,
            l.height,
            expected,
            if l.is_uniform() { "one flat color" } else { "painted" }
        ),
        None => "the window was never captured".into(),
    })
}

fn fail(e: impl std::fmt::Display) -> ! {
    eprintln!("pr-review-desktop: {e}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::{Cli, PathBuf, parse_args};

    fn args(s: &str) -> Result<Cli, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn desktop_flags_come_off_and_the_rest_go_to_the_tui_parser() {
        let Ok(Cli::Run(o)) =
            args("--playback x.ndjson --snapshot o.png --window 800x600 --actions down,open --pr a/b#1")
        else {
            panic!()
        };
        assert_eq!(o.tui.playback, Some(PathBuf::from("x.ndjson")));
        assert_eq!(o.tui.pr.as_deref(), Some("a/b#1"));
        assert_eq!(o.snapshot, Some(PathBuf::from("o.png")));
        assert_eq!(o.window, (800., 600.));
        assert_eq!(o.steps.len(), 2);
        assert_eq!(args("--actions down").err().as_deref(), Some("--actions needs --snapshot and --playback"));
        assert_eq!(args("--bogus").err().as_deref(), Some("unknown argument --bogus"));
        assert!(args("--window 10x10").is_err());
    }
}
