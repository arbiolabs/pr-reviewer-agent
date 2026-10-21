# Design: tracking findings across commits

Status: proposal. Nothing here is built. Base: `main` at `f328e79`.

The README says each commit gets a fresh review, and that tracking a finding across commits, marking it resolved, and replying in review threads are not built (`README.md:233`). This document designs that tracking. It covers how a finding on commit N is matched on commit N+1, who decides its new status, what appears on GitHub, the store, protocol and benchmark changes, and a build plan of small PRs.

Every claim about current behavior cites `file:line`. A claim marked *inferred* comes from reading code, not from running it. A claim marked *GitHub docs* comes from GitHub's API documentation and needs a check against the live API before the PR that relies on it.

## Recommendation

1. Identity is content, not line numbers. Each observation stores the normalized text of the head-side evidence it cites. On a new head, code searches for that text, first in the same file, then in the renamed file, then once across the tree. Lines that move and files that are renamed keep the finding. Rewritten code does not, so code marks that finding as doubted.
2. A new candidate joins an earlier finding when it names the finding (`priorFindingId`) and code confirms that the locations agree, or when its fingerprint or evidence text matches. Otherwise it is a new finding.
3. Code decides the observation, inside VERIFY. A finding re-reported and verified again is `STILL_PRESENT`. Evidence found unchanged is `STILL_PRESENT`. Evidence that is gone from a file that still exists is `POTENTIALLY_RESOLVED`. A deleted file is `OUTDATED`.
4. Only a doubted finding can reach `RESOLVED`. The model must submit a resolution claim with head-side evidence, and code checks it the way VERIFY checks a candidate. A changed line alone never resolves a finding, as `src/domain/machines.ts:194` requires.
5. On GitHub, nothing changes in the write path. Each revision still posts exactly one `COMMENT` review through the outbox. Inline comments go only to findings never posted before. The review body lists the carried findings as still present, possibly fixed, or resolved. This needs no new permission and no new GitHub write.
6. Thread replies and GraphQL thread resolution stay out of the default build. Each is a separately gated PR for the operator to decide on.
7. The protocol needs no breaking change. All seven finding statuses already exist in protocol v1 and the TUI (`src/protocol/v1.ts:75`, `tui/src/theme.rs:304-315`). History entries gain one optional field.
8. The benchmark gains multi-commit cases. They score the false-resolution rate first, then identity kept, the duplicate-comment rate, and resolution recall.

The first PR adds the multi-commit benchmark and records today's duplicate-comment rate. The second removes the duplicate inline comments with about 40 lines of code, with no migration and no prompt change.

## How it works today

### A new head creates a new revision

- Under `serve`, a webhook delivery goes through `Coordinator.handle` (`src/coordinator/coordinator.ts:121-140`). It reads the live PR, debounces, and calls `supersedeOlder` for every unfinished revision with another head (`coordinator.ts:162-177`). A parked revision moves to `SUPERSEDED` on the spot (`coordinator.ts:165-167`). A revision before `PUBLISH` is aborted (`coordinator.ts:67`, `coordinator.ts:170-175`). Once the debounce ends, `tick` calls `start` (`coordinator.ts:109-118`), which calls `startReview` (`src/server/service.ts:67-69`).
- The CLI `review` command calls `startReview` directly (`src/cli/main.ts:143`).
- `startReview` derives the revision from `(pr, base, head, policy, reviewer)` (`src/runtime/app.ts:126-138`, `src/persistence/store.ts:184-193`). A new head is a new revision row in `COLLECT_CONTEXT`.
- The durable task runs collect, triage, analyze, verify, prepare, publish and reconcile (`src/runtime/task.ts:381-386`). Each phase re-checks the live head and moves to `SUPERSEDED` if the head moved (`task.ts:112-115`, `task.ts:231-235`, `task.ts:288-291`). The abort handler does the same (`task.ts:388-403`).

### Findings are stored per PR, identified by a fingerprint

