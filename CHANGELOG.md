# Changelog

This file records every notable change to pr-review. Its format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version covers the CLI, the service, the daemon, the terminal client (`tui/`) and the version 1 protocol between the client and the daemon.

## [Unreleased]

### Added

- **Real-PR eval.** `pnpm eval:snapshot` saves the open PRs of a repository, their inline review comments and reviews, and a full bare clone to a gitignored dataset directory, using GitHub GET requests only. `pnpm eval:real` reviews each saved PR offline in dry run, and refuses to start when the publish mode is not `dry_run` or a review would get `--publish`. `pnpm eval:report` reports overlap with the human comments on the head commit, run-to-run stability, Jev routing, the verifier's discards and cost proxies, and writes a grading sheet whose verdicts give a precision number. See `bench/REAL.md` (#28).
- **Eval: another AI reviewer as a comparator.** The real-PR eval sets apart comments from [pr-review-agent](https://github.com/vikrrantshah/pr-review-agent), which posts from a person's account and so read as human labels, compares its findings with pr-review's, and puts them on the grading sheet. The sheet also lists the candidates the verifier discarded (#30).

## [1.2.0] - 2026-10-21

### Added

- **Benchmark.** `pnpm bench -- --model provider/id` runs the dry-run review over 12 labeled cases in `bench/cases/` (8 with one seeded bug, 4 known-clean) and prints precision, recall, the verifier's discards, and one row per case. `--runs N` repeats the set to show variance, `--json` writes a results file, and `pnpm bench:compare` compares two of them. It calls a real model, so it never runs in CI (#22).
- **Multi-commit benchmark cases.** A case with a `commits` list reviews the PR again after each later commit, which can also delete files, and publishes every review to an in-process fake GitHub. Eight cases (`fix`, `leave`, `move`, `rewrite-still-buggy`, `partial-fix`, `worse`, `fix-elsewhere`, `regress`) report the false-resolution rate, identity kept, the duplicate-comment rate, and resolution recall. `bench/baselines/` holds the first run, which records how today's reviewer tracks findings (#25).
- **Carried findings in the review body.** A review of a later commit lists the findings from earlier reviews under "Still present from an earlier commit", "Possibly fixed, the cited code changed" and "Outdated, the cited file is gone", each with the commit it was first reported on, at most 20 per section and then "and N more". A body that would exceed GitHub's 65,536-character limit is cut at a line end, with a notice, and keeps its marker. A finding is found again by the code it cites, so it keeps its identity when its lines move, its file is renamed, or its text moves to one other file. A re-reported finding keeps its ID only when the report sits in the same file and category and names the same title where the finding was, cites the same code from the finding's own file, or cites the finding's lines from within three lines of it; a shared helper both cite is no link, and anything else is a new finding, which may repeat a comment but never hides a defect. Migration 055 records which GitHub review carries each finding's inline comment, and the terminal client's finding history gains an optional `basis` per entry. Marking a finding resolved is not built yet, so no finding reaches `RESOLVED`. `bench/baselines/finding-carry-over-2026-10-21.json` records the multi-commit benchmark after this change, and `finding-carry-over-2026-10-21-r2.json` one run after the audit fixes to the link rules (#26).
- **Setup checks in the terminal client.** The daemon sends the `pr-review setup --check` results in `welcome.checks`, an optional field that older clients ignore. Settings lists them under **Setup**, and an empty inbox names the missing checks and their fixes. The client never asks for or sends a secret (#23).

### Fixed

- **Duplicate inline comments on every push.** Each push posted a new inline comment for every finding, including the ones already commented on the last push. An inline comment now goes only to a finding never posted before; a finding reported again, or found unchanged, is listed in the review body instead. A finding whose review failed to post is commented on by the next push, unless its write may have reached GitHub, in which case `pr-review reconcile` decides first. Under `silent_no_findings`, a push posts only when it has a new finding or a first comment on a carried one (#26).
- **Jev skip reason.** `pr-review serve` and `pr-review daemon --allow-commands` record `jev_skipped_no_key` when they skip Jev for lack of `TYPESAFE_API_KEY`, instead of `jev_skipped_private_repo`, which was wrong for public repositories. A per-PR `review` of a private repository still records `jev_skipped_private_repo` (#21).

## [1.1.0] - 2026-10-21

### Added

- **Setup command.** `pr-review setup` checks git, the config file, the model, the GitHub account, Jev, and the terminal client, and offers to log in, pick a default model, and choose a publish mode. `setup --check [--json]` prints the same checks without prompting and exits 1 when a review cannot run. Saving the default model from the `review` picker now writes to `PR_REVIEW_CONFIG` when it is set (#19).

## [1.0.1] - 2026-10-21

### Changed

- **Spinners.** The terminal client marks running work (the PR rail, the live review, the status line and posting to GitHub) with ratatui-spinner's braille spinner (`⣾⣷⣯⣟⡿⢿⣽⣻`) instead of the turning quadrant. The phase steppers show the running phase as a filled dot instead of a spinner. In the live phase bar, the running phase's segment bounces an arc like ratatui-spinner's `BarSpinner` in its `Thick` style, instead of walking a highlight. `--ascii` keeps its glyphs (#13).

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

[Unreleased]: https://github.com/diego-arbio-dev/pr-reviewer/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/diego-arbio-dev/pr-reviewer/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/diego-arbio-dev/pr-reviewer/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/diego-arbio-dev/pr-reviewer/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/diego-arbio-dev/pr-reviewer/releases/tag/v1.0.0
