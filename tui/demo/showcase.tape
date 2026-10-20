# Records tui/demo/showcase.gif from the showcase playback. From the repository root:
#   cargo build --release --manifest-path tui/Cargo.toml && vhs tui/demo/showcase.tape
# The playback clock starts when the client launches, so the sleeps below line up with its events:
# #1291's review finds two issues at about 14 s and 15 s, and completes at about 18 s.

Output tui/demo/showcase.gif

Require tui/target/release/pr-review-tui

Set Shell "bash"
Set FontSize 15
Set Width 1200
Set Height 760
Set Padding 16
Set Framerate 24
Set TypingSpeed 450ms
Set CursorBlink false
Set Theme { "name": "pr-review", "background": "#171717", "foreground": "#dfe3ea", "selection": "#232a3a", "cursor": "#82aaff", "black": "#171717", "red": "#ff7a8a", "green": "#8fd18a", "yellow": "#e6b673", "blue": "#82aaff", "magenta": "#c5a3ff", "cyan": "#7fc8e8", "white": "#dfe3ea", "brightBlack": "#9aa2b2", "brightRed": "#ff6b7f", "brightGreen": "#a8d68a", "brightYellow": "#e6c36a", "brightBlue": "#82aaff", "brightMagenta": "#c5a3ff", "brightCyan": "#7fd6c8", "brightWhite": "#ffffff" }

Hide
Type "clear && COLORTERM=truecolor tui/target/release/pr-review-tui --playback protocol/playback/showcase.ndjson"
Enter
Sleep 600ms
Show

# The client lands on the most severe finding: #1287's critical refund bug.
Sleep 3.5s

# J scrolls the explanation while the code stays in view.
Type "JJ"
Sleep 2s

# d opens the diff at the finding's line.
Type "d"
Sleep 3.5s

Escape
Sleep 1.5s

# w watches the running review on #1291 until it completes with two findings.
Type "w"
Sleep 10s

# t opens #1291's timeline, oldest first under phase headings.
Type "t"
Sleep 6s
