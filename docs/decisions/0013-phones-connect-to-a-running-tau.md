# 0013: Phones connect to a running tau

- Status: accepted
- Date: 2026-09-30
- Builds on [0007](0007-gpui-interface.md) ("Phones"): agents do not run
  on the phone. Research:
  [gpui-mobile](../research/gpui-mobile.md).

## Context

tau-ui has a phone layout, and since it moved to `gpui-pre` it can build
for Android and iOS through `gpui-pre-mobile`. A phone cannot run tau
itself: iOS forbids spawning processes, Android has no toolchain, and
the repositories, the tools, the store and the ChatGPT sign-in all live
on a workstation.

So the phone app connects to a tau that is already running, and shows
and steers its runs. What it connects to, how it gets there, and what
crosses the wire is this decision.

In tau-ui, the host and the interface already talk in messages:

- the interface emits about 40 `WorkspaceEvent`s (start a run, steer,
  cancel, fork, land, edit a rule, sign in…) and `Host::attach`
  subscribes to them;
- the host drives the interface with the runs' `RunEvent`s and about 25
  direct calls on the `Workspace` (`set_catalog`, `show_alert`,
  `set_pull_request_state`, `set_landing_preview`, `set_rate_limits`,
  `add_history`…).

A phone is another interface on the same host.

## Decision

### The desktop app serves phones, first

- tau-ui on the workstation is the host a phone connects to. It serves
  phones only while it runs, and only once the person turns it on
  ("Allow phones", off by default).
- The protocol lives in its own crate, `tau-remote`, with no GPUI in
  it, so a headless `tau-host` daemon can serve it later, with the
  desktop app as one more client. That daemon is not part of this
  decision.

### On the person's own network

- The host listens on an address the person picks: the LAN, or a VPN
  address such as Tailscale's. tau runs no relay and needs no account
  of its own.
- The connection is a WebSocket over TLS. The host makes a self-signed
  certificate once and keeps it with its credentials. A phone trusts
  the certificate by its fingerprint, never by a certificate authority.

### Pairing

- The desktop shows a QR code with the host's address, the
  certificate's fingerprint and a one-time pairing secret, valid for a
  few minutes.
- The phone scans it, connects, and trades the secret for a device
  token of its own, which it stores in the platform's keystore. From
  then on it connects with that token.
- The desktop lists paired phones by name and last use, and can revoke
  any of them. A revoked token is refused at once, and its open
  connection is closed.

### The protocol

- JSON messages over the socket, versioned: the first message states
  the protocol version, and a host refuses one it does not speak.
- **Downstream (host to phone):**
  - `RunEvent`s, each with the store's sequence number, as the desktop
    gets them;
  - `HostUpdate`s: one enum in place of the host's direct calls on
    the `Workspace` (catalog, alerts, pull request states, landing
    previews, plan limits…). The desktop's `Workspace` applies the
    same enum, so both interfaces take the same path.
- **Upstream (phone to host):** `WorkspaceEvent`s, checked by the host
  exactly as the desktop's are.
- **Sync.** On connect the phone gets a snapshot: the catalog, and the
  timelines of the runs it shows, from which it builds its views as
  history does today (`RunView::from_timeline`). Then it follows the
  live events. On reconnect it says the last sequence number it saw,
  and gets what it missed.
- `RunEvent`, `WorkspaceEvent`, `HostUpdate` and what they carry derive
  serde's traits. In `tau-agent` the derives sit behind a `serde`
  feature, so a library user who wants none pays nothing (0001).

### The phone app

- The same tau-ui code, the same `Workspace` and screens, built for
  Android with `gpui-pre-mobile`. In place of `Host`, a `Remote` from
  `tau-remote` does what `Host::attach` does: it subscribes to the
  `Workspace`'s events and sends them up, and applies what comes down.
- A phone build leaves out what only a host needs: the agent, the
  tools, tau-terminal's command runner (its view stays, to draw
  terminal output), jj, the store, the embedding model.
- **Android first:** it builds from this Linux machine with the NDK,
  and installs as an APK without a store. iOS follows on the same code
  once Android works, with a Mac for its build.
