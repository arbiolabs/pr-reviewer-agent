#!/bin/sh
# Prints the id of PID's largest on-screen window, waiting up to 30 s for it to appear.
# usage: winid.sh PID
set -eu
pid=${1:?usage: winid.sh PID}
n=0
while :; do
  id=$(swift -e '
import CoreGraphics
let pid = Int(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
let mine = list.filter { ($0[kCGWindowOwnerPID as String] as? Int) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
let area = { (w: [String: Any]) -> Double in let b = w[kCGWindowBounds as String] as! [String: Double]; return b["Width"]! * b["Height"]! }
if let w = mine.max(by: { area($0) < area($1) }) { print(w[kCGWindowNumber as String] as! Int) }
' "$pid" 2>/dev/null || true)
  [ -n "$id" ] && { echo "$id"; exit 0; }
  kill -0 "$pid" 2>/dev/null || { echo "winid: pid $pid exited" >&2; exit 1; }
  n=$((n + 1))
  [ $n -lt 30 ] || { echo "winid: no window for pid $pid in 30 s" >&2; exit 1; }
  sleep 1
done
