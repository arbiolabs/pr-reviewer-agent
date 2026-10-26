# Reviewer Start/Stop

The title bar always shows the reviewer's state, "Reviewer · Live", "Offline", "Stopped by you" or "Starting…", with the one action that changes it: Stop while the daemon answers, Start while it doesn't. A spinner shows while either is under way, and an action that can't run is dimmed, with the reason in its tooltip. Stop sends `daemon.stop`. When a review is running, the app asks first. Start spawns `daemon_command`, here the instance's `daemon.sh`, or kicks the LaunchAgent, which this map never uses.

## Sub-features

- `pill-state`: the pill text for live, offline, starting and stopped.
- `stop`: Stop leads to "Stopped by you", which is not the same as offline. The daemon exits 0 and removes its socket.
- `stop-confirm`: Stop while a review runs asks first.
- `start`: Start (the pill, ⌘R on the offline screen, or the palette) leads to "Starting…", then "Live", within 20 s.
- `start-failed`: a start that never answers says why and names the log.
- `restart`: the Reviewer menu and the palette.
- `readonly-locked`: on a read-only daemon, Stop and Restart are off, with the reason.

## How to get to it (user POV)

- Click the pill at the top right of the title bar.
- Choose "Start reviewer" (⌘R) on the offline screen.
- In the palette (⌘K), choose "Stop reviewer", "Start reviewer" or "Restart reviewer".
- Use the Reviewer menu, or the Reviewer card in Settings.

## Driving it with keys.sh on a live engine daemon

Preconditions: run `$S/daemon-up.sh v1`, then `$S/app-up.sh v1`. Doctor must show `daemon.stop` among the capabilities.

- **Live.** The pill reads "Reviewer · Live". Capture `01-live.png`.
- **Stop.** Run `$S/keys.sh <pid> cmd:k type:stop reviewer wait:500 return`. The pill reads "Stopped by you". `/private/tmp/v1/s.sock` is gone, the daemon's pid has exited, and `daemon.log` has no error. Capture `02-stopped.png`.
- **Start.** Run `$S/keys.sh <pid> cmd:r`, or the palette's "start reviewer". The pill goes to "Starting…", then "Reviewer · Live". `pids` gains the spawned daemon, and `doctor.sh v1` passes again. Capture `03-starting.png` and `04-live-again.png`.
- **Deterministic states.** Run `desktop/snap.sh .verify-evidence/<run> 19-reviewer-stopped-dark 19b-starting-dark 19c-stop-confirm-dark 12-readonly-dark`.

## Gotchas

- The pill has no shortcut. Click it only when the pill itself is under test. To click, compute the point from `winid.sh` bounds; never use the full screen.
- The app's label for the LaunchAgent comes from `desktop.toml`, and here it does not exist, so Start always spawns. Never test the LaunchAgent path on this machine, because the real agent is the operator's.
- A fixture daemon can't be stopped: it does not grant `daemon.stop`.
- The spawned daemon outlives the app, since it runs in its own process group. `down.sh` kills it through `pids`.
