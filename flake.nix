{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # The toolchain and the package builder: Rust's own release
    # manifests, and a builder that keeps compiled dependencies in a
    # derivation of their own.
    rust-dev-tools = {
      url = "github:cfcosta/rust-dev-tools.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # libghostty-vt, the terminal emulator behind heiko-tools' `terminal`
    # feature (docs/decisions/0010-terminal-rendering.md). Pinned at the
    # Ghostty commit that the `libghostty-vt-sys` revision in Cargo.toml
    # builds against (GHOSTTY_COMMIT in its build.rs). The two must move
    # together: a bindings bump that pins a new commit needs this input
    # bumped in the same change. Its own nixpkgs and Zig are kept, so its
    # Zig build matches what Ghostty tests.
    ghostty.url = "github:ghostty-org/ghostty/108cacb17ed8990840cd14abfce43d8ed8503ca1";
    # The jj CLI the tests compare heiko-vcs-host with: it must be the
    # version of the jj-lib in Cargo.lock (0.47.0), so the two move
    # together. A nixos-unstable revision whose jujutsu is that version;
    # only the dev shell uses it, and only through `HEIKO_TEST_JJ`.
    nixpkgs-jj.url = "github:NixOS/nixpkgs/f9a75f41be3a74eab55d3a7679d34ad6cc0da977";
  };

  outputs =
    {
      ghostty,
      nixpkgs,
      nixpkgs-jj,
      rust-dev-tools,
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
              pkgs = import nixpkgs { inherit system; };

              # Stable from rust-toolchain.toml, with rustfmt from a pinned
              # nightly: rustfmt.toml's import options are nightly-only.
              # The linker stays the platform default.
              setup =
                rust:
                rust-dev-tools.setup pkgs {
                  name = "heiko";
                  inherit rust;
                  overrides = {
                    linux.useMold = false;
                    darwin.useLLD = false;
                  };
                };
              rdt = setup (
                rust-dev-tools.version.fromToolchainFile ./rust-toolchain.toml
                // {
                  rustfmt = rust-dev-tools.version.fromToolchain "nightly" "2026-08-04";
                }
              );
              inherit (rdt) rust;

              # The phone build (docs/decisions/0013-phones-connect-to-a-running-heiko.md):
              # the same toolchain with the Android target, and an Android
              # SDK with the NDK. The SDK is unfree and needs its license
              # accepted, so it gets a nixpkgs of its own, and only the
              # opt-in `android` dev shell and the phone's packages pull it
              # in.
              androidRust =
                (setup (
                  rust-dev-tools.version.fromToolchainFile ./rust-toolchain.toml
                  // {
                    targets = [ "aarch64-linux-android" ];
                  }
                )).rust;
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

              # heiko (GPUI) links xcb and xkbcommon, and loads Vulkan,
              # Wayland and X11 at runtime.
              guiLibs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
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
                zig = ghostty.inputs.zig.packages.${system}."0.17.0";
              };

              # Shared by every Rust package: the git dependencies' hashes.
              cargoLock = {
                lockFile = ./Cargo.lock;
                outputHashes = {
                  "gpui-pre-mobile-0.1.0" = "sha256-OsZGo2xypJdkXHT0Fng1yVAuVG7ZeVnhWSFONTZ3IGk=";
                  "libghostty-vt-0.2.1" = "sha256-e5YkGJxvKF6kNaFlRC9M1VCftcKrk3YgnirkW/EXlCQ=";
                };
              };

              # Compiled dependencies live in `heiko.cargoArtifacts`, so a
              # change to heiko's own crates rebuilds only them.
              heiko = rdt.buildRustPackage {
                pname = "heiko";
                version = "0.1.0";

                src = pkgs.lib.fileset.toSource {
                  root = ./.;
                  fileset = pkgs.lib.fileset.unions [
                    ./Cargo.toml
                    ./Cargo.lock
                    ./crates
                    # heiko's Luau plugins (ADR 0051).
                    ./plugins
                  ];
                };

                inherit cargoLock;
                cargoPackages = [
                  "heiko"
                  "heiko-plugin"
                ];
                checkPackages = [
                  "heiko-plugin"
                  "heiko-ui"
                ];
                # Each test in a process of its own, as everywhere else: a
                # crate's tests are one binary, and some keep state for
                # the process (docs/reference/testing.md).
                useNextest = true;

                # The sqlx query metadata is committed; no database at build time.
                env.SQLX_OFFLINE = "true";

                # Cargo's unit graph, so each registry crate comes ready-made
                # from a derivation of its own, shared with any build that
                # compiles it the same way: `nix run
                # github:cfcosta/rust-dev-tools.nix#plan` after dependency
                # changes.
                unitPlan = ./rust-units.json;
                # Planned crates get only the native inputs they declare.
                crateOverrides.yeslogic-fontconfig-sys = _: { buildInputs = [ pkgs.fontconfig ]; };

                nativeBuildInputs = [
                  pkgs.pkg-config
                  pkgs.makeWrapper
                  pkgs.copyDesktopItems
                  pkgs.librsvg
                ];
                buildInputs = guiLibs ++ [ libghostty-vt ];
                # The tests make fixture repositories with git and push to
                # them.
                nativeCheckInputs = [ pkgs.git ];

                # The menu entry. It and the icon are named for the
                # window's app id, `heiko-ui`, so the desktop matches the
                # window to them.
                desktopItems = [
                  (pkgs.makeDesktopItem {
                    name = "heiko-ui";
                    desktopName = "Heiko";
                    comment = "Work with coding agents on your repositories";
                    exec = "heiko";
                    icon = "heiko-ui";
                    categories = [ "Development" ];
                    startupWMClass = "heiko-ui";
                  })
                ];

                # heiko's Luau plugins, which it reads at startup (ADR 0051), and
                # the icon, drawn as scalable and at the sizes menus look
                # for first.
                postInstall = ''
                  mkdir -p $out/share/heiko
                  cp -r plugins $out/share/heiko/plugins
                  icon=crates/heiko/assets/heiko-ui.svg
                  install -Dm644 $icon $out/share/icons/hicolor/scalable/apps/heiko-ui.svg
                  for size in 16 24 32 48 64 128 256 512; do
                    dir=$out/share/icons/hicolor/''${size}x''${size}/apps
                    mkdir -p $dir
                    rsvg-convert -w $size -h $size $icon -o $dir/heiko-ui.png
                  done
                '';

                # GPUI opens Vulkan, Wayland and X11 with dlopen, so the
                # binary needs them on its library path. jj-lib's push
                # runs `git`, the one thing heiko runs it for (ADR 0023).
                # An agent's commands run in `bwrap` (ADR 0037).
                postFixup = ''
                  wrapProgram $out/bin/heiko \
                    --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath guiLibs} \
                    --prefix PATH : ${
                      pkgs.lib.makeBinPath [
                        pkgs.git
                        pkgs.bubblewrap
                      ]
                    }
                '';

                # What proves the package has its plugins: CI does not build
                # it, so this runs on a hand-run `nix build` (ADR 0051). No
                # display is needed: these answer before a window opens.
                doInstallCheck = true;
                installCheckPhase = ''
                  runHook preInstallCheck
                  $out/bin/heiko-plugin check
                  $out/bin/heiko-plugin test
                  test -f $out/share/heiko/plugins/heiko-goal/fixtures/working.json
                  test -f $out/share/heiko/plugins/examples/word-count/heiko.toml
                  runHook postInstallCheck
                '';

                meta = {
                  description = "A GPUI interface for heiko agents";
                  mainProgram = "heiko";
                  platforms = pkgs.lib.platforms.linux;
                };
              };

              # The phone's packages: `nix build .#heiko-phone-apk`. Only on
              # x86_64-linux, the one host the NDK ships prebuilt for.
              phone = pkgs.callPackage ./nix/heiko-phone.nix {
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
                      "crates/heiko-phone/android/deps.json"
                      # Written by `nix run github:cfcosta/rust-dev-tools.nix#plan`.
                      "rust-units.json"
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
                heiko
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
          heiko,
          ...
        }:
        pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          inherit heiko;
          default = heiko;
        }
        // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          heiko-phone-lib = phone.lib;
          heiko-phone-apk = phone.apk;
          # `scripts/update-android-deps.sh` builds and runs it.
          heiko-phone-update-deps = phone.updateDeps;
        }
      );

      apps = forEachSupportedSystem (
        {
          phone,
          pkgs,
          system,
          heiko,
          ...
        }:
        pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          default = {
            type = "app";
            program = pkgs.lib.getExe heiko;
          };
        }
        // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          heiko-phone-install = {
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
          system,
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
              name = "heiko-agent-android";

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
              # libheiko_phone.so needed libc++_shared.so at load time. The
              # phone links Luau no longer (heiko-ui-remote leaves
              # heiko-codemode's host half out); this keeps cargo-ndk
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
            name = "heiko-agent";

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
                # heiko-direnv's test of the real direnv runs it.
                direnv
                jq
                sqlx-cli
              ]
              # An agent's commands run in `bwrap` (ADR 0037); its tests
              # run real ones.
              ++ pkgs.lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.bubblewrap
              ++ guiLibs
              ++ [ libghostty-vt ];

            nativeBuildInputs = [ pkgs.pkg-config ];

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiLibs;
            # What nextest preloads into tests so they skip disk syncs
            # (scripts/no-disk-syncs.sh).
            HEIKO_TEST_PRELOAD = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "${pkgs.libeatmydata}/lib/libeatmydata.so";
            # The jj CLI at jj-lib's version, which heiko-vcs-host's tests
            # compare heiko with. Not on `PATH`: the shell's `jj` stays the
            # person's own.
            HEIKO_TEST_JJ = "${nixpkgs-jj.legacyPackages.${system}.jujutsu}/bin/jj";
          };
        }
      );
    };
}
