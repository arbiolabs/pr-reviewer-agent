#!/bin/sh
# Records desktop/demo/showcase.gif: a tour of the showcase playback, one --snapshot capture per frame, with no daemon
# and no network. Each frame is played in virtual time, so the same build gives the same GIF.
# usage: desktop/demo.sh [THEME]   (any --appearance theme; default: dark)   Needs macOS and ffmpeg.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
look=${1:-dark}
cargo build --release --quiet --manifest-path "$here/Cargo.toml"
bin=$here/target/release/pr-review-desktop
# Relative, so the endpoint the window shows is the same in every checkout.
cd "$here/.."
script=protocol/playback/showcase.ndjson
frames=desktop/target/demo
rm -rf "$frames"
mkdir -p "$frames" desktop/demo
list=$frames/frames.txt
n=0

# One frame shown for HOLD seconds: the window at MS milliseconds into the playback, after STEPS. Retried up to three
# times when the app exits without its PNG or runs past 60 s.
frame() {
  hold=$1 ms=$2 steps=$3
  n=$((n + 1))
  png=$frames/$(printf %03d $n).png
  set -- --playback "$script" --window 1280x800 --appearance "$look" --snapshot-after-ms "$ms" --snapshot "$png"
  [ -n "$steps" ] && set -- "$@" --actions "$steps"
  for try in 1 2 3; do
    rm -f "$png"
    "$bin" "$@" &
    pid=$!
    s=0
    while kill -0 "$pid" 2>/dev/null && [ $s -lt 60 ]; do
      sleep 1
      s=$((s + 1))
    done
    kill "$pid" 2>/dev/null || true
    if wait "$pid" && [ -f "$png" ]; then
      printf "file '%s'\nduration %s\n" "$(basename "$png")" "$hold" >> "$list"
      echo "$png"
      return 0
    fi
    echo "retry $(basename "$png")" >&2
  done
  echo "gave up on $(basename "$png")" >&2
  return 1
}

# The queue, once the showcase has played to its parked review.
frame 3   20000 ""
# #1287's critical finding: selected in the queue, then open beside its cited code.
frame 2   20000 "wait:19000,pr:1287,finding:1"
frame 3.5 20000 "wait:19000,pr:1287,finding:1,finding:1"
# The diff, with the finding under its line.
frame 3.5 20000 "wait:19000,pr:1287,finding:1,open-diff"
# #1291's review, live, from its first phase until it parks for approval.
ms=1500
while [ $ms -le 18500 ]; do
  frame 0.45 $ms "wait:$((ms - 500)),pr:1291"
  ms=$((ms + 1000))
done
frame 2   20000 "wait:19000,pr:1291"
# The proof of exactly what will be posted, and the sheet that asks before posting it.
frame 3.5 20000 "wait:19000,pr:1291,publish"
frame 3   20000 "wait:19000,pr:1291,publish,confirm-post"
# Settings.
frame 4   20000 "wait:19000,goto:settings"
# The concat demuxer takes the last frame's duration only when the file is named again.
echo "file '$(printf %03d $n).png'" >> "$list"

# 2560x1600 captures on a Retina display, shown at the window's 1280x800 points.
ffmpeg -loglevel error -y -f concat -i "$list" \
  -vf "scale=1280:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
  -fps_mode vfr -loop 0 desktop/demo/showcase.gif
ls -l desktop/demo/showcase.gif
