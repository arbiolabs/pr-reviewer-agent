# pr-review (M1)

pr-review is a local, durable PR reviewer built on Pi Durable, Jev and pi-ai. It reviews one GitHub pull request revision and prints evidence-backed findings as Markdown or JSON. It never writes to GitHub. `SPEC.md` is the full design. This branch implements milestone M1, the MVP definition of done in section 18.

## Run

Node 22.19 or later and pnpm.

```bash
pnpm install
node bin/pr-review.mjs models                          # agent CLIs found on PATH, and the models you can use
node bin/pr-review.mjs review https://github.com/owner/repo/pull/123 --dry-run [--json] [--no-jev] [--model anthropic/claude-opus-5-5] [--retry]
node bin/pr-review.mjs inspect --pr owner/repo#123      # every phase transition, the routing decision, findings and discards
node bin/pr-review.mjs login anthropic [--api-key]      # store a credential in pi's auth file (OAuth where offered)
pnpm typecheck && pnpm test
```

- **GitHub access.** The tool reads `GITHUB_TOKEN`, then `gh auth token`. Without a token it can read public repositories only. With a token it can review private repositories: the token goes to the GitHub API, and to `git clone` and `git fetch` as a header given on that one command. It is not put in the environment, not written to the bare clone's config, and redacted from git error messages.
- **Closed and merged PRs.** `review` does not check whether the PR is open. M1 reviews closed and merged PRs on purpose, so you can review historic PRs.
- **Models.** Credentials come from `~/.pi/agent/auth.json`, which pi shares, or from provider environment variables such as `ANTHROPIC_API_KEY`. The tool picks the model in this order: `--model`, then `PR_REVIEW_MODEL`, then `models.default` in `pr-review.config.json`. If none of these is set, a TTY shows a picker. Without a TTY, the tool takes the first available model in `models.preferred`. The model is part of the reviewer identity, so reviewing the same head with another model creates a new revision.
- **Jev.** By default the tool uses the keyless OpenCode Zen tier. If `TYPESAFE_API_KEY` is set, it uses TypeSafe instead. `--no-jev` skips Jev and uses the fallback route.
- **Jev and private repositories.** The PR title, description and a diff excerpt go to Jev. For a private repository without `TYPESAFE_API_KEY`, the tool does not call the keyless endpoint: it takes the fallback route and records the reason `jev_skipped_private_repo`. To allow the keyless endpoint for private repositories, set `"jev": { "allow_keyless_for_private": true }` in `pr-review.config.json`.
- **Config.** `pr-review.config.json` is optional. It overrides the defaults in `src/config/config.ts`. An unknown key or a wrong type is an error.
- **State.** All state lives in `.data/` (or `PR_REVIEW_DATA_DIR`):
  - `app.sqlite` holds business records.
  - `durable.sqlite` holds Pi Durable checkpoints and conversations.
  - `repos/` holds the bare clones.

If you kill the process and run the same command again, the review resumes. A revision that already finished prints its stored result. Only one `pr-review review` can use a data directory at a time. A second one exits with "another pr-review is running on <dir>". After a `kill -9`, the next run takes over the lock within about 5 seconds.

A revision that FAILED stays failed. A rerun prints the failure reason and tells you to use `--retry`. `review <url> --retry` reopens a FAILED revision. This is a manual override: the audit log records it as `FAILED --MANUAL_RETRY--> COLLECT_CONTEXT`, and the retry runs as a new durable task. It reuses the stored context, routing decision and candidates where they exist. `--retry` is refused for a COMPLETED or SUPERSEDED revision.

## Control daemon and TUI

`pr-review daemon` serves the TUI protocol (`protocol/README.md`) on a Unix socket. It runs in the foreground until SIGINT or SIGTERM, so run it under your own supervisor.

```bash
node bin/pr-review.mjs daemon [--data .data] [--socket PATH] [--heartbeat 15]   # reads the app DB of a data dir
node bin/pr-review.mjs daemon --fixture normal                                   # plays protocol/playback/normal.ndjson; no GitHub, no model
node bin/pr-review.mjs tui [args]                                                # runs pr-review-tui from PATH or tui/target/release/
```

- **Socket.** The default path is `$XDG_RUNTIME_DIR/pr-review/reviewer.sock`, else `~/Library/Application Support/pr-review/reviewer.sock` on macOS, else `~/.local/state/pr-review/reviewer.sock`. The daemon creates the directory with mode 0700 and the socket with mode 0600. It refuses a directory that is a symlink, belongs to another user, or has a looser mode. It replaces a socket file that nothing listens on, and exits if another daemon is listening.
- **Read-only.** The daemon reads `app.sqlite` on a read-only connection and takes no data-dir lock, so it runs beside `pr-review review`. Its only write is applying migrations when it starts. It never opens `durable.sqlite`. Disconnecting or closing the TUI changes nothing in a running review.
- **Commands.** This version has no write capabilities. The daemon answers every `command` with `CAPABILITY_DENIED`.
- **Fixtures.** `--fixture` takes any script in `protocol/playback/`. The script clock starts at the first snapshot request. Fixture strings are not sanitized, because `hostile` exists to test the client.

## Layout

| Directory | Contents |
|---|---|
| `src/domain` | Pure transition tables for the review, coordinator, candidate and finding machines |
| `src/persistence` | The app SQLite store. Every revision transition is audited in the same transaction |
| `src/source` | Git reads pinned by SHA (`git show`, `grep`, `log`, `diff`). Nothing reads a working tree |
| `src/github` | PR metadata over plain `fetch` |
| `src/jev` | The Jev adapter and the deterministic routing policy |
| `src/reasoning` | Read-only model tools, the candidate schema, the request text and deterministic verification |
| `src/runtime` | The `pr-review.revision` durable task and how it is wired |
| `src/models` | The credential store over pi's auth file, harness and model detection, and model choice |
| `src/cli` | The command-line interface and the report rendering |
| `src/protocol` | The TUI protocol v1 schemas (TypeBox), fixtures and playback scripts |
| `src/control` | The control daemon: socket server, the app DB and fixture sources, diff paging |

## What M1 does not do

- It does not publish to GitHub. `--publish` exits with "publishing lands in M2".
- It has no webhook server, coordinator scheduling, leases or outbox. The coordinator state machine exists and is tested, but nothing drives it yet.
- It runs no tests in a sandbox. Verification is static. There is no second verification pass by an LLM.
- It does not re-review across revisions or classify findings as resolved. That is M3.
- It has no evals.
