use std::fs::OpenOptions;
use std::os::unix::fs::DirBuilderExt;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, MouseEventKind};
use pr_review_tui::app::dispatch;
use pr_review_tui::config::{self, Source};
use pr_review_tui::model::Model;
use pr_review_tui::net::{self, NetCmd, NetConfig, QUEUE};
use pr_review_tui::playback;
use pr_review_tui::terminal::TerminalGuard;
use pr_review_tui::update::Msg;
use pr_review_tui::view::view;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

const FRAME: Duration = Duration::from_millis(16);
const TICK: Duration = Duration::from_millis(125);

fn main() -> ExitCode {
    let args = match config::parse_args(std::env::args().skip(1)) {
        Ok(config::Cli::Run(a)) => a,
        Ok(config::Cli::Print(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("pr-review-tui: {e}\n\n{}", config::USAGE);
            return ExitCode::from(2);
        }
    };
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let settings = match config::resolve(args, no_color, pr_review_tui::terminal::detect_palette) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pr-review-tui: {e}");
            return ExitCode::from(2);
        }
    };
    let script = match &settings.source {
        Source::Playback(p) => match playback::load(p) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("pr-review-tui: {e}");
                return ExitCode::from(2);
            }
        },
        Source::Socket(_) => None,
    };
    init_logging();
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("pr-review-tui: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(run(settings, script)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pr-review-tui: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Logs go to `<state dir>/tui.log`, never to the terminal the UI owns.
fn init_logging() {
    let dir = config::state_dir();
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(dir.join("tui.log")) else { return };
    let _ = tracing_subscriber::fmt().with_writer(Mutex::new(file)).with_ansi(false).try_init();
}

async fn run(settings: config::Settings, script: Option<playback::Script>) -> std::io::Result<()> {
    let (msg_tx, mut msgs) = mpsc::channel::<Msg>(QUEUE);
    let (cmds, cmd_rx) = mpsc::channel::<NetCmd>(QUEUE);
    let endpoint = match &settings.source {
        Source::Socket(p) => format!("unix:{}", p.display()),
        Source::Playback(p) => format!("playback:{}", p.display()),
    };
    match (script, &settings.source) {
        (Some(s), _) => drop(tokio::spawn(playback::run(s, msg_tx.clone(), cmd_rx))),
        (None, Source::Socket(socket)) => {
            let cfg = NetConfig {
                socket: socket.clone(),
                heartbeat_timeout: settings.heartbeat_timeout,
                reconnect: settings.reconnect,
            };
            drop(tokio::spawn(net::run(cfg, msg_tx.clone(), cmd_rx)));
        }
        (None, Source::Playback(_)) => unreachable!("playback scripts are loaded before run"),
    }
    let mut model = Model::new(endpoint, settings.look, settings.start, settings.wanted_pr);
    model.request_timeout_ms = settings.request_timeout.as_millis() as u64;
    tracing::info!(endpoint = %model.diag.endpoint, config = %settings.config_path.display(), "start");

    // Raw mode turns Ctrl-C into a key, so SIGINT only comes from kill; all three quit through the guard's restore.
    let mut term = signal(SignalKind::terminate())?;
    let mut hup = signal(SignalKind::hangup())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut guard = TerminalGuard::enter(settings.mouse)?;
    let size = guard.terminal.size()?;
    model.size = (size.width, size.height);
    let input = msg_tx.clone();
    for k in pr_review_tui::terminal::typed_before_start() {
        let _ = msg_tx.try_send(Msg::Key(k));
    }
    let opened = Instant::now();
    std::thread::spawn(move || {
        let mut late = pr_review_tui::terminal::LateReply::default();
        loop {
            let msg = match event::read() {
                Ok(TermEvent::Key(k)) if late.swallow(&k, opened.elapsed()) => continue,
                Ok(TermEvent::Key(k)) => Msg::Key(k),
                // Motion events would wake the app on every cell the pointer crosses.
                Ok(TermEvent::Mouse(e)) if !matches!(e.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) => {
                    Msg::Mouse(e)
                }
                Ok(TermEvent::Resize(w, h)) => Msg::Resize(w, h),
                Ok(_) => continue,
                Err(_) => return,
            };
            if input.blocking_send(msg).is_err() {
                return;
            }
        }
    });

    let started = Instant::now();
    let mut frame = tokio::time::interval(FRAME);
    frame.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let panic_for_test = std::env::var_os("PR_REVIEW_TUI_TEST_PANIC").is_some();
    loop {
        tokio::select! {
            msg = msgs.recv() => {
                let Some(msg) = msg else { break };
                if dispatch(&mut model, msg, &cmds) {
                    break;
                }
            }
            _ = term.recv() => break,
            _ = hup.recv() => break,
            _ = int.recv() => break,
            _ = tick.tick() => {
                dispatch(&mut model, Msg::Tick(started.elapsed().as_millis() as u64), &cmds);
            }
            // At most one frame per 16 ms, however many messages arrive, and only when something changed.
            _ = frame.tick() => {
                if model.dirty {
                    guard.terminal.draw(|f| view(&model, f))?;
                    model.dirty = false;
                    if panic_for_test {
                        panic!("PR_REVIEW_TUI_TEST_PANIC");
                    }
                }
            }
        }
    }
    let _ = cmds.try_send(NetCmd::Close);
    drop(guard);
    tracing::info!("quit");
    Ok(())
}
