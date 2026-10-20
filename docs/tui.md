# The terminal client

`pr-review-tui` shows what the reviewer is doing: tracked PRs, each review's phase, its findings, the diff behind them, and the event timeline. It talks to `pr-review daemon` over a Unix socket. Quitting the client never stops or changes a review.

## Start the client

1. Build the client:

	```bash
	(cd tui && cargo build --release)
	```

2. Start the daemon in its own terminal. The daemon runs in the foreground until SIGINT or SIGTERM.

	```bash
	node bin/pr-review.mjs daemon
	```

3. Start the client:

	```bash
	node bin/pr-review.mjs tui
	```

	`pr-review tui` runs `pr-review-tui` from your `PATH`, or from `tui/target/release/`, and passes its arguments through.

To open one PR directly, pass `--pr https://github.com/owner/repo/pull/123`.

To try the client without GitHub or a model, play a scripted session:

```bash
./tui/target/release/pr-review-tui --playback protocol/playback/normal.ndjson
```

`protocol/playback/` has more scripts: a missing event, a duplicate, a stream reset, a superseded run, hostile text, a burst of 5,000 events, and account selection. You can also serve a script to a real client with `node bin/pr-review.mjs daemon --fixture <name>`.

## Client options

| Option | Effect |
|---|---|
| `--socket PATH` | Connect to this socket instead of the default |
| `--pr URL` | Open this PR, as a URL or as `owner/repo#N` |
| `--ascii` | Use ASCII glyphs and borders only |
| `--theme NAME` | Use `auto`, `dark`, `light`, or `mono`. This overrides `theme` in the config file. |
| `--playback FILE` | Play a scripted session instead of connecting |
| `--config FILE` | Read this config file instead of `<config dir>/pr-review/config.toml` |
| `NO_COLOR` | When set to any non-empty value, turns colors off. Every status keeps its word or icon. |

## Views

| Key | View | Shows |
|---|---|---|
| `1` | Inbox | Tracked PRs, grouped by repository and then by PR number, newest first. Each row has its review state, head commit, and findings by severity. |
| `2` | Review | One PR as a document: its phase stepper, the live review while one runs, and a card per finding |
| `3` | Diff | Files and hunks pinned to the reviewed base and head, with each finding written under its line |
| `4` | Timeline | The PR's events, oldest first, under one heading per phase |
| `5` | Settings | GitHub accounts, connection state, display settings, and diagnostics |

The client starts on Review, on the PR with the most severe current finding. The PR list holds its order while reviews run. A new PR takes its sorted place, and the selection stays on the same PR.

The layout adapts to the terminal size:

- At 100 columns or more, the PR list sits beside the reading column. Below 100 columns, the Inbox is a full-width list and `Enter` opens the PR.
- At 110 columns or more, the diff shows its file list beside the code. Below 110 columns, the code takes the full width and `h` shows the file list.
- At 120×38, the open finding's card and the head of the next card are both in view.
- Below 60×16, the client shows a resize hint.

In Review, the open card shows as much of the explanation and the quoted code as fits. A cut explanation ends with `… N more, J scrolls`. `J` scrolls the explanation inside the card while the code stays in view. When the explanation has nothing left below, `J` scrolls the document.

In Diff, a long line wraps onto up to four rows. Each finding is written under its line. When several findings share a line, they stack, and `n` steps through them.

In Timeline, `Enter` opens the selected event's detail under its row, and `Esc` closes it.

## Keys

| Key | Action |
|---|---|
| `j` and `k`, or the arrow keys | Move the selection |
| `h` and `l` | Move between panes. In Review, `h` moves to the PR list. In a narrow Diff, `h` shows the file list. |
| `Enter` | Open the selected item, or a timeline event's detail |
| `Esc` | Close a dialog, a timeline event's detail, or go back |
| `g` and `G` | Go to the top or the bottom |
| `Ctrl-d` and `Ctrl-u` | Page down or up |
| `/` | Filter the PR list by a fuzzy match. In Diff, jump to `path:line`. |
| `w` | Watch the running review. Press `w` again for the next running PR. |
| `J` and `K` | Scroll the open finding's explanation down or up, in Review |
| `d` | Open the diff at the selected finding's line |
| `t` | Open the selected PR's timeline |
| `n` and `N` | Next or previous finding, in Review and Diff. In Diff, `n` also steps through findings stacked on one line. |
| `:` | Jump to `path:line`, in Diff |
| `f`, `v`, and `P` | Change the timeline's scope, category, or phase filter |
| `A` | Open the GitHub accounts in Settings |
| `r` | Refresh from the daemon, or retry a timed-out request |
| `?` | Show help. The help page is built from the keymap. When it is taller than the window, `j`, `k`, the wheel, PgUp, and PgDn scroll it, and its last row shows how many lines remain below. |
| `q` | Quit the client |

