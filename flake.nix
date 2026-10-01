{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # libghostty-vt, the terminal emulator behind tau-tools' `terminal`
    # feature (docs/decisions/0010-terminal-rendering.md). Pinned at the
    # Ghostty commit that `libghostty-vt-sys` 0.2.2 builds against
    # (GHOSTTY_COMMIT in its build.rs). The two must move together: a
    # crate bump that pins a new commit needs this input bumped in the
    # same change. Its own nixpkgs is kept, so its Zig build matches
    # what Ghostty tests.
    ghostty.url = "github:ghostty-org/ghostty/a887df42c56f6de86c0fe6da9c4eeca37931e083";
  };

  outputs =
    {
      ghostty,
      nixpkgs,
      rust-overlay,
      treefmt-nix,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forEachSupportedSystem =
        f:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          f (
            let
              pkgs = import nixpkgs {
                inherit system;
                overlays = [ (import rust-overlay) ];
              };

              rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

              # The phone build (docs/decisions/0013-phones-connect-to-a-running-tau.md):
              # the same toolchain with the Android target, and an Android
              # SDK with the NDK. The SDK is unfree and needs its license
              # accepted, so it gets a nixpkgs of its own, and only the
              # opt-in `android` dev shell and the phone's packages pull it
              # in.
              androidRust = rust.override { targets = [ "aarch64-linux-android" ]; };
              androidPkgs = import nixpkgs {
                inherit system;
                config = {
                  allowUnfree = true;
                  android_sdk.accept_license = true;
                };
              };
              # What gpui-pre-mobile's Gradle project asks for: compileSdk
              # 34 and Android Gradle Plugin 9 (build tools 36). The NDK
              # links the Rust library (minSdk 31: Android 12).
              androidSdk =
                (androidPkgs.androidenv.composeAndroidPackages {
                  platformVersions = [
                    "34"
                    "35"
                    "36"
                  ];
                  buildToolsVersions = [
                    "34.0.0"
                    "35.0.0"
                    "36.0.0"
                  ];
                  includeNDK = true;
                  ndkVersions = [ "29.0.14206865" ];
                  includeEmulator = false;
                  includeSystemImages = false;
                }).androidsdk;

              # tau-ui (GPUI) links xcb and xkbcommon, and loads Vulkan,
              # Wayland and X11 at runtime.
              guiLibs = pkgs.lib.optionals pkgs.stdenv.isLinux (
                with pkgs;
                [
                  fontconfig
                  freetype
                  libx11
                  libxcb
                  libxcursor
                  libxi
                  libxkbcommon
                  libxrandr
                  vulkan-loader
                  wayland
                ]
              );

              # Found through pkg-config by `libghostty-vt-sys`'s
              # `pkg-config` feature, so cargo never runs git or Zig. Its
              # `dev` output holds the static library and the `.pc` files.
              libghostty-vt = ghostty.packages.${system}.libghostty-vt;

              # Nix builds libghostty-vt for the host only. For the phone,
              # `libghostty-vt-sys` builds it itself from the same Ghostty
              # source, with the Zig and the Zig packages Ghostty pins, so
              # the build never fetches.
              ghosttyVendor = {
                GHOSTTY_SOURCE_DIR = "${ghostty}";
                GHOSTTY_ZIG_SYSTEM_DIR = "${libghostty-vt.deps}";
                zig = ghostty.inputs.nixpkgs.legacyPackages.${system}.zig_0_15;
              };

              rustPlatform = pkgs.makeRustPlatform {
                cargo = rust;
                rustc = rust;
              };

              # Shared by every Rust package: the git dependencies' hashes.
              cargoLock = {
                lockFile = ./Cargo.lock;
                outputHashes = {
                  "docbert-pylate-1.0.0" = "sha256-Wp/raPgyXbaWaGHJIF6aVDLoVqgcA5636GaLHiXKrHc=";
                  "gpui-pre-mobile-0.1.0" = "sha256-OsZGo2xypJdkXHT0Fng1yVAuVG7ZeVnhWSFONTZ3IGk=";
                };
              };

              tau-ui = rustPlatform.buildRustPackage {
                pname = "tau-ui";
                version = "0.1.0";

                src = pkgs.lib.fileset.toSource {
                  root = ./.;
                  fileset = pkgs.lib.fileset.unions [
                    ./Cargo.toml
                    ./Cargo.lock
                    ./crates
                  ];
                };

                inherit cargoLock;
                cargoBuildFlags = [
                  "--package"
                  "tau-ui"
                ];
                cargoTestFlags = [
                  "--package"
                  "tau-ui"
                ];

                # The sqlx query metadata is committed; no database at build time.
                SQLX_OFFLINE = "true";

                nativeBuildInputs = [
                  pkgs.pkg-config
                  pkgs.makeWrapper
                ];
                buildInputs = guiLibs ++ [ libghostty-vt ];
                # The tests make fixture repositories with git; the app
                # itself does not need it.
                nativeCheckInputs = [ pkgs.git ];

                # GPUI opens Vulkan, Wayland and X11 with dlopen, so the
                # binary needs them on its library path.
                postFixup = ''
                  wrapProgram $out/bin/tau-ui \
                    --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath guiLibs}
                '';

                meta = {
                  description = "A GPUI interface for tau agents";
                  mainProgram = "tau-ui";
                  platforms = pkgs.lib.platforms.linux;
                };
              };

              # The phone's packages: `nix build .#tau-phone-apk`. Only on
              # x86_64-linux, the one host the NDK ships prebuilt for.
              phone = pkgs.callPackage ./nix/tau-phone.nix {
                inherit androidRust androidSdk ghosttyVendor;
                inherit cargoLock;
              };

              formatter =
                (treefmt-nix.lib.evalModule pkgs {
                  projectRootFile = "flake.nix";

                  settings = {
                    allow-missing-formatter = true;
                    verbose = 0;

                    global.excludes = [
                      "*.lock"
                      # Generated by `cargo sqlx prepare`; formatting it causes churn.
                      "crates/*/.sqlx/*"
                      # Written by scripts/update-android-deps.sh.
                      "crates/tau-phone/android/deps.json"
                    ];

                    formatter = {
                      nixfmt.options = [ "--strict" ];

                      rustfmt = {
                        package = rust;

                        options = [
                          "--config-path"
                          "${./rustfmt.toml}"
                        ];
                      };
                    };
                  };

                  programs = {
                    nixfmt.enable = true;
                    oxfmt.enable = true;
                    rustfmt = {
                      enable = true;
                      package = rust;
                    };
                    taplo.enable = true;
                  };
                }).config.build.wrapper;
            in
            {
              inherit
                androidRust
                androidSdk
                formatter
                ghosttyVendor
                guiLibs
                libghostty-vt
                phone
                pkgs
                rust
                system
                tau-ui
                ;
            }
          )
        );
    in
    {
      formatter = forEachSupportedSystem ({ formatter, ... }: formatter);

      packages = forEachSupportedSystem (
        {
          phone,
          pkgs,
          system,
          tau-ui,
          ...
        }:
        pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          inherit tau-ui;
          default = tau-ui;
        }
        // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          tau-phone-lib = phone.lib;
          tau-phone-apk = phone.apk;
          # `scripts/update-android-deps.sh` builds and runs it.
          tau-phone-update-deps = phone.updateDeps;
        }
      );

      apps = forEachSupportedSystem (
        {
          phone,
          pkgs,
          system,
          tau-ui,
          ...
        }:
        pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          default = {
            type = "app";
            program = pkgs.lib.getExe tau-ui;
          };
        }
        // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          tau-phone-install = {
            type = "app";
            program = pkgs.lib.getExe phone.install;
          };
        }
      );

      devShells = forEachSupportedSystem (
        {
          androidRust,
          androidSdk,
          pkgs,
          rust,
          formatter,
          ghosttyVendor,
          guiLibs,
          libghostty-vt,
          ...
        }:
        {
          # `nix develop .#android`: the phone build. Kept apart from the
          # default shell, which it would grow by several gigabytes. No C
          # compiler of its own: cargo-ndk points cc-rs at the NDK's clang,
          # and a host `CC` or `NIX_CFLAGS_COMPILE` would win over it.
          android =
            let
              sdk = "${androidSdk}/libexec/android-sdk";
            in
            pkgs.mkShellNoCC {
              name = "tau-agent-android";

              buildInputs = with pkgs; [
                formatter
                androidRust
                androidSdk
                cargo-ndk
                gradle_9
                jdk21
                ghosttyVendor.zig
              ];

              nativeBuildInputs = [ pkgs.pkg-config ];

              ANDROID_HOME = sdk;
              ANDROID_SDK_ROOT = sdk;
              ANDROID_NDK_HOME = "${sdk}/ndk/29.0.14206865";
              ANDROID_NDK_ROOT = "${sdk}/ndk/29.0.14206865";
              JAVA_HOME = pkgs.jdk21.home;
              inherit (ghosttyVendor) GHOSTTY_SOURCE_DIR GHOSTTY_ZIG_SYSTEM_DIR;
              # Gradle's own aapt2 comes from Maven, dynamically linked
              # against a filesystem NixOS does not have; the SDK's is
              # patched for Nix.
              GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${sdk}/build-tools/36.0.0/aapt2";
              # Luau is C++, linked against the NDK's `c++_shared`, so
              # libtau_phone.so needed libc++_shared.so at load time. The
              # phone links Luau no longer (tau-ui-remote leaves
              # tau-codemode's host half out); this keeps cargo-ndk
              # copying it into jniLibs until an APK without it is shown
              # to load.
              CARGO_NDK_LINK_LIBCXX_SHARED = "true";

              # Whatever the host toolchain leaves behind, from Nix's setup
              # hooks or an outer shell, would go to the NDK's clang too.
              shellHook = ''
                unset CC CXX AR LD AS NM RANLIB STRIP OBJCOPY OBJDUMP READELF SIZE STRINGS
                unset CC_FOR_TARGET CXX_FOR_TARGET AR_FOR_TARGET LD_FOR_TARGET AS_FOR_TARGET
                unset NIX_CFLAGS_COMPILE NIX_LDFLAGS NIX_CC_FOR_TARGET NIX_BINTOOLS_FOR_TARGET
              '';
            };

          default = pkgs.mkShell {
            name = "tau-agent";

            buildInputs =
              with pkgs;
              [
                formatter
                rust

                bacon
                cargo-deny
                cargo-mutants
                cargo-nextest
                cargo-watch

                curl
                jq
                sqlx-cli
              ]
              ++ guiLibs
              ++ [ libghostty-vt ];

            nativeBuildInputs = [ pkgs.pkg-config ];

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiLibs;
          };
        }
      );
    };
}