- **Android 12 and later** (API 31, decided 2026-10-01). gpui-mobile
  calls `android_get_device_api_level`, which libc exports only from
  API 29, so the library would not load on Android 8 or 9.

## Alternatives considered

- **A headless daemon first.** Runs would outlive the desktop window
  from the start, but the desktop app would have to become a protocol
  client before any phone could connect. Keeping the protocol in its
  own crate leaves that door open without paying for it now.
- **iroh, paired by QR code.** QUIC with NAT traversal and relays,
  dialed by public key: a phone would find the host anywhere, with
  nothing to set up. It is a larger dependency and leans on relays that
  someone runs. The person's own network, or their own VPN, is enough
  to start.
- **A tau relay server.** Works anywhere, but it is infrastructure to
  run, and every message would cross it.
- **Running tau on the phone without its coding tools.** It could chat
  and search memory, but tau's work is in repositories, and those stay
  on the workstation.

## Consequences

- A phone reaches tau only while the desktop app runs and the phone can
  reach it on the network. Away from home, that means a VPN.
- `Host`'s direct calls on the `Workspace` become `HostUpdate`s: a
  refactor of tau-ui, worth it on its own because it puts everything
  the host tells an interface in one enum.
- `tau-agent` gets an optional `serde` feature for `RunEvent`.
- tau-ui builds in two shapes: a desktop app with a host, and a phone
  app with a remote. Features or targets keep the host's dependencies
  out of the phone build.
- The flake grows an Android build: the SDK, the NDK, and packaging an
  APK. `nix build .#tau-phone-apk` builds a signed debug APK offline
  and reproducibly, from Gradle dependencies recorded in
  `crates/tau-phone/android/deps.json` by
  `scripts/update-android-deps.sh`; the `android` dev shell keeps
  `cargo-ndk` and a networked Gradle for working on the app. The steps
  are in `crates/tau-phone/src/lib.rs`.
- No push notifications: a phone learns of a finished run while the
  app is open. Push would need Apple's and Google's services, and a
  server to call them; that is a later decision.
- The host's port is a new attack surface. It is closed by default,
  answers only over TLS, and takes only paired tokens.

## Order of work

1. `HostUpdate` in tau-ui: the host's direct calls become one enum,
   applied by the `Workspace`. No behavior changes.
2. serde for `RunEvent`, `WorkspaceEvent` and `HostUpdate`, and
   round-trip tests for each.
3. `tau-remote`: the protocol, the server with pairing and tokens, and
   the client; tested with a desktop host and a remote `Workspace` in
   one process.
4. "Allow phones" in tau-ui: the listener, the QR code, and the list of
   paired phones.
5. The Android build of tau-ui with a `Remote`, in the flake.

## As built

All five steps are in. Where the code went its own way:

- **One enum downstream.** `HostUpdate` carries the run events too
  (`HostUpdate::Event`), so a phone applies one stream in order.
- **The server numbers the messages**, not the store: numbers start from
  the clock, so they only grow across restarts. It keeps the last 4096
  for phones that come back, and a phone that missed more gets a new
  snapshot.
- **The snapshot is the computer's Workspace**: its runs and its catalog,
  as `HostUpdate::Snapshot`, rather than timelines for the phone to
  rebuild. Onboarding's updates stay on the computer.
- **What a phone may ask for** is `WorkspaceEvent::from_phone`: not
  signing in or out, keys, pairing, or paths on the computer. A phone
  also sends its name, which renames it in the computer's list.
- **Allow phones is on a Phones screen** in the sidebar, not under a
  Settings screen, which tau does not have. Its settings, the
  certificate and the paired phones' token hashes are in
  `~/.config/tau/phones`.
- **The phone scans in its own viewfinder**: a full-screen camera
  preview (CameraX, no Google Play services) whose frames go, one at a
  time, to a decoder in Rust (`rqrr`), until one holds a tau pairing
  code; other QR codes are passed over. It asks for the camera
  permission when it first opens. A photo from the camera app was tried
  first and did not work: the camera app opened, and the code never came
  back. Typing the address instead, as when the camera is not allowed,
  shows the whole certificate to compare.
- **The phone keeps its token in the app's private directory**, not yet
  in the Android Keystore.
