# Running the service

`pr-review serve` runs the webhook receiver, the worker loop, and the reviews in one process. This page covers setting it up as a GitHub App, its settings, its endpoints, and how it handles events.

## Set up the GitHub App

1. Register a GitHub App from `github-app.yml`. The manifest lists the permissions (metadata read, contents read, pull requests write, issues read) and the events (`pull_request`, `issue_comment`). Generate a private key and set a webhook secret.
2. Install the app on the repositories you want reviewed.
3. Copy `.env.example` to `.env` and fill it in. See [Environment](#environment).
4. Create a config file that turns publishing on, and point `PR_REVIEW_CONFIG` at it:

	```json
	{ "review": { "publish_mode": "comment" } }
	```

	The default `publish_mode` is `dry_run`. In dry-run mode the service records reviews and posts nothing.
5. Start the service:

	```bash
	node bin/pr-review.mjs serve
	```

6. Point the app's webhook URL at `/webhooks/github`. For a local test, forward GitHub's deliveries with a tunnel such as smee.io or `cloudflared`.

To run it in a container, build the `Dockerfile`. The image runs `serve` as the non-root `node` user, with git installed and state on the `/data` volume.

The receiver accepts request bodies up to 25 MB, GitHub's maximum payload. On a public address, put a reverse proxy with connection and rate limits in front of it.

## Environment

| Variable | Required | Purpose |
|---|---|---|
| `GITHUB_WEBHOOK_SECRET` | yes | The webhook secret from the app settings |
| `GITHUB_APP_ID` | for App auth | The app's ID. Without it, the service uses token auth from `GITHUB_TOKEN`. |
| `GITHUB_APP_PRIVATE_KEY_FILE` or `GITHUB_APP_PRIVATE_KEY` | for App auth | The app's private key |
| `PR_REVIEW_BOT_LOGIN` | with an installation token | The app's bot login, so reconciliation can match its reviews |
| `PR_REVIEW_ADMIN_TOKEN` | for admin endpoints | Bearer token for the admin endpoints. Without it, they always answer 401. |
| `PR_REVIEW_MODEL` | yes | The review model, as `provider/id` |
| `ANTHROPIC_API_KEY` | for Anthropic models | The model provider's credential |
| `TYPESAFE_API_KEY` | no | Jev access. Without it, the service skips Jev. |
| `PR_REVIEW_CONFIG` | to publish | Path to the config file |
| `PORT` | no | The listening port |
| `PR_REVIEW_DATA_DIR` | no | The data directory |

The service uses Jev only with `TYPESAFE_API_KEY`, or with `jev.allow_keyless_for_private`. One service reviews every installed repository, and the keyless tier would otherwise receive private diffs.

## Endpoints

| Endpoint | Auth | Answer |
|---|---|---|
| `POST /webhooks/github` | `X-Hub-Signature-256` | 202 for a new delivery, 200 for a redelivery, 401 for a bad signature |
| `GET /healthz` | none | 200 while the process is up |
| `GET /readyz` | admin bearer | The state of the database, the worker loop, and the data directory |
| `GET /reviews/:revisionId` | admin bearer | The revision's state, transitions, findings, and outbox |
| `POST /reviews/:revisionId/retry` | admin bearer | 202 after an audited retry of a failed revision. 409 for any other state. |
| `GET /webhooks/dead` | admin bearer | Deliveries that failed every attempt, with the last error |

The receiver checks the signature over the raw body before it parses JSON. It stores each delivery ID once.

## Events

The worker handles `pull_request` events (opened, reopened, synchronize, closed, and review_requested). It also handles an `issue_comment` on a PR that mentions `@<app-slug> review`.

- **Live state decides.** For each event, the worker reads the PR from GitHub. GitHub's answer decides whether the PR is open, closed, or merged, whatever order the deliveries arrive in.
- **Latest head wins.** The review of the current head starts `debounce_seconds` after the last event. A new head supersedes older revisions and aborts their tasks, unless they are already publishing. Closing or merging the PR does the same.
- **No reply loops.** The worker ignores bot senders and its own comments. A mention or review request for a commit that already has a final review starts nothing. Its outcome is recorded as `already_reviewed:<state>`.
- **Failing deliveries back off.** A delivery that fails waits `webhook_backoff_ms`, doubled after each failure. After `webhook_max_attempts` (6) it is recorded as dead. A PR that GitHub answers with 401, 403, or 404 is dead at once.
- **Leases.** A per-PR lease in `app.sqlite` serializes the work on each PR. A restarted service takes back its own leases at once. Another owner's lease expires after 5 minutes.

On startup, the service resumes unfinished durable tasks, and recreates a task for any revision that has none. Before anything new is posted, it reconciles every outbox whose write may have reached GitHub.

## Known limits

A review started from the CLI is stored under installation 0, and one started by the app under its installation ID. A PR reviewed both ways therefore has two PR records.
