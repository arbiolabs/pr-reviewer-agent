#!/bin/sh
# Builds the desktop app (incremental) and opens it on instance NAME's socket with its scratch config.
# Run daemon-up.sh NAME first (or daemon-up.sh NAME --no-start to open it on a stopped reviewer).
# usage: app-up.sh NAME [--appearance THEME]
. "$(dirname "$0")/lib.sh"
scratch "${1:-}"
shift
[ -f "$dir/desktop.toml" ] || die "no $dir/desktop.toml; run daemon-up.sh $name first"
cargo build --release --quiet --manifest-path "$repo/desktop/Cargo.toml"
bin=$repo/desktop/target/release/pr-review-desktop
. "$dir/env.sh"
"$bin" --socket "$sock" --config "$dir/desktop.toml" --window 1280x800 "$@" >> "$dir/app.log" 2>&1 &
pid=$!
echo "$pid" >> "$pids"
id=$("$here/winid.sh" "$pid")
echo "pid $pid"
echo "window $id"
