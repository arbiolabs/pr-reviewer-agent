# Settings

Settings (⌘,) opens on Appearance, with a tile for each theme: System, Light, Dark, Solarized Light, Solarized Dark and Nord. A choice applies at once and is saved as `theme` in the config's `[desktop]` section. Below that are the setup checks, Risk check (Jev), which saves or removes the TypeSafe key in the login Keychain, Agent, a searchable model picker that saves `models.default` through `settings.set`, the Reviewer card and the GitHub accounts.

## Sub-features

- `theme-pick`: a theme tile applies at once and writes `theme = "<id>"` into `desktop.toml`, leaving every other line as it was.
- `theme-palette`: the palette's "Theme: …" rows and View → Appearance.
- `theme-restore`: the saved theme returns on the next launch, and `--appearance` overrides it.
- `model-pick`: the Agent section lists the models by provider, and Save writes `models.default` to the daemon's `PR_REVIEW_CONFIG`.
- `jev-save`: a key pasted into the masked field is saved, and the page says "A key is saved in your Keychain".
- `jev-remove`: Remove leads back to "No key saved".
- `checks`: the daemon's setup checks.

## How to get to it (user POV)

- Press ⌘, or choose Settings in the sidebar.
- In the palette, choose "Settings and accounts" or a "Theme:" row.
- Click the account footer to open Settings at the GitHub accounts.

## Driving it with keys.sh on a live engine daemon

Preconditions: run `$S/daemon-up.sh v1`, then `$S/app-up.sh v1`. `env.sh` gives both processes `PR_REVIEW_KEYCHAIN_SERVICE=io.github.diego-arbio-dev.pr-review.verify-v1`. Before you touch the Jev field, confirm that the test item is absent with `security find-generic-password -s io.github.diego-arbio-dev.pr-review.verify-v1 -a TYPESAFE_API_KEY`, which must fail.

- **Theme.** Run `$S/keys.sh <pid> cmd:k type:theme: nord wait:400 return`. The window turns Nord, and `grep theme /private/tmp/v1/desktop.toml` reads `theme = "nord"`. Capture a shot.
- **Restore.** Kill the app with `kill <pid>` and run `$S/app-up.sh v1` again. It opens in Nord.
- **Model.** Run `$S/keys.sh <pid> cmd:,`, then tab to Agent and pick a model. After Save, `cat /private/tmp/v1/config.json` has `models.default`, and `probe.mjs` still welcomes. The next review's run shows that `model`.
- **Jev save.** Focus the "Paste your TypeSafe key" field, `type:` a dummy such as `verify-dummy-key`, then save. The page says "A key is saved in your Keychain". Read the test item back with `security find-generic-password -s io.github.diego-arbio-dev.pr-review.verify-v1 -a TYPESAFE_API_KEY -w`.
- **Jev remove.** Choose Remove. The page says "No key saved", and the `find-generic-password` above fails again.
- **Deterministic.** Run `desktop/snap.sh .verify-evidence/<run> 08-settings-dark 21d-appearance-settings`.

## Gotchas

- Without `PR_REVIEW_KEYCHAIN_SERVICE`, the app and the daemon use the real item `io.github.diego-arbio-dev.pr-review`. Run `cat /private/tmp/v1/env.sh` and check that the app was launched through `app-up.sh`.
- Never type a real key. A dummy key makes the next triage call TypeSafe and fail. That is harmless, but it shows up as a Jev skip in the run.
- `settings.set` is refused while `PR_REVIEW_MODEL` is set in the daemon's environment.
- The Agent list's order and focus target can change. Prefer the field's search to Tab counting, and confirm the result from `config.json`, not from the screen.
- `down.sh` deletes the test Keychain item, so a leftover never carries into the next run.
