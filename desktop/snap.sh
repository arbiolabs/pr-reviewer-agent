#!/bin/sh
# Renders named states of the showcase (and two small sessions beside it) to PNGs, dark and light, and the classic
# themes on the overview and a finding.
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

# Derived sessions, kept under target/ at paths every checkout shares. A script under target/playback answers
# requests from target/fixtures, a copy of protocol/fixtures with one change: the showcase's third finding cites
# no code, so the empty evidence state renders. The script's own summaries of that finding say so too, so the
# list and the detail count the same evidence.
mkdir -p desktop/target/playback
rm -rf desktop/target/fixtures
cp -R protocol/fixtures desktop/target/fixtures
derived_script=desktop/target/playback/showcase.ndjson
python3 -c 'import json, sys
p, src, out = sys.argv[1:]
d = json.load(open(p))
d["finding"]["evidence"] = []
d["finding"]["evidenceCount"] = 0
json.dump(d, open(p, "w"))
fid = d["finding"]["id"]
def zero(x):
    if isinstance(x, dict):
        if x.get("id") == fid and "evidenceCount" in x:
            x["evidenceCount"] = 0
        for v in x.values():
            zero(v)
    elif isinstance(x, list):
        for v in x:
            zero(v)
with open(out, "w") as w:
    for line in open(src):
        o = json.loads(line)
        zero(o)
        w.write(json.dumps(o, separators=(",", ":")) + "\n")' \
  desktop/target/fixtures/server.finding.detail.showcase-3.json "$script" "$derived_script"
# A read-only daemon: the showcase with its welcome rewritten, answering from the fixtures as they are.
rm -rf desktop/target/readonly
mkdir -p desktop/target/readonly/playback
cp -R protocol/fixtures desktop/target/readonly/fixtures
readonly_script=desktop/target/readonly/playback/readonly.ndjson
sed '1s/"capabilities":\["account.select","review.add","review.start","review.retry","review.cancel","review.publish"\],"mode":"engine"/"capabilities":["account.select"],"mode":"read-only","readOnlyReason":"another pr-review holds the data directory; start this daemon with --allow-commands"/' \
  "$script" > "$readonly_script"
# No GitHub account: the showcase with every account list emptied, answering from the fixtures as they are.
rm -rf desktop/target/no-account
mkdir -p desktop/target/no-account/playback
cp -R protocol/fixtures desktop/target/no-account/fixtures
no_account_script=desktop/target/no-account/playback/no-account.ndjson
python3 -c 'import json, sys
def empty(x):
    if isinstance(x, dict):
        for k, v in x.items():
            x[k] = [] if k == "accounts" else empty(v)
    elif isinstance(x, list):
        x[:] = [empty(v) for v in x]
    return x
with open(sys.argv[2], "w") as w:
    for line in open(sys.argv[1]):
        w.write(json.dumps(empty(json.loads(line)), separators=(",", ":")) + "\n")' "$script" "$no_account_script"
# Review again: the showcase with review.again granted, which the showcase itself doesn't script.
mkdir -p desktop/target/again/playback
rm -rf desktop/target/again/fixtures
cp -R protocol/fixtures desktop/target/again/fixtures
again_script=desktop/target/again/playback/again.ndjson
sed '1s/"review.start",/"review.start","review.again",/' "$script" > "$again_script"

# One capture, retried up to three times when the app exits without its PNG or never writes it within 60 s.
capture() {
  png=$1
  shift
  for try in 1 2 3; do
    rm -f "$png"
    "$bin" "$@" --snapshot "$png" &
    pid=$!
    n=0
    while kill -0 "$pid" 2>/dev/null && [ $n -lt 60 ]; do
      sleep 1
      n=$((n + 1))
    done
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      echo "retry $(basename "$png"): no frame in 60 s" >&2
      continue
    fi
    if wait "$pid" && [ -f "$png" ]; then
      echo "$png"
      return 0
    fi
    echo "retry $(basename "$png")" >&2
  done
  echo "gave up on $(basename "$png")" >&2
  return 1
}

snap() {
  name=$1 appearance=$2 ms=$3 actions=$4 from=${5:-$script}
  case $only in "  " | *" $name "*) ;; *) return 0 ;; esac
  set -- --playback "$from" --window 1280x800 --appearance "$appearance" --snapshot-after-ms "$ms"
  [ -n "$actions" ] && set -- "$@" --actions "$actions"
  capture "$out/$name.png" "$@"
}

