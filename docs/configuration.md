# Configuration

This page lists every setting pr-review reads: GitHub credentials, the review model, Jev, the config file, and the data directory.

## GitHub token

pr-review takes the first token it finds, in this order:

1. `GITHUB_TOKEN`.
2. `GH_TOKEN`.
3. The account selected with `pr-review accounts use <login>`. pr-review reads its token with `gh auth token --hostname <host> --user <login>`.
4. The active `github.com` account in `gh`.

Without a token, pr-review reads public repositories only.

- **Selected account.** The selection is stored per data directory. Only `github.com` accounts can be selected. `accounts use` does not change which account is active in `gh` itself.
- **No silent fallback.** If the selected account disappears from `gh` or has an auth problem, the review fails and names the account. pr-review never switches to another identity.
- **Environment tokens win.** While `GITHUB_TOKEN` or `GH_TOKEN` is set, selecting an account is refused, because the selection would have no effect.
- **Where the token goes.** The token goes to the GitHub API, and to `git clone` and `git fetch` as a header given on that one command. It is not put in the environment, not written to the clone's git config, and redacted from git error messages. No command prints it.

`pr-review accounts` lists the `gh` accounts it found, their state, and which one is in use:

```text
gh: /opt/homebrew/bin/gh
  github.com/diego-arbio-dev               ok     active          gist, read:org, repo, workflow
using: github.com/diego-arbio-dev (gh active account)
```

## Review model

pr-review picks the model in this order:

1. `--model provider/id`.
2. `PR_REVIEW_MODEL`.
3. `models.default` in `pr-review.config.json`.
4. In a terminal, a picker that lists the available models and offers to save your choice.
5. Without a terminal, the first available model in `models.preferred`.

A model is available when its provider has a credential. pr-review reads credentials from `~/.pi/agent/auth.json`, which it shares with the `pi` CLI, and from provider environment variables such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and `OPENROUTER_API_KEY`. Set `PR_REVIEW_AUTH_FILE` to use a different auth file.

pr-review writes pi's auth file only under pi's own file lock, so a token refresh never logs `pi` out. It does not read the credentials of `claude`, `codex`, or `opencode`. `pr-review models` lists those CLIs as installed with credentials not shared.

The model is part of the reviewer's identity. Reviewing the same commit with another model creates a new revision instead of returning the stored result. Each revision is reviewed with the model its identity names, so a review that resumes after a restart keeps its model even when the configured one changed meanwhile.

### Change the model from the desktop app

Settings → Agent in the desktop app lists the models the daemon can review with, grouped by provider, with the one in use and its source. Save sends the protocol's `settings.set`. The daemon checks the model against that list, writes it to `models.default` in its config file (`PR_REVIEW_CONFIG`, or `pr-review.config.json` in its working directory) and keeps the other keys. Reviews started after that use the new model. The daemon does not need a restart, and a review in flight keeps its model.

Only a daemon in engine mode accepts the change. While the daemon's environment sets `PR_REVIEW_MODEL`, the daemon refuses the change, because the environment would win again at the next start.

## Jev

Jev classifies the risk of a change, and the classification decides how deep the review goes.

- With `TYPESAFE_API_KEY` set, pr-review uses TypeSafe.
- Without it, the CLI uses the keyless OpenCode Zen tier for public repositories.
- For a private repository without `TYPESAFE_API_KEY`, pr-review skips Jev and records the reason `jev_skipped_private_repo`. The PR title, description, and a diff excerpt would otherwise leave the machine. To allow the keyless tier for private repositories, set `"jev": { "allow_keyless_for_private": true }`.
- `pr-review serve` and `pr-review daemon --allow-commands` share one Jev client across every repository. Without `TYPESAFE_API_KEY` and without `allow_keyless_for_private`, they skip Jev for every repository, public ones included, and record the reason `jev_skipped_no_key`.
- `--no-jev` skips Jev and uses the standard review.

When Jev fails, times out, or is skipped, the review runs at standard depth. Jev can raise the depth. It can never lower it below standard.

### TypeSafe key in the macOS Keychain

A daemon that runs from a LaunchAgent has no `TYPESAFE_API_KEY` in its environment. On macOS, save the key in the desktop app under Settings → Risk check (Jev) instead. pr-review looks for the key each time it triages a review: first `TYPESAFE_API_KEY`, then the Keychain. A saved or removed key applies to the next review without a restart. `pr-review setup` reports `TypeSafe (key in Keychain)` when the key comes from the Keychain. On other platforms, set `TYPESAFE_API_KEY`.

