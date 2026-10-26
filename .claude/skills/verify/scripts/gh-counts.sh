#!/bin/sh
# Read-only: counts a PR's reviews, review comments and issue comments on GitHub, to prove a run posted nothing.
# Run it before and after a drive and diff the two.
# usage: gh-counts.sh OWNER/REPO NUMBER
set -eu
repo=${1:?usage: gh-counts.sh OWNER/REPO NUMBER}
n=${2:?usage: gh-counts.sh OWNER/REPO NUMBER}
for p in "pulls/$n/reviews" "pulls/$n/comments" "issues/$n/comments"; do
  printf '%s %s\n' "$p" "$(gh api "repos/$repo/$p" --paginate --jq length | awk '{s+=$1} END {print s+0}')"
done
