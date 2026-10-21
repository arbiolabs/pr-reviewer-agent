# Changelog

This file records every notable change to pr-review. Its format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version covers the CLI, the service, the daemon, the terminal client (`tui/`) and the version 1 protocol between the client and the daemon.

## [Unreleased]

### Changed

- **Spinners.** The terminal client marks running work (the PR rail, the live review, the status line and posting to GitHub) with ratatui-spinner's braille spinner (`⣾⣷⣯⣟⡿⢿⣽⣻`) instead of the turning quadrant. The phase steppers show the running phase as a filled dot instead of a spinner. In the live phase bar, the running phase's segment bounces an arc like ratatui-spinner's `BarSpinner` in its `Thick` style, instead of walking a highlight. `--ascii` keeps its glyphs.

## [1.0.0] - 2026-10-21

This is the first release.

### Added

- **Durable reviewer.** The `pr-review review <url>` command reviews one PR as a durable task, with checkpoints after collect context, triage, analyze, verify, prepare and publish. A crash or restart resumes the task where it stopped. Code checks every finding against the diff and the pinned commit before it can be reported (#1).
- **Model detection.** pr-review detects the installed harnesses and their models, and lets you choose a model with `--model` or interactively (#1).
- **GitHub App service.** `pr-review serve` reviews PRs when webhooks arrive. Each webhook's HMAC signature is verified, a lease coordinates the work, and bursts of webhooks are debounced (#3).
- **Exactly-once publication.** Reviews are published through an outbox. Before the first POST, pr-review lists GitHub's reviews to find any it already posted. It commits `sending` before posting, and reconciles against GitHub when a write's result is uncertain (#3).
- **Terminal client.** `pr-review tui` is a Ratatui client with Inbox, Review, Diff, Timeline and Settings views. It opens on the most severe finding, and `w` follows a running review. It supports mouse input, Ink (dark) and Paper (light) palettes with WCAG contrast of at least 4.5, light-background detection over OSC 11, `--ascii` and `NO_COLOR` modes, and a playback mode that runs the showcase with no daemon (#4, #9).
- **Control daemon.** `pr-review daemon` serves the client over a local socket. It speaks version 1 of the protocol, which has a JSON Schema, fixtures and a sequenced event stream (#4).
- **gh account selector.** The client and the daemon detect `gh` accounts without reading tokens, and the client can switch between them (#4).
- **Review actions.** With `pr-review daemon --allow-commands`, the client can start a review (`s`), retry a failed one (`R`) and cancel a running one (`c`). Each action asks a plain question before it is sent (#10).
- **Approve to publish.** With `review.publish_mode: "approval"`, a finished review waits in `AWAITING_APPROVAL`. `p` previews exactly what will be posted, `m` toggles the raw markdown, `y` publishes it once, and `o` opens the posted review (#10).
- **PR titles.** Migration 054 stores each PR's GitHub title, so the live stream shows it from the first event (#10).

### Changed

- The client's footer offers a key only when that key acts in the current view, and gives its room to action keys before the view list (#11).
- The client shows refusals and empty states as plain sentences. Refusals the client makes without asking the daemon are marked "not sent" (#11).
- The client sends any start the daemon may accept, and refuses up front only a start for a head whose review is running or parked (#11).

### Fixed

- The client no longer turns a late terminal reply to the colour query, or an Alt+arrow typed at startup, into keypresses (#11).
- `J`/`K` scrolling stops at the end of the document, and the scroll is clamped again after a resize (#11).
- `j`/`k` load the next page of a paged diff or timeline (#11).
- The daemon sends real code as real code as a finding's evidence, and never fetches from the network. Offline reads need git 2.45 or later, for `GIT_NO_LAZY_FETCH` (#8).
- The client uses a distinct glyph for critical findings, adds a severity legend, and shows the PR in the timeline panel (#7).

### Security

- `pr-review daemon` is read-only by default. It takes no lock and runs no engine until commands are enabled with `--allow-commands` or `control.allow_commands`.
- Before it publishes, `review.publish` checks four things:
  - the review is still waiting for approval;
  - the preview matches the stored payload;
  - the head on GitHub has not moved;
  - the PR is still open.
- Command idempotency keys are bound to each run attempt, so a replayed command never has a second effect.

[Unreleased]: https://github.com/diego-arbio-dev/pr-reviewer/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/diego-arbio-dev/pr-reviewer/releases/tag/v1.0.0