- The model submits candidates through `submit_candidates` (`src/reasoning/tools.ts:131-148`). The schema has no field that refers to an earlier finding (`src/reasoning/schema.ts:7-30`).
- VERIFY is deterministic (`src/reasoning/verify.ts:65-105`). The fingerprint is a hash of category, file, and the sorted title words (`verify.ts:25-28`). A comment there calls it a crude stand-in, because rewording or moving a defect changes it (`verify.ts:21-22`).
- The finding ID is `stableId("finding", prId, fingerprint)` (`verify.ts:100`). It is scoped to the PR, not the revision. The same fingerprint on a later head therefore reuses the same `findings` row.
- `writeVerification` upserts the finding with status `DETECTED` and only updates the severity on conflict (`store.ts:506-513`). It inserts one `finding_observations` row per finding and revision with `disposition = 'VERIFIED'` (`store.ts:514-519`, unique key at `migrations/001_init.sql:48`).
- `markFindingsPublished` moves only `DETECTED` findings of the revision to `PUBLISHED` (`store.ts:343-350`). No code applies any other `FINDING_MACHINE` event. A search for `OBSERVED_` and `STILL_PRESENT` outside `machines.ts` finds only protocol schemas and the TUI.
- `findings.github_thread_id` exists (`migrations/001_init.sql:35`) and no code writes it.
- The model can call `previous_findings` (`tools.ts:119-130`), which returns ID, status, severity, title and file, with no evidence and no location (`store.ts:539-552`). The prompt says to use it "to avoid repeating a known issue" (`prompts/reviewer.md:11`). Nothing checks whether the model did.

### Measured: a later head re-posts and duplicates

I ran a throwaway vitest probe against the real `AppStore` and `buildPayload`. It wrote one finding on revision 1 and marked it published. On revision 2 it wrote the same finding moved to another line, plus the same defect with a reworded title. The probe printed:

```text
r2 findings [{"id":"c3203d20","status":"PUBLISHED","line":14},{"id":"d11bbcfc","status":"DETECTED","line":14}]
comments on r2: 2
```

So today a second push re-posts the moved finding as a new inline comment, and posts the reworded one as a third comment. The status stays `PUBLISHED` and never reflects the new head. A finding that the model does not report again gets no observation and keeps its old status forever (*inferred* from `store.ts:506-520`, which only writes the verified set).

### The outbox posts one review per revision

- PREPARE builds the payload and inserts the outbox row in the same transaction that leaves `PREPARE_PUBLICATION` (`task.ts:236-242`). The idempotency key is `revisionId:PUBLISHER_VERSION` (`task.ts:241`, `src/publish/publisher.ts:8`).
- The payload has one inline comment per verified finding whose head line is in a diff hunk (`publisher.ts:33-40`). The body carries the marker `<!-- durable-review:<revision>:<hash> -->` (`publisher.ts:15`, `publisher.ts:43`).
- PUBLISH lists the PR's reviews before any POST (`task.ts:294-302`), commits `sending` before the POST (`task.ts:311`), and sends anything uncertain to RECONCILE, which never posts (`task.ts:347-379`). `docs/publishing.md:15-28` describes the same steps.

## Constraints

- **Code verifies, the model proposes.** VERIFY passes a candidate on anchoring, evidence and policy alone (`verify.ts:61-64`). Tracking must keep that rule for every status change, and above all for `RESOLVED`.
- **One review per revision.** SPEC invariant 3 (`docs/SPEC.md:43`) and the outbox design (`docs/publishing.md:5-13`). Any extra GitHub write needs its own outbox row and reconciliation.
- **A changed line alone never resolves** (`machines.ts:194`, `docs/SPEC.md:148`).
- **Thread APIs must not be invented.** SPEC says the MVP can update finding state in its own database and publish a follow-up summary instead of resolving GitHub threads (`docs/SPEC.md:409`). It lists thread resolution as an open decision (`docs/SPEC.md:644`).
- **M3 acceptance.** A fixture with "bug introduced, bot reports, partial fix, complete fix" must produce the correct status progression and no repeated duplicate comment (`docs/SPEC.md:555`).

## Designs considered

I worked out designs A and B. An independent runner on a second model, given the same grounding, rejected both and proposed C. The recommendation, R, combines C's identity with A's resolution check.

**A. Line mapping plus a targeted model re-check.** Code diffs the old head against the new head and maps each open finding's anchor and evidence ranges through the hunks. Each finding gets a class, either untouched, touched or gone. The request lists only the touched and gone findings, and the model returns a structured verdict for each. Code then checks each verdict.

