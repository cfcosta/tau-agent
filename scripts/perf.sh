#!/usr/bin/env bash
# Where heiko's time goes under load, as heiko-perf keeps it
# (docs/reference/performance.md), in two parts:
#
# - the interface: the demo's stress run played in a headless sway of its
#   own for a while, with HEIKO_PERF set: how long each update took to
#   apply, how late the interface's thread beat, and its stalls;
# - the host: a scripted run of many turns with every built-in plugin
#   on, the model answering at once: what each plugin's hooks took.
#
# Run it by hand, from the repository's root, in the dev shell, after
# `cargo build --release -p heiko`.
#
#   scripts/perf.sh [rounds] [seconds] [-- flags for heiko]
#
# As the screenshots' script, it never draws on the person's desktop.
set -euo pipefail

rounds=${1:-200}
seconds=${2:-60}
shift $(($# < 2 ? $# : 2))
[ "${1:-}" = "--" ] && shift

root=$(pwd)
heiko="${CARGO_TARGET_DIR:-$root/target}/release/heiko"
[ -x "$heiko" ] || { echo "build heiko first: cargo build --release -p heiko" >&2; exit 1; }

run=$(mktemp -d /tmp/heiko-perf.XXXX)
trap 'rm -rf "$run"' EXIT
sway=$(nix build --out-link "$run/tool" --print-out-paths nixpkgs#sway | head -1)/bin/sway

mkdir -p "$run/xdg" "$run/config" "$run/data"
printf 'output HEADLESS-1 resolution 1440x900\ndefault_border none\n' > "$run/sway.cfg"
host_socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_DISPLAY:-none}"
env -u WAYLAND_DISPLAY -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u SWAYSOCK \
  XDG_RUNTIME_DIR="$run/xdg" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
  "$sway" -d -c "$run/sway.cfg" > "$run/sway.log" 2>&1 &
sway_pid=$!
socket=""
for _ in $(seq 1 50); do
  socket=$(grep -o "Running compositor on wayland display '[^']*'" "$run/sway.log" |
    sed "s/.*'\(.*\)'/\1/" || true)
  [ -n "$socket" ] && break
  sleep 0.2
done
if [ -z "$socket" ] || [ "$run/xdg/$socket" = "$host_socket" ] || [ ! -S "$run/xdg/$socket" ]; then
  echo "no headless sway socket (got '$socket'); refusing" >&2
  kill "$sway_pid" 2>/dev/null || true
  exit 1
fi
env -u WAYLAND_DISPLAY -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u SWAYSOCK \
  XDG_RUNTIME_DIR="$run/xdg" WAYLAND_DISPLAY="$socket" \
  XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" \
  HEIKO_PERF="$run/perf.json" \
  "$heiko" --demo --stress "$rounds" "$@" > "$run/heiko.log" 2>&1 &
heiko_pid=$!
sleep "$seconds"
kill "$heiko_pid" "$sway_pid" 2>/dev/null || true
wait "$heiko_pid" "$sway_pid" 2>/dev/null || true
echo "== the interface: $rounds rounds of the stress run, $seconds s"
cat "$run/perf.txt"

echo "== the host: a scripted run with every built-in plugin"
cargo nextest run --release -p heiko-ui --run-ignored only --no-capture \
  -E 'test(a_scripted_run_of_many_turns_is_measured)' 2>&1 |
  sed -n '/^running 1 test/,/^test result/p' | sed '1d;$d'
