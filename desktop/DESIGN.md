# pr-review desktop: the design

The app answers one question: what needs me right now? The home is a queue of decisions, most urgent first, each one a card with its key fact, one primary action and its shortcut. Around it sit a calm board (a live strip of running reviews, every PR in the sidebar) and a detail pane that reads like a document: a serif title, one sentence on where things stand, then the argument and the code it rests on.

Three directions were built and judged against the product brief. The triage queue won as the base. From the editorial direction it takes the typographic voice, the state sentences, the quoted finding and the phase-sectioned timeline. From the workstation direction it takes the two-column finding, the scrubbable live track and the palette that explains itself.

## The queue

`src/queue.rs` reads the TUI's `Model` as cards. It holds no state, so the queue can't drift from the reducer.

| Rank | Card | Headline | Primary action |
|---|---|---|---|
| 0 | Setup | Reviews can't run: no models available | Show setup |
| 1 | Approve | the PR's title, "Posts 2 findings · 1 high, 1 medium" | Review post, then ⌘⏎ posts after one sheet |
| 2 | Posting | the PR's title, what GitHub said | Open |
| 3–4 | Findings (critical, high) | the PR's title, the top finding quoted in the serif | Open finding |
| 5 | Failed | the PR's title, the reason, the next step | Retry |
| 7 | Findings (medium, low) | as above | Open finding |
| 8 | Start | the PR's title | Start review |

A parked review ranks above a critical finding because the reviewer is blocked on the reader, and approving takes seconds. Cards group under four tier labels: Decide now, Blocking findings, Unblock, and When you have a minute. A running review is never a card. It is a pill in the strip until it needs you. The selected card wears an accent ring and a pointer on its right edge, because it is the source of the pane beside it; once selected, it stops listing the findings the pane now shows.

The same module derives the sidebar rows (`RowState`: running with its phase, parked, failed, findings with the worst severity and a count, posted, clean, merged, closed, new), the connection `Stage` (offline, connecting, ready) and the title bar's sentence ("One review is waiting on you. #1287 has a critical finding."). `setup_issues` is the one count the sidebar badge, the setup card and the setup page show.

## Layout at 1280 × 800

| Region | Size | Notes |
|---|---|---|
| Title bar | 52 pt | The traffic lights, the page title, its one-sentence state, and the ⌘K field. It drags the window. |
| Sidebar | 232 pt | Needs you, Done and Setup with their counts, then every PR grouped by repository with a glyph, a status word and a badge, then the connection. |
| Live strip | 44 pt | One pill per running review: a breathing dot, the number, the title, the progress gauge, the phase and its step count. Shown only once a snapshot exists. |
| Queue column | 400 pt | Cards 16 pt apart, a 32 pt bottom inset and a fade where more cards scroll under. |
| Detail | the rest | Sections 24 pt apart, prose at a 440 pt measure (68 characters). The action bar is pinned at 56 pt and the page keeps an inset of the same height, so nothing sits under it. |

A finding opens in two columns: the argument on the left at the reading measure (the claim, why the reviewer believes it, the suggested fix, the status sentence, the confidence in words, the history with run ids), and on the right the evidence pinned `path:Lstart–end @sha`, the tool summaries, the hunk the finding sits in and Open full diff. Code never clips at a pane edge: a line longer than its column wraps with a `⤷` continuation mark in the gutter.

The diff replaces the queue column with a 264 pt file list whose rows carry per-file finding counts. It draws the cursor's rows with 8 rows of lead, so the anchored line sits in the upper third.

## Tokens (`src/tokens.rs`)

