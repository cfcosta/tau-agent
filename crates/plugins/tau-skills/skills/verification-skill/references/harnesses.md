# Harness recipes

Use the repository's own harness when it has one: end-to-end specs, a demo or test mode, a debug port, a test IPC. These recipes are the fallback. Every recipe isolates the run from the person's own instance and settings: its own data dir, port, profile, and (for a GUI) display.

## CLI

Run each command with `exec_command`, with a scratch `HOME` or the app's config and data variables pointed at the run's folder. Capture stdout, stderr, and the exit code, for example `notes create --title T > out.txt 2> err.txt; echo "exit=$?"`. Prefer a machine-readable output flag (`--format json`) for assertions.

## Interactive CLI or TUI

`exec_command` keeps a long command running and returns a process id. Use `write_stdin {process_id, chars}` to send input and to poll output. That works for programs that read lines from stdin.

A full-screen TUI, or a program that checks whether it runs in a terminal, needs a pseudo-terminal. Use a terminal multiplexer or pseudo-terminal tool the repository or the machine already has. With tmux, for example:

```sh
tmux new-session -d -s "verify-$RUN_ID" -x 120 -y 40 "<app command>"
tmux send-keys -t "verify-$RUN_ID" "search quarterly" Enter
tmux capture-pane -t "verify-$RUN_ID" -p > "$EVIDENCE/search.txt"
tmux kill-session -t "verify-$RUN_ID"
```

The session name carries the run id, so cleanup kills only this run's session. Without tmux, `script -q -c "<app command>" /dev/null` gives the program a pseudo-terminal under `exec_command`.

## HTTP service

Drive it with an HTTP client the repository or the machine already has, and keep the status, the headers, and the body as evidence. With `curl`, for example:

```sh
curl -sS -D "$EVIDENCE/h.txt" -o "$EVIDENCE/body.json" -w '%{http_code}\n' http://127.0.0.1:$PORT/notes
```

Pick a free port per run and pass it to the app. Doctor checks that the port belongs to the process this run started.

## Browser app

In order of preference:

1. A browser tool the run already has (an MCP server's `mcp__<server>__*` tools), pointed at a fresh profile.
2. The repository's own browser tests, their driver, and their selectors.
3. A headless browser the run starts itself, with a fresh profile, driven over its debugging protocol by a small client script the skill ships as a helper. For Chromium, for example: `chromium --headless=new --remote-debugging-port=$CDP_PORT --user-data-dir=$RUN/profile <url>`.

Never attach to the person's own browser or profile. Use accessibility roles and names as handles. Capture a screenshot and an accessibility snapshot for each proof.

## Desktop GUI on Linux

Run the app inside a headless display server, such as a headless Wayland compositor, with a screenshot tool that talks to it. The example below uses sway's headless backend and `grim`. Use the compositor and screenshot tool the repository already uses, if it has them. **Never fall back to the person's real display.** If the headless compositor does not come up, the launch fails. It must never quietly use the inherited `WAYLAND_DISPLAY` or `DISPLAY`. An agent that falls back captures the person's real desktop, and drives it.

If the compositor or the screenshot tool is missing, use the repository's dev environment if it provides them, or tell the person which packages to install.

The launch helper, as a sketch:

```sh
set -eu
run=$(mktemp -d /tmp/verify-XXXX)   # keep it short: a socket path over 108 bytes kills sway
mkdir -m 700 "$run/xdg" "$run/config" "$run/data"
host_sock="${XDG_RUNTIME_DIR:-none}/${WAYLAND_DISPLAY:-none}"
printf 'output HEADLESS-1 resolution 1440x900\ndefault_border none\n' > "$run/sway.cfg"

env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK -u HYPRLAND_INSTANCE_SIGNATURE \
  XDG_RUNTIME_DIR="$run/xdg" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
  sway -d -c "$run/sway.cfg" 2> "$run/sway.log" &
echo $! > "$run/sway.pid"

# sway names its socket only in its debug log
wl=
for _ in $(seq 100); do
  wl=$(sed -n "s/.*Running compositor on wayland display '\(wayland-[0-9]*\)'.*/\1/p" "$run/sway.log" | head -n 1)
  [ -n "$wl" ] && [ -S "$run/xdg/$wl" ] && break
  sleep 0.1
done
[ -n "$wl" ] && [ -S "$run/xdg/$wl" ] || { echo "headless sway did not start; refusing to launch" >&2; exit 1; }
[ "$run/xdg/$wl" != "$host_sock" ] || { echo "socket is the host's display; refusing" >&2; exit 1; }

env -u DISPLAY XDG_RUNTIME_DIR="$run/xdg" WAYLAND_DISPLAY="$wl" \
  XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" \
  <app command> > "$run/app.log" 2>&1 &
echo $! > "$run/app.pid"
```

The guard rules the generated skill must keep:

- Unset the host's Wayland and X variables (`WAYLAND_DISPLAY`, `DISPLAY`, `SWAYSOCK`, and the host compositor's own, such as `HYPRLAND_INSTANCE_SIGNATURE`) for the compositor and for the app.
- Give the compositor its own short `XDG_RUNTIME_DIR`. The app and `grim` get the same one, so they cannot reach the host's socket at all.
- Read the socket name from the compositor's own log. Refuse to continue if it is missing, or if its full path equals the host's socket path. Compare paths, not names: a private runtime dir can reuse the host's name, such as `wayland-1`.
- Pass `WAYLAND_DISPLAY` explicitly to the app and to `grim`. Never rely on the inherited value.
- Unset `DISPLAY` for the app so an X11 fallback has nothing to reach.
- Point the app's config and data dirs at the run's folder so it does not read or change the person's settings.
- Kill the app and the compositor by the process ids in `app.pid` and `sway.pid`. Never by name.

Screenshot, once Doctor sees the window:

```sh
XDG_RUNTIME_DIR="$run/xdg" WAYLAND_DISPLAY="$wl" grim "$EVIDENCE/shot.png"
```

Then show it to the person with `canvas_show`. For input, prefer the app's own automation (a demo mode, a flag that opens a given screen, a test IPC). Otherwise use `swaymsg` against the run's own IPC socket in `$run/xdg` to focus and resize, and `wtype` to type, if installed.

On macOS, Windows, or a Linux setup with no headless compositor, there is no safe generic recipe. Use the app's own test or demo mode, or stop and tell the person what is missing. Never screenshot the person's screen.