**B. The model re-adjudicates everything.** The request includes every open finding with its evidence. `submit_candidates` must return a verdict per finding (present, resolved or unsure) next to the new candidates. Code checks only the verdict's structure.

**C. Content-addressed carry-over, resolution from re-review behavior.** Each observation stores a hash of the cited evidence text. Code searches for that text at the new head, which handles line moves and renames without hunk arithmetic and without the old commit. If the text is gone, the finding is doubted. A doubted finding becomes `RESOLVED` when the model read that file at head during the run and did not report the finding again.

**R (recommended). C's identity with A's resolution check.** The identity and locating step comes from C. Resolution needs an explicit claim from the model with head-side evidence that code checks, as in A. Reading a file is not enough.

| | A: line mapping and re-check | B: model re-adjudicates | C: content address, inferred resolution | R: C identity, checked resolution |
|---|---|---|---|---|
| Correctness | Good for moves inside a file. Renames need `-M` plumbing. A force-push can lose the old commit's objects, so mapping fails (*inferred* from the `blob:none` fetch at `src/source/git.ts:79`). | Status is a model opinion. Code can check structure but not truth. SPEC forbids resolution by prediction (`docs/SPEC.md:148`). | Locating is robust to moves, renames and force-pushes. Resolution is weak, because "read the file and stayed silent" is not evidence. | Locating as in C. Resolution as strong as VERIFY's bar for new findings. |
| GitHub spam risk | Low, with summary-only output. | High. Verdicts can flip between pushes, and each flip changes the summary. A dropped finding has no defined status. | Low, with summary-only output. | Low, with summary-only output. |
| Cost per review | A local `git diff`, plus prompt text for doubted findings only. | Prompt and tool calls grow with the number of open findings, every push. | Local searches plus a short list in the prompt. | Same as C, plus a small schema field. |
| Complexity | Hunk-mapping code and a verdict protocol. | Smallest code, largest prompt and eval surface. | Smallest. | C plus one checked claim type, about the size of `structuralProblem` (`verify.ts:31-51`). |
| How it fails | A missing old commit sends every carried finding to `OUTDATED`. A fix outside the cited lines leaves the finding `STILL_PRESENT`. | False `RESOLVED` from anchoring or skimming, plus status flapping. | False `RESOLVED` after a skim. That is the worst failure, because `RESOLVED` is terminal (`machines.ts:202`). | A fix outside the cited code leaves the finding `STILL_PRESENT` until the model files a resolution claim. Copied code elsewhere can relocate a finding wrongly. Both errors stay open, never falsely resolved. |

R fails toward "still open", which costs a stale summary line. The alternatives fail toward "resolved", which hides a live defect. That asymmetry decides it.

## The recommended design

### Data shapes

```ts
// The text a finding points at, enough to find it again without the old commit.
type EvidenceText = { path: string; start: number; end: number; text: string }; // text normalized: trimmed lines, collapsed whitespace

// location_json of an observation (column exists: migrations/001_init.sql:46).
type ObservedLocation = { file: string; newLine: number | null; headSha: string; evidence: EvidenceText[] };

// What locating one open finding at the new head found.
type Located =
  | { kind: "same"; location: ObservedLocation }                 // all head-side evidence text found, maybe moved or renamed
  | { kind: "changed"; file: string }                            // the file exists, some evidence text is gone
  | { kind: "file_gone" };                                       // the file and every rename candidate are absent

// A model claim, checked by code before it counts.
type ResolutionClaim = { findingId: string; codeEvidence: CodeEvidence[]; reason: string; evidenceKind: EvidenceKind };

// One observation per open finding per revision; disposition is the finding state it ended in.
type Observation = { findingId: string; path: FindingState[]; basis: "reverified" | "located" | "changed" | "file_gone" | "resolution_checked" | "resolution_refused" };
```

### Identity

The finding ID stays the key across revisions. New findings get `stableId("finding", prId, firstRevisionId, fingerprint)` in place of `stableId("finding", prId, fingerprint)` (`verify.ts:100`). Identity then comes from explicit linking, not from the ID's inputs. A defect that comes back after `RESOLVED` therefore gets a new finding instead of colliding with the terminal one. Existing IDs stay valid, because they are opaque strings.