## Use the mouse

- Click a PR in the list to select it.
- Click a finding's card to select it. Click the selected card again to read the finding in full.
- Scroll the wheel to move the reading column: the open finding, the help page, the document, the diff, or the timeline.

While the client captures the mouse, a plain drag no longer selects text in the terminal. To select text, hold the terminal's bypass modifier while you drag: Shift in most terminals, or Option in iTerm2 and macOS Terminal. To give plain drag back to the terminal, turn mouse capture off in the config file:

```toml
[ui]
mouse = false   # the default is true
```

With `mouse = false`, clicks and the wheel do nothing in the client. The keys do everything the mouse does.

## Change the theme and the background

The client reads `<config dir>/pr-review/config.toml`, or the file that `--config` names. The `[ui]` table sets the display:

```toml
[ui]
theme = "auto"          # or "dark", "light", or "mono", which turns colors off like NO_COLOR
background = "theme"    # or "terminal"
ascii = false           # true is the same as --ascii
start_view = "review"   # inbox, review, diff, timeline, or settings
mouse = true            # false leaves drag-to-select to the terminal
show_activity = true    # false hides the "Reviewing now" log and the tool-call rates
```

The client has two palettes. `dark` is "Ink", light text on a near-black ground. `light` is "Paper", dark text on a warm off-white ground, with its own severity, syntax, diff, and selection colors.

With `theme = "auto"`, the default, the client asks the terminal for its background color (the OSC 11 query) before it draws. A background with a relative luminance above 0.5 selects `light`, and any other background selects `dark`. If the terminal does not answer within 100 ms, the client reads `COLORFGBG`, where a background of 7 or 15 selects `light`. Otherwise it uses `dark`. The client skips the query when stdin or stdout is not a terminal, or when `NO_COLOR` is set. A terminal that never answers delays startup by at most 100 ms.

With `background = "theme"`, the default, the client paints its palette's background on every cell. The contrast test requires 4.5:1 for every text color on every painted surface, in both palettes, so the contrast holds in any terminal. The worst pair is 5.17:1 in `dark` and 5.04:1 in `light`.

With `background = "terminal"`, the client leaves the terminal's own background in place and still paints the panels, code blocks, selection, and diff lines. This setting assumes that the palette matches the terminal, which `theme = "auto"` arranges. The contrast test checks `dark` against black, `#1e1e1e`, and `#282c34`, with a worst pair of 5.11:1. It checks `light` against white, `#eeeeee`, and `#fdf6e3`, with a worst pair of 5.55:1. A mismatched pair fails: `dark` on a white terminal measures 1.29:1, and `light` on a black terminal measures 1.32:1.

## Choose a GitHub account

Settings shows the GitHub identity the reviewer uses on its **publishes as** row. To change it:

1. Press `A`.
2. Select an account and press `Enter`.
3. Press `y` to confirm.

The client shows the new account only after the daemon confirms the change. The daemon checks the account against `gh` before accepting it. While `GITHUB_TOKEN` or `GH_TOKEN` is set, every account shows `env token wins`, because the environment token takes precedence. [Configuration](configuration.md#github-token) describes the full precedence.

Starting, cancelling, retrying, and publishing reviews from the client are not built yet. Their keys show the action as unavailable.

## Daemon

The daemon serves [protocol v1](../protocol/README.md) on a Unix socket.

- **Socket path.** The default is `$XDG_RUNTIME_DIR/pr-review/reviewer.sock`. Without that variable, it is `~/Library/Application Support/pr-review/reviewer.sock` on macOS and `~/.local/state/pr-review/reviewer.sock` elsewhere. A socket path can be at most 103 bytes on macOS and 107 bytes on Linux.
- **Permissions.** The daemon creates the socket directory with mode 0700 and the socket with mode 0600. It refuses a directory that is a symlink, belongs to another user, or has a looser mode. It replaces a leftover socket file only when nothing listens on it.
- **Read-only for reviews.** The daemon reads `app.sqlite` on a read-only connection, takes no data-directory lock, and never opens `durable.sqlite`. It writes only migrations at startup, the selected GitHub account, and a log row when the account list changes.
- **Commands.** `account.select` is the only command the daemon accepts. It answers every other command with `CAPABILITY_DENIED`.
