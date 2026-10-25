# Publishing

GitHub's API has no exactly-once write. A request can time out after GitHub already stored the review, and a process can die between the POST and saving the answer. If pr-review retried every failed POST, some PRs would get the same review twice. This page explains how pr-review avoids that, and what to do when it cannot decide.

## One review per revision

A revision is one PR at one base and head commit, reviewed with one policy and one model. pr-review posts at most one review per revision. Every review body carries a hidden marker with the revision ID:

```text
<!-- durable-review:<revision-id>:<payload-hash> -->
```

The marker lets pr-review find its own earlier review on GitHub, even when its local database is gone. A rerun from an empty data directory finds the review and adopts its ID instead of posting a second one.

## The publication steps

1. **Prepare.** pr-review re-reads the PR head and stores the review payload in the outbox, in the same database write that moves the revision to publishing. The payload holds the summary, the marker, `commit_id` set to the head, `event: COMMENT`, and an inline comment for each finding whose line is in the diff and that no earlier review commented on. A finding carried from an earlier review is listed in the summary instead, as still present, possibly fixed, outdated, uncertain, or resolved. The summary is the only place resolution is reported: the bot never replies in a review thread and never resolves one on GitHub. The outbox records which findings the payload comments on, and once the review is posted each of them records that review's ID.
2. **Check.** pr-review re-reads the PR. If the head moved, or the PR is closed or merged, the revision is superseded and nothing is posted. It then lists every page of the PR's reviews. If one of its own reviews on the head commit carries this revision's marker, the revision completes with that review's ID.
3. **Post.** pr-review marks the outbox `sending` and commits that before the POST. It saves the returned review ID after the POST.
4. **Reconcile.** If the result is uncertain, pr-review lists the PR's reviews again instead of posting again. Uncertain results are a 5xx, a dropped connection, a timeout, and a crash that left the outbox in `sending`.

During reconciliation:

- If pr-review finds the review, the revision completes with that ID.
- A timed-out write can still land later. pr-review first waits `github.timeout_ms`, then needs two complete listings, at least that far apart, that both lack the review. Only then does it post again, up to `max_retries_per_phase` times.
- If a listing fails, the outbox stays `needs_reconciliation`, the revision fails with `cannot_safely_decide`, and nothing is posted.

The same caution applies to the inline comments of a later push. A finding whose outbox is `sending` or `needs_reconciliation` may already be on GitHub, so the next revision lists it as carried and does not comment on it, until `reconcile` decides: found marks it posted, and absent leaves its outbox `pending` on the failed revision, after which the next revision comments on it once. A `pending` outbox of a revision that failed, or of a superseded one, was never sent, so its findings are not posted and the next revision comments on them.

pr-review retries a 429 or a rate-limit 403 after `Retry-After` or the rate-limit reset. GitHub sends those answers before it processes the request, so the retry cannot create a second review.

## Recover an uncertain publication

If a revision failed with `cannot_safely_decide`:

1. When GitHub is reachable again, decide the outbox:

	```bash
	node bin/pr-review.mjs reconcile --revision <revision-id>
	```

	`reconcile` lists the PR's reviews and never posts.
2. If `reconcile` finds the review, you are done.
3. If `reconcile` proves the review absent, post it:

	```bash
	node bin/pr-review.mjs review <pr-url> --publish --retry
	```

`--retry` reopens a failed revision. The audit log records it as `FAILED --MANUAL_RETRY--> COLLECT_CONTEXT`. pr-review refuses `--retry` for a completed or superseded revision.

`--again` reviews the current head from scratch as a new revision, whatever the stored result was: completed, failed, superseded or parked for approval. The earlier revision is recorded as `REVIEW_AGAIN --> SUPERSEDED`, and a parked payload is dropped unsent. pr-review refuses `--again` while a review of the PR runs, and while a publication of that head may have reached GitHub: reconcile it first.

## Approval mode

With `"review": { "publish_mode": "approval" }`, pr-review prepares the review but does not post it. The run stops in the state `AWAITING_APPROVAL` with the payload stored in the outbox, and the CLI prints the approve command:

```bash
node bin/pr-review.mjs approve --revision <revision-id> --preview <payload-hash>
```

`--preview` is the hash of the stored payload. It makes the approval name exactly the text that is posted: a payload with another hash is refused. The terminal client shows the same payload, with its inline comments and the target commit, and sends the same approval through the daemon.

An approval re-reads the PR first. If the head moved or the PR closed, the run is superseded and nothing is posted. Otherwise the approval is recorded in the audit log as `AWAITING_APPROVAL --APPROVED--> PUBLISH`, and the publication steps above run unchanged: the listing before the first post, `sending` committed before the post, and reconciliation on an uncertain result. A repeated approval, or an `approve` run again after a crash, resumes that publication instead of posting again.

While a run awaits approval:

- A new head on the PR supersedes it. The stored payload is abandoned, because nothing was sent.
- A cancel from the client fails it with the reason `cancelled from the TUI`. `review --retry` reopens it as a new attempt, which awaits approval again.
- `pr-review review` on the same head shows the stored result instead of starting over.

Approval mode is for the daemon and the CLI. Under `pr-review serve`, runs also park in `AWAITING_APPROVAL`, but the daemon cannot take the data directory lock while `serve` runs, so approve them with `pr-review approve` once `serve` has stopped.

## Match reviews by author

Reconciliation also checks that the review's author is pr-review itself. With a personal token, pr-review reads its login from `GET /user`. A GitHub App installation token (`ghs_...`) cannot read `/user`. With such a token, set `github.bot_login` in the config, or `PR_REVIEW_BOT_LOGIN`, to the app's bot login, such as `my-app[bot]`. Without it, reconciliation cannot decide and leaves the outbox `needs_reconciliation`.

## Dry runs and publishing

Publishing is part of the policy version (`1+publish`). An earlier dry run of a commit therefore does not block publishing that commit later.
