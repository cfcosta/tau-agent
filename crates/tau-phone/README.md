# tau-phone

tau's interface on an Android phone, through gpui-pre-mobile. The phone
runs no agent. It pairs with the tau running on a computer, then shows and
steers that computer's runs, with the same `Workspace` and screens as the
desktop. To pair, it reads the computer's QR code in tau's own viewfinder,
or the person types the address and code.

## What it provides

The crate is a `cdylib`: the native library Android's NativeActivity loads.

| Item               | What it is                                                                            |
| ------------------ | ------------------------------------------------------------------------------------- |
| `android_main`     | The symbol Android calls: opens a `Workspace` and calls `remote::connect`             |
| `qr::Frame`        | A camera frame's luminance: a byte per pixel, `stride` bytes per row                  |
| `qr::pairing_code` | Reads a tau pairing code from a frame, or `None`                                      |
| `android/`         | The Gradle project: the manifest, `TauViewfinder` (CameraX) and `TauScanner`, in Java |

The viewfinder is a Java activity on CameraX, without Google Play services.
It shows the camera's preview and hands each frame's luminance to
`qr::pairing_code` over JNI until one holds a pairing code. It asks for the
camera the first time.

On any target but Android, the crate only builds `qr`, so the workspace
still builds and tests on the desktop.

## How it fits

It builds on `tau-ui-remote` (the interface, and its phone-side `remote`)
and `tau-remote` (`PairingCode`). It brings each plugin's UI half and none
of their host halves, so no tools, Luau, jj-lib or MCP client end up in the
APK. Nothing depends on it.

## Building

The APK builds through Nix, on x86_64-linux only, from the repository's
root:

```sh
nix build .#tau-phone-apk      # result/tau-phone-debug.apk
nix run .#tau-phone-install    # adb install -r on the phone adb sees
```

Nix cross-compiles the crate with the NDK's clang (API 31, arm64), then runs
the Gradle project in `android/` offline, from the dependencies recorded in
`android/deps.json`. After changing what Gradle fetches, record them again:

```sh
bash scripts/update-android-deps.sh
```

The same build by hand, in the flake's `android` shell (`nix develop
.#android`), fetching from Google's Maven:

```sh
cargo ndk -t arm64-v8a -P 31 -o crates/tau-phone/android/app/src/main/jniLibs \
    build -p tau-phone --release
gradle -p crates/tau-phone/android assembleDebug
```

That APK is
`crates/tau-phone/android/app/build/outputs/apk/debug/app-debug.apk`.

Every debug build is signed with `android/debug.keystore`, a public key
committed so builds install over each other. Never sign a release with it.

## Testing

```sh
cargo nextest run --release -p tau-phone
```

This runs the `qr` tests on the desktop: codes drawn with `qrcode`, read
back upright, turned, blurred, scaled and among other QR codes, with Hegel
property tests for sizes and noise. The Android side has no automated
tests.

## Further reading

- [docs/decisions/0013-phones-connect-to-a-running-tau.md](../../docs/decisions/0013-phones-connect-to-a-running-tau.md)
