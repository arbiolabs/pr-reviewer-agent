# Review a PR (⌘N)

A user pastes or types a PR (a URL or `owner/repo#N`) into the Review a PR sheet. The daemon reads the PR from GitHub with its own account, tracks it and starts its review. When the reviewer is stopped or offline, the same sheet starts it first, then sends the PR.

## Sub-features

- `add-open`: ⌘N, the sidebar's "Review a PR…", the palette's "Review a PR by its URL…" and the empty state each open the sheet.
- `add-parse`: the sheet echoes what it read (`octocat/Hello-World · #11609`) and enables "Review PR ⏎".
- `add-submit`: ⏎ sends `review.add`. The sheet closes on the new PR, whose review runs to its end state.
- `add-start-stopped`: with nothing on the socket, ⏎ starts the reviewer ("Starting the reviewer, then adding …"), then adds the PR.
- `add-refused`: a closed PR, one GitHub won't show, or text that isn't a PR link is refused in place. A PR that is already tracked opens instead.
- `add-readonly`: on a read-only daemon the entries are locked, with the reason.

## How to get to it (user POV)

- Press ⌘N anywhere in the window.
- Choose "Review a PR…" in the sidebar.
- In the palette (⌘K), choose "Review a PR by its URL…".

## Driving it with keys.sh on a live engine daemon

Preconditions:

- `octocat/Hello-World#11609` is open. Check with `gh pr view 11609 --repo octocat/Hello-World --json state`.
- `gh` is logged in, and a model credential exists.
- Write the GitHub baseline first: `$S/gh-counts.sh octocat/Hello-World 11609 > .verify-evidence/<run>/github-before.txt`.

Steps:

- **Stopped reviewer.** Run `$S/daemon-up.sh v1 --no-start`, then `$S/app-up.sh v1 --appearance dark`. The window reads "Offline" and the pill reads "Reviewer · Offline". Capture it with `$S/shot.sh <pid> .verify-evidence/<run>/01-offline.png`.
- **Open and fill.** Run `$S/keys.sh <pid> cmd:n wait:600 type:octocat/Hello-World#11609 wait:600`. The "Review a PR" sheet shows the text and the green echo `octocat/Hello-World · #11609`. Capture `02-sheet-filled.png`.
- **Submit.** Run `$S/keys.sh <pid> return`, then capture `03-starting.png` straight away. The pill reads "Reviewer · Starting…" and the page reads "Starting the reviewer". `cat /private/tmp/v1/pids` gains a second pid, the daemon spawned through `daemon.sh`.
- **Added and reviewed.** Wait about 15 s, then capture `04-added.png`. The pill reads "Reviewer · Live". The sidebar lists `octocat/Hello-World` › "Update Hello World documentation". The PR page reads "Reviewed … with <model>. Nothing found." with the stepper ending in "Not posted", and the waterfall shows collect, triage, analyze, verify and prepare.
- **Second view.** Run `node $S/probe.mjs /private/tmp/v1/s.sock snapshot > .verify-evidence/<run>/snapshot.txt`. It lists one PR, headSha `adaac5a…`, with a run whose `phase` and `status` are `COMPLETED`.
- **Nothing posted.** Run `$S/gh-counts.sh octocat/Hello-World 11609 > github-after.txt`, then `diff github-before.txt github-after.txt`. The diff must be empty.
- **Refused.** Open the sheet again and type the same PR: it opens the tracked PR. Type `not a pr`: the sheet refuses it in place.

The snapshot harness has the sheet's static states: `desktop/snap.sh .verify-evidence/<run> 18a-add-pr-empty-dark 18b-add-pr-valid-dark 18c-add-pr-error-dark`.

## Gotchas

- A fixture daemon grants no `review.add`, and the sheet is locked there. Use `--engine` or `--no-start`.
- "Starting the reviewer, then adding …" shows only until the daemon answers. On this machine that is under a second, so `03-starting.png` usually catches the "Starting the reviewer" page with the sheet already closed. Both prove the start path.
- In a scratch data dir the account footer reads "No GitHub account" even though the daemon reads GitHub through `gh`. That comes from the footer's account rule, not a failed read.
- The review costs model tokens. Keep to the tiny public PR.
- Any `publish_mode` other than `dry_run` or `approval` is refused by `daemon-up.sh`. Keep it that way.
