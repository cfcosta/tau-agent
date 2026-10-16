#!/bin/sh
# A stand-in for direnv, for tests: `status --json`, `export json` and
# `exec DIR CMD...`, as tau calls them.
#
# - An `.envrc` is allowed when a quoted path in $DIRENV_CONFIG/direnv.toml
#   (tau's whitelist) is a prefix of its directory. A deny is tested with
#   the real direnv.
# - `export json` sources the `.envrc` (failing as it fails), after
#   $FAKE_DIRENV_GATE exists, when that is set, and prints `{}`.
# - `exec` sources it into the environment, when allowed, then runs CMD.
# - Each call appends its first argument to $FAKE_DIRENV_LOG, if set.

[ -n "$FAKE_DIRENV_LOG" ] && echo "$1" >> "$FAKE_DIRENV_LOG"

state() { # dir -> 0 allowed, 1 not allowed
  for root in $(grep -o '"[^"]*"' "$DIRENV_CONFIG/direnv.toml" 2>/dev/null | tr -d '"'); do
    case "$1" in "$root"*) echo 0; return ;; esac
  done
  echo 1
}

case "$1" in
status)
  printf '{"state":{"foundRC":{"allowed":%s,"path":"%s/.envrc"}}}\n' "$(state "$PWD")" "$PWD"
  ;;
export)
  if [ -n "$FAKE_DIRENV_GATE" ]; then
    while [ ! -e "$FAKE_DIRENV_GATE" ]; do sleep 0.01; done
  fi
  [ "$(state "$PWD")" = 0 ] || { echo "direnv: $PWD/.envrc is blocked" >&2; exit 1; }
  ( . "$PWD/.envrc" ) > /dev/null || exit $?
  echo '{}'
  ;;
exec)
  dir="$2"
  shift 2
  if [ "$(state "$dir")" = 0 ]; then
    set -a
    . "$dir/.envrc" > /dev/null
    set +a
  fi
  exec "$@"
  ;;
*)
  echo "fake direnv: unknown command $1" >&2
  exit 2
  ;;
esac