Code links a verified candidate to an open finding of the PR inside VERIFY. Every link needs the candidate in the finding's category and in the finding's file at this head, after the rename the diff reports. One exception: when the finding's file is gone and the diff names no rename, which is what `git diff base...head` reports for a file the PR itself added and a later commit renamed, a substantial equal cited text (rule 3) is what says where the code went, so that link alone may cross files. The benchmark's `move` case is this exact shape, and its registration block gains two routes in the same commit, so the tree-wide search for that block misses while the handler body still matches. Within that, from strongest to weakest:

1. The candidate sets `priorFindingId`, a new optional schema field (build step 4, not built yet). Code accepts the link when the candidate's line is within a fixed distance of the located anchor or one of its evidence ranges overlaps the located evidence. Otherwise code treats the candidate as new.
2. The candidate's title words agree with the finding's: the fingerprint compared without the file, since the finding's own fingerprint was built from its first path.
3. A cited text agrees. Both texts must be substantial, at least two lines and 40 characters, and equal. A broad range that merely contains the finding's text does not count, and neither does `}`, an empty line, or one short statement, however equal.
4. The candidate's anchor is within three lines of the located anchor and its cited ranges overlap the finding's located evidence. Two defects on neighbouring lines in one category are told apart by what they cite: a report that cites none of the finding's code is a new finding.

Linking is a one-to-one assignment per revision. The best-scoring pair links first, then the next pair whose candidate and finding are both unclaimed, and so on. A candidate left over is a new finding, never a duplicate of the one that claimed its prior. When unsure, a fresh finding is better than a hidden one: a possible duplicate comment is cheaper than a lost defect. Two candidates that get the same new ID, which means the same title words in one file, are one defect reported twice, and the second is a duplicate, as `verify.ts` always did for fingerprints.

Locating searches the new head for each open finding's stored evidence text. It searches the same path first, then the head path of a rename (`git diff -M` already reports renames, `src/source/git.ts:112`), then the whole tree with the existing `grep` (`git.ts:143`). The tree-wide search runs only when the snippet is substantial and exactly one file holds it, and only for a finding not yet doubted: the head that doubted it already searched the tree, so a defect that comes back elsewhere is a new finding. A snippet below the floor is found only at its old line, never relocated. A finding whose evidence cites only the base SHA uses the anchor line and two lines on each side. None of this needs the old head's objects, so a force-push does not break it.

### Re-observation

VERIFY already owns every deterministic judgment (`task.ts:208-223`), and its writes commit with the `CANDIDATES_DISPOSED` transition (`task.ts:219-221`). Carry-over joins the same transaction. For every finding of the PR that is not `RESOLVED`, code walks `FINDING_MACHINE` (`machines.ts:195-203`) and records the path, as `verify.ts:70-75` does for candidates:

| This revision | Event | Ends in |
|---|---|---|
| A verified candidate links to it | `OBSERVED_PRESENT` | `STILL_PRESENT` |
| Located `same` | `OBSERVED_PRESENT` | `STILL_PRESENT` |
| Located `changed` | `OBSERVED_POSSIBLY_FIXED` | `POTENTIALLY_RESOLVED` |
| Located `file_gone` | `OBSERVED_OUTDATED` | `OUTDATED` |
| Doubted, and a resolution claim passes the check | then `CONFIRMED_RESOLVED` | `RESOLVED` |
| The model claims it present, but no linked candidate passes VERIFY | then `OBSERVED_UNCERTAIN` | `UNCERTAIN` |

The resolution check reuses the structural rules for candidates (`verify.ts:31-51`). Every evidence range cites the head SHA and exists there. The evidence kind is `confirmed_by_static_trace`. At least one range is in the finding's located file or its rename. The cited text differs from every stored evidence text of the finding, which proves the claim points at changed code. No candidate in this revision links to the finding. A claim that fails is refused, and the finding keeps its doubted state.

A claim may also name a `STILL_PRESENT` finding whose fix lies outside the cited lines. The same check applies, and on success the path is `OBSERVED_POSSIBLY_FIXED` then `CONFIRMED_RESOLVED`. This is the only route around the "fix elsewhere" gap, and it still needs code-checked evidence.

