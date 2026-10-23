# pr-review-desktop

`pr-review-desktop` is a macOS client for pr-review, built on [GPUI](https://www.gpui.rs) through [gpui-kit](https://crates.io/crates/gpui-kit) 0.8.0. The window is a triage inbox: a queue of the decisions waiting on you, each with its detail beside it, a live strip of running reviews, every PR in the sidebar, the diff with findings anchored, a timeline, and a command palette (⌘K). [DESIGN.md](DESIGN.md) describes the design and its tokens.

The app links the terminal client in `../tui` as a library and runs its reducer, `update(&mut Model, Msg) -> Vec<Effect>`, and its network task unchanged. The protocol rules, the key guards and the playback all come from there. The terminal client stays as it is.

## Build and run

The build needs Rust and macOS. It does not need Xcode.

```bash
cd desktop
cargo build --release
./target/release/pr-review-desktop --socket /tmp/pr-review/reviewer.sock
```

The app takes the terminal client's flags: `--socket PATH`, `--pr URL`, `--playback FILE.ndjson` and `--config FILE`. With no `--socket` it uses the socket from the config file, then the platform default. `--help` lists every flag.

To run it against a daemon that serves the showcase fixture:

```bash
mkdir -m 700 /tmp/prd
node bin/pr-review.mjs daemon --fixture showcase --socket /tmp/prd/s.sock    # terminal 1, from the repository root
desktop/target/release/pr-review-desktop --socket /tmp/prd/s.sock            # terminal 2
```

Keep the socket path short. macOS refuses a socket path longer than 103 bytes, and the daemon refuses a socket directory that is a symlink, such as `/tmp` itself.

### Review a new PR

Against a daemon started with `--allow-commands`, press ⌘N, or choose **Review a PR…** in the sidebar, the palette or the empty state. Paste the PR's URL (⌘V), or type `owner/repo#N`. The sheet shows the repository and number it read, and ⏎ asks the daemon to add it. The daemon reads the PR from GitHub with its own account and starts its review; the sheet closes on the new PR. If the daemon refuses, the sheet says why in place: a PR its account can't see, a closed or merged PR, or text that isn't a PR link. A PR the window already tracks opens instead. On a read-only daemon the entries are locked with the reason.

To play a scripted session with no daemon:

```bash
desktop/target/release/pr-review-desktop --playback protocol/playback/showcase.ndjson
```

The app runs as a plain binary, without an app bundle, so it sets its own Dock and app-switcher icon at launch from `assets/app-icon.png`. That file is the icon on Apple's 1024 px grid, with the artwork about 824 px wide so it sits at the same size as other apps' icons.

## The snapshot harness

`--snapshot OUT.png` captures the app's own window to a PNG and quits with exit code 0. The capture needs no Screen Recording permission, because the app reads back a window it owns.

```bash
pr-review-desktop --playback protocol/playback/showcase.ndjson --snapshot out.png \
  --snapshot-after-ms 20000 --actions "wait:19000,pr:1287,finding:2,open-diff" \
  --window 1280x800 --appearance dark
```

| Flag | Meaning |
|---|---|
| `--snapshot OUT.png` | Capture the window to `OUT.png`, then quit. |
| `--snapshot-after-ms N` | The moment to capture, in milliseconds from launch. The default is 1000. |
| `--actions STEPS` | Steps to apply before the capture. Needs `--playback`. |
| `--window WxH` | The window size in points. The default is 1280x800. |
| `--appearance dark\|light` | The gpui-kit theme mode. The default follows the system. |

The window must fit on the screen, because macOS shrinks a larger one and the capture then never matches the requested size. The PNG is the window's content area at the display's scale, so `--window 1280x800` gives a 2560x1600 image on a Retina display. The title bar's chrome above the content area is cropped off; the app's own title bar, with the traffic lights, is in the capture.

### Determinism

With `--playback`, the script plays in virtual time before the window opens. Script lines, the 125 ms ticks and the steps apply in time order up to `--snapshot-after-ms`, and the window then shows that model without further changes. The same flags give the same pixels on the same machine, however slow the build, wherever the pointer is and whichever app is in front: a snapshot window ignores the keyboard, covers itself with a layer that takes the pointer so nothing under it hovers or presses, and activates itself before every capture so the traffic lights are colored. The harness captures only after the window shows the same painted frame at its full size on two captures in a row. It retries a capture that is one flat color, asks for another frame when one does not present within 1.5 s, and exits with code 1 if the window never settles. `snap.sh` retries a run that exits without its PNG or runs past 60 s, three times.

The window shows the endpoint, so a path to the script that differs gives different pixels. `snap.sh` passes a relative path for this reason. A snapshot's window is opaque, while a normal window blurs the desktop through its sidebar and title bar, so a capture never depends on what is behind the window.

With `--socket`, the capture happens `--snapshot-after-ms` milliseconds after launch in real time, and `--actions` is refused.

### Action syntax

`--actions` takes comma-separated steps. Steps apply in order, and each one applies at the time that the `wait` steps before it add up to.

| Step | What it does |
|---|---|
| `wait:N` | Wait N milliseconds before the next step. |
| `pr:N` | Select PR number N, as a click on its row does. |
| `finding:N` | Select the selected PR's Nth finding, counted from 1, as a click on it does. A second `finding:N` on the same finding opens it. |
| `goto:SCREEN` | Go to `inbox`, `review`, `diff`, `timeline` or `settings`. |
| `type:TEXT` | Type TEXT into the focused input, one character at a time. TEXT cannot contain a comma. |
| an action name | Do what the key for that action does. |
| `tab:done`, `tab:queue` | Show the done list or the queue. |
| `palette:TEXT` | Open the command palette with TEXT typed. |
| `keys` | Open the shortcut sheet. |
| `confirm-post` | Open the sheet that asks before posting the proof in front. |
| `add-pr`, `add-pr:TEXT` | Open the sheet that adds a PR, with TEXT in its field. |
| `add:TEXT` | Send what ⏎ in that sheet sends for TEXT. The playback refuses a PR it doesn't track as GitHub would. |
| `scrub:N` | Hold the live log at its Nth event, counted from 1. |
| `discards` | Open every "Verifier discarded" group on the timeline. |
| `flagged` | List only the diff's files with findings. |

The fixture daemon (`node bin/pr-review.mjs daemon --fixture showcase`) answers `finding.get` with the same recorded detail the playback does, so a finding opened against it shows its real explanation, evidence and history. It grants no review command, so the window reads it as read-only.

The action names are `quit`, `up`, `down`, `page-up`, `page-down`, `top`, `bottom`, `left`, `right`, `focus-next`, `focus-prev`, `open`, `back`, `search`, `jump-to`, `open-diff`, `open-timeline`, `watch`, `scroll-down`, `scroll-up`, `next-finding`, `prev-finding`, `refresh`, `help`, `accounts`, `start`, `retry`, `cancel`, `publish`, `cycle-scope`, `cycle-category`, `cycle-phase`, `backspace`, `submit`, `cancel-input`, `confirm`, `open-url` and `toggle-raw`. Each one maps onto `keymap::Action` in the terminal client.

An action acts only where a key for it would act. For example, `confirm` does nothing unless a confirmation is open, and only input steps reach a focused input. A `pr:N` or `finding:N` step that names something absent fails the run with exit code 2.

### Render the showcase states

`snap.sh` renders a named set of showcase states into a directory and prints each PNG's path:

```bash
desktop/snap.sh                      # every state, into desktop/snapshots/
desktop/snap.sh /tmp/snaps diff help # two states, into /tmp/snaps
```

The states are listed at the end of `snap.sh`, one per line: a name, the appearance, the capture time, the steps and, optionally, another playback script. `08-disconnected` dials a socket that doesn't exist and captures in real time. `00-connecting`, `11-empty-queue` and `12-readonly` play the sessions in `desktop/playback/` and a read-only rewrite of the showcase that `snap.sh` writes under `target/`. `15-tracking`, `15b-reopened` and `16-discards` play `protocol/playback/tracking.ndjson`, a PR reviewed on three heads, and `17-big-diff` plays `big-diff.ndjson`, a 60-file PR. `18a-add-pr-empty`, `18b-add-pr-valid` and `18c-add-pr-error` show the add-a-PR sheet empty, with a URL it read, and with the daemon's refusal.

## How the parts connect

- A multi-threaded tokio runtime runs the terminal client's `net::run`, or `playback::run`, and a task that sends `Msg::Tick` every 125 ms, the terminal client's cadence. Both send into one `mpsc` channel.
- A GPUI foreground task awaits that channel and applies each message to the `Model` inside the window's view on the main thread, through the terminal client's `app::dispatch`. The view re-renders when the model is dirty.
- `dispatch` carries out the effects: `Effect::Send` goes to the network task, `Effect::OpenUrl` opens the URL with `open` (the reducer only emits `https://github.com/` URLs), and `Effect::Quit` quits the app.
- A view sends `Msg::Action(keymap::Action)` for an intent and `Msg::Pick(view::Hit)` for a click target. Both act only where the terminal client's key or click would. The desktop keeps only its own view state: which list the home shows, what floats over the window, and where the live log is held.
- The ⌘ shortcuts are GPUI actions bound to the menu bar, so the menus show them. Plain keys go through `Shell::on_key`.

## Develop

```bash
cd desktop
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`Cargo.lock` started as a copy of `tui/Cargo.lock`. The terminal client pins `tokio = "=1.54.0"`, which crates.io has since yanked, and Cargo uses a yanked version only when a lock file already names it.

GPUI notes for the next view:

- `use gpui_kit::*;` is GPUI, and it exports a `test` macro. In a test module, import what you need by name instead of `use super::*;`, or `#[test]` resolves to GPUI's macro and fails with a recursion-limit error.
- `gpui_kit::init(cx)` sets the light theme. Call `Theme::change(mode, None, cx)` after it.
- An `async move` closure captures only the fields it uses. A struct moved in for one field drops its other fields when the outer closure returns, which is how an early version dropped the tokio runtime.
- macOS animates a new window in, so a capture right after the first frame can be smaller than the window.
