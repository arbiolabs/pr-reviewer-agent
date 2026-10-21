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

`protocol/playback/` has more scripts: a missing event, a duplicate, a stream reset, a superseded run, hostile text, a burst of 5,000 events, account selection, and an approved publication. You can also serve a script to a real client with `node bin/pr-review.mjs daemon --fixture <name>`.

## Client options

| Option | Effect |
|---|---|
| `--socket PATH` | Connect to this socket instead of the default |
| `--pr URL` | Open this PR, as a URL or as `owner/repo#N` |
| `--ascii` | Use ASCII glyphs and borders only |
| `--theme NAME` | Use `auto`, `dark`, `light`, or `mono`. This overrides `theme` in the config file. |
| `--playback FILE` | Play a scripted session instead of connecting |
| `--config FILE` | Read this config file instead of `<config dir>/pr-review/config.toml` |
| `--version` | Print the client's version and exit |
| `NO_COLOR` | When set to any non-empty value, turns colors off. Every status keeps its word or icon. |

## Views

| Key | View | Shows |
|---|---|---|
| `1` | Inbox | Tracked PRs, grouped by repository and then by PR number, newest first. Each row has its review state, head commit, and findings by severity. |
| `2` | Review | One PR as a document: its phase stepper, the live review while one runs, and a card per finding |
| `3` | Diff | Files and hunks pinned to the reviewed base and head, with each finding written under its line |
| `4` | Timeline | The PR's events, oldest first, under one heading per phase |
| `5` | Settings | Setup checks, GitHub accounts, connection state, display settings, and diagnostics |

The client starts on Review, on the PR with the most severe current finding. The PR list holds its order while reviews run. A new PR takes its sorted place, and the selection stays on the same PR.

The layout adapts to the terminal size:

- At 100 columns or more, the PR list sits beside the reading column. Below 100 columns, the Inbox is a full-width list and `Enter` opens the PR.
- The PR list is 34 columns wide below 140 columns and widens to 48 at 200. The reading column is at most 112 columns wide.
- At 120 columns or more, the footer lists all five views, Settings included.
- At 110 columns or more, the diff shows its file list beside the code. Below 110 columns, the code takes the full width and `h` shows the file list.
- At 120×38, the open finding's card and the head of the next card are both in view.
- Below 60×16, the client shows a resize hint.

In Review, the open card shows as much of the explanation and the quoted code as fits. A cut explanation ends with `… N more, J scrolls`. `J` scrolls the explanation inside the card while the code stays in view. When the explanation has nothing left below, `J` scrolls the document.

In Diff, a long line or hunk header wraps onto up to four rows. Each finding is written under its line. When several findings share a line, they stack, and `n` steps through them.

In Timeline, `Enter` opens the selected event's detail under its row, and `Esc` closes it. The detail shows what the event carries: a tool call's whole summary, or a finding's title, severity, `file:line`, and the start of its explanation.

The help page marks a write key `(not offered)` when the connected daemon does not grant its capability. Against a read-only daemon the note says `(daemon is read-only)` or `(commands are off)`, and the page's last line gives the whole reason.

### Check the setup

The client shows the daemon's setup checks, the same ones `pr-review setup --check` prints: git, the config file, the
model, the GitHub account, and Jev. The daemon runs the checks in its own environment and sends them when the client
connects. The client only shows them. It never asks for a key or a token, and you fix a check with `pr-review setup`
in a terminal.

- **Settings** lists every check under **Setup**, with its status as a symbol and a word (`✓ ok`, `◔ warn`,
  `✗ missing`; `+ ok`, `~ warn`, `x missing` with `--ascii`), its detail, and its fix in dim text.
- **The empty inbox.** When nothing is tracked yet and a check is `missing`, the inbox lists the missing checks with
  their fixes and tells you to run `pr-review setup`. Against a read-only daemon with commands off, it also says
  `commands are off (run pr-review setup, then --allow-commands)`, because a daemon started with `--allow-commands`
  stays read-only while it has no model.

The checks are from when the client connected. After you fix one, restart the client, or the daemon, to see the new
result. `pr-review-tui --playback protocol/playback/setup.ndjson` shows a missing model with no daemon.

## Keys

