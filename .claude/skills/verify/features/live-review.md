# Live review and waterfall

While a review runs, its PR page shows the phase stepper (Collect, Triage, Analyze, Verify, Prepare, then Posted or Not posted), a live log of `tool.activity` lines, and a waterfall of phases, steps, model turns and tool calls with timings and a now line. A quiet review shows "last activity Ns ago". After the run, the waterfall stays on the PR page.

## Sub-features

- `live-strip`: the strip of running reviews above the queue.
- `live-log`: the live log, and scrubbing it back to an earlier event.
- `waterfall-live`: the waterfall with its now line and folded turns. A clicked span shows its detail.
- `waterfall-after`: the waterfall on a finished run's PR page.
- `awaiting-approval`: under `approval`, a run parks in AWAITING_APPROVAL and Needs you shows "Review and post #N".

## How to get to it (user POV)

- Add or start a review, then select the running PR in the sidebar or the live strip.
- In the palette, choose "Watch #N live".

## Driving it with desktop/snap.sh and a live engine daemon

Preconditions: `desktop/` builds. The live part needs an engine daemon and the Hello-World PR, as in [review-a-pr.md](./review-a-pr.md).

- **Running, deterministic.** Run `desktop/snap.sh .verify-evidence/<run> 05-running-dark 05b-replay-dark 05c-waterfall-dark`. PR #1291 runs. The replay holds the log at event 12. The waterfall has "turn 4" open.
- **Live.** Run `$S/daemon-up.sh v1`, then `$S/app-up.sh v1`, then `$S/keys.sh <pid> cmd:n wait:600 type:octocat/Hello-World#11609 return`. Capture shots every 2 s with `for i in 1 2 3 4 5; do $S/shot.sh <pid> …/live-$i.png; sleep 2; done`. They show the stepper advancing and the waterfall growing.
- **After.** Once the run completes, the PR page keeps the waterfall. `probe.mjs … snapshot` shows the run `COMPLETED`.
- **Approval.** Run `$S/daemon-up.sh v1 --mode approval` and add the PR the same way. The run parks, and `probe.mjs` shows `phase` `AWAITING_APPROVAL` with `status` `RUNNING`. Stop there and never confirm the post.

## Gotchas

- A review of the tiny Hello-World PR takes about 6 s, which leaves little time for the live shots. Start the shot loop in the same command as the ⏎.
- Activity lines carry counts and timings only. Model text never appears there, so its absence is not a bug.
- `scrub:` and `span:` are playback steps only.
