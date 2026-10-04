#!/bin/sh
# A nextest setup script: tests run with libeatmydata preloaded, which
# turns fsync and its kin into no-ops. Tests write throwaway jj and Git
# repositories, and jj syncs every write to disk; on an SSD those syncs
# were most of what the repository models took. The dev shell names the
# library in TAU_TEST_PRELOAD (on Linux); without it, tests sync as they
# always did.
if [ -n "$TAU_TEST_PRELOAD" ] && [ -f "$TAU_TEST_PRELOAD" ]; then
  echo "LD_PRELOAD=$TAU_TEST_PRELOAD${LD_PRELOAD:+ $LD_PRELOAD}" >>"$NEXTEST_ENV"
fi
