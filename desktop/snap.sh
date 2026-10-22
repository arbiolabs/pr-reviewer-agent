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
  name=$1 appearance=$2 ms=$3 actions=$4
  case $only in "  " | *" $name "*) ;; *) return 0 ;; esac
  set -- --playback "$script" --window 1280x800 --appearance "$appearance" --snapshot-after-ms "$ms" \
    --snapshot "$out/$name.png"
  [ -n "$actions" ] && set -- "$@" --actions "$actions"
  "$bin" "$@"
  echo "$out/$name.png"
}

# name           appearance  ms     actions
snap start       dark        0      ""
snap inbox       dark        500    "goto:inbox"
snap running     dark        6000   ""
snap findings    dark        20000  "wait:19000,pr:1287"
snap findings-light light    20000  "wait:19000,pr:1287"
snap finding     dark        20000  "wait:19000,pr:1287,finding:2,finding:2"
snap diff        dark        20000  "wait:19000,pr:1287,finding:2,open-diff"
snap timeline    dark        20000  "wait:19000,pr:1287,open-timeline"
snap help        dark        20000  "wait:19000,help"
snap ready       dark        83000  ""
snap settings    dark        20000  "wait:19000,goto:settings"
