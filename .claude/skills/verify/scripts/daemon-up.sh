#!/bin/sh
# Starts this checkout's daemon in /private/tmp/NAME and waits for its listening line.
# usage: daemon-up.sh NAME [--fixture FIXTURE | --engine] [--mode dry_run|approval] [--no-start]
#   --engine (default)  engine mode with --allow-commands: real GitHub reads and a real model, publish_mode dry_run
#   --fixture NAME      a canned playback from protocol/playback (showcase, normal, tracking, ...); no GitHub, no commands
#   --no-start          write the scratch dir and the daemon wrapper only, for an app that starts the reviewer itself
. "$(dirname "$0")/lib.sh"
scratch "${1:-}"
shift
fixture="" mode=dry_run start=1
while [ $# -gt 0 ]; do
  case $1 in
    --fixture) fixture=$2; shift ;;
    --engine) fixture="" ;;
    --mode) mode=$2; shift ;;
    --no-start) start=0 ;;
    *) die "unknown argument $1" ;;
  esac
  shift
done
case $mode in dry_run | approval) ;; *) die "--mode is dry_run or approval; nothing here may post" ;; esac

mkdir -p "$dir"
chmod 700 "$dir"
mkdir -p "$dir/data" "$dir/run" "$dir/xdg-config" "$dir/xdg-state"
touch "$pids"

printf '{"review":{"publish_mode":"%s"}}\n' "$mode" > "$dir/config.json"

# Every process of this instance runs through env.sh: scratch data, config, socket dirs and Keychain service, and
# no TypeSafe key from the caller's shell.
cat > "$dir/env.sh" <<EOF
export PR_REVIEW_DATA_DIR=$dir/data
export PR_REVIEW_CONFIG=$dir/config.json
export PR_REVIEW_KEYCHAIN_SERVICE=$keychain_service
export XDG_RUNTIME_DIR=$dir/run
export XDG_CONFIG_HOME=$dir/xdg-config
export XDG_STATE_HOME=$dir/xdg-state
unset TYPESAFE_API_KEY
EOF

# The daemon wrapper: records its pid (exec keeps it), logs to the scratch dir, runs the checkout's CLI. The app's
# daemon_command points here, so a Start from the app is also this instance and also lands in pids.
cat > "$dir/daemon.sh" <<EOF
#!/bin/sh
. $dir/env.sh
echo \$\$ >> $pids
exec >> $dir/daemon.log 2>&1
exec node $cli "\$@"
EOF
chmod 700 "$dir/daemon.sh"

# The desktop app's config. The LaunchAgent label does not exist, so Start/Stop always spawn daemon.sh.
cat > "$dir/desktop.toml" <<EOF
[connection]
socket = "$sock"

[desktop]
daemon_command = "$dir/daemon.sh"
launch_agent = "io.github.diego-arbio-dev.pr-review.verify-$name-none"
EOF
[ -e "$HOME/Library/LaunchAgents/io.github.diego-arbio-dev.pr-review.verify-$name-none.plist" ] && die "the stand-in LaunchAgent label exists"

[ $start = 1 ] || { echo "dir $dir"; echo "socket $sock"; exit 0; }
[ -S "$sock" ] && die "$sock exists; run down.sh $name first"

if [ -n "$fixture" ]; then
  set -- daemon --fixture "$fixture" --socket "$sock"
else
  set -- daemon --allow-commands --socket "$sock"
fi
touch "$dir/daemon.log"
offset=$(wc -c < "$dir/daemon.log")
"$dir/daemon.sh" "$@" &
pid=$!
n=0
until tail -c +"$((offset + 1))" "$dir/daemon.log" 2>/dev/null | grep -q "pr-review daemon: listening on $sock"; do
  kill -0 "$pid" 2>/dev/null || { tail -20 "$dir/daemon.log" >&2; die "daemon exited before listening"; }
  n=$((n + 1))
  [ $n -lt 600 ] || { tail -20 "$dir/daemon.log" >&2; die "no listening line in 60 s"; }
  sleep 0.1
done
tail -c +"$((offset + 1))" "$dir/daemon.log" | grep "pr-review daemon:"
echo "pid $pid"
echo "socket $sock"
