//! tau-ui on an Android phone.
//!
//! The phone runs no agent: it pairs with the tau on a computer and
//! shows and steers its runs (decision 0013), through tau-ui's
//! [`remote`](tau_ui::remote). Pairing reads the computer's code in
//! tau's own viewfinder: a Java activity (`TauViewfinder`, on CameraX,
//! without Google Play services) shows the camera's preview full screen
//! and hands each frame's luminance, one at a time, to [`qr`] over JNI,
//! until one holds a tau pairing code. It asks for the camera the first
//! time; without it, the address and code can be typed instead.
//!
//! To build the APK, in the flake's `android` shell (`nix develop
//! .#android`), from the repository's root:
//!
//! ```text
//! cargo ndk -t arm64-v8a -P 26 -o crates/tau-phone/android/app/src/main/jniLibs \
//!     build -p tau-phone --release
//! gradle -p crates/tau-phone/android assembleDebug
//! ```
//!
//! Gradle fetches CameraX and AndroidX Activity from Google's Maven. The
//! APK is `crates/tau-phone/android/app/build/outputs/apk/debug/app-debug.apk`;
//! `adb install` it.

pub mod qr;

#[cfg(target_os = "android")]
mod android;
