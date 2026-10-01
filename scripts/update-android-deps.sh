#!/usr/bin/env bash
# Records the phone's Gradle dependencies into
# crates/tau-phone/android/deps.json, which `nix build .#tau-phone-apk`
# builds from offline.
#
# Run it after changing a dependency, a plugin version or anything else
# Gradle fetches in crates/tau-phone/android. It runs Gradle's build
# through nixpkgs' mitm-cache, which records every file Gradle fetches
# and its hash; it needs the network, and builds no Rust.
#
# Usage: bash scripts/update-android-deps.sh
set -euo pipefail

script_path="$(realpath -- "${BASH_SOURCE[0]}")"
root_dir="$(dirname -- "$(dirname -- "$script_path")")"
cd "$root_dir"

update="$(nix build --no-link --print-out-paths .#tau-phone-update-deps)"
# The script writes deps.json relative to the current directory.
"$update"
