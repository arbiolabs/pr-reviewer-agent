# Direction C: the triage inbox

The app answers one question: what needs me right now? The home is not a list of PRs. It is a queue of decisions, most urgent first, each one a card with its key fact, one primary action and its shortcut. A calm air-traffic board sits around it: running reviews in a live strip, finished ones in Done.

## The queue

`src/queue.rs` reads the TUI's `Model` as cards. It holds no state, so the queue can't drift from the reducer.

| Rank | Card | Headline | Primary action |
|---|---|---|---|
| 0 | Setup | Reviews can't run: no models available | Show setup |
| 1 | Approve | Approve and post #1291 | Review post, then ⌘⏎ posts |
| 2 | Posting | Posting to #N failed | Open |
| 3–4 | Findings (critical, high) | Critical in #1287: Any signed-in customer can refund any order | Open finding |
| 5 | Failed | Review failed on #345: retry? | Retry |
| 7 | Findings (medium, low) | Medium in #N: … | Open finding |
| 8 | Start | Start a review of #1296 | Start review |

A parked review ranks above a critical finding because the reviewer is blocked on the reader, and approving takes seconds. Cards group under four tier labels: Decide now, Blocking findings, Unblock, and When you have a minute. A running review is never a card. It is a pill in the strip until it needs you. Selecting a card dispatches `Msg::Pick(Hit::Pr)`. An approval card also dispatches `Hit::Ready`, so the detail pane is the exact preview.

## Layout grid at 1280 × 800

| Region | Size | Notes |
|---|---|---|
| Sidebar | 216 pt | The traffic lights sit in its 52 pt top band. It holds Needs you, Done, Running and Setup, and the connection state at the foot. |
| Toolbar | 52 pt | The view title and its count, and the ⌘K field. You drag the window from it. |
| Live strip | 44 pt | One pill per running review: a breathing dot, the number, the title, six phase segments, the phase and its step count. |
| Queue column | 408 pt | Cards on a 16 pt gutter with 8 pt between them. |
| Detail | the rest | Prose stops at a 600 pt measure, about 72 characters. The action bar is pinned at 56 pt. |

The diff replaces the queue column with a 264 pt file list. It draws the cursor's rows with 8 rows of lead, so the anchored line sits in the upper third.

## Tokens (`src/tokens.rs`)

- **Color.** Dark is blue-graphite (`bg #0F1217`, `surface #161A21`). Light is warm paper (`bg #F5F4F0`, `surface #FFFFFF`). Each hue has one meaning:
  - accent (periwinkle `#8E95FF` dark, indigo `#4A51D4` light) means "needs you" and the primary action;
  - cyan means running;
  - green means done or OK;
  - amber means setup or a warning;
  - red means failed.

  Severity has its own set: critical rose, high orange, medium gold and low steel. A test checks every text token at 4.5:1 or more on `bg`, `surface`, `selected` and `sidebar`, in both appearances.
- **Severity is never only a color.** Each severity has a shape and a word: ◆ Critical, ▲ High, ● Medium, ○ Low. The card's 3 pt left rail, the diff's anchored line wash and the gutter bar all use that severity's hue.
- **Type.** The UI uses the system font, SF Pro. The scale is 11, 12, 13 (body), 15 (card headlines), 18 and 24 (detail titles). Code uses Menlo 12 on a 19 pt line. Prose sits on a 21 pt line.
- **Spacing.** A 4 pt grid: 4, 8, 12, 16, 20, 24 and 32.
- **Radii.** There is one radius per element size, and sizes never mix them. Chips and key caps use 4, buttons and rows 6, cards and code 10, sheets and the palette 14, and pills a full radius.
- **Motion.** Only the live dot moves. It breathes over 1.6 s to say a review is working. A step that just landed is lit for 2.4 s of model time. Reduce motion, `PR_REVIEW_REDUCE_MOTION=1` or a snapshot stills both.

## Interaction

- **Keyboard first.** J and K walk the cards, and ⏎ does what the card's button says. N and ⇧N walk findings, D opens the diff at the finding, and S, ⇧R, C and P start, retry, cancel and preview. ⌘K and / open the palette. ⌘1 and ⌘2 switch between Needs you and Done. ⌘, opens setup, ⌘O opens the PR on GitHub, ⌘R refreshes and ? shows every shortcut.
- **Mouse complete.** Every action is also a button or a palette row. A click selects a card, and a second click does what its button says.
- **States.** The selected card gets an accent ring and a filled button. Hover raises the border, and press darkens the fill.
- **Posting takes two steps.** The approval card opens the preview, and only ⌘⏎ (or the reducer's `y`) on that preview posts it. A stale preview says why it can't post, and an accepted approval reads "waiting for GitHub" until the post is confirmed.
- **Start, retry and cancel.** Each opens a sheet that names its target and says what happens.

## Trust

- The preview shows the body and every inline comment exactly as GitHub will render them. It also names the account that will post them, with M for the raw markdown.
- A finding shows its confidence as a number and a word. It shows the cited code with the finding's line marked, the tool evidence, the reasoning and the history across commits. Each tracking status says why it holds: still present, looks resolved, or reopened.
- When the app is disconnected, the queue never claims "nothing needs you".
