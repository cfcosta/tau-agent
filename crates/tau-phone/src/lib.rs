//! tau's interface on an Android phone.
//!
//! The phone runs no agent: it pairs with the tau on a computer and
//! shows and steers its runs (decision 0013), through tau-ui-remote's
//! [`remote`](tau_ui_remote::remote). Pairing reads the computer's code in
//! tau's own viewfinder: a Java activity (`TauViewfinder`, on CameraX,
//! without Google Play services) shows the camera's preview full screen
//! and hands each frame's luminance, one at a time, to [`qr`] over JNI,
//! until one holds a tau pairing code. It asks for the camera the first
//! time; without it, the address and code can be typed instead.
//!
//! To build the APK, from the repository's root:
//!
//! ```text
//! nix build .#tau-phone-apk      # result/tau-phone-debug.apk
//! nix run .#tau-phone-install    # adb install -r on the phone adb sees
//! ```
//!
//! Nix cross-compiles this crate with the NDK's clang (API 31, arm64),
//! then runs the Gradle project in `android/` offline, from the
//! dependencies recorded in `android/deps.json`. After changing what
//! Gradle fetches (a plugin, CameraX, any dependency), rerun
//! `scripts/update-android-deps.sh` to record them again. Every debug
//! build is signed with `android/debug.keystore`, a public key committed
//! so that builds install over each other; it is never for a release.
//!
//! The same build by hand, in the flake's `android` shell (`nix develop
//! .#android`), fetching from Google's Maven:
//!
//! ```text
//! cargo ndk -t arm64-v8a -P 31 -o crates/tau-phone/android/app/src/main/jniLibs \
//!     build -p tau-phone --release
//! gradle -p crates/tau-phone/android assembleDebug
//! ```
//!
//! The phone depends on tau-ui-remote, with each plugin's views and none
//! of their host halves: none of the plugins' tools, nor Luau, jj-lib or
//! the MCP client,
//! come along. The APK still carries the NDK's `libc++_shared.so`,
//! which Luau needed, until one without it is shown to load
//! (`CARGO_NDK_LINK_LIBCXX_SHARED` in the shell).
//! The shell's APK is
//! `crates/tau-phone/android/app/build/outputs/apk/debug/app-debug.apk`.

pub mod qr;

#[cfg(target_os = "android")]
mod android;