The model sees prior findings in the first message, not through an optional tool. `buildReviewRequest` (`src/reasoning/request.ts:14-32`) gains a section listing each open finding's ID, title, located file and line, and whether its cited code changed. It asks the model to report a finding again with `priorFindingId` if it persists, and to file a resolution claim only with evidence. `previous_findings` (`tools.ts:119-130`) stays for detail. The section is trusted text built from the store, but the titles came from an earlier model run, so they are fenced like the PR text (`request.ts:11`, `request.ts:25-30`).

One change to the machine is needed. `DETECTED` today leaves only through `PUBLISHED_TO_GITHUB` (`machines.ts:196`). Dry runs never publish (`task.ts:238`), so a dry-run user would never see tracking. `DETECTED` gets the same three observation events as `PUBLISHED`. Publication becomes a fact recorded in a column (see below), so a carried finding that was never posted can still be posted once.

### What happens on GitHub

Default, with no new write:

- The payload gets an inline comment only for a finding with no recorded GitHub review. That covers findings first seen in this revision, and findings carried from a revision that never published. The filter replaces the current one at `publisher.ts:33-34`.
- The body, built by `renderSummary` (`src/cli/report.ts:47-61`), gets sections for new findings, findings still present (with the commit first reported on), findings possibly fixed or outdated, and findings resolved since the last review.
- `PUBLISHER_VERSION` moves to `p2` because the payload shape changes (`publisher.ts:7-8`). An in-flight revision keeps its stored `p1` outbox, because `ensureOutbox` runs only on the transition out of `PREPARE_PUBLICATION` (`task.ts:230-242`), and `findReview` matches by revision marker under any hash (`publisher.ts:17`, `publisher.ts:82`). *Inferred*, and the PR adds a test for it.

Exactly-once is unchanged, because the write path is unchanged. One race needs a rule. Revision N can be in `PUBLISH` while N+1 runs VERIFY, because the coordinator aborts only states before `PUBLISH` (`coordinator.ts:67`). A finding counts as posted while an outbox may still put it on GitHub: one that is `sending` or `needs_reconciliation`, whatever became of its revision, until `reconcile` decides, or one a live revision holds `pending`. A missed comment is cheaper than a duplicate. A `pending` outbox of a finished revision was never sent, or two complete listings proved it absent, so its findings are not posted and the next revision comments on them. That rule matters because a revision can end `FAILED` with its outbox `pending` (every POST rate-limited) or `needs_reconciliation` (a listing failed before or after the POST), and supersession never abandons a `FAILED` revision. The body still carries the finding in every case, so nothing is hidden by the wait, only its inline comment.

The review body is bounded. Each carried section lists at most 20 findings and then "and N more", and a body that would exceed GitHub's 65,536-character limit with its marker line is cut at a line end, with a notice, so the marker `findReview` matches on stays whole and last.

Gated option, thread replies (operator decision 1). This is a second kind of outbox row, reusing `OUTBOX_MACHINE` (`machines.ts:105-111`):

- After the review posts, code lists the review's comments (*GitHub docs*: `GET /repos/{owner}/{repo}/pulls/{n}/reviews/{review_id}/comments`). It matches each comment by path, line and body hash, and stores the comment ID on the finding. The create-review response returns no comment IDs (*inferred* from `publisher.ts:58`, which reads only `id`).
- A reply row is keyed by `(findingId, revisionId)` and is created in PREPARE with the review payload. Its body carries `<!-- durable-review-reply:<finding>:<revision> -->`. It posts to the reply endpoint (*GitHub docs*: `POST /repos/{owner}/{repo}/pulls/{n}/comments/{comment_id}/replies`). It reconciles by listing the PR's review comments (*GitHub docs*: `GET /repos/{owner}/{repo}/pulls/{n}/comments`) and matching the marker and author, as `findReview` does (`publisher.ts:76-87`).
- Replies post only after the review is `published`. A reply that fails or is abandoned does not fail the revision. The audit log records it.
- Permission: `pull_requests: write`, which the app already has (`github-app.yml:14`).

Gated option, resolve threads (operator decision 2):

