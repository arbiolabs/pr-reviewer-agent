#!/bin/sh
# Types into PID's window through System Events, after making PID frontmost and checking it is. Refuses to type
# when anything else is frontmost, so keystrokes never land in another app.
# usage: keys.sh PID STEP...
#   type:TEXT        types TEXT
#   cmd:K            ⌘K (also cmd-shift:K, shift:K)
#   return | escape | tab | down | up
#   wait:MS          sleeps MS milliseconds
set -eu
pid=${1:?usage: keys.sh PID STEP...}
shift
front() {
  osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $pid) to true" >/dev/null
  sleep 0.3
  now=$(osascript -e 'tell application "System Events" to get unix id of first process whose frontmost is true')
  [ "$now" = "$pid" ] || { echo "keys: pid $now is frontmost, not $pid; not typing" >&2; exit 1; }
}
esc() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
for step in "$@"; do
  case $step in
    wait:*) sleep "$(echo "${step#wait:}" | awk '{print $1/1000}')"; continue ;;
  esac
  front
  case $step in
    type:*) as="keystroke \"$(esc "${step#type:}")\"" ;;
    cmd:*) as="keystroke \"$(esc "${step#cmd:}")\" using {command down}" ;;
    cmd-shift:*) as="keystroke \"$(esc "${step#cmd-shift:}")\" using {command down, shift down}" ;;
    shift:*) as="keystroke \"$(esc "${step#shift:}")\" using {shift down}" ;;
    return) as="key code 36" ;;
    escape) as="key code 53" ;;
    tab) as="key code 48" ;;
    down) as="key code 125" ;;
    up) as="key code 126" ;;
    *) echo "keys: unknown step $step" >&2; exit 2 ;;
  esac
  osascript -e "tell application \"System Events\" to $as"
  echo "sent $step"
done
