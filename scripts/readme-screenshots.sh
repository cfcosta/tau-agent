#!/usr/bin/env bash
# The README's screenshots, taken from tau's demo (`--demo --open
# <screen>`) into docs/screenshots/. Run it by hand, from the repository's
# root, after `cargo build --release -p tau`; it reads the binary from
# $CARGO_TARGET_DIR (or target/). Pass names to retake only those shots.
#
# Each shot runs in its own headless sway and is grabbed with grim, so
# the person's own desktop is never drawn or captured: the script unsets
# the host's Wayland variables, gives sway a runtime directory of its
# own, and refuses to go on unless sway's socket is there and is not the
# host's.
set -euo pipefail

root=$(pwd)
out="$root/docs/screenshots"
tau="${CARGO_TARGET_DIR:-$root/target}/release/tau"
[ -x "$tau" ] || { echo "build tau first: cargo build --release -p tau" >&2; exit 1; }

tools=$(nix build --no-link --print-out-paths nixpkgs#sway nixpkgs#grim nixpkgs#imagemagick nixpkgs#oxipng)
sway=$(echo "$tools" | grep -- '-sway-' | head -1)/bin/sway
grim=$(echo "$tools" | grep -- '-grim-' | head -1)/bin/grim
magick=$(echo "$tools" | grep -- '-imagemagick-' | grep -v -- '-dev' | head -1)/bin/magick
oxipng=$(echo "$tools" | grep -- '-oxipng-' | head -1)/bin/oxipng

# name, the demo screen, how long to wait, then any other flags. A screen
# that adds its card at start needs `--steps 0`, or the demo's script
# pushes the card out of view; `--finished` opens the run done.
shots='
run                run               7
crew               crew              7
land               land              7
land-queue         land-queue        7
vcs                log               12 --finished
compare            compare           7
pull-request       pr                7
pushed             pushed            7  --steps 0
ask                ask               7
canvas-image       canvas-image      7
canvas-document    canvas-document   7
uploads            uploads           7  --steps 0
agents             agents            7
agent              agent             7
scheduled          scheduled         7
plugins            plugins           7
plugins-yours      plugins-yours     7
codemode           codemode          7  --steps 0
memory             memory            7
constitution       constitution      7
mcp                mcp               7
ledger             ledger            12 --finished
direnv             envrc-consent     7
skill              skill             7  --steps 0
goal               goal              7
watcher            watcher           30
reasoning          note-open         7  --steps 4
history            history           7
search             search            7
welcome            welcome           7
model              model             7
phones             phones            7
phone-run          run               7  --phone
phone-pair         pair-scan         7  --phone
phone-agents       agents-home       7  --phone
phone-ask          ask               7  --phone
phone-canvas       canvas-image      7  --phone
'

shoot() {
  local name=$1 screen=$2 wait=$3
  shift 3
  local host_socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_DISPLAY:-none}"
  # A short path: sway dies when its socket's path passes 108 bytes.
  local run
  run=$(mktemp -d /tmp/tau-shot.XXXX)
  mkdir -p "$run/xdg" "$run/config" "$run/data"
  printf 'output HEADLESS-1 resolution 1440x900\ndefault_border none\n' > "$run/sway.cfg"
  env -u WAYLAND_DISPLAY -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u SWAYSOCK \
    XDG_RUNTIME_DIR="$run/xdg" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    "$sway" -d -c "$run/sway.cfg" > "$run/sway.log" 2>&1 &
  local sway_pid=$! tau_pid=""
  local socket=""
  for _ in $(seq 1 50); do
    socket=$(grep -o "Running compositor on wayland display '[^']*'" "$run/sway.log" |
      sed "s/.*'\(.*\)'/\1/" || true)
    [ -n "$socket" ] && break
    sleep 0.2
  done
  if [ -z "$socket" ] || [ "$run/xdg/$socket" = "$host_socket" ] || [ ! -S "$run/xdg/$socket" ]; then
    echo "$name: no headless sway socket (got '$socket'); refusing" >&2
    kill "$sway_pid" 2>/dev/null || true
    rm -rf "$run"
    return 1
  fi
  env -u WAYLAND_DISPLAY -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u SWAYSOCK \
    XDG_RUNTIME_DIR="$run/xdg" WAYLAND_DISPLAY="$socket" \
    XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" \
    "$tau" --demo --open "$screen" --frame 1440x900 "$@" > "$run/tau.log" 2>&1 &
  tau_pid=$!
  sleep "$wait"
  XDG_RUNTIME_DIR="$run/xdg" WAYLAND_DISPLAY="$socket" "$grim" "$out/$name.png"
  kill "$tau_pid" "$sway_pid" 2>/dev/null || true
  wait "$tau_pid" "$sway_pid" 2>/dev/null || true
  rm -rf "$run"
  # The phone layout draws a 390×844 phone in the middle of the screen.
  case $name in
    phone-*) "$magick" "$out/$name.png" -crop 390x844+525+28 +repage "$out/$name.png" ;;
  esac
  "$oxipng" -o 4 --strip safe -q "$out/$name.png"
  echo "$name"
}

mkdir -p "$out"
echo "$shots" | while read -r name screen wait flags; do
  [ -n "$name" ] || continue
  if [ $# -gt 0 ] && ! printf '%s\n' "$@" | grep -qx "$name"; then continue; fi
  # shellcheck disable=SC2086
  shoot "$name" "$screen" "$wait" $flags
done
