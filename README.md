# pr-review (M2)

pr-review is a durable PR reviewer built on Pi Durable, Jev and pi-ai. It reviews a GitHub pull request revision and reports evidence-backed findings. It can print them (`--dry-run`) or post them as one `COMMENT` review (`--publish`), and it can run as a GitHub App service that reviews PRs from webhooks. `SPEC.md` is the full design. This branch implements milestones M1 and M2.

## Run

Node 22.19 or later and pnpm.

```bash
pnpm install
node bin/pr-review.mjs models                          # agent CLIs found on PATH, and the models you can use
node bin/pr-review.mjs review https://github.com/owner/repo/pull/123 [--dry-run | --publish] [--json] [--no-jev] [--model anthropic/claude-opus-5-5] [--retry]
node bin/pr-review.mjs reconcile --revision <revision-id>   # decide an uncertain publication; never posts
node bin/pr-review.mjs serve                            # webhook receiver, worker and reviews; see "Service" below
node bin/pr-review.mjs inspect --pr owner/repo#123      # every phase transition, the routing decision, findings and discards
node bin/pr-review.mjs login anthropic [--api-key]      # store a credential in pi's auth file (OAuth where offered)
pnpm typecheck && pnpm test
```

- **GitHub access.** The tool reads `GITHUB_TOKEN`, then `gh auth token`. Without a token it can read public repositories only. With a token it can review private repositories: the token goes to the GitHub API, and to `git clone` and `git fetch` as a header given on that one command. It is not put in the environment, not written to the bare clone's config, and redacted from git error messages.
- **Closed and merged PRs.** `review` does not check whether the PR is open, so you can review historic PRs. The service never starts a review for a closed or merged PR.
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

## Publishing

`review --publish` needs a GitHub token with write access to pull requests (`GITHUB_TOKEN` or `gh auth token`). It posts at most one review per revision, through an outbox in `app.sqlite`:

1. PREPARE_PUBLICATION re-reads the PR head and stores the payload with the transition into PUBLISH. The payload has the summary body, the marker `<!-- durable-review:<revision>:<payload-hash> -->`, `commit_id` set to the head, `event: COMMENT`, and an inline comment for each verified finding whose line is in the diff.
2. PUBLISH re-reads the PR again. If it is no longer open (`pr_not_open`) or the head moved, the revision is SUPERSEDED and nothing is posted. A dry run of a closed PR still works. Then it lists every page of the PR's reviews. If one of ours on the head commit carries this revision's marker, with any payload hash, the revision completes with that review's ID and nothing is posted. This holds for a rerun from a fresh data dir, whose model answer differs. If the listing fails, nothing is posted. PUBLISH retries under `max_retries_per_phase`, then leaves the outbox `needs_reconciliation`.
3. The outbox is set to `sending` and committed before the POST. The returned review ID is saved after it.
4. If the result is uncertain (a 5xx, a dropped connection, a timeout, or a crash that left the outbox in `sending`), RECONCILE lists every page of the PR's reviews. It looks for one with the revision's marker, the head commit and our author.
   - If one is found, the revision completes with that ID.
   - A timed-out write can still land, so RECONCILE first waits `github.timeout_ms`. It claims the review absent only after two complete listings at least that far apart. Then PUBLISH tries again, up to `max_retries_per_phase` more times.
   - If the listing fails, the outbox stays `needs_reconciliation` and the revision FAILS with `cannot_safely_decide`. Then run `reconcile --revision <id>` when GitHub is reachable. If that proves the review absent, `review <url> --publish --retry` posts it.

Publishing is part of the policy version (`1+publish`), so an earlier dry run of a head does not block publishing it. A 429 or a rate-limit 403 is retried after `Retry-After` or the reset time. GitHub answers these before it processes the request, so a retry cannot post a second review. Retries and the per-installation concurrency cap are in the `github` config section.

## Service

`serve` runs the webhook receiver, the worker loop and the reviews in one process. Configure it with environment variables, listed in `.env.example`:

