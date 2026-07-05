#!/usr/bin/env bash
# Rebuild the caliban debug and release binaries after a merge.
#
# By default: pulls main (ff-only), builds `caliban` in debug then release,
# prints binary sizes, and smoke-tests each with `--version`.
#
# Flags:
#   --no-pull   Skip `git pull`; build the current HEAD as-is.
#   --release   Build only the release binary.
#   --debug     Build only the debug binary.
#   -h, --help  Show this help.
set -euo pipefail

do_pull=1
do_debug=1
do_release=1

usage() {
  sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

for arg in "$@"; do
  case "$arg" in
    --no-pull) do_pull=0 ;;
    --release) do_debug=0; do_release=1 ;;
    --debug)   do_release=0; do_debug=1 ;;
    -h|--help) usage 0 ;;
    *) echo "rebuild: unknown argument '$arg'" >&2; usage 1 ;;
  esac
done

# Run from the workspace root (this script lives in scripts/).
cd "$(dirname "$0")/.."

if [[ "$do_pull" == 1 ]]; then
  echo "==> git pull --ff-only"
  git pull --ff-only
fi
echo "==> HEAD: $(git log --oneline -1)"

report() {
  local profile="$1" path="$2"
  if [[ -x "$path" ]]; then
    local size
    size=$(du -h "$path" | cut -f1)
    printf '    %-8s %s  (%s)  ->  %s\n' "$profile" "$path" "$size" "$("$path" --version 2>&1 | head -1)"
  fi
}

if [[ "$do_debug" == 1 ]]; then
  echo "==> cargo build -p caliban (debug)"
  cargo build -p caliban
fi
if [[ "$do_release" == 1 ]]; then
  echo "==> cargo build -p caliban --release"
  cargo build -p caliban --release
fi

echo "==> binaries:"
[[ "$do_debug" == 1 ]]   && report debug   target/debug/caliban
[[ "$do_release" == 1 ]] && report release target/release/caliban
echo "==> done"
