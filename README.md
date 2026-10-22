# pr-review

pr-review reviews GitHub pull requests and reports only findings it can back with evidence from the code. It tracks each PR across its commits, so a crash or restart resumes the review instead of starting over. Publishing is designed to post at most one review per commit, even when a write to GitHub times out or the process is killed partway through.

You can run it three ways:

- As a CLI that reviews one PR and prints the result, or posts it as a `COMMENT` review.
- As a GitHub App service that reviews PRs when webhooks arrive.
- From a terminal, `pr-review tui`, which opens on the most severe finding, shows each running review live, and puts every finding beside its code and its diff.

```text
$ node bin/pr-review.mjs review https://github.com/diego-arbio-dev/pr-reviewer/pull/2 --publish --model anthropic/claude-opus-5-5
model: anthropic/claude-opus-5-5 (--model)
revision d3defce78587d5d2fc4c3febae2da604 (diego-arbio-dev/pr-reviewer#2 @ 7683015c4b62) durable task 7
publication published: review 5586025680
## Automated PR review

Reviewed commit: `7683015c4b62b1b3ba765daa799368e4e073c52f`

Found 1 actionable issue.

- Critical: DELETE /orders/:id has no authentication or authorization (`src/routes/orders.js:16`)

Review performed with isolated read-only tools. Human review is still required.
```

## How a review works

Each review runs as a durable task with fixed phases: collect context, triage, analyze, verify, prepare, and publish. Every phase saves a checkpoint before the next one starts.

- **Triage.** Jev, a fast classifier, scores the risk of the change. It can raise the review depth above standard, never lower it. Path rules force a deeper review for areas such as auth, payments, and migrations.
- **Analyze.** A reasoning model reads the change through read-only tools: the diff, files and search at the pinned commit, and git history. It cannot write files, run commands, or post to GitHub.
- **Verify.** Code, not the model, checks every candidate finding. The finding must sit on a line in the diff, and its evidence must exist at the cited commit. Candidates that fail are discarded and their reasons recorded.
- **Publish.** The review goes through an outbox. pr-review checks GitHub for its own earlier review before posting. When a write's result is uncertain, it reconciles against GitHub instead of retrying blindly.

### The review state machine

Each commit's review moves through these states, as defined by `REVIEW_MACHINE` in [`src/domain/machines.ts`](src/domain/machines.ts):

```mermaid
stateDiagram-v2
    direction LR
    [*] --> COLLECT_CONTEXT
    COLLECT_CONTEXT --> TRIAGE: CONTEXT_PERSISTED
    TRIAGE --> ANALYZE: ROUTING_PERSISTED
    ANALYZE --> VERIFY: CANDIDATES_PERSISTED
    VERIFY --> PREPARE_PUBLICATION: CANDIDATES_DISPOSED
    PREPARE_PUBLICATION --> PUBLISH: HEAD_VERIFIED
    PREPARE_PUBLICATION --> AWAITING_APPROVAL: APPROVAL_REQUIRED
    AWAITING_APPROVAL --> PUBLISH: APPROVED
    AWAITING_APPROVAL --> FAILED: CANCELLED
    PUBLISH --> COMPLETED: REVIEW_RECONCILED
    PUBLISH --> COMPLETED: NO_PUBLICATION_RECORDED
    PUBLISH --> RECONCILE: WRITE_UNCERTAIN
    RECONCILE --> COMPLETED: REVIEW_RECONCILED
    RECONCILE --> PUBLISH: CONFIRMED_NOT_POSTED
    RECONCILE --> FAILED: TERMINAL_ERROR
    FAILED --> COLLECT_CONTEXT: MANUAL_RETRY
    FAILED --> COMPLETED: REVIEW_RECONCILED
    COMPLETED --> [*]
    SUPERSEDED --> [*]
```

To keep the diagram readable, two kinds of transition are left out:

- Every state from `COLLECT_CONTEXT` through `PUBLISH` moves to `FAILED` on `TERMINAL_ERROR`. `AWAITING_APPROVAL` is the exception: it fails only when the review is cancelled.
- Every state from `COLLECT_CONTEXT` through `PUBLISH`, `AWAITING_APPROVAL` included, moves to `SUPERSEDED` on `NEWER_HEAD` or `PR_NOT_OPEN`. `PUBLISH` does so only while nothing has been sent. `RECONCILE` never does, because a write may already have reached GitHub, and only reconciliation can decide.

