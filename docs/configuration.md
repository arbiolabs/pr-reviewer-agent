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

The model is part of the reviewer's identity. Reviewing the same commit with another model creates a new revision instead of returning the stored result.

## Jev

Jev classifies the risk of a change, and the classification decides how deep the review goes.

- With `TYPESAFE_API_KEY` set, pr-review uses TypeSafe.
- Without it, the CLI uses the keyless OpenCode Zen tier for public repositories.
- For a private repository without `TYPESAFE_API_KEY`, pr-review skips Jev and records the reason `jev_skipped_private_repo`. The PR title, description, and a diff excerpt would otherwise leave the machine. To allow the keyless tier for private repositories, set `"jev": { "allow_keyless_for_private": true }`.
- `--no-jev` skips Jev and uses the standard review.

When Jev fails, times out, or is skipped, the review runs at standard depth. Jev can raise the depth. It can never lower it below standard.

## Config file

`pr-review.config.json` in the working directory is optional. `PR_REVIEW_CONFIG` names another path. The file overrides the defaults in `src/config/config.ts`. An unknown key or a wrong type is an error.

```json
{
	"review": { "publish_mode": "comment", "max_retries_per_phase": 3, "debounce_seconds": 30 },
	"routing": { "mandatory_deep_paths": ["**/auth/**", "**/payments/**", "**/booking/**", "**/migrations/**"] },
	"models": { "default": "anthropic/claude-opus-5-5" },
	"jev": { "allow_keyless_for_private": false }
}
```

`src/config/config.ts` lists every key and its default.

## State

All state lives in `.data/`, or in the directory that `PR_REVIEW_DATA_DIR` names:

| File | Contents |
|---|---|
| `app.sqlite` | PRs, revisions, findings, the publication outbox, the webhook inbox, and the audit log |
| `durable.sqlite` | Pi Durable checkpoints and the model conversations |
| `repos/` | Bare git clones, one per repository |

Only one `pr-review` process can use a data directory at a time. A second one exits with `another pr-review is running on <dir>`. After a `kill -9`, the next run takes over the lock within about 5 seconds. The control daemon only reads `app.sqlite`, so it runs beside a review.
