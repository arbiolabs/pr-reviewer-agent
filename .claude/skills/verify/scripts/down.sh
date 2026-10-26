#!/bin/sh
# Stops every process instance NAME recorded (the daemon, the app, a daemon the app started), by pid only,
# removes its test Keychain item, and deletes /private/tmp/NAME. Evidence lives elsewhere and is left alone.
# Safe to run twice and after a failed launch.
# usage: down.sh NAME
. "$(dirname "$0")/lib.sh"
scratch "${1:-}"
if [ -f "$pids" ]; then
  for sig in TERM KILL; do
    while read -r pid; do
      [ -n "$pid" ] && ours "$pid" && kill -"$sig" "$pid" 2>/dev/null && echo "$sig $pid"
    done < "$pids"
    [ $sig = TERM ] && sleep 2
  done
fi
# Only the instance's own test service; the real item is never named here.
security delete-generic-password -s "$keychain_service" -a TYPESAFE_API_KEY >/dev/null 2>&1 && echo "removed Keychain item $keychain_service"
rm -rf "$dir"
echo "removed $dir"
