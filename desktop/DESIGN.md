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

A parked review ranks above a critical finding because the reviewer is blocked on the reader, and approving takes seconds. Cards group under four tier labels: Decide now, Blocking findings, Unblock, and When you have a minute. A running review is never a card. It is a pill in the strip until it needs you. The selected card is filled and wears a 1 pt accent stroke, the one stroke in the window that means "this is the source of the pane beside it"; a selected row in the pane is filled only. Once a card is selected, it stops listing the findings the pane now shows. A card's meta line and its action sit on their own rows, so neither cuts the other short.

An empty queue does not get a page of its own. The home shows the done list under a "You're clear" header, so the reader sees what was finished instead of a blank column beside a populated pane.

The same module derives the sidebar rows (`RowState`: running with its phase, parked, failed, findings with the worst severity and a count, posted, clean, merged, closed, new), the connection `Stage` (offline, connecting, ready) and the title bar's sentence ("One review is waiting on you. #1287 has a critical finding."). `SetupCount` is the one vocabulary the sidebar badge, the setup card and the setup page share: blocking checks keep reviews from running, limited checks let them run with less ("1 blocking · 2 limited").

## Layout at 1280 × 800

| Region | Size | Notes |
|---|---|---|
| Title bar | 52 pt | The traffic lights, then a 20 pt inset on every view, then the page title (15 pt semibold), its one-sentence state (12 pt, muted) and the 220 pt ⌘K field. It drags the window. |
| Sidebar | 232 pt | Needs you, Done and Setup with their counts, then every PR grouped by repository: a glyph, the title on up to two lines, the number and the status word under it, and one 44 pt accessory slot (a severity count, the phase pips, or nothing). Then the connection. |
| Live strip | 44 pt | One pill per running review: a breathing dot, the number, the whole title (pills shrink together when the row is tight), the progress gauge, the phase and its step count. Shown once a snapshot exists, and only on the home, since a finding, the proof, the diff and setup are the reader's deep pages. |
| Queue column | 400 pt | Cards 16 pt apart, a 32 pt bottom inset and a fade where more cards scroll under. |
| Detail | the rest | Sections 24 pt apart, prose at a 440 pt measure (68 characters). The action bar is a 56 pt row of the layout, opaque, with a 1 px hairline on top; a 24 pt fade over the end of the scroll area says the page goes on under it. |

A finding opens in two columns: the argument on the left at the reading measure (the claim, why the reviewer believes it, the suggested fix, the status sentence, the confidence in words, the history with run ids), and on the right the evidence pinned `path:Lstart–end @sha`, the tool summaries, the hunk the finding sits in and Open full diff. A finding with no evidence says "No code cited" and points at the diff. Code never cuts mid-token: a code block scrolls sideways, and a 32 pt fade at its right edge appears when a line is longer than the block fits. The diff is the exception, because a finding's card sits between its rows: there a long line soft-wraps at a space, a comma or a bracket (else before an operator), and the continuation starts under the line's own indent plus two with a `↳` in the gutter.

The diff replaces the queue column with a 264 pt file list whose rows carry per-file finding counts, with the M/A/D letter on the filename's line. It draws the cursor's rows with 8 rows of lead, so the anchored line sits in the upper third, and repeats the hunk's `@@` header above them when the lead skipped it. Each row carries one marker: a finding's severity rail and wash, or the cursor's accent rail. The diff's action bar is the same as every other page's: Back, Next finding, Open on GitHub.

## Tokens (`src/tokens.rs`)

- **Color.** Dark is blue-graphite (`bg #0F1217`, `surface #161A21`). Light is warm paper (`bg #F5F4F0`, `surface #FFFFFF`). Each hue has one meaning: accent (periwinkle `#8E95FF` dark, indigo `#4247C8` light) means "needs you" and the primary action; cyan means running; green means done or OK; amber means setup or a warning; red means failed, and filled red is the one irreversible button. Severity has its own set: critical rose, high orange, medium gold and low steel. A chip or a count badge is its hue as text over a wash of the same hue (`tint`: 14 % dark, 12 % light) inside an outline at twice that alpha. A control or a palette row that can't act now is set in `disabled`, the text color dimmed to about half and stored as one opaque color, with an outline of the same color at 80 %. A test checks every text token on every surface it sits on, including chips over their wash, disabled text and its outline (3:1), the syntax colors and the gutter on the code background, and the diff's text over its added and deleted washes, at 4.5:1 or more in both appearances. The light added-line wash is a desaturated green, so it tints without shouting.
- **Severity is never only a color.** Each severity has a shape and a word: ◆ Critical, ▲ High, ● Medium, ○ Low. The card's 3 pt left rail, the diff's anchored line wash and the gutter bar all use that severity's hue.
- **Type.** The interface is set in the system sans (SF Pro) at 11, 12, 13 (body), 15 and 18. What is said is set in the system serif (New York) at 24: page and PR titles, finding titles, the quoted finding on a blocking card, the numbers on the board. Code and ids use Menlo 12 on a 19 pt line. Prose sits on a 21 pt line. Every number that sits in a column or beside another number (the live stats, count badges, the replay counter, line numbers) uses tabular figures, so digits line up and nothing shifts as they change.
- **Spacing.** A 4 pt grid: 16 pt between cards, 24 pt between detail sections.
- **Radii.** Two: 8 for controls, chips and rows; 12 for cards, code, sheets and the palette. Dots and count badges are circles.
- **Motion.** The live dot breathes over 1.6 s. A log row that just landed fades in over 150 ms, and wears the running hue on its left edge while it is the newest. A spinner turns while the window retries or connects. Nothing else moves. Reduce Motion, `PR_REVIEW_REDUCE_MOTION=1` or a snapshot stills all three. Entering replay moves nothing: the activity header keeps one height whether it follows or replays.
- **Window.** The window's background is blurred (`WindowBackgroundAppearance::Blurred`), and the sidebar and title bar let it through at 90 % so the desktop tints them faintly; every text token keeps AA against the solid color underneath. A snapshot's window is opaque, since its pixels must not depend on what is behind it.