- **Color.** Dark is blue-graphite (`bg #0F1217`, `surface #161A21`). Light is warm paper (`bg #F5F4F0`, `surface #FFFFFF`). Each hue has one meaning: accent (periwinkle `#8E95FF` dark, indigo `#4A51D4` light) means "needs you" and the primary action; cyan means running; green means done or OK; amber means setup or a warning; red means failed. Severity has its own set: critical rose, high orange, medium gold and low steel. A test checks every text token, including the diff and syntax colors, at 4.5:1 or more on every surface it sits on, in both appearances.
- **Severity is never only a color.** Each severity has a shape and a word: ◆ Critical, ▲ High, ● Medium, ○ Low. The card's 3 pt left rail, the diff's anchored line wash and the gutter bar all use that severity's hue.
- **Type.** The interface is set in the system sans (SF Pro) at 11, 12, 13 (body), 15 and 18. What is said is set in the system serif (New York) at 24: page and PR titles, finding titles, the quoted finding on a blocking card, the numbers on the board. Code and ids use Menlo 12 on a 19 pt line. Prose sits on a 21 pt line.
- **Spacing.** A 4 pt grid: 16 pt between cards, 24 pt between detail sections.
- **Radii.** Two: 8 for controls, chips and rows; 12 for cards, code, sheets and the palette. Dots and count badges are circles.
- **Motion.** The live dot breathes over 1.6 s. A log row that just landed fades in over 150 ms. Nothing else moves. Reduce Motion, `PR_REVIEW_REDUCE_MOTION=1` or a snapshot stills both.

## Interaction

- **Keyboard first.** J and K walk the cards, and ⏎ does what the card's button says. N and ⇧N walk findings, D opens the diff at the finding, T the timeline, and S, ⇧R, C and P start, retry, cancel and preview. ⇥ moves between controls, each of which wears a 2 pt accent ring when it has the keyboard. ⌘K and / open the palette. ⌘1 and ⌘2 switch between Needs you and Done. ⌘, opens setup, ⌘O opens the PR on GitHub, ⌘R refreshes, ⌘W closes the window, ⌘Q quits and ? shows every shortcut. The ⌘ shortcuts are the menu bar's items.
- **Mouse complete.** Every action is also a button or a palette row. A click selects a card, and a second click does what its button says. Every control shows hover (a 4 % wash), pressed (8 %) and a tooltip on icon buttons. Rows are 44 pt where the list allows it.
- **Friction where it's irreversible.** Posting to GitHub is the one irreversible act. The proof page's Post opens a sheet that names the account, the commit, the counts of body and inline comments and the review kind, says it can't be undone, makes Cancel the default (⏎ and esc both close it) and posts only on ⌘⏎, which sends the reducer's own `Confirm`. Canceling a running review asks too, with Keep running as the default. Starting and retrying are cheap and reversible, so they go straight through: the reducer's confirmation opens and the desktop answers it at once; a quiet toast says it is sending, and the sheet shows only if the daemon refuses. Every reducer guard stays authoritative: the desktop never bypasses `can_mutate`, `can_confirm` or the confirmation flow.
- **The palette explains itself.** Unavailable actions stay listed, dimmed, with the reducer's reason ("its latest run has not failed"), and every row shows its shortcut or "—". A query matches a title's prefix first, then word prefixes, then word initials, then a scattered subsequence; groups stay together under their best match.

## States

- **Offline.** No daemon answers and no snapshot ever came: a disconnected glyph, "Reviewer offline" with the reason, the daemon command in a copyable block, Retry ⌘R and Setup. The queue, the strip and the board stay hidden; the window never claims the reader is clear.
- **Connecting.** Skeletons hold the queue's and the page's shapes until the first snapshot.
- **Clear.** Only with a snapshot: "You're clear · N PRs reviewed".
- **No findings.** A reviewed PR with nothing to judge says what the reviewer read and verified; a failed one says to retry for a verdict.
- **Too large.** A review the daemon refused for size reads "Too large to review" with the reason and the next step: exclude generated files or split the PR, then retry.
- **Read-only daemon.** Every action button stays visible, dimmed, with the reason in its tooltip, and the action bar names it.
- **Posted.** The proof's kicker turns to PUBLISHED with the URL; the Done list holds the PR.

## Trust

- The proof shows the body and every inline comment exactly as GitHub will render them, the marker comment GitHub hides, and the account that posts them, with M for the raw markdown. Its kicker reads "Proof · nothing is posted yet" until it is.
- A finding shows its confidence as a number and a sentence, and its status as a word and the sentence that explains it: still present, looks resolved, reopened, uncertain. The history lists every run that judged it.
- Every sentence the app writes about the reviewer's state comes from the model, never from the desktop's own bookkeeping.
