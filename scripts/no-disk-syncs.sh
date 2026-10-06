#!/bin/sh
# A nextest setup script: tests run with libeatmydata preloaded, which
# turns fsync and its kin into no-ops. Tests write throwaway jj and Git
# repositories, and jj syncs every write to disk; on an SSD those syncs
# were most of what the repository models took. The dev shell names the
# library in TAU_TEST_PRELOAD (on Linux); without it, tests sync as they
# always did.
#
# Tests also run the system's /bin/sh and /bin/bash, which the preload
# reaches too. Where those link another glibc than the library's (on
# Ubuntu, under the dev shell's nix glibc), they fail to start with it,
# so it is left out there.
[ -n "$TAU_TEST_PRELOAD" ] && [ -f "$TAU_TEST_PRELOAD" ] || exit 0
for shell in /bin/sh /bin/bash; do
  if [ -x "$shell" ] && ! LD_PRELOAD="$TAU_TEST_PRELOAD" "$shell" -c true 2>/dev/null; then
    exit 0
  fi
done
echo "LD_PRELOAD=$TAU_TEST_PRELOAD${LD_PRELOAD:+ $LD_PRELOAD}" >>"$NEXTEST_ENV"
