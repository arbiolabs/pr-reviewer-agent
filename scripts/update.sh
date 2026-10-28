#!/bin/sh
# Updates this install of pr-review to a release tag: the source checkout this script sits in, the desktop app built
# from it, and the reviewer daemon. The app runs it from Settings → General → Updates; it runs by hand the same way.
#
# usage: scripts/update.sh --check     print "latest vX.Y.Z", the newest release tag on origin
#        scripts/update.sh [vX.Y.Z]    update to that tag, else the newest
#
# The source moves only by fast-forward, and the app's binary is replaced only once the new one has built, so a
# failure leaves the install as it was. Each step prints one line; the commands' own output goes to the log.
#
# PR_REVIEW_APP           the app bundle to install into (default ~/Applications/PR Reviewer Agent.app)
# PR_REVIEW_LAUNCH_AGENT  the daemon's LaunchAgent label, restarted when it is running
set -eu

src=$(cd "$(dirname "$0")/.." && pwd)
app=${PR_REVIEW_APP:-$HOME/Applications/PR Reviewer Agent.app}
label=${PR_REVIEW_LAUNCH_AGENT:-io.github.diego-arbio-dev.pr-review.daemon}
log=$HOME/Library/Logs/pr-review/update.log
# The app starts from Finder with a bare PATH; the tools live in the usual install places.
export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export GIT_TERMINAL_PROMPT=0

say() { printf '%s\n' "$*"; printf '%s %s\n' "$(date '+%F %T')" "$*" >&3; }
die() { say "error: $*"; exit 1; }
mkdir -p "$(dirname "$log")"
exec 3>>"$log"

git -C "$src" fetch --quiet --tags origin >&3 2>&3 || die "can't fetch release tags from origin; see $log"
latest=$(git -C "$src" tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n 1)
[ -n "$latest" ] || die "origin has no release tags"
if [ "${1:-}" = --check ]; then
  echo "latest $latest"
  exit 0
fi
tag=${1:-$latest}
git -C "$src" rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no release $tag"

lock=$src/.update.lock
mkdir "$lock" 2>/dev/null || die "another update is running (remove $lock if none is)"
trap 'rmdir "$lock"' EXIT
[ -z "$(git -C "$src" status --porcelain --untracked-files=no)" ] || die "$src has local changes"

before=$(git -C "$src" rev-parse HEAD)
undo() {
  say "putting the source back"
  git -C "$src" reset -q --hard "$before" >&3 2>&3
  (cd "$src" && pnpm install --frozen-lockfile) >&3 2>&3 || true
}

say "updating the source to $tag"
git -C "$src" merge -q --ff-only "$tag" >&3 2>&3 || die "can't fast-forward $src to $tag"
say "installing dependencies"
(cd "$src" && pnpm install --frozen-lockfile) >&3 2>&3 || { undo; die "pnpm install failed; see $log"; }
say "building the app"
(cd "$src/desktop" && cargo build --release) >&3 2>&3 || { undo; die "the build failed; see $log"; }

if [ -d "$app" ]; then
  say "installing the app"
  bin=$app/Contents/MacOS/pr-review-desktop
  version=$(sed -n 's/^ *"version": "\(.*\)",$/\1/p' "$src/package.json" | head -n 1)
  plist=$app/Contents/Info.plist
  # The old files wait outside the bundle: anything left inside it when it is signed becomes part of its seal.
  keep=$(mktemp -d)
  cp "$bin" "$keep/bin"
  cp "$plist" "$keep/Info.plist"
  cp "${CARGO_TARGET_DIR:-$src/desktop/target}/release/pr-review-desktop" "$bin.new"
  mv -f "$bin.new" "$bin"
  plutil -replace CFBundleShortVersionString -string "$version" "$plist"
  plutil -replace CFBundleVersion -string "$version" "$plist"
  if ! codesign --force --deep -s - "$app" >&3 2>&3; then
    mv -f "$keep/bin" "$bin"
    mv -f "$keep/Info.plist" "$plist"
    codesign --force --deep -s - "$app" >&3 2>&3 || true
    die "can't sign $app; the app is unchanged"
  fi
  rm -rf "$keep"
fi

# A reviewer stopped from the app stays stopped: its next start runs the new code.
if launchctl print "gui/$(id -u)/$label" 2>/dev/null | grep -q 'state = running'; then
  say "restarting the reviewer"
  launchctl kickstart -k "gui/$(id -u)/$label" >&3 2>&3 || die "can't restart the reviewer; see $log"
fi
say "updated to $tag"
