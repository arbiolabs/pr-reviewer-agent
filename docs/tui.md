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
| `--playback FILE` | Play a scripted session instead of connecting |
| `--config FILE` | Read this config file instead of `<config dir>/pr-review/config.toml` |
| `NO_COLOR` | When set to any non-empty value, turns colors off. Every status keeps its word or icon. |

## Views

| Key | View | Shows |
|---|---|---|
| `1` | Inbox | Tracked PRs, with review state, head commit, and findings by severity |
| `2` | Review | The phase stepper, the findings, and recent tool activity |
| `3` | Diff | Files and hunks pinned to the reviewed base and head, with finding markers |
| `4` | Timeline | Phase changes, tool activity, and finding updates in order |
| `5` | Settings | GitHub accounts, connection state, and diagnostics |

The layout adapts to the terminal width. At 110 columns or more, the client shows a sidebar, the main view, and a context pane. From 80 to 109 columns, details open over the main view. From 60 to 79 columns, the client shows one pane at a time. Below 60×16, it shows a resize hint.

## Keys

| Key | Action |
|---|---|
| `j` and `k`, or the arrow keys | Move the selection |
| `h` and `l` | Move between panes |
| `Enter` | Open the selected item |
| `Esc` | Close a dialog or go back |
| `g` and `G` | Go to the top or the bottom |
| `Ctrl-d` and `Ctrl-u` | Page down or up |
| `d` | Open the selected finding's diff |
| `t` | Open the timeline |
| `n` and `N` | Next or previous finding, in Review and Diff |
| `:` | Jump to `path:line`, in Diff |
| `f`, `v`, and `P` | Change the timeline's scope, category, or phase filter |
| `A` | Open the GitHub accounts |
| `r` | Refresh from the daemon, or retry a timed-out request |
| `?` | Show help |
| `q` | Quit the client |

## Choose a GitHub account

The header shows the GitHub identity the reviewer uses: `gh: <login>`, `env token`, or `no GitHub account`. To change it:

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