- *GitHub docs*: the GraphQL mutation `resolveReviewThread(input: { threadId })`. The thread node ID comes from a `pullRequest.reviewThreads` query, matched by the first comment's `databaseId`. Resolving is a state change, not a new object, so reconciliation reads `isResolved` and needs no marker.
- Permission: `pull_requests: write` for the app. A personal token needs write access to pull requests. Check both against GitHub's docs before the PR, as `docs/SPEC.md:386` and `docs/SPEC.md:409` require.
- Only on `RESOLVED`, never on `POTENTIALLY_RESOLVED` or `OUTDATED`.

### Store and schema

One migration, `055_finding_tracking.sql`. The next number follows `054_pull_title.sql`.

```sql
ALTER TABLE finding_observations ADD COLUMN path_json TEXT;   -- the FINDING_MACHINE states this observation walked
ALTER TABLE finding_observations ADD COLUMN basis TEXT;       -- reverified | located | changed | file_gone | resolution_checked | resolution_refused
ALTER TABLE findings ADD COLUMN github_review_id INTEGER;     -- the review whose inline comment first carried it; NULL = never posted
ALTER TABLE publication_outbox ADD COLUMN inline_finding_ids_json TEXT; -- which findings this payload comments on
```

- `finding_observations.disposition` now holds the finding state after the observation, not the constant `'VERIFIED'` (`store.ts:517`). Old rows keep `'VERIFIED'`. The control DB already maps an unknown disposition to `DETECTED` (`src/control/appdb.ts:221`).
- `location_json` holds `ObservedLocation`. Old rows have `{file, newLine}` only (`store.ts:519`), and code locates them by anchor line.
- `markFindingsPublished` (`store.ts:343-350`) sets `github_review_id` for the IDs in `inline_finding_ids_json`. It applies `PUBLISHED_TO_GITHUB` only to findings still `DETECTED`. The column, not the status, answers "is it on GitHub", which removes the race with a newer revision's VERIFY.
- The journal trigger on status change (`migrations/050_stream_meta.sql:17-20`) stays. `appdb.ts:390-391` notes that a `FINDING_UPDATED` row replays the current status, and says to fix that once later observations are journaled. This design journals them, so `FINDING_OBSERVED` replays the observation's own disposition, and the note goes away in the same PR.
- The replies option adds `findings.github_comment_id` and a `reply_outbox` table shaped like `publication_outbox` (`migrations/001_init.sql:59-69`, `migrations/003_publication.sql:2-14`). The thread option reuses `findings.github_thread_id`.

### Protocol v1 and the TUI

All changes are additive.

- The finding status enum already has all seven states (`src/protocol/v1.ts:75`, `protocol/schema/v1/finding.json:62-63`). The TUI already labels each one (`tui/src/theme.rs:304-315`) and maps unknown values to `unknown` (`protocol/README.md:149-150`).
- `FindingDetail.history[].disposition` is already a finding state (`v1.ts:101-107`). Once observations store real states, the history shows the progression with no schema change. Each entry gains an optional `basis` string, which clients ignore if unknown (`protocol/README.md:149`).
- `Finding.runId` is the latest observation's revision and `stale` compares that revision's head to the PR head (`appdb.ts:30-36`). Every open finding now gets an observation on each new head, so it is no longer shown as stale. A `RESOLVED` finding gets no further observations and turns stale after the next push. That reads correctly as "resolved on an older head".
- New fixtures: a finding detail with a `PUBLISHED`, `STILL_PRESENT`, `POTENTIALLY_RESOLVED`, `RESOLVED` history, and a playback where one finding crosses two runs. TUI work is optional and belongs in its own PR, for example dimming resolved findings in the rail.

### Benchmark

Today a case is one base and one head (`bench/run.ts:32-51`), a head cannot delete a file (`bench/README.md:47`), and matching is by line (`bench/score.ts:19-34`).

Case format, backward compatible. A case without `commits` is the current single step.

```json
{
  "title": "Let admins change a user's role",
  "body": "...",
  "labels": [{ "id": "role-auth", "file": "src/routes/users.ts", "line_range": [15, 27], "severity_at_least": "high", "category": "security", "note": "..." }],
  "commits": [
    { "dir": "commit2", "delete": [], "expect": [{ "label": "role-auth", "status": "resolved" }], "labels": [] }
  ]
}
```

