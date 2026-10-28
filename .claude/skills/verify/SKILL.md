---
name: verify
description: Drive pr-review the way a user does and capture proof. Covers the macOS desktop app "PR Reviewer Agent" (primary surface), the reviewer daemon over its protocol v1 socket, and the terminal client. Use it to prove a UI, daemon or engine change works in the real app, not only in tests, on an isolated scratch instance that never touches the operator's install.
---

# Verify pr-review

pr-review has three surfaces. The desktop app (`desktop/`, GPUI, "PR Reviewer Agent") is the one a user touches most. It is a client of the daemon (`node bin/pr-review.mjs daemon`), which serves protocol v1 over a Unix socket and hosts the review engine. The terminal client (`tui/`, ratatui) is the same client core in a terminal. Every helper below lives in `.claude/skills/verify/scripts/` and runs from the repository root. Read [features/README.md](features/README.md) before you drive anything.

## Isolation rules

These rules are absolute. Break one and the run is void.

- **The operator's instance is off-limits.** This machine runs a real install: the LaunchAgent `io.github.diego-arbio-dev.pr-review.daemon`, its socket `~/Library/Application Support/pr-review/reviewer.sock`, its data in `~/Library/Application Support/pr-review/`, the app `~/Applications/PR Reviewer Agent.app`, and the shim `~/.local/bin/pr-review`, which runs a separate runtime clone, not this checkout. Never drive, stop, restart or modify any of them. Never run git inside its data repos. If you must list one, use `GIT_NO_LAZY_FETCH=1`, because a listing in a blob-less clone can fetch.
- **Run this checkout only.** Use `bin/pr-review.mjs` through the helpers, with a socket and data dir under `/private/tmp/<name>` (mode 700). Never run `pr-review` from `PATH`. Never start the daemon, the app or the TUI without `--socket` and `--config`: their defaults are the operator's socket and config files.
- **The desktop app gets a scratch config.** `daemon-up.sh` writes `/private/tmp/<name>/desktop.toml`. Its `launch_agent` label does not exist, and its `daemon_command` is the instance's `daemon.sh` wrapper, so Start and Stop only ever spawn this checkout.
- **Never post to GitHub.** Use `publish_mode` `dry_run` (the default) or `approval`. Never approve or confirm a post on a live daemon. Prove it with `gh-counts.sh` before and after the drive.
- **Never touch real Keychain items.** Every process gets `PR_REVIEW_KEYCHAIN_SERVICE=io.github.diego-arbio-dev.pr-review.verify-<name>`. `down.sh` deletes that item, and only that one.
- **Screenshots by window id only.** Use `shot.sh`. Never capture the full screen, which shows the user's other windows.
- **Kill what you started, by pid.** Use `down.sh`. Never `pkill` or `killall` by name: the operator's daemon is also `node … pr-review.mjs daemon`.
- **Keep private data out of commits.** Commit no names or data from private organisations or repositories, no keys and no tokens. Use public PRs such as `octocat/Hello-World#11609`. `.verify-evidence/` is gitignored.

## Launch

Pick a short name for the instance, such as `v1`. Short names keep the socket path under macOS's 103-byte limit.

```bash
S=.claude/skills/verify/scripts
$S/daemon-up.sh v1                      # engine mode, dry_run: real GitHub reads, a real model
$S/daemon-up.sh v1 --mode approval      # engine mode, reviews park in AWAITING_APPROVAL
$S/daemon-up.sh v1 --fixture showcase   # canned data from protocol/playback/showcase.ndjson; no GitHub, no commands
$S/daemon-up.sh v1 --no-start           # scratch dir and wrapper only, so the app starts the reviewer itself
$S/app-up.sh v1 [--appearance dark]     # builds desktop/ (incremental), opens the app on the instance
```

`daemon-up.sh` creates `/private/tmp/v1/` (mode 700) and these files in it:

- `config.json`: `{"review":{"publish_mode":"dry_run"}}`, which is `PR_REVIEW_CONFIG`.
- `env.sh`: the scratch `PR_REVIEW_DATA_DIR`, `PR_REVIEW_CONFIG`, `PR_REVIEW_KEYCHAIN_SERVICE` and the `XDG_*` dirs. It unsets `TYPESAFE_API_KEY`.
- `daemon.sh`: the wrapper that records its pid in `pids` and logs to `daemon.log`.
- `desktop.toml`: the app's config.
- `s.sock`: the socket.

