#!/usr/bin/env bash
# Compare `ctsq path` routes (call and data-flow) on Mavexa backend and on the
# fixtures the tests/path*.rs suites use, between the reference binary and
# ./target/release/ctsq. Logic in path-differential.py. Override with
# REF=/path/to/ctsq NEW=/path/to/ctsq JOBS=8.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
REF="${REF:-$HOME/.claude/jobs/30494ad5/tmp/pc-ref/target/release/ctsq}"
NEW="${NEW:-$repo/target/release/ctsq}"
exec python3 -I "$here/path-differential.py" "$REF" "$NEW" "$repo" "${JOBS:-8}"
