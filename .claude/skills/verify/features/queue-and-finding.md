# Review queue and a finding

The home screen is a triage inbox. "Needs you" lists the decisions that wait on the user: a post to approve, findings to judge, a failure to retry. "Done" lists the rest. Selecting a PR shows its findings. Opening a finding shows its explanation, its evidence with the cited code, and its history. The diff shows every finding under its line.

## Sub-features

- `queue-needs-you`: the Needs you list with its count badge, which shows even at zero.
- `queue-done`: the Done list (⌘2).
- `finding-list`: a PR's findings by severity.
- `finding-detail`: a finding's explanation, evidence and history.
- `finding-no-evidence`: a finding that cites no code shows the empty evidence state.
- `diff-anchored`: the diff (D) with findings anchored, and "flagged" to show only the files with findings.
- `timeline`: a PR's timeline, including the discarded groups.

## How to get to it (user POV)

- Launch the app. It opens on Needs you.
- Press ⌘1 or ⌘2, or choose the sidebar's Needs you or Done.
- Click a card or a sidebar PR, then a finding.
- Press D for the diff. Use the palette for "Diff of the selected PR" and "Timeline of the selected PR".

## Driving it with desktop/snap.sh and a fixture daemon

Preconditions: `desktop/` builds. The live drive needs `$S/daemon-up.sh v1 --fixture showcase`.

- **Overview.** Run `desktop/snap.sh .verify-evidence/<run> 01-overview-dark 20-footer-account-dark`. Needs you lists the showcase's cards with counts.
- **A finding.** Run `desktop/snap.sh .verify-evidence/<run> 02-pr-dark 03-finding-dark 14-finding-no-evidence-dark`. PR #1287's finding opens with its cited code. The third finding shows no evidence.
- **Diff and timeline.** Run `desktop/snap.sh .verify-evidence/<run> 04-diff-dark 17b-big-diff-flagged-dark 10-timeline-dark 16-discards-dark`.
- **Tracking.** Run `desktop/snap.sh .verify-evidence/<run> 15-tracking-dark 15b-reopened-dark`. These show a finding carried across heads, then reopened.
- **Live, on the fixture.** Run `$S/app-up.sh v1`, then `$S/keys.sh <pid> cmd:2 wait:500 cmd:1 wait:500 cmd:k type:1287 wait:400 return`. Capture `$S/shot.sh <pid> …/queue-live.png`. The same queue renders from the socket.

## Gotchas

- The showcase plays on a clock. Its states settle about 19 s in, which is why the harness waits `wait:19000`. The fixture daemon's clock starts at its first snapshot, so wait about 20 s after launch before you judge it.
- `snap.sh` with no OUT_DIR writes `desktop/snapshots/`. Always pass the evidence dir.
- A step such as `pr:1287` or `finding:2` works only under `--playback`. On a live app, use the keyboard.
