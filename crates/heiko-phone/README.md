# heiko-phone

heiko's interface on an Android phone, through gpui-pre-mobile. The phone
runs no agent. It pairs with the heiko running on a computer, then shows and
steers that computer's runs, with the same `Workspace` and screens as the
desktop. To pair, it reads the computer's QR code in heiko's own viewfinder,
or the person types the address and code.

## What it provides

The crate is a `cdylib`: the native library Android's NativeActivity loads.

| Item               | What it is                                                                            |
| ------------------ | ------------------------------------------------------------------------------------- |
| `android_main`     | The symbol Android calls: opens a `Workspace` and calls `remote::connect`             |
| `qr::Frame`        | A camera frame's luminance: a byte per pixel, `stride` bytes per row                  |
| `qr::pairing_code` | Reads a heiko pairing code from a frame, or `None`                                      |
| `android/`         | The Gradle project: the manifest, `HeikoViewfinder` (CameraX) and `HeikoScanner`, in Java |

The viewfinder is a Java activity on CameraX, without Google Play services.
It shows the camera's preview and hands each frame's luminance to
`qr::pairing_code` over JNI until one holds a pairing code. It asks for the
camera the first time.

On any target but Android, the crate only builds `qr`, so the workspace
still builds and tests on the desktop.

## How it fits

It builds on `heiko-ui-remote` (the interface, and its phone-side `remote`)
and `heiko-remote` (`PairingCode`). It draws plugins' views as the computer
builds them, and sends their clicks, commands and keys to the computer,
which carries them out
([0046](../../docs/decisions/0046-the-phone-draws-what-the-computer-builds.md)).
It links no plugin crate, as the kernel names none, so no plugin's code,
tools, Luau, jj-lib or MCP client end up in the APK; a new plugin is no
new phone build. What waits on the person, such as an agent's notes, it
shows as Android notifications through `heiko_ui_plugin::waiting::Watch`.
Nothing depends on it.

## Building

The APK builds through Nix, on x86_64-linux only, from the repository's
root:

```sh
nix build .#heiko-phone-apk      # result/heiko-phone-debug.apk
nix run .#heiko-phone-install    # adb install -r on the phone adb sees
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
cargo ndk -t arm64-v8a -P 31 -o crates/heiko-phone/android/app/src/main/jniLibs \
    build -p heiko-phone --release
gradle -p crates/heiko-phone/android assembleDebug
```

That APK is
`crates/heiko-phone/android/app/build/outputs/apk/debug/app-debug.apk`.

Every debug build is signed with `android/debug.keystore`, a public key
committed so builds install over each other. Never sign a release with it.

## Testing

```sh
cargo nextest run --release -p heiko-phone
```

This runs the `qr` tests on the desktop: codes drawn with `qrcode`, read
back upright, turned, blurred, scaled and among other QR codes, with Hegel
property tests for sizes and noise. The Android side has no automated
tests.

## Further reading

- [docs/decisions/0013-phones-connect-to-a-running-heiko.md](../../docs/decisions/0013-phones-connect-to-a-running-heiko.md)
