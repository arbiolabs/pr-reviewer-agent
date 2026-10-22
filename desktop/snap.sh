#!/bin/sh
# Renders named states of protocol/playback/showcase.ndjson to PNGs.
# usage: desktop/snap.sh [OUT_DIR [STATE...]]   (default OUT_DIR: desktop/snapshots, every state)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/snapshots}
[ $# -gt 0 ] && shift
only=" $* "
mkdir -p "$out"
out=$(cd "$out" && pwd)
cargo build --release --quiet --manifest-path "$here/Cargo.toml"
bin=$here/target/release/pr-review-desktop
# Relative, so the endpoint the window shows is the same in every checkout.
cd "$here/.."
script=protocol/playback/showcase.ndjson

snap() {
  name=$1 appearance=$2 ms=$3 actions=$4 from=${5:-$script}
  case $only in "  " | *" $name "*) ;; *) return 0 ;; esac
  set -- --playback "$from" --window 1280x800 --appearance "$appearance" --snapshot-after-ms "$ms" \
    --snapshot "$out/$name.png"
  [ -n "$actions" ] && set -- "$@" --actions "$actions"
  "$bin" "$@"
  echo "$out/$name.png"
}

# The design review's states, dark and light. 08 plays a daemon with no PRs and a missing model.
for look in dark light; do
  snap 01-overview-$look   $look 20000 ""
  snap 02-pr-$look         $look 20000 "wait:19000,pr:1287,finding:2"
  snap 03-finding-$look    $look 20000 "wait:19000,pr:1287,finding:1,finding:1"
  snap 04-diff-$look       $look 20000 "wait:19000,pr:1287,finding:1,open-diff"
  snap 05-running-$look    $look 10000 "wait:9000,pr:1291"
  snap 06-approval-$look   $look 20000 "wait:19000,pr:1291,publish"
  snap 07-palette-$look    $look 20000 "wait:19000,palette:re"
  snap 08-setup-$look      $look 1000  "" protocol/playback/setup.ndjson
  snap 08-setup-checks-$look $look 1000 "goto:settings" protocol/playback/setup.ndjson
  case $only in "  " | *" 08-disconnected-$look "*)
    "$bin" --socket /private/tmp/prdc/s.sock --window 1280x800 --appearance $look --snapshot-after-ms 1500 \
      --snapshot "$out/08-disconnected-$look.png" && echo "$out/08-disconnected-$look.png" ;;
  esac
  snap 09-done-$look       $look 30000 "wait:20000,pr:1291,publish,wait:200,confirm,wait:6000,tab:done"
  snap 10-confirm-$look    $look 20000 "wait:19000,pr:345,retry"
done