# The design review's states, dark and light.
for look in dark light; do
  snap 00-connecting-$look     $look 600   "" desktop/playback/slow.ndjson
  snap 01-overview-$look       $look 20000 ""
  snap 02-pr-$look             $look 20000 "wait:19000,pr:1287,finding:2"
  snap 03-finding-$look        $look 20000 "wait:19000,pr:1287,finding:1,finding:1"
  snap 04-diff-$look           $look 20000 "wait:19000,pr:1287,finding:1,open-diff"
  snap 05-running-$look        $look 10000 "wait:9000,pr:1291"
  snap 05b-replay-$look        $look 10000 "wait:9000,pr:1291,scrub:12"
  snap 05c-waterfall-$look     $look 10000 "wait:9000,pr:1291,span:turn 4"
  snap 06-approval-$look       $look 20000 "wait:19000,pr:1291,publish"
  snap 06b-confirm-post-$look  $look 20000 "wait:19000,pr:1291,publish,confirm-post"
  snap 07-palette-$look        $look 20000 "wait:19000,palette:re"
  snap 07b-palette-readonly-$look $look 20000 "wait:19000,pr:1287,palette:start" "$readonly_script"
  snap 08-settings-$look       $look 1000  "" protocol/playback/setup.ndjson
  case $only in "  " | *" 08-disconnected-$look "*)
    capture "$out/08-disconnected-$look.png" --socket /private/tmp/prdc/s.sock --window 1280x800 \
      --appearance $look --snapshot-after-ms 1500 ;;
  esac
  snap 09a-posting-$look       $look 20000 "wait:19900,pr:1291,publish,confirm"
  snap 09-published-$look     $look 30000 "wait:20000,pr:1291,publish,wait:200,confirm,wait:6000"
  snap 10-timeline-$look       $look 20000 "wait:19000,pr:1287,open-timeline"
  snap 11-empty-queue-$look    $look 1000  "" desktop/playback/clear.ndjson
  snap 12-readonly-$look       $look 20000 "wait:19000,pr:1296" "$readonly_script"
  snap 13-huge-pr-$look        $look 20000 "wait:19000,pr:345"
  snap 14-finding-no-evidence-$look $look 20000 "wait:19000,pr:1287,finding:3,finding:3" "$derived_script"
  snap 15-tracking-$look       $look 3000  "wait:2000,pr:1302" protocol/playback/tracking.ndjson
  snap 15b-reopened-$look      $look 3000  "wait:2000,pr:1302,finding:2,finding:2" protocol/playback/tracking.ndjson
  snap 16-discards-$look       $look 3000  "wait:2000,pr:1302,open-timeline,discards" protocol/playback/tracking.ndjson
  snap 17-big-diff-$look       $look 3000  "wait:2000,pr:1310,open-diff" protocol/playback/big-diff.ndjson
  snap 17b-big-diff-flagged-$look $look 3000 "wait:2000,pr:1310,open-diff,flagged" protocol/playback/big-diff.ndjson
  snap 18a-add-pr-empty-$look  $look 20000 "wait:19000,add-pr"
  snap 18b-add-pr-valid-$look  $look 20000 "wait:19000,add-pr:https://github.com/acme/web/pull/77"
  snap 18c-add-pr-error-$look  $look 20000 "wait:19000,add:acme/web#77,add-pr:acme/web#77"
  snap 19-reviewer-stopped-$look $look 20000 "wait:19000,reviewer:stopped"
  snap 19b-starting-$look      $look 600   "reviewer:starting" desktop/playback/slow.ndjson
  snap 19c-stop-confirm-$look  $look 10000 "wait:9000,pr:1291,reviewer:stop-confirm"
  snap 20-footer-account-$look $look 20000 ""
  snap 22-review-again-$look   $look 20000 "wait:19000,pr:1287" "$again_script"
  snap 22b-again-confirm-$look $look 20000 "wait:19000,pr:1287,again" "$again_script"
  snap 22c-again-palette-$look $look 10000 "wait:9000,pr:1291,palette:again" "$again_script"
done
snap 20b-footer-no-account     dark  20000 "" "$no_account_script"

# The classic themes, each on the overview and on a finding, and the picker in Settings.
snap 21-theme-solarized-light         solarized-light 20000 ""
snap 21-theme-solarized-light-finding solarized-light 20000 "wait:19000,pr:1287,finding:1,finding:1"
snap 21b-solarized-dark               solarized-dark  20000 ""
snap 21b-solarized-dark-finding       solarized-dark  20000 "wait:19000,pr:1287,finding:1,finding:1"
snap 21c-nord                         nord            20000 ""
snap 21c-nord-finding                 nord            20000 "wait:19000,pr:1287,finding:1,finding:1"
snap 21d-appearance-settings          nord            20000 "wait:19000,goto:settings"