- The loader overlays each `commitN/` on the previous head, deletes the listed paths, commits, and moves `refs/pull/1/head`. It runs `review` again on the same data directory, so the store carries the findings.
- Multi-commit cases run with `--publish` against the in-process `FakeGitHub` (`tests/fake-github.ts:10-31`), which `main` accepts as `deps.github` (`src/cli/main.ts:27-30`). A dry run never publishes (`task.ts:238`), so it cannot show whether a carried finding is posted again. The fake stores every review with its inline comments, so the scorer counts the comments that would really reach GitHub, through the real outbox. Single-commit cases keep `--dry-run`.
- `expect.status` is one of `resolved` (must be `RESOLVED`), `open` (must be `STILL_PRESENT` and must not be `RESOLVED`), or `not_resolved` (anything but `RESOLVED`, for partial fixes). An expected label may give a new `line_range` for where it now sits.
- The step-1 match gives each label a finding ID. Later steps look up that ID in the step's JSON output.

Cases:

| Case | Commit 2 | Expected |
|---|---|---|
| `fix` | Adds the missing check | `resolved` |
| `leave` | Unrelated edit in another file | `open`, same ID, no inline comment |
| `move` | Renames the file and shifts the bug by 20 lines | `open`, same ID, no inline comment |
| `rewrite-still-buggy` | Rewrites the function, the bug remains | `open` or `not_resolved`, and no second finding with an inline comment |
| `partial-fix` | Fixes one of two paths (SPEC M3, `docs/SPEC.md:555`) | `not_resolved` |
| `worse` | Leaves the bug and adds a second instance | first `open`, plus a new label matched by a new finding |
| `fix-elsewhere` | Fixes the bug in a file the evidence does not cite | `resolved`, reported as resolution recall. A miss here is the known gap. |
| `regress` | Commit 2 fixes the bug, commit 3 brings it back | `resolved`, then a new finding ID with one inline comment |

Metrics, in priority order:

1. **False-resolution rate.** Labels expected `open` or `not_resolved` that ended `RESOLVED`, over all such labels. This is the release gate, because a false `RESOLVED` hides a live defect.
2. **Identity kept.** Steps where the label's finding ID did not change, over steps that expect `open`.
3. **Duplicate-comment rate.** Inline comments after step 1 on a label that already had one, over all inline comments. This is SPEC's duplicate rate (`docs/SPEC.md:592`).
4. **Resolution recall.** Labels expected `resolved` that ended `RESOLVED`. This is SPEC's resolution accuracy (`docs/SPEC.md:593`) split by direction.

Single-step precision and recall stay as they are.

## Build plan

Each PR ships alone, leaves `main` green, and ends in a check that fails without it.