- `GITHUB_WEBHOOK_SECRET` is required.
- `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_FILE` (or `GITHUB_APP_PRIVATE_KEY`) select GitHub App auth. Without them, the service uses token auth.
- `PR_REVIEW_ADMIN_TOKEN` guards the admin endpoints.
- `PR_REVIEW_MODEL` and the provider's key select the review model.
- `PORT` sets the listening port.
- `PR_REVIEW_CONFIG` points at the config file. The default `publish_mode` is `dry_run`, so set `{"review": {"publish_mode": "comment"}}` there to post reviews.

The service uses Jev only with `TYPESAFE_API_KEY`, or with `jev.allow_keyless_for_private`. One engine serves every repository, so the keyless tier would otherwise receive private diffs.

| Endpoint | Auth | |
|---|---|---|
| `POST /webhooks/github` | `X-Hub-Signature-256` | Verified over the raw body, stored once per delivery ID, answered 202 (200 for a redelivery) |
| `GET /healthz` | none | Process is up |
| `GET /readyz` | admin bearer | DB, worker loop and data dir |
| `GET /reviews/:revisionId` | admin bearer | State, transitions, findings, outbox and its events |
| `POST /reviews/:revisionId/retry` | admin bearer | Audited MANUAL_RETRY of a FAILED revision |
| `GET /webhooks/dead` | admin bearer | Deliveries that failed every attempt, with the last error |

The worker handles `pull_request` events (opened, reopened, synchronize, closed, review_requested). It also handles an `issue_comment` that mentions `@<app-slug> review` on a PR. It ignores bot senders and its own comments. For each event, it reads the PR from GitHub, and GitHub's state decides open, closed and merged, whatever order the deliveries arrive in. The review of the live head starts `debounce_seconds` after the last event. A new head marks older revisions `superseded_requested` and aborts their tasks, unless they are already publishing. Closing or merging the PR does the same to every in-flight revision. A per-PR lease in `app.sqlite` serializes this. The lease owner is stored once per data dir (in the `meta` table), so a restarted process takes back the leases it left at once. Any other owner's lease expires after 5 minutes.

A delivery whose handling fails waits `webhook_backoff_ms`, doubled after each failure, and only due deliveries are fetched, so a failing one never holds up fresh ones. After `webhook_max_attempts` (6) it is recorded dead with its reason. A PR that GitHub answers 401, 403 or 404 for is dead at once. Aborts run beside the worker loop, and its GitHub reads give up on a rate-limit wait longer than `github.max_rate_limit_sleep_ms` (30 s), so the delivery backs off instead of stalling every PR.

On startup, the service resumes durable tasks and recreates a task for any unfinished revision. It also reconciles outboxes of finished revisions whose write may have reached GitHub, before anything new is posted.

**Deploy.** The `Dockerfile` runs `serve` as the non-root `node` user, with git installed and state on the `/data` volume. Register the app from `github-app.yml`, which lists the permissions and events. Then install the app on a repository and point its webhook URL at `/webhooks/github`. For a local test, a forwarding tunnel such as smee.io or `cloudflared` works.

## Layout

| Directory | Contents |
|---|---|
| `src/domain` | Pure transition tables for the review, outbox, coordinator, candidate and finding machines |
| `src/persistence` | The app SQLite store. Every revision transition is audited in the same transaction |
| `src/source` | Git reads pinned by SHA (`git show`, `grep`, `log`, `diff`). Nothing reads a working tree |
| `src/github` | The GitHub client (token or App auth, rate limits, one response classifier) and the PR reader |
| `src/publish` | The review payload, the POST and reconciliation against the PR's reviews |
| `src/coordinator` | Webhook routing, the PR coordinator machine, debounce, leases and supersede requests |
| `src/server` | The HTTP endpoints and the service that wires them to the worker and recovery |
| `src/jev` | The Jev adapter and the deterministic routing policy |
| `src/reasoning` | Read-only model tools, the candidate schema, the request text and deterministic verification |
| `src/runtime` | The `pr-review.revision` durable task and how it is wired |
| `src/models` | The credential store over pi's auth file, harness and model detection, and model choice |
| `src/cli` | The command-line interface and the report rendering |

## What M2 does not do

- It runs no tests in a sandbox. Verification is static. There is no second verification pass by an LLM.
- It does not re-review across revisions, classify findings as resolved, or reply to review threads. That is M3.
- It has no evals.
- A review started from the CLI is keyed by installation 0, and one started by the App by its installation ID. So the same PR reviewed both ways has two PR records.
