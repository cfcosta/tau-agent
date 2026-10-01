# The phone app (docs/decisions/0013-phones-connect-to-a-running-tau.md),
# built without cargo-ndk or a networked Gradle:
#
# - `lib`: tau-phone cross-compiled for arm64 Android (API 31) with the
#   NDK's clang, as `lib/arm64-v8a/libtau_phone.so`, next to the NDK's
#   `libc++_shared.so`. Luau needed it, and the phone links Luau no
#   longer (tau-ui-remote leaves tau-codemode's host half out); it stays
#   until an APK without it is shown to load.
# - `apk`: the Gradle project, offline, with those libraries as its
#   jniLibs, signed with the debug key committed beside it.
#
# Gradle's dependencies come from `crates/tau-phone/android/deps.json`,
# recorded by `scripts/update-android-deps.sh`; run it after changing a
# dependency or a plugin version in the Gradle files.
{
  lib,
  stdenvNoCC,
  writeShellApplication,
  gradle_9,
  jdk21,
  androidRust,
  androidSdk,
  ghosttyVendor,
  cargoLock,
  makeRustPlatform,
}:

let
  sdk = "${androidSdk}/libexec/android-sdk";
  ndkVersion = "29.0.14206865";
  ndk = "${sdk}/ndk/${ndkVersion}";
  ndkBin = "${ndk}/toolchains/llvm/prebuilt/linux-x86_64/bin";
  # minSdk in app/build.gradle.kts.
  api = "31";
  target = "aarch64-linux-android";
  abi = "arm64-v8a";

  rustPlatform = makeRustPlatform {
    cargo = androidRust;
    rustc = androidRust;
  };

  lib' = rustPlatform.buildRustPackage {
    pname = "tau-phone-lib";
    version = "0.1.0";

    # The Rust sources only: the Gradle project, and whatever Gradle left
    # in it, stays out.
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.difference (lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../crates
      ]) ../crates/tau-phone/android;
    };

    inherit cargoLock;

    nativeBuildInputs = [ ghosttyVendor.zig ];

    # The sqlx query metadata is committed; no database at build time.
    SQLX_OFFLINE = "true";
    inherit (ghosttyVendor) GHOSTTY_SOURCE_DIR GHOSTTY_ZIG_SYSTEM_DIR;

    # What cargo-ndk sets: the NDK's clang for the target's C and C++
    # (cc-rs reads the target's own variables before CC and CXX, so the
    # host's compiler still builds the build scripts) and as the linker.
    CARGO_BUILD_TARGET = target;
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "${ndkBin}/${target}${api}-clang";
    CC_aarch64_linux_android = "${ndkBin}/${target}${api}-clang";
    CXX_aarch64_linux_android = "${ndkBin}/${target}${api}-clang++";
    AR_aarch64_linux_android = "${ndkBin}/llvm-ar";
    ANDROID_NDK_HOME = ndk;
    ANDROID_NDK_ROOT = ndk;

    # buildRustPackage builds for the host; this is a cross build.
    buildPhase = ''
      runHook preBuild
      export HOME="$TMPDIR"
      # Panic locations in the standard library's generic code name its
      # sources in the toolchain's store path, which would drag the
      # toolchain into the library's closure, and the APK's.
      rustSrc="$(realpath "$(rustc --print sysroot)/lib/rustlib/src/rust")"
      export RUSTFLAGS="''${RUSTFLAGS:-} --remap-path-prefix=$rustSrc=/rustc/src"
      cargo build --frozen --release --package tau-phone --target ${target}
      runHook postBuild
    '';

    doCheck = false;

    installPhase = ''
      runHook preInstall
      mkdir -p $out/lib/${abi}
      ${ndkBin}/llvm-strip --strip-unneeded \
        -o $out/lib/${abi}/libtau_phone.so \
        target/${target}/release/libtau_phone.so
      cp ${ndk}/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/${target}/libc++_shared.so \
        $out/lib/${abi}/
      runHook postInstall
    '';

    # Android libraries: the host's strip and patchelf have no business
    # with them.
    dontFixup = true;

    meta = {
      description = "tau-phone's native library for arm64 Android";
      platforms = [ "x86_64-linux" ];
    };
  };

  # The APK, with `jniLibs` as its native libraries, or with none, which
  # is enough for recording Gradle's dependencies without building Rust.
  mkApk =
    jniLibs:
    stdenvNoCC.mkDerivation {
      pname = "tau-phone-apk";
      version = "0.1.0";

      src = lib.fileset.toSource {
        root = ../crates/tau-phone/android;
        fileset = lib.fileset.unions [
          ../crates/tau-phone/android/build.gradle.kts
          ../crates/tau-phone/android/settings.gradle.kts
          ../crates/tau-phone/android/gradle.properties
          ../crates/tau-phone/android/debug.keystore
          ../crates/tau-phone/android/app/build.gradle.kts
          ../crates/tau-phone/android/app/src/main/AndroidManifest.xml
          ../crates/tau-phone/android/app/src/main/java
        ];
      };

      nativeBuildInputs = [
        gradle_9
        jdk21
      ];

      mitmCache = gradle_9.fetchDeps {
        pkg = mkApk null;
        data = ../crates/tau-phone/android/deps.json;
      };

      ANDROID_HOME = sdk;
      ANDROID_SDK_ROOT = sdk;
      JAVA_HOME = jdk21.home;

      gradleFlags = [
        "-Dorg.gradle.java.home=${jdk21.home}"
        # Gradle's own aapt2 comes from Maven, linked against a
        # filesystem Nix does not have; the SDK's is patched for it.
        "-Pandroid.aapt2FromMavenOverride=${sdk}/build-tools/36.0.0/aapt2"
      ];
      gradleBuildTask = "assembleDebug";
      # Recording runs the build itself: the default, resolving every
      # configuration, trips over the Android plugin's ambiguous ones,
      # and would miss what the plugin fetches while it builds.
      gradleUpdateTask = "assembleDebug";

      preBuild = ''
        export ANDROID_USER_HOME="$TMPDIR/android-user-home"
      ''
      + lib.optionalString (jniLibs != null) ''
        mkdir -p app/src/main/jniLibs
        cp -r ${jniLibs}/lib/. app/src/main/jniLibs/
        chmod -R u+w app/src/main/jniLibs
      '';

      installPhase = ''
        runHook preInstall
        install -Dm444 app/build/outputs/apk/debug/app-debug.apk $out/tau-phone-debug.apk
        runHook postInstall
      '';

      dontFixup = true;

      meta = {
        description = "tau on an Android phone, as a debug APK";
        platforms = [ "x86_64-linux" ];
        sourceProvenance = with lib.sourceTypes; [
          fromSource
          binaryBytecode # Gradle's dependencies
          binaryNativeCode # the Android SDK's tools
        ];
      };
    };

  apk = mkApk lib';
in
{
  lib = lib';
  inherit apk;

  # `nix run .#tau-phone-install`: installs the APK on the phone adb
  # sees, over the last one.
  install = writeShellApplication {
    name = "tau-phone-install";
    text = ''
      exec ${sdk}/platform-tools/adb install -r "$@" ${apk}/tau-phone-debug.apk
    '';
  };

  # Rewrites deps.json; run from the repository's root.
  updateDeps = apk.mitmCache.updateScript;
}