`COMPLETED` and `SUPERSEDED` are final. `FAILED` stays final unless a manual retry restarts the review, or a later reconcile finds the review on GitHub.

[`docs/SPEC.md`](docs/SPEC.md) is the full design.

## The terminal client

![The terminal client lands on a critical finding, scrolls its explanation, opens the diff at its line, watches a review park with two findings, opens its timeline, previews the publication, approves it, starts the first review of a new PR and watches it finish, and cancels a running review.](tui/demo/showcase.gif)

The client opens on the worst finding across the tracked PRs, and `w` follows a review while it runs. [The terminal client](docs/tui.md) covers its views, keys, and settings.

## Get started

You need Node 22.19 or later, pnpm, and git. The terminal client also needs a Rust toolchain.

To try the terminal client first, play the recorded showcase session. It needs no daemon, no credentials and no network:

```bash
(cd tui && cargo build --release) && ./tui/target/release/pr-review-tui --playback protocol/playback/showcase.ndjson
```

Press `?` for the keys and `q` to quit. To review your own PRs:

1. Install the dependencies:

	```bash
	pnpm install
	```

2. Check your setup:

	```bash
	node bin/pr-review.mjs setup
	```

	`setup` checks git, the config file, the model, the GitHub account, Jev, and the terminal client. It offers to log in, to pick a default model, and to choose a publish mode. Rerun it at any time to see what is done. In CI or a container, run `setup --check`. It only prints the checks, and exits 1 when a review cannot run.

	pr-review finds credentials in `~/.pi/agent/auth.json`, which it shares with the `pi` CLI, and in provider environment variables such as `ANTHROPIC_API_KEY`. `node bin/pr-review.mjs models` lists them. If no model is found, run `node bin/pr-review.mjs login anthropic`.

3. Review a PR without posting anything:

	```bash
	node bin/pr-review.mjs review https://github.com/owner/repo/pull/123
	```

	Public repositories need no GitHub token. For private ones, pr-review uses `GITHUB_TOKEN`, or your `gh` login.

4. See every phase transition, the routing decision, and the discarded candidates:

	```bash
	node bin/pr-review.mjs inspect --pr owner/repo#123
	```

To post the review to the PR, add `--publish`. pr-review posts one `COMMENT` review with inline comments. It never approves, requests changes, or merges.

### Configure it

`pr-review.config.json` in the working directory is optional and overrides the defaults:

```json
{
	"review": { "publish_mode": "approval" },
	"models": { "default": "anthropic/claude-opus-5-5" }
}
```

`review.publish_mode` decides what happens to a finished review:

- `dry_run`, the default, records the review and posts nothing.
- `comment` posts it to the PR.
- `approval` holds it until you approve it in the terminal client or with `pr-review approve`.

To run the terminal client against your own reviews, build it, then start the daemon and the client in two terminals:

```bash
(cd tui && cargo build --release)
node bin/pr-review.mjs daemon --allow-commands   # terminal 1
node bin/pr-review.mjs tui                       # terminal 2
```

To review PRs when webhooks arrive, run `pr-review serve` as a GitHub App. [Running the service](docs/service.md) covers the setup.

### Set up Jev

Jev classifies the risk of a change during triage. It can raise the review depth above standard, never lower it, so without Jev every review runs at standard depth.

| Situation | What Jev uses |
|---|---|
| Public repository, no key | The free OpenCode Zen tier. Nothing to set up. |
| `TYPESAFE_API_KEY` set | TypeSafe's API, for every repository, private ones included. |
| Private repository, no key | Nothing. Jev is skipped with `jev_skipped_private_repo`, so the PR's diff never reaches the free tier. |

To use Jev on private repositories, get an API key from TypeSafe and export it:

```bash
export TYPESAFE_API_KEY=...
```

To send private diffs to the free tier instead, set `"jev": { "allow_keyless_for_private": true }` in `pr-review.config.json`.

`pr-review serve` and `pr-review daemon --allow-commands` share one Jev client across every repository. Without a key, and without `allow_keyless_for_private`, they skip Jev for public repositories too, with the reason `jev_skipped_no_key`.

To check what Jev decided, run `node bin/pr-review.mjs inspect --pr owner/repo#123`. Its `mode` line shows the review depth and the reasons, including why Jev was skipped. `--no-jev` skips Jev for one review.

[Configuration](docs/configuration.md) lists every setting.

## Commands

