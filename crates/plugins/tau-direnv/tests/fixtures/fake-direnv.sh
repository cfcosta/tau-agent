#!/bin/sh
# A stand-in for direnv, for tests: `status --json`, `export json` and
# `exec DIR CMD...`, as tau calls them.
#
# - An `.envrc` is allowed when a quoted path in $DIRENV_CONFIG/direnv.toml
#   (tau's whitelist) is a prefix of its directory, and denied when
#   direnv's deny record for it is in $XDG_DATA_HOME/direnv/deny.
# - `export json` sources the `.envrc` (failing as it fails), after
#   $FAKE_DIRENV_DELAY seconds, and prints `{}`.
# - `exec` sources it into the environment, when allowed, then runs CMD.
# - Each call appends its first argument to $FAKE_DIRENV_LOG, if set.

[ -n "$FAKE_DIRENV_LOG" ] && echo "$1" >> "$FAKE_DIRENV_LOG"

state() { # dir -> 0 allowed, 1 not allowed, 2 denied
  rc="$1/.envrc"
  hash=$(printf '%s\n' "$rc" | sha256sum | cut -d' ' -f1)
  if [ -f "${XDG_DATA_HOME:-$HOME/.local/share}/direnv/deny/$hash" ]; then
    echo 2; return
  fi
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
  sleep "${FAKE_DIRENV_DELAY:-0}"
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
