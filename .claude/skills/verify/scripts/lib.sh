# Sourced by every helper. Resolves the scratch instance for NAME and refuses anything that could reach the
# operator's install. Usage: . "$(dirname "$0")/lib.sh"; scratch NAME
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
cli=$repo/bin/pr-review.mjs

die() { echo "verify: $*" >&2; exit 1; }

# NAME is short and plain: the socket path /private/tmp/NAME/s.sock must stay under macOS's 103-byte limit.
scratch() {
  name=${1:-}
  case $name in '' | *[!a-z0-9-]*) die "name must be [a-z0-9-], such as v1" ;; esac
  [ ${#name} -le 16 ] || die "name must be 16 characters or fewer"
  dir=/private/tmp/$name
  sock=$dir/s.sock
  pids=$dir/pids
  # A Keychain service only this instance uses; never the real io.github.diego-arbio-dev.pr-review.
  keychain_service=io.github.diego-arbio-dev.pr-review.verify-$name
}

# A pid we recorded is still ours only while its command line names this scratch dir or this checkout.
ours() {
  cmd=$(ps -o command= -p "$1" 2>/dev/null) || return 1
  case $cmd in *"$dir"* | *"$repo"*) return 0 ;; *) return 1 ;; esac
}