| Key | Action |
|---|---|
| `j` and `k`, or the arrow keys | Move the selection |
| `h` and `l` | Move between panes. In Review, `h` moves to the PR list. In a narrow Diff, `h` shows the file list. |
| `Enter` | Open the selected item, or a timeline event's detail |
| `Esc` | Close a dialog, a timeline event's detail, or go back |
| `g` and `G` | Go to the top or the bottom |
| `Ctrl-d` and `Ctrl-u` | Page down or up |
| `/` | Search what the screen shows. On Inbox and Review, filter the PR list by a fuzzy match. On Timeline, filter the events by text. On Diff, search the lines; `n` and `N` then step through the matches, and `Esc` clears the search. |
| `w` | Watch the running review. Press `w` again for the next running PR, then the PRs waiting for approval. |
| `J` and `K` | Scroll the open finding's explanation down or up, in Review |
| `d` | Open the diff at the selected finding's line |
| `t` | Open the selected PR's timeline |
| `n` and `N` | Next or previous finding, in Review, Diff, and the finding reader. In Diff, `n` also steps through findings stacked on one line. |
| `:` | Jump to `path:line`, in Diff |
| `f`, `v`, and `P` | Change the timeline's scope, category, or phase filter |
| `A` | Open the GitHub accounts in Settings |
| `s` | Start a review of the selected PR at its head |
| `R` | Retry the selected PR's failed run |
| `c` | Cancel the selected PR's running or parked review |
| `p` | Preview the review that a parked run would publish |
| `y` | Confirm, only in a confirmation panel or the publication preview |
| `o` | Open the published review in the browser, from the publication preview |
| `r` | Refresh from the daemon, or retry a timed-out request |
| `?` | Show help. The help page is built from the keymap. When it is taller than the window, `j`, `k`, the wheel, PgUp, and PgDn scroll it, and its last row shows how many lines remain below. |
| `q` | Quit the client |

## Use the mouse

- Click a PR in the list to select it.
- Click a finding's card to select it. Click the selected card again to read the finding in full.
- In Diff, click a file to show it, or a line to move the cursor there. Click the cursor's line again to read its finding.
- In Timeline, click an event to select it. Click it again to open its detail.
- Click a view's name in the footer to go there.
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

## Take actions

The daemon can start, retry, cancel, and publish reviews on the client's behalf. These commands are off by default. The client shows an action as unavailable until the daemon grants it.

1. Start the daemon with commands enabled:

	```bash
	node bin/pr-review.mjs daemon --allow-commands
	```

	Or set `"control": { "allow_commands": true }` in `pr-review.config.json`.

2. With commands enabled, the daemon runs in **engine mode**: it takes the data directory lock at startup and runs reviews itself. Without them it is **read-only**: it takes no lock, runs nothing, and grants only the account selector. A daemon with commands enabled is also read-only while `pr-review serve` or `pr-review review` holds the lock. The daemon's start line reports the mode, and the `welcome` message carries it as `mode`, with `readOnlyReason` saying why a read-only daemon is read-only. The client's Settings view shows both on its **mode** row.