## Interaction

- **Keyboard first.** J and K walk the cards, and ⏎ does what the card's button says. N and ⇧N walk findings, D opens the diff at the finding, T the timeline, and S, ⇧R, C and P start, retry, cancel and preview. ⇥ moves between controls. ⌘K and / open the palette. ⌘1 and ⌘2 switch between Needs you and Done. ⌘, opens setup, ⌘⇧O opens the PR on GitHub, ⌘R refreshes, ⌘W closes the window, ⌘Q quits and ? shows every shortcut. ⌘O stays free because on macOS it means Open a file, and a reader who presses it expecting that should get nothing surprising. The ⌘ shortcuts are the menu bar's items.
- **Three states, three looks.** Hover is a background tint only (a 4 % wash; pressed is 8 %). Keyboard focus is a 2 pt accent ring drawn 2 pt off the control's edge, so it never reads as a border. Selected is a fill, plus the one accent stroke on the selected card. No two panes show the same stroke.
- **Mouse complete.** Every action is also a button or a palette row. A click selects a card, and a second click does what its button says. Icon buttons carry tooltips. Rows are 44 pt where the list allows it. A toast anchors 16 pt above the action bar, never over a control.
- **Friction where it's irreversible.** Posting to GitHub is the one irreversible act. The proof page's Post opens a sheet that names the account, the commit, the counts of body and inline comments and the review kind, says it can't be undone, and follows macOS: Cancel is the default and shows esc (⏎ closes it too), and the filled red Post acts only on ⌘⏎, which sends the reducer's own `Confirm`. The scrim is 40 % ink in light and 50 % in dark, and the page's own Post dims under it, so one primary is on screen. Canceling a running review asks the same way: Keep running on esc and ⏎, Cancel review in filled red on ⌘⏎. Starting and retrying are cheap and reversible, so they go straight through: the reducer's confirmation opens and the desktop answers it at once; a quiet toast says it is sending, and the sheet shows only if the daemon refuses. Every reducer guard stays authoritative: the desktop never bypasses `can_mutate`, `can_confirm` or the confirmation flow. GPUI has no backdrop blur for an element, so the scrim darkens without blurring.
- **The palette explains itself.** Unavailable actions stay listed in the disabled color, with the reducer's reason ("its latest run has not failed"); a row without a shortcut leaves the slot blank. The characters the query lit are set bold: a prefix run first, then a word start, then any character. A query matches a title's prefix first, then word prefixes, then word initials, then a scattered subsequence; groups stay together under their best match.

## States

- **Offline.** No daemon answers and no snapshot ever came: a card centered in the pane with a disconnected glyph, "Reviewer offline" with the reason as a sentence, a spinner that says the app retries on its own, the daemon command in a copyable block, Retry ⌘R and Setup. The sidebar's lists dim, since nothing sits behind them; the window never claims the reader is clear.
- **Connecting.** Skeletons hold the shapes the snapshot will fill: the strip's pill, the queue's cards, the page's header, tab row, run block and finding rows, at the same radii and borders, so nothing jumps on connect.
- **Clear.** Only with a snapshot: "You're clear · N PRs reviewed" over the done list.
- **No findings.** A reviewed PR with nothing to judge says what the reviewer read and verified, under a check; a PR no review has looked at says so under a dashed circle, never a check.
- **Failed.** The gauge colors only the phase the run stopped in, taken from the last phase change the window saw. The run block carries the reason and the next step once; the sentence under the title says only that the review did not finish.
- **Too large.** A review the daemon refused for size reads "Too large to review" with the reason and the next step: exclude generated files or split the PR, then retry.
- **Read-only daemon.** Every action button stays visible, outlined grey, with the reason in its tooltip, and the action bar says "Read-only daemon" with the full sentence on hover. The daemon's own wording never reaches the page.
- **Posted.** The proof's kicker turns to PUBLISHED with the URL, the title reads "Posted to acme/shop #1291", one sentence gives the account, the commit and the counts, and the title bar says "posted"; the Done list holds the PR.
- **Setup.** A check's fix renders its commands as mono chips with a copy button, and the prose around them as prose.

## Trust

- The proof shows the body and every inline comment exactly as GitHub will render them, the marker comment GitHub hides, and the account that posts them, with M for the raw markdown. Its kicker reads "Proof · nothing is posted yet" until it is.
- A finding shows its confidence as a number and a sentence, and its status as a word and the sentence that explains it: still present, looks resolved, reopened, uncertain. The history lists every run that judged it.
- Every sentence the app writes about the reviewer's state comes from the model, never from the desktop's own bookkeeping.