The daemon is ready when `daemon.log` has `pr-review daemon: listening on /private/tmp/v1/s.sock (data dir /private/tmp/v1/data, engine mode, commands enabled)`. A fixture daemon says `(fixture showcase)` instead. The script waits for that line and prints the pid and socket. Migrations run from `migrations/` on the first open of an empty data dir, before that line. `app-up.sh` prints the app's pid and window id once the window is on screen.

Engine mode needs the user's `gh` login, which it uses for GitHub reads, and a model credential (`~/.pi/agent/auth.json` or a provider env var). The daemon picks the model from `models.preferred` unless you set `PR_REVIEW_MODEL`. When it can't open the engine it logs `read-only mode (<why>)` and still listens. Doctor shows it as `mode: read-only` with the reason.

The terminal client has no server to keep alive. Build it with `cargo build --release --manifest-path tui/Cargo.toml`, then run each drive in its own tmux session with the instance's environment:

```bash
tmux new-session -d -s v1-tui -x 120 -y 38 ". /private/tmp/v1/env.sh; tui/target/release/pr-review-tui --socket /private/tmp/v1/s.sock --config /private/tmp/v1/desktop.toml"
tmux capture-pane -p -t v1-tui > .verify-evidence/<run>/tui.txt
tmux kill-session -t v1-tui
```

`env.sh` sets `XDG_STATE_HOME`, so `tui.log` lands in the scratch dir, not in the operator's `~/Library/Application Support/pr-review/`. The TUI reads `desktop.toml` too: it takes `[connection]` and ignores `[desktop]`. A `--config` path that does not exist is an error. Use `--playback protocol/playback/<name>.ndjson` instead of `--socket` for a session with no daemon.

## Doctor

Run `$S/doctor.sh v1` first, and again whenever something looks off. It is read-only. It checks:

- `/private/tmp/v1` is ours with mode 700.
- Each recorded pid is still one of ours: its command line names the scratch dir or this checkout.
- The socket is ours, and the process on it is one we started.
- The checkout's commit and branch.
- A protocol `hello` gets a `welcome`. The welcome gives the `mode`, the `readOnlyReason` and the `capabilities`. An engine daemon grants `review.add` … `settings.set`, `daemon.stop`. A fixture daemon grants none.

If doctor fails, run `down.sh` and launch again. Never point a helper at a socket outside `/private/tmp/<name>`.

## Drive

Pick the path by what the change touches:

- **Snapshot harness (deterministic, for UI changes).** `desktop/snap.sh OUT_DIR [STATE...]` renders named states from `protocol/playback/showcase.ndjson` and a few derived scripts. It plays them in virtual time, so the same flags give the same pixels on any machine. No daemon, network or Keychain is involved. Use it for layout, copy, themes and any state a playback can reach. The state names are in `desktop/snap.sh`, such as `03-finding-dark`, `05c-waterfall-dark`, `18b-add-pr-valid-dark` and `21d-appearance-settings`. Write to `.verify-evidence/<run>/`, not to the default `desktop/snapshots/`. For a one-off state, run the binary directly: `desktop/target/release/pr-review-desktop --playback protocol/playback/showcase.ndjson --snapshot OUT.png --snapshot-after-ms 20000 --actions "wait:19000,pr:1287,finding:2" --window 1280x800 --appearance dark`. The steps are in `desktop/src/script.rs` and `desktop/README.md`. A playback never starts or stops a daemon. `reviewer:stopped|starting|start-failed|stop-confirm` only stages those states.
- **Live daemon (for behaviour across the engine).** Use it for anything that crosses the socket or the engine: commands, Start/Stop, migrations, the model, GitHub reads, the Keychain. Run `daemon-up.sh`, then `app-up.sh`, then drive with the keyboard:

```bash
$S/keys.sh <app-pid> cmd:n wait:600 type:octocat/Hello-World#11609 wait:600 return
$S/keys.sh <app-pid> cmd:k type:stop reviewer wait:400 return
```

`keys.sh` makes the pid frontmost through System Events. It checks that the pid is really frontmost before every step and refuses to type otherwise. Its steps are `type:TEXT`, `cmd:K`, `cmd-shift:K`, `shift:K`, `return`, `escape`, `tab`, `up`, `down` and `wait:MS`.

The app's shortcuts are bound in `desktop/src/ui.rs`:

