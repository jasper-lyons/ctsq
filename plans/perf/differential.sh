#!/usr/bin/env bash
# Compare callers, callees and explain at depth 1, 3 and 6 for 20+ names per
# repo between the reference binary and ./target/release/ctsq. Logic in
# differential.py. Override with REF=/path/to/ctsq NEW=/path/to/ctsq JOBS=8.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
REF="${REF:-$HOME/.claude/jobs/30494ad5/tmp/pc-ref/target/release/ctsq}"
NEW="${NEW:-$repo/target/release/ctsq}"
exec python3 -I "$here/differential.py" "$REF" "$NEW" "${JOBS:-8}"
