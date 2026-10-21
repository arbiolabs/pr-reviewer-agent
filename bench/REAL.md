# Real-PR eval

The real-PR eval runs the dry-run review over open pull requests of a real repository and reports how the findings compare with the human review comments, how stable they are from run to run, how Jev routed each review, what the verifier discarded, and what the reviews cost. Graded findings give a precision number.

Unlike the [benchmark](README.md), the eval has no seeded bugs. Human comments are weak labels, and only a person who grades the findings can say which ones are real.

## Rules

- **Dry run only.** The runner writes a config that pins `review.publish_mode` to `dry_run`, points `PR_REVIEW_CONFIG` at it, and refuses to start a review when the mode is anything else or the review's arguments hold `--publish`. Reviews read the pull from the snapshot and the code from the snapshot's clone. The GitHub client they get points at a closed local port, so no request reaches GitHub.
- **The snapshot uses GET requests only.** Every GitHub call goes through `gh api -X GET`.
- **The data stays local.** A snapshot of a private repository holds its code, its PR text and its review comments. Keep the dataset under `.data/`, which git ignores. The snapshot refuses to write to a directory that git does not ignore. Never commit a dataset, a results file, a grading sheet, or report output.
- **Jev needs a key for a private repository.** With `--jev`, the runner refuses to start without `TYPESAFE_API_KEY`, because a private diff never goes to the keyless tier. Pass the key only in the environment of the command.

## Snapshot

```sh
pnpm eval:snapshot -- --repo owner/name --dataset .data/evals/name [--pr N]... [--limit N]
```

The snapshot lists the open pulls, or reads the ones `--pr` names, and keeps a pull unless it is a draft, has a bot author, has a title that starts with `Flush:`, changes no files, changes more than 60 files, or changes more than 3000 lines. `--limit N` stops after N kept pulls.

For each kept pull, `prs/<N>/` holds:

- `pull.json`, the pull's title, body, author, base and head SHAs and refs, size, and visibility. The runner builds the reviewer's pull from it.
- `comments.json`, the inline review comments: path, line, side, original line, commit, original commit, reply target, author login and type, and body.
- `reviews.json`, the reviews: author, state, commit, time, and body.

`repo.git` is a full bare clone with `refs/pull/<N>/head` pinned to each kept pull's snapshot head, so a review needs no network. `manifest.json` lists the kept pulls with their SHAs and sizes, the snapshot time, and the reason for each skipped pull.

A rerun skips every pull that already has a `pull.json` and keeps its snapshot, even when the pull has new commits.

## Run

```sh
TYPESAFE_API_KEY=... pnpm eval:real -- --dataset .data/evals/name --model provider/id [--runs N] [--pr N]... [--jev | --no-jev] [--out results.json] [--concurrency N]
```

Each review runs `pr-review review --dry-run --json` in process, with its own data directory under `work/` in the dataset. `--out` is resolved against the dataset directory and defaults to `results.json`. The runner writes the file after every review, and a rerun with the same `--out` skips each review that already completed. `--concurrency N` runs N reviews at a time.

For each review, the results file holds the state, any error, the duration in seconds, the review mode and its reasons, Jev's judgment (category, risk, needs-deep-review probability and confidence), the findings (file, line, severity, category, title, explanation), and the discarded candidates with the verifier's reasons.

## Report and grade

```sh
pnpm eval:report -- --results .data/evals/name/results.json [--labels .data/evals/name/labels.json]
```

The report prints one row per pull and these sections:

- **Human-comment overlap.** A finding agrees with a human comment when the path is the same and the lines are at most 3 apart. Only top-level comments by users that sit on the snapshot head's right side count. The report says how many comments it left out, and why: an older commit, a reply, a bot, or no head-side line. Humans also comment on style and ask questions, so low overlap does not mean low precision.
- **Run-to-run stability.** Two runs' findings match one to one by path, category, and a line at most 3 apart. The report gives the Jaccard index per pull and the mean. A pull whose runs found nothing gets no score.
- **Jev routing.** The mode distribution, the mode Jev suggested, and whether Jev, a path rule, both or neither raised the depth.
- **Verifier discards.** The count and a histogram of the reasons.
- **Cost proxies.** Seconds per review, and findings per 1000 changed lines.

`--labels FILE` writes a grading sheet with one entry per unique finding: the pull, path, line, category, severity, title, explanation, the runs that reported it, a reference without a link, and `verdict: null`. Set each verdict to `real`, `not_real` or `unsure`, then run the report again. The rerun keeps every verdict whose finding is still in the results, and reports the precision over the findings graded `real` or `not_real`, and how the real findings agree with the human comments.

The sheet also lists each unique candidate the verifier discarded, with `kind: "discarded"` and its `discardReason`. Reported findings have `kind: "reported"`. The report reads the discarded candidates from each review's own store under `work/`, because `results.json` keeps only the reasons. Grade them the same way: the report then prints the share of graded discards that were real, which is what the verifier's strictness costs. A discarded entry's id starts with `discarded:`, so it never collides with a reported finding.

**Another AI reviewer.** Some PRs carry reviews from [vikrrantshah/pr-review-agent](https://github.com/vikrrantshah/pr-review-agent), which posts from a person's account, so its comments look human. The report sets its inline comments (written `Critical: ...` or `Important: ...`) apart from the human ones. It also rebuilds that agent's findings on the snapshot head, from its inline comments and the findings its reviews list in the body. A section compares the two reviewers one to one, by path and line. The sheet lists its findings with `kind: "agent"`, and grading them gives its precision next to pr-review's. Its findings are unverified model output, so agreement with it is not correctness.

## Limits

- **Human comments are weak labels.** They cover what a reviewer chose to mention, on the lines they chose. A defect that no human commented on is not a false positive, and a comment about naming is not a missed defect.
- **The match is by line.** As in the benchmark, the rule does not read the text. Two runs that describe one defect at lines 5 lines apart count as two findings.
- **Older comments drop out.** A comment that GitHub no longer places on the head commit has no head-side line to compare, so pulls with long histories have fewer usable comments.
- **Old pulls are reviewed as they stand.** A pull that has been open for a long time is reviewed against its own base, not the current default branch.
- **One grader.** A precision number from one person's verdicts carries that person's judgment.
