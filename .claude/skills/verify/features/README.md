# PR Reviewer Agent verification map

This directory is the maintained source for verifying what a user of pr-review sees and does. Read this index, then use the feature's file as the recipe. Commands assume the repository root and `S=.claude/skills/verify/scripts`.

## Baseline preconditions

- Follow the isolation rules in [../SKILL.md](../SKILL.md). This map never drives the operator's install.
- Use one scratch instance per run, `/private/tmp/<name>`, made by `$S/daemon-up.sh <name> …`. It starts empty: no tracked PRs and no saved Keychain item.
- Run `$S/doctor.sh <name>` before each drive. It must report the expected `mode`: `engine`, or no mode for a fixture.
- Each feature names its path. A snapshot-harness state is a deterministic playback. A live drive uses a fixture or an engine daemon.

## Driving conventions

- Live UI goes through `$S/keys.sh <app-pid> …`: shortcuts first, then the palette (⌘K plus a command's title), and coordinates never.
- Identify state by visible text: "Reviewer · Live", "Reviewer · Offline", "Review a PR", "Not posted", "Nothing found on this head", "Risk check (Jev)", "No key saved".
- Read stored state with `node $S/probe.mjs /private/tmp/<name>/s.sock snapshot`.
- Never confirm a post (⌘⏎, "Confirm") on a live daemon.

## Proof and skip reporting

- Keep each run's proof in `.verify-evidence/<run>/`. Record the feature ID and the entry point used.
- Capture a window shot before the action and one after it, with "PR Reviewer Agent" visible in the title bar.
- A live mutation needs a second view: `probe.mjs … snapshot`, and the daemon log.
- An engine drive needs `gh-counts.sh` from before and after it, and the two must match.
- A path you could not reach is a failure. Report the step you tried and the precondition that was missing. Never report it as verified through another path.

## Feature entry contract

Each feature file has an H1 title and one paragraph, then four H2s in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with <harness>`, `Gotchas`.

## Features

- [Review queue and a finding](./queue-and-finding.md): Needs you and Done, a PR's findings, a finding's evidence, the diff.
- [Live review and waterfall](./live-review.md): a running review's phases, live log and waterfall.
- [Review a PR (⌘N)](./review-a-pr.md): adding a PR by URL, including starting a stopped reviewer. Proven live in `.verify-evidence/review-a-pr-1/`.
- [Reviewer Start/Stop](./reviewer-start-stop.md): the title-bar pill, the palette, the offline screen.
- [Settings](./settings.md): themes, the model picker, the Jev key in the Keychain under a test service.
- Review again (⇧S) and the post proof sheet are reachable in the snapshot harness (`22-review-again-*`, `22b-again-confirm-*`, `06-approval-*`, `06b-confirm-post-*`). They have no feature file yet.