| Shortcut | Action |
|---|---|
| ⌘N | Review a PR |
| ⌘K | Palette |
| ⌘, | Settings |
| ⌘R | Start reviewer, on the offline screen |
| ⌘1 | Needs you |
| ⌘2 | Done |
| ⌘4 | Usage; then D, W, M group it, T, R, A, P split it, Esc leaves |
| ⇧S | Review again |
| ⌘⏎ | Post. Never on a live daemon. |

The palette reaches every command by its title, such as "Stop reviewer", "Start reviewer" or "Theme: Nord". Prefer it to clicking. The title bar's Start/Stop pill has no shortcut, so click it only when the pill itself is the change under test. Read the pill's text from a screenshot.
- **Protocol directly.** `node $S/probe.mjs /private/tmp/v1/s.sock snapshot` prints the welcome, then every PR, run and finding. It is the read-only second view of the stored state after a mutation.

## Evidence

Put each run's proof in `.verify-evidence/<run>/` at the repository root, such as `.verify-evidence/review-a-pr-1/`. It is gitignored, outside the scratch dir, and survives `down.sh`.

- Capture the action and the result: a `shot.sh <pid> <run-dir>/NN-name.png` before the action, during it when it has a transient state, and after it.
- Capture the stored state: `probe.mjs … snapshot > snapshot.txt`, which shows the PR tracked and its run's `phase` and `status`.
- Capture the side effects: copy `/private/tmp/<name>/daemon.log` into the run dir before cleanup, along with `doctor.txt`.
- Prove what dry run skipped. Run `$S/gh-counts.sh octocat/Hello-World 11609 > github-before.txt` before the drive and again after it, then diff the two. The PR page should also say "Not posted". dry_run still reads GitHub and clones the repo into the scratch data dir. That is expected, and it is the only network use besides the model.
- Drive the real user path: the keyboard, the palette, the sheet. A `probe.mjs` command or a playback step is not proof for a live feature.
- Mocks are acceptable only where a production boundary already isolates the system. The fixture daemon and the playbacks are such a boundary for the UI. They are not one for the engine.

## Cleanup

```bash
$S/down.sh v1
```

`down.sh` sends TERM, then KILL, to each pid in `/private/tmp/v1/pids`, and only while that pid's command line still names the scratch dir or this checkout. The list covers the daemon, the app and any daemon the app started, because they all go through `daemon.sh`. It then deletes the `verify-v1` Keychain item and removes `/private/tmp/v1`. It never touches `.verify-evidence/`. It is safe to run twice. Run it after every failed attempt too, then check that `ls .verify-evidence/<run>` still lists the proof.

## Helpers

| Script | Use |
|---|---|
| `daemon-up.sh NAME [--engine \| --fixture F] [--mode dry_run\|approval] [--no-start]` | Make the scratch instance and start the daemon; prints `pid`, `socket` |
| `doctor.sh NAME` | Read-only health check, ends with the welcome |
| `app-up.sh NAME [--appearance THEME]` | Build and open the desktop app on the instance; prints `pid`, `window` |
| `keys.sh PID STEP...` | Type into that app only, after checking it is frontmost |
| `shot.sh PID OUT.png` | Capture that pid's window by id (`screencapture -x -o -l`) |
| `winid.sh PID` | The pid's window id, from `CGWindowListCopyWindowInfo` |
| `probe.mjs SOCKET [snapshot]` | Read-only hello, and the snapshot |
| `gh-counts.sh OWNER/REPO N` | Read-only counts of reviews and comments on a PR |
| `down.sh NAME` | Kill by pid, delete the test Keychain item, remove the scratch dir |

## Gotchas

- `--socket` or `--config` left off is the operator's instance. The helpers always pass both. A hand-typed command must pass them too.
- The app writes a spawned daemon's output to `~/Library/Logs/pr-review/daemon.log`. It opens that file for append, so the file is created when missing. `daemon.sh` redirects its own output to the scratch dir straight away, so nothing is written to that file, but the open still happens.
- A live app may fetch the GitHub account's avatar into `~/Library/Caches/pr-review/avatars/`. That cache is shared with the operator's app and harmless.
- The engine reads model credentials from the operator's `~/.pi/agent/auth.json`. A token refresh writes to that file, as the operator's daemon would.
- The model costs tokens. Use a tiny public PR, such as `octocat/Hello-World#11609`.
- A fixture daemon grants no commands. Review a PR, Start/Stop, Settings' model picker and Review again need `--engine`.
- `keys.sh` needs Accessibility permission for the terminal that runs `osascript`. Without it, every step fails with an error that names System Events.
