#!/bin/sh
# Captures PID's window, and only that window, to OUT.png. Never captures the full screen.
# usage: shot.sh PID OUT.png
set -eu
pid=${1:?usage: shot.sh PID OUT.png}
out=${2:?usage: shot.sh PID OUT.png}
id=$("$(dirname "$0")/winid.sh" "$pid")
mkdir -p "$(dirname "$out")"
screencapture -x -o -l "$id" "$out"
echo "$out"
