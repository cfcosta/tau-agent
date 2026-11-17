#!/usr/bin/env bash
# The reflink tests (ADR 0061) on real filesystems: Btrfs, XFS with
# reflinks, and ext4, which cannot share blocks. Run it by hand, from the
# repository's root, in the dev shell. It needs Docker, not sudo: each
# filesystem is a 1 GiB image file, made and mounted as a loop device in
# a privileged container that sees only /nix/store and the image's
# directory. Extra arguments go to the test binary.
#
# On Btrfs and XFS every reflink test must pass. On ext4 the two that
# make real clones must fail, since it cannot clone, and the rest must
# pass, since jj writes every file there.
set -euo pipefail

if [ "${1:-}" = --inside ]; then
  shift
  mkdir -p /tmp /root /mnt
  real=(reflink::tests::a_real_clone_is_independent_of_its_source
    reflink::tests::a_run_on_a_real_reflink_filesystem_clones_its_files)
  status=0
  for fs in btrfs xfs ext4; do
    echo "=== $fs"
    truncate -s 1G "/work/$fs.img"
    case $fs in
      btrfs) mkfs.btrfs -q "/work/$fs.img" ;;
      xfs) mkfs.xfs -q -m reflink=1 "/work/$fs.img" ;;
      ext4) mkfs.ext4 -q "/work/$fs.img" ;;
    esac
    mkdir -p "/mnt/$fs"
    mount -o loop "/work/$fs.img" "/mnt/$fs"
    export HEIKO_TEST_REFLINK_DIR="/mnt/$fs"
    if [ "$fs" = ext4 ]; then
      /work/tests --test-threads 1 reflink:: "${real[@]/#/--skip=}" "$@" || status=1
      for test in "${real[@]}"; do
        if /work/tests --exact "$test" >/dev/null 2>&1; then
          echo "$test passed on ext4, which cannot clone"
          status=1
        fi
      done
    else
      /work/tests --test-threads 1 reflink:: "$@" || status=1
    fi
    umount "/mnt/$fs"
    rm -f "/work/$fs.img"
  done
  exit $status
fi

tests=$(cargo test --release -p heiko-vcs-host --lib --no-run --message-format=json |
  sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1)

path=
for package in bash coreutils util-linux gnugrep git btrfs-progs xfsprogs e2fsprogs; do
  for out in $(nix build --no-link --print-out-paths --inputs-from "path:$PWD" "nixpkgs#$package^*"); do
    [ -d "$out/bin" ] && path=${path:+$path:}$out/bin
  done
done
bash=$(nix build --no-link --print-out-paths --inputs-from "path:$PWD" "nixpkgs#bash^out")/bin/bash

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$tests" "$work/tests"
cp "$0" "$work/test-reflink.sh"

# An empty image: everything the container runs comes from /nix/store.
image=heiko-reflink-empty
docker image inspect "$image" >/dev/null 2>&1 ||
  tar cf - --files-from /dev/null | docker import - "$image" >/dev/null

docker run --rm --privileged \
  -v /nix/store:/nix/store:ro -v "$work:/work" \
  -e PATH="$path" -e HOME=/root -e TMPDIR=/tmp \
  "$image" "$bash" /work/test-reflink.sh --inside "$@"
