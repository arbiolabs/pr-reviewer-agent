# Benchmark

The benchmark measures how well pr-review finds real bugs and how often it reports something that is not one. It runs the real review pipeline over a set of labeled PRs and scores precision and recall.

Each case is a small PR in TypeScript, Go or Python. Most cases have one seeded bug. The rest are known-clean changes, where any finding is a false positive.

The benchmark calls a real model, so every run costs money. It never runs in CI, and vitest does not load `bench/`. Only the scoring function has a unit test, `tests/bench-score.test.ts`.

## Run it

Run every case once:

```sh
pnpm bench -- --model anthropic/claude-opus-5-5
```

The options are:

- `--model provider/id` names the model. `pr-review models` lists the ones you can use. This option is required.
- `--case id` runs one case. Repeat it to run several.
- `--runs N` runs every case N times. Model output varies from run to run, so use `--runs 3` or more before you compare two numbers.
- `--json out.json` writes the results to a file. The file holds the model, the commit SHA of pr-review, the date, the line tolerance, every case's findings and matches for each run, and the totals.

Compare two results files:

```sh
pnpm bench:compare a.json b.json
```

Each case runs `pr-review review --dry-run --json --no-jev` against a local repository, with a fresh temporary data directory. Nothing is published. Jev is skipped so that no case calls a third-party service and the review mode depends only on the path rules. A case that touches a path such as `migrations/` or `auth` still gets a deeper review.

The table has one row per case. With several runs, each cell lists one value per run, separated by commas:

- `TP` counts labels that a finding matched.
- `FN` counts labels that no finding matched.
- `FP` counts findings that matched no label.
- `discarded` counts candidates that the verifier rejected. Below the table, each rejected candidate is listed with the reason that the verifier recorded. These are the same dispositions that `pr-review inspect` shows.
- `state` is the final state of the review. Any state other than `COMPLETED` is an error, and its labels count as false negatives.

Below the table, the runner lists every false positive and every match whose severity or category differs from the label.

## Add a case

A case is a directory under `bench/cases/<id>/`:

- `base/` holds the repository before the PR.
- `head/` holds only the files that the PR adds or changes. The loader copies it over `base/` to make the head commit. A case cannot delete a file.
- `case.json` holds the PR's `title` and `body`, and the `labels`.

Each label describes one expected finding:

```json
{
  "file": "src/routes/users.ts",
  "line_range": [15, 27],
  "severity_at_least": "high",
  "category": "security",
  "note": "PUT /users/:id/role is registered with requireUser only, so any member can make themselves admin."
}
```

- `file` is the path in the head commit.
- `line_range` is the first and last line in the head file where a finding about this bug can sit. Include the lines a reviewer could reasonably point at, such as a handler and the line that registers it.
- `severity_at_least` is one of `critical`, `high`, `medium` and `low`. `category` is one of the categories in `src/domain/types.ts`. Both are reported, not scored.
- `note` is the ground truth. Write what the bug is and why it is wrong, so the next person can judge a disputed finding.

An empty `labels` list marks a known-clean change.

The reviewer sees only the diff and the repository at the head commit. Make the bug visible from those, and keep the rest of the change free of other real problems. A real problem that you did not label counts as a false positive.

## How findings match labels

A finding matches a label when the file is the same and the finding's line is inside `line_range`, widened by `LINE_TOLERANCE` (3 lines) on each side. Each label takes the nearest unmatched finding, so a label matches at most one finding, and a finding matches at most one label. Severity and category do not need to match.

Precision is TP / (TP + FP). Recall is TP / (TP + FN). A run with no findings has no precision, and the table shows `n/a`.

## Limits

- **Matching is static and by line.** The rule does not read the finding's text. A finding on the right line for the wrong reason counts as a true positive. A correct finding that points at a different line, such as a call site, counts as a false positive, and the label counts as a false negative.
- **One label is one finding.** A second valid finding about the same bug, or a real problem that the case did not label, counts as a false positive. Read the false-positive list before you trust the precision number.
- **We wrote the labels.** The cases and their ground truth come from the same people who build the reviewer. The seeded bugs are textbook examples, and real PRs are larger and less clear.
- **N is small.** With 8 labels, one finding moves recall by 12.5 points. Treat a difference of one or two findings as noise unless it holds across several runs.