| Command | What it does |
|---|---|
| `setup [--check] [--json]` | Check what a review needs and offer to fix it. `--check` never prompts, and exits 1 when a review cannot run. |
| `review <pr-url> [--dry-run \| --publish] [--json] [--model provider/id] [--no-jev] [--retry]` | Review one PR. Dry run is the default. Rerunning resumes an interrupted review or prints the stored result. |
| `inspect --pr owner/repo#N [--json]` | Show a PR's revisions, transitions, routing, findings, and discards. |
| `reconcile --revision <id>` | Decide an uncertain publication by listing the PR's reviews. Never posts. |
| `serve` | Run the webhook receiver, the worker, and the reviews in one process. |
| `models [--json]` | List the installed agent CLIs and the models you can use. |
| `login [provider] [--api-key]` | Store a model credential in pi's auth file. |
| `accounts [--json]` | List your `gh` accounts and show which one pr-review uses. |
| `accounts use <login>` | Use that `gh` account for GitHub reads and publishing in this data directory. |
| `approve --revision <id> --preview <hash>` | Publish a review that waits for approval (`publish_mode: "approval"`), exactly the stored payload, once. |
| `daemon [--socket PATH] [--data DIR] [--fixture NAME] [--allow-commands]` | Serve review state to the terminal client over a Unix socket, read-only by default; with `--allow-commands` it hosts the engine and runs the client's start, retry, cancel, and publish commands. |
| `tui [args]` | Start the terminal client, `pr-review-tui`. |

Run every command as `node bin/pr-review.mjs <command>`.

## Documentation

- [Configuration](docs/configuration.md) covers GitHub tokens and accounts, model choice, Jev, the config file, and where state lives.
- [Publishing](docs/publishing.md) explains how pr-review avoids posting a review twice, and how to recover an uncertain publication.
- [Running the service](docs/service.md) covers GitHub App setup, environment variables, endpoints, and deployment.
- [The terminal client](docs/tui.md) covers the daemon, the client's views, keys, mouse, and background setting, and the account selector.
- [Protocol v1](protocol/README.md) is the reference for the messages between the daemon and the TUI.
- [`docs/SPEC.md`](docs/SPEC.md) and [`docs/TUI_SPEC.md`](docs/TUI_SPEC.md) are the design specifications.

## Develop

```bash
pnpm typecheck && pnpm test             # TypeScript engine, daemon, and protocol
(cd tui && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check)
pnpm protocol:emit                      # regenerate protocol/schema, fixtures, and playback after changing src/protocol
```

The TypeScript code lives in `src/`, one directory per part of the system:

| Directory | Contents |
|---|---|
| `src/domain` | State machines for reviews, the outbox, the PR coordinator, candidates, and findings |
| `src/runtime` | The durable review task and the phases it runs |
| `src/reasoning` | The model's read-only tools, the candidate schema, and verification |
| `src/jev` | The Jev adapter and the routing policy |
| `src/source` | Git reads pinned by commit SHA |
| `src/github` | The GitHub client, PR reads, and `gh` account detection |
| `src/publish` | The review payload, posting, and reconciliation |
| `src/coordinator` | Webhook routing, debounce, leases, and superseding older revisions |
| `src/server` | HTTP endpoints and the service loop |
| `src/persistence` | The application SQLite store and its audit log |
| `src/models` | Model and harness detection, and the shared credential store |
| `src/protocol`, `src/control` | The TUI protocol and the control daemon |
| `src/cli` | The command-line interface |

The Rust terminal client is in `tui/`. Migrations are in `migrations/`. Commits follow [Conventional Commits](https://www.conventionalcommits.org/).

## Limits

- Verification is static. pr-review runs no tests against PR code, and no second model double-checks a finding.
- A finding is tracked across commits by the code it cites: a later commit's review comments on it once, then lists it as still present, possibly fixed, outdated, or resolved. A finding is resolved only when its cited code changed, compared as code tokens so that comments, blank lines, spacing and semicolons never count, and the model's claim, with head-side evidence, passes the same checks as a finding. A fix outside the cited code stays listed as still present, and code cannot tell a real fix from a rename inside the cited code, so past that line the status rests on the model's claim. The bot never replies in review threads and never resolves them on GitHub; "looks resolved" in the review body is all it says.
- The daemon accepts start, retry, cancel, and publish commands only when enabled, which is also when it takes the data directory itself.
- The [benchmark](bench/README.md) measures precision and recall on 12 small labeled PRs that we wrote ourselves, matched to findings by line. It shows how a change moves the numbers, but it does not predict precision and recall on real PRs. Eight more cases push later commits and score how findings are tracked across them.
