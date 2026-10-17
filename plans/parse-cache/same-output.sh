#!/usr/bin/env bash
# Diff stdout, stderr and exit code of a fixed call set between the reference
# binary (built from 1665e83, before the parse cache) and ./target/release/ctsq.
# Prints no DIFF lines when the outputs are identical. Logic in same-output.py.
# Override with REF=/path/to/ctsq NEW=/path/to/ctsq JOBS=4 (parallel calls). Reference output is cached, see plans/perf/refcache.py.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
REF="${REF:-$HOME/.claude/jobs/30494ad5/tmp/pc-ref/target/release/ctsq}"
NEW="${NEW:-$repo/target/release/ctsq}"
exec python3 -I "$here/same-output.py" "$REF" "$NEW" "${JOBS:-1}"
