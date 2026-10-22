//! The macOS client: the TUI crate's reducer and network task behind a GPUI window.

mod capture;
mod cards;
mod code;
mod detail;
mod diff;
mod kit;
mod live;
mod overlay;
mod publish;
mod queue;
mod script;
mod setup;
mod timeline;
mod tokens;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
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
        let view = View { model, cmds, desk: desk(&opts.steps), still: true, snapshot: true };
        run_app(view, None, snapshot, opts.window, opts.appearance);
        drop(cmd_rx);
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
    let snapshot = opts.snapshot.clone().map(|out| (out, opts.after_ms));
    let is_snapshot = opts.snapshot.is_some();
    // `rt` lives until the process exits, since `run_app` returns only after the app quits.
    let view = View { model, cmds, desk: Vec::new(), still: is_snapshot || reduce_motion(), snapshot: is_snapshot };
    run_app(view, Some(msgs), snapshot, opts.window, opts.appearance);
    drop(rt);
    Ok(())
}

/// What the window opens with: the model, the network's queue, and the desktop's own view steps.
struct View {
    model: Model,
    cmds: mpsc::Sender<NetCmd>,
    desk: Vec<script::Desk>,
    still: bool,
    snapshot: bool,
}

fn desk(steps: &[script::Step]) -> Vec<script::Desk> {
    steps.iter().filter_map(|s| if let script::Step::Desk(d) = s { Some(d.clone()) } else { None }).collect()
}

/// macOS's Reduce motion setting, or `PR_REVIEW_REDUCE_MOTION=1`.
fn reduce_motion() -> bool {
    if std::env::var_os("PR_REVIEW_REDUCE_MOTION").is_some_and(|v| v == "1") {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
    #[cfg(not(target_os = "macos"))]
    false
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
    view: View,
    msgs: Option<mpsc::Receiver<Msg>>,
    snapshot: Option<(PathBuf, u64)>,
    (w, h): (f32, f32),
    appearance: Option<ThemeMode>,
) {
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        match appearance {
            Some(mode) => Theme::change(mode, None, cx),
            None => Theme::sync_system_appearance(None, cx),
        }
        cx.on_action(|_: &ui::Quit, cx| cx.quit());
        // A snapshot ignores the keyboard: its window takes focus, and keys typed elsewhere must not land in it.
        if !view.snapshot {
            cx.bind_keys(ui::bindings());
        }
        cx.set_menus(ui::menus());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))),
            // The sidebar and the title bar let a blur of the desktop through; a snapshot stays opaque, since its
            // pixels must not depend on what is behind the window.
            window_background: if view.snapshot {
                WindowBackgroundAppearance::Opaque
            } else {
                WindowBackgroundAppearance::Blurred
            },
            titlebar: Some(TitlebarOptions {
                title: Some("pr-review".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(18.), px(18.))),
            }),
            window_min_size: Some(size(px(960.), px(600.))),
            ..Default::default()
        };
        let View { model, cmds, desk, still, snapshot: is_snapshot } = view;
        let built = gpui_kit::open_window(options, cx, |window, cx| {
            let shell = cx.new(|cx| ui::Shell::new(model, cmds, still, is_snapshot, &desk, cx));
            let focus = shell.read(cx).focus.clone();
            window.focus(&focus, cx);
            shell
        });
        let (window, shell) = match built {
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

/// How long one frame may take to present before the harness asks for another.
const FRAME_TIMEOUT: Duration = Duration::from_millis(1500);

/// Captures once the window has settled: key (so the traffic lights are colored), at its full size (macOS animates
/// a new window in), painted (a frame before the first paint is one flat color) and showing the same pixels on two
/// captures running. A frame that never presents is asked for again, up to the attempt budget.
async fn capture_settled(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    size: (f32, f32),
) -> Result<capture::Image, String> {
    let (mut last, mut expected): (Option<capture::Image>, usize) = (None, 0);
    let mut timeouts = 0;
    for _ in 0..50 {
        let presented = Arc::new(AtomicBool::new(false));
        let flag = presented.clone();
        let scale = cx
            .update_window(window, |_, window, cx| {
                cx.activate(true);
                window.activate_window();
                window.refresh();
                window.on_next_frame(move |_, _| flag.store(true, Ordering::SeqCst));
                window.scale_factor()
            })
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        while !presented.load(Ordering::SeqCst) {
            if started.elapsed() > FRAME_TIMEOUT {
                timeouts += 1;
                break;
            }
            cx.background_executor().timer(Duration::from_millis(20)).await;
        }
        if !presented.load(Ordering::SeqCst) {
            continue;
        }
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
            "the window never settled to the same painted frame twice; the last capture was {}x{} (expected width {}), {}, after {timeouts} frames that never presented",
            l.width,
            l.height,
            expected,
            if l.is_uniform() { "one flat color" } else { "painted" }
        ),
        None => format!("the window never presented a frame ({timeouts} attempts timed out)"),
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