The key is a generic password in the login keychain, with the service `io.github.diego-arbio-dev.pr-review` and the account `TYPESAFE_API_KEY`. `PR_REVIEW_KEYCHAIN_SERVICE` changes the service, for tests. The test suite sets it, so no test reaches the real item.

How the key is kept and why:

- **Every Keychain call goes through `/usr/bin/security`.** A keychain item's access list trusts the program that created it. An item the desktop app wrote through the Security framework would trust only the app's code signature, and the daemon's read through `/usr/bin/security` would show an access prompt, or fail with no one at the screen. Written by `/usr/bin/security`, the item trusts `/usr/bin/security`, which the daemon reads it with, so the read shows no prompt. The trade-off: any process that runs as you can read the item through `/usr/bin/security` without a prompt, as it can read your environment. The login keychain still protects the key at rest and from other users.
- **The key is never in a command line.** The app runs `security -i` and writes the `add-generic-password -U` command, with the key, to its standard input, so `ps` shows only `/usr/bin/security -i`. `add-generic-password -w` with no value is not used: it reads the password from the terminal when the app has one, and it cuts the password at 128 characters. The daemon runs `security find-generic-password -s <service> -a TYPESAFE_API_KEY -w` and reads the key from standard output.
- **The key stays out of files and the protocol.** It is never written to a file, a plist, the config, a log, a snapshot or a protocol message, and the app never sends it to the daemon. The field is masked, is cleared after Save, and the page shows only whether a key is saved. The setup check and the app's status come from the item's attributes, not from reading the key.
- **Save updates in place.** `-U` replaces the password of the existing item, so a second Save does not add a second item. Remove deletes the item.
- **The key format is restricted.** The app accepts a key without spaces, quotes, backslashes or non-ASCII characters, which `security -i` would read differently.

## Config file

`pr-review.config.json` in the working directory is optional. `PR_REVIEW_CONFIG` names another path. The file overrides the defaults in `src/config/config.ts`. An unknown key or a wrong type is an error.

```json
{
	"review": { "publish_mode": "comment", "max_retries_per_phase": 3, "debounce_seconds": 30 },
	"routing": { "mandatory_deep_paths": ["**/auth/**", "**/payments/**", "**/booking/**", "**/migrations/**"] },
	"models": { "default": "anthropic/claude-opus-5-5" },
	"jev": { "allow_keyless_for_private": false },
	"control": { "allow_commands": false }
}
```

`src/config/config.ts` lists every key and its default.

`pr-review setup` reports the config file, the model, the GitHub account, and the Jev tier described on this page. It can write `models.default` and `review.publish_mode` into the config file and keeps the other keys. It writes to `PR_REVIEW_CONFIG` when that is set.

### Publish mode

`review.publish_mode` decides what happens to a finished review:

| Value | Effect |
|---|---|
| `dry_run` | The default. Records the review and posts nothing. |
| `comment` | Posts one `COMMENT` review per revision through the outbox. |
| `silent_no_findings` | As `comment`, but posts only a review with something new: a new finding, a first inline comment on a carried finding, or a newly resolved finding. |
| `approval` | Stores the payload and parks the run in `AWAITING_APPROVAL` until `pr-review approve` or the terminal client approves it. See [Publishing](publishing.md#approval-mode). |

### Control commands

`control.allow_commands` lets `pr-review daemon` accept the review commands (start, retry, cancel, publish) from the terminal client. The default is `false`: the daemon stays read-only, takes no lock on the data directory, runs no review, and grants only the account selector. `pr-review daemon --allow-commands` enables them for one daemon, which then hosts the review engine. The daemon grants `review.publish` only under `publish_mode: "approval"`. With an engine daemon running, add PRs from a client (`a` in the terminal client, ⌘N in the desktop app) rather than with `pr-review review`, which can't take the lock the daemon holds. See [Take actions](tui.md#take-actions).

## State

All state lives in `.data/`, or in the directory that `PR_REVIEW_DATA_DIR` names:

| File | Contents |
|---|---|
| `app.sqlite` | PRs, revisions, findings, the publication outbox, the webhook inbox, and the audit log |
| `durable.sqlite` | Pi Durable checkpoints and the model conversations |
| `repos/` | Bare git clones, one per repository |

Only one `pr-review` process can use a data directory at a time. A second one exits with `another pr-review is running on <dir>`. After a `kill -9`, the next run takes over the lock within about 5 seconds. The control daemon takes the lock only with commands enabled, and only when it can; otherwise it runs in read-only mode beside the process that holds it.
