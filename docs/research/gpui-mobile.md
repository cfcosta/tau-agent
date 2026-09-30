# Research: gpui-mobile, GPUI on iOS and Android

- Status: research only; nothing decided
- Date: 2026-09-30

## Question

tau-ui already has a phone layout: one column below 720 px, with the
side panel in a bottom sheet. Could it run natively on iOS and Android
through gpui-mobile? What is gpui-mobile, how mature is it, and what
would tau have to change?

## Answer, in short

gpui-mobile implements GPUI's `Platform` trait for iOS (UIKit, Metal
through wgpu, CoreText) and Android (NDK, Vulkan or GL ES through
wgpu, cosmic-text). It works: its example app runs on real devices.
It calls itself experimental, though: mobile IME composition,
accessibility and some lifecycle hooks are unfinished.

The line to follow is longbridge's fork, published as
`gpui-pre-mobile`. It builds on `gpui-pre`, weekly crates.io snapshots
of Zed's GPUI, rather than on a Zed Git pin.

For tau, two things stand in the way, and neither of them is
gpui-mobile:

1. **GPUI version.** tau-ui and tau-terminal are on `gpui 0.2.2`
   (crates.io, 2025-10-22). gpui-mobile needs the GPUI of 2026
   (`gpui-pre 0.3.x`), with the same exact version pinned across the
   app, the platform and the renderer. tau would move to `gpui-pre`
   first, on the desktop, and that is a migration of its own.
2. **Where the agent runs.** A phone cannot run tau as it is today.
   iOS forbids spawning processes, so `bash` cannot run. On Android,
   processes run in a sandbox with no developer toolchain.
   tau-terminal's libghostty (a Zig build), docbert's embedding model,
   and the repository checkouts are all built for a workstation. A
   mobile tau would be a client of a tau host running elsewhere, and
   tau has no remote host protocol yet.

## What gpui-mobile is

| Platform | Windowing                          | Renderer                                  | Text                |
| -------- | ---------------------------------- | ----------------------------------------- | ------------------- |
| iOS      | UIKit (`UIWindow`, `CAMetalLayer`) | Metal through `gpui_wgpu`                 | CoreText (font-kit) |
| Android  | NDK (`ANativeWindow`, `ALooper`)   | Vulkan, or GL ES 3.0, through `gpui_wgpu` | cosmic-text, swash  |

- **Architecture.** It mirrors Zed's own platform crates
  (`gpui_linux`, `gpui_macos`): `IosPlatform` and `AndroidPlatform`
  implement `gpui::Platform`, with a window, a display, a dispatcher
  (Grand Central Dispatch; `ALooper` plus a thread pool) and a
  keyboard map each. An app hands the platform to
  `Application::with_platform(..).run(..)` instead of calling
  `gpui_platform::application()`.
- **Entry points.**
  - **Android:** `android-activity` calls the app's `android_main`,
    where `Platform::run` blocks, driving the native event loop. The
    app's first window opens once the system hands over a surface.
    The fork adds a host-driven entry point that survives Activity
    recreation, and `GpuiInputActivity` in place of
    `NativeActivity` for IME composition.
  - **iOS:** an Objective-C app delegate (`main.m`) calls the Rust
    library's C functions (`gpui_ios_initialize`, the
    did-finish-launching and foreground/background hooks).
- **Input.** Touches are translated into mouse events, with a
  tap-versus-scroll state machine and momentum scrolling. The fork
  uses Android's own scroll physics. Hardware keyboards are
  supported, and so are safe-area insets, dark mode and emoji (a
  bundled CBDT font on Android).