3. To approve publications from the client, set `"review": { "publish_mode": "approval" }`. [Publishing](publishing.md#approval-mode) describes the approval flow.

### Start, retry, or cancel a review

1. Select the PR.
2. Press `s` to start, `R` to retry its failed run, or `c` to cancel its running or parked review. A panel names the exact target, for example `Start a review of #1291 at f86859b?`.
3. Press `y` to send the command, or `n` or `Esc` to close the panel without sending anything.

The panel shows `requested…` and then `accepted…` until the event that proves the effect arrives: the new run starting, the retried run moving, or the cancelled run ending. Then the panel closes and the footer says what happened. A rejected command stays in the panel with its code and message, for example `REVISION_STALE: the head of acme/shop#1291 is now 1a2b3c4`.

### Approve a publication

In approval mode a finished review parks in `AWAITING_APPROVAL`. The client marks it in amber as waiting for you, never as running, failed, or done: `◔ awaiting you` on its PR row, `◔ Publish` in the phase stepper, `◔ #1291 waiting for you` in the status line, and a `Ready to publish · p previews` line above its findings.

1. Select the parked PR, or press `w` until it is selected.
2. Press `p`. The publication preview shows the target (the repository, the PR, the head commit, and the event `COMMENT`), the summary exactly as it will be posted, and each inline comment with its `path:line` and the diff around it. GitHub does not render the hidden marker at the end of the summary, so the preview names it and shows it dimmed. The **Positioning** section says whether each comment sits on a line of the diff.
3. Press `y` to publish exactly that preview, or `Esc` to close it without publishing.

The page then follows the publisher: `requested…`, `approval recorded; publishing…`, `publishing…`, and finally `published on GitHub` with the review's URL, or `uncertain` or `failed` with the reason. Press `o` to open a published review in the browser. The key works only for an `https://github.com/` URL.

`y` is off, with the reason, when the preview is stale: the run has left `AWAITING_APPROVAL`, or the PR has a newer head than the preview was written for. Close the page and press `p` again for the current preview.

The timeline lists each publication status as a `publication` row under `AWAITING APPROVAL` or `PUBLISH`.

The daemon checks every command itself, whatever the client showed:

| Command | Does | Refuses |
|---|---|---|
| Start | Reviews the PR at the head the client saw, with the configured publish mode. The run appears in the client through its events. | A PR the daemon has not seen (`NOT_FOUND`). A head that is no longer GitHub's head (`REVISION_STALE`, with the live head). A head that already has a finished or parked review, or a closed PR (`CONFLICT`). |
| Retry | Reopens a failed run as a new attempt, recorded as a manual override. | A run that is not failed (`INVALID_TRANSITION`). |
| Cancel | Fails a running or parked run with the reason `cancelled from the TUI`. A parked run always fails. A running run whose head moved meanwhile, or whose PR closed, is superseded instead. | A run in `PUBLISH` or `RECONCILE`, where a GitHub write may be in flight (`CONFLICT`). A finished run (`INVALID_TRANSITION`). |
| Publish | Approves exactly the preview the client showed, for exactly the head it showed, and posts it once through the outbox. | A run that is not awaiting approval (`INVALID_TRANSITION`). A preview that is not the stored payload (`CONFLICT`). A head that moved or a PR that closed, which supersedes the run with nothing posted (`REVISION_STALE`). |

A repeated command, for example after a reconnect, answers the first result and does nothing again, as long as the run is still on the attempt that command ended or opened. Once a retry or an approval has opened a later attempt, the same command acts on that attempt: a run that failed twice can be retried twice, and a retried run can be cancelled again. An accepted command means the daemon recorded the request. The client shows the outcome when the events arrive: a published review only on the `published` publication status, never on the acceptance.

Quitting the client never stops the daemon or its reviews. Stopping the daemon suspends the reviews in flight; they resume when the daemon, `serve`, or `review` next opens the data directory.

## Daemon

The daemon serves [protocol v1](../protocol/README.md) on a Unix socket.

- **Socket path.** The default is `$XDG_RUNTIME_DIR/pr-review/reviewer.sock`. Without that variable, it is `~/Library/Application Support/pr-review/reviewer.sock` on macOS and `~/.local/state/pr-review/reviewer.sock` elsewhere. A socket path can be at most 103 bytes on macOS and 107 bytes on Linux.
- **Permissions.** The daemon creates the socket directory with mode 0700 and the socket with mode 0600. It refuses a directory that is a symlink, belongs to another user, or has a looser mode. It replaces a leftover socket file only when nothing listens on it.
- **Read-only mode.** The default. The daemon reads `app.sqlite` on a read-only connection, takes no lock, and never opens `durable.sqlite`. It writes only migrations at startup, the selected GitHub account, and a log row when the account list changes. A daemon started with commands enabled is also read-only while another `pr-review` holds the lock, or when its engine fails to open; `welcome.readOnlyReason` says which.
- **Engine mode.** With `--allow-commands` or `control.allow_commands`, the daemon takes the data directory lock and opens the review engine with the GitHub token and model that `review` would use, resumes unfinished reviews, and reconciles uncertain publications. It needs a model credential; without one it falls back to read-only mode and says why.
- **Commands.** `account.select` is always accepted. The review commands need engine mode; the daemon answers an ungranted command with `CAPABILITY_DENIED` and the reason.
