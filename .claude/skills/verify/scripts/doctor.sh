#!/bin/sh
# Read-only: is the instance NAME worth driving? Checks the recorded pids, that the socket is ours, and a hello.
# usage: doctor.sh NAME
. "$(dirname "$0")/lib.sh"
scratch "${1:-}"
[ -d "$dir" ] || die "no $dir; run daemon-up.sh $name"
[ "$(stat -f %u "$dir")" = "$(id -u)" ] && [ "$(stat -f %Lp "$dir")" = 700 ] || die "$dir is not ours with mode 700"
echo "dir      $dir (ours, 700)"
while read -r pid; do
  [ -n "$pid" ] || continue
  if ours "$pid"; then echo "pid      $pid alive: $(ps -o command= -p "$pid" | cut -c1-140)"; else echo "pid      $pid gone"; fi
done < "$pids"
[ -S "$sock" ] || die "no socket at $sock (daemon down?)"
[ "$(stat -f %u "$sock")" = "$(id -u)" ] || die "$sock is not ours"
listener=$(lsof -t "$sock" 2>/dev/null | head -1)
[ -n "$listener" ] && grep -qx "$listener" "$pids" || die "the process on $sock ($listener) is not one this instance started"
echo "socket   $sock (ours, served by pid $listener)"
echo "checkout $(git -C "$repo" rev-parse --short HEAD) $(git -C "$repo" branch --show-current)"
node "$here/probe.mjs" "$sock"