- **Extras.** It includes Glass-style and Material components, native
  view embedding (`PlatformView`, after Flutter's platform views),
  and about thirty Flutter-style device packages behind features, all
  on by default: camera, location, notifications, clipboard, and
  others. tau would turn them off.
- **Build.** iOS needs macOS, Xcode 15 and XcodeGen
  (`aarch64-apple-ios`, iOS 13+). Android needs the SDK, NDK r25+ and
  `cargo-ndk` (arm64 tested, API 26+; armv7 and x86_64 untested).
  `example/build.sh ios|android --device|--simulator` builds and runs
  the example on either.
- **Size.** About 56k lines of Rust, most of them in the device
  packages and the components.

## The two lines

| Line                                                                                               | Crate                                                                   | GPUI                                                                                     | Activity                   |
| -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | -------------------------- |
| [itsbalamurali/gpui-mobile](https://github.com/itsbalamurali/gpui-mobile), the original, 372 stars | `gpui-mobile 0.1.0` (2026-03-03)                                        | Zed Git pin `5688167d` (2026-04-14), plus Zed's wgpu, font-kit and async-task forks      | pushed 2026-09-25          |
| [longbridge/gpui-mobile](https://github.com/longbridge/gpui-mobile), a fork, 17 commits ahead      | `gpui-pre-mobile 0.1.0` (2026-09-15), library still named `gpui_mobile` | `gpui-pre` and `gpui-pre-wgpu` pinned exactly: `=0.3.4` on crates.io, `=0.3.7` on `main` | weekly, last on 2026-09-29 |

The fork's commits so far (all September 2026):

- touch hover and cancellation fixed on both platforms;
- mobile IME, file picking, and native touch gestures;
- demand-driven frames, so an idle screen costs no CPU;
- Android frames paced to vsync;
- a held Backspace repeats on the iOS keyboard;
- the Metal layer pinned instead of stretched;
- a host can register its own `AssetSource`;
- an iOS window deregisters itself when dropped.

`gpui-pre` is published by huacnlee, who maintains gpui-component at
Longbridge. It snapshots Zed's GPUI with its platform crates
(`gpui-pre-platform`, `-linux`, `-macos`, `-windows`, `-web`,
`-wgpu`); 0.3.7 is Zed at `1a28cff`, from 2026-09-28. Eight releases
came out between 2026-09-03 and 2026-09-28, and 38 crates depend on
it, gpui-component 0.7 among them.

## Gaps, as the code stands

- **IME, accessibility, lifecycle.** The fork's README says the
  switch to `gpui-pre` "does not complete mobile IME composition,
  accessibility, or the mobile lifecycle hooks". Screen readers are
  still a TODO.
- **Android `open_url`.** It logs the request instead of starting an
  `ACTION_VIEW` Intent. tau opens pull requests and sign-in pages in
  the browser, so this would matter.
- **One window.** Android's window comes from the system, not from
  the app (`open_window` hands back the system's). tau-ui uses one
  window, so this fits.
- **Menus and dock.** No-ops, as expected on a phone.
- **Credentials.** They go to the Android Keystore under a fixed user
  name, since GPUI's trait only passes a URL.

## What tau would need

1. **Move to `gpui-pre`, on the desktop first.**
   - Replace `gpui 0.2.2` in tau-ui and tau-terminal with `gpui-pre`
     at one exact version (`package = "gpui-pre"` keeps `use gpui::`).
     Desktop windows then come from `gpui-pre-platform`.
   - Tried on 2026-09-30 with `gpui-pre =0.3.7`: 26 changed lines in
     13 files, and every tau-ui and tau-terminal test passes (205).
     The changes: `focus` takes the app context, `ShapedLine::paint`
     takes an alignment, `flex_grow` takes a factor, `Corner` is
     `Anchor`, a list's `max_offset_for_scrollbar` is a point, the
     async context's `update` no longer fails, and `Application::new()`
     is `gpui_platform::application()`.
   - `gpui-pre-platform` needs its `wayland` and `x11` features, or
     the Linux build has no window backend (its default is none).
   - The renderer changes from blade to wgpu (Vulkan), and cosmic-text
     from 0.14 to 0.19. Tests draw with GPUI's test platform, so only
     running the app shows whether text and drawing still look
     right.
   - Because the versions are pinned exactly, gpui-pre-mobile and
     gpui-pre only move together.
2. **Decide where the agent runs on a phone.** Each option has a cost:
   - **A client of a remote tau host.** Runs, events and the
     transcript would stream from a desktop or a server. tau-ui
     already draws everything from `RunEvent`s and the store, so the
     screens carry over. The work is a host protocol: authentication,
     event streaming, starting and steering runs.
   - **A local agent without the coding tools.** Chat, memory search
     and the model client (tau-ai speaks WebSocket over rustls, which
     works on both platforms) could run on the device. `bash`, the
     terminal and the repository tools could not.
3. **Leave out the desktop-only crates** from a mobile build, behind
   features or targets:
   - tau-terminal's libghostty (its Zig build has not been tried for
     mobile targets);
   - `bash`, which spawns processes;
   - docbert's model: memory can use its keyword index, as tests do;
   - the GitHub device flow's browser launch, until Android `open_url`
     works.
4. **Use the layout tau already has.** The one-column phone layout,
   the bottom sheet, and a frame for previewing it on the desktop
   (`--phone`) exist today. Touch-to-mouse translation means clicks
   work unchanged. Hover states, and targets under 44 px, need a pass.

## A spike, if we want one

1. On a branch, move tau-ui and tau-terminal to `gpui-pre =0.3.7` and
   get the desktop app building and its tests passing. This is the
   real cost, and it pays off even without mobile.
2. Build gpui-pre-mobile's example for Android (`cargo-ndk`,
   `aarch64-linux-android`) from this machine, to confirm the
   toolchain. iOS needs a Mac.
3. Put tau-ui's phone layout in an Android build, with
   `--demo`-style data and no agent, to judge text, scrolling and
   input on a device.

## Licenses

gpui-mobile and gpui-pre-mobile are GPL-3.0-or-later, AGPL-3.0-or-later
or Apache-2.0, at the user's choice. Apache-2.0 leaves tau's own
license, not yet chosen, open. gpui-pre carries Zed's GPUI license
(Apache-2.0).

## Sources

- [itsbalamurali/gpui-mobile](https://github.com/itsbalamurali/gpui-mobile):
  README, `Cargo.toml`, `src/`, `example/`, `TODO.md`,
  `PACKAGES-TO-IMPL.md` at `main`, read 2026-09-30.
- [longbridge/gpui-mobile](https://github.com/longbridge/gpui-mobile):
  README, `Cargo.toml` and `src/android/platform.rs` at `main`, and
  its comparison with the original.
- crates.io: [gpui-mobile](https://crates.io/crates/gpui-mobile),
  [gpui-pre-mobile](https://crates.io/crates/gpui-pre-mobile),
  [gpui-pre](https://crates.io/crates/gpui-pre) (versions, owners,
  reverse dependencies), [gpui](https://crates.io/crates/gpui).
- [gpui_platform on docs.rs](https://docs.rs/gpui-pre-platform/latest/gpui_platform/).
- [docs.rs: gpui_mobile](https://docs.rs/gpui-mobile),
  [lib.rs: gpui-mobile](https://lib.rs/crates/gpui-mobile).