**Update after the baseline (#25):** steps 2 and 3 ship as one PR. The baseline kept identity in 0 of 3 cases, because the fingerprint is built from the title's words (`verify.ts:22-25`) and the model rewords the title on every review. A fingerprint match alone, which is all step 2 adds, would therefore catch almost no re-posts. Matching by cited text (step 3) is what stops them.

1. **Multi-commit benchmark, baseline only.** Adds the case format, `delete`, publishing to `FakeGitHub` for multi-commit cases, the four metrics, the eight cases, and a unit test for the scorer in `tests/bench-score.test.ts`. It changes no review behavior. The check is the scorer test. The first run records today's baseline. Based on the probe above, I expect duplicates to be well above zero and no status past `PUBLISHED` (*guess* until measured).
2. **Stop re-posting.** The inline filter becomes "status is `DETECTED`". A fingerprint match on a posted finding applies `OBSERVED_PRESENT`. Adds the "still present" summary section and `PUBLISHER_VERSION = "p2"`. No migration. In this PR a status other than `DETECTED` still means the finding was posted, because only `PUBLISHED_TO_GITHUB` leaves `DETECTED` (`machines.ts:196`). The check is a store and publisher test: two revisions with one finding give one inline comment in total, and the finding ends `STILL_PRESENT`. Also a test that an in-flight `p1` outbox still completes. The benchmark duplicate rate drops on `leave`.
3. **Content-addressed carry-over.** Adds migration 055, `github_review_id` as the record of posting, and then the `DETECTED` observation events in `FINDING_MACHINE`, which are safe only once the column exists. Also adds `ObservedLocation` with evidence text, the locate step, the carry rows for `same`, `changed` and `file_gone`, the new ID derivation, the journal replay fix at `appdb.ts:390`, and the protocol `basis` field with fixtures. Nothing reaches `RESOLVED` yet, which is the safe direction. The checks are unit tests over a real git fixture for move, rename, rewrite and delete, and a protocol playback fixture. Benchmark identity is kept on `move`.
4. **Checked resolution.** Adds `priorFindingId` and `resolutions` to `submit_candidates`, the prior-findings section in the request, the resolution check, and `CONFIRMED_RESOLVED` and `OBSERVED_UNCERTAIN`. Adds the "resolved" summary section. The check is a verify test where every refusal reason has a case: wrong SHA, unchanged text, contradicting candidate, and wrong file. The PR merges only when the benchmark false-resolution rate is zero over three runs, and it reports resolution recall on `fix`.
5. **TUI polish (optional).** Shows "first seen" and dims resolved findings. Golden snapshots change.
6. **Thread replies (gated by decision 1, declined: not built).** Adds comment-ID capture, `reply_outbox`, and its reconciliation. Crash tests mirror `tests/publish-crash.test.ts` for the reply row.
7. **Thread resolution (gated by decision 2, declined: not built).** Adds the GraphQL client call, the `isResolved` reconciliation, and a docs update to `docs/publishing.md`.

PRs 1 through 4 deliver the README's "tracking a finding across commits" and "marking it resolved". PRs 6 and 7 were declined, so "replying in review threads" stays a documented limit. The README Limits line changes in PR 4.

## Decisions for the operator

These are product calls. Each has my default, and the build plan above assumes it.

**Decided by the operator on 2026-10-21:** decisions 1, 2, 3 and 6 take the defaults below. The bot never replies in threads and never resolves them, so build plan steps 6 and 7 are not built. Decisions 4, 5 and 7 keep their defaults.

1. **Does the bot reply in existing review threads?** Default: no. Carried status goes in the one review body per push. Replies add one GitHub write per finding per push, which is more noise, plus a second outbox to keep exactly-once. SPEC already accepts summary-only (`docs/SPEC.md:409`). Revisit if authors say they miss status changes that sit in the summary.
2. **Does the bot ever resolve GitHub threads?** Default: never. Resolving collapses a conversation that humans may still be having, on the strength of a static check. A human resolves with one click. If you enable it, limit it to `RESOLVED` findings whose thread has no human reply.
3. **How noisy may a push be?** Default: under `comment`, every revision still posts its one review, as today (`docs/configuration.md:82`), now with carried sections instead of duplicates. Under `silent_no_findings`, a revision posts only when it has a new finding or a newly resolved one. The alternative is one review per push only when something changed, which is quieter but leaves a push with no visible sign that it was reviewed.
4. **Report a new finding on code the last review already saw?** Default: yes, as a normal new finding. The anchor must be in the PR diff (`verify.ts:41`), so it is the author's code. Hiding it to avoid looking inconsistent loses a real defect. The summary can say it was missed before.
5. **Does a changed severity post anything?** Default: a summary line only, with no new inline comment. A new inline comment for the same defect is the duplicate this design removes.
6. **Does a fixed defect that comes back get a new comment?** Default: yes. It is a new finding with a new ID and one inline comment, because the old thread may be resolved or collapsed.
7. **Show resolved findings in the TUI inbox?** Default: shown and dimmed, using the existing `resolved` label (`tui/src/theme.rs:311`). Hiding them would make a push that fixed things look identical to one that found nothing.

## Risks and open questions

- **Copied code.** The tree-wide search can relocate a finding to copied code. The one-match and two-line rules reduce this. The failure leaves the finding open, never resolved.
- **Model compliance.** If the model ignores `priorFindingId`, the fallbacks are fingerprint and evidence text. If both miss, the result is a duplicate finding, which the benchmark's duplicate rate will show.
- **Prompt injection through prior titles.** Titles come from an earlier model run that read untrusted code. The request fences them (`request.ts:11`).
- **Developer replies.** SPEC lists developer replies as a reasoning input (`docs/SPEC.md:261`). This design does not read them. A "won't fix" reply is a natural follow-up, and it needs the `issue_comment` and review-comment events that the app subscribes to only partly today (`github-app.yml:16-18`).
