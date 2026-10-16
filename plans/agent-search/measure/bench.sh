#!/usr/bin/env bash
# Time the base binary against the new one over three repos.
# BASE defaults to the build's base commit 5f867dc, built out of tree:
#   git archive -o base.tar 5f867dc, untar into $BASE_DIR, cargo build --release there.
# Usage: bash plans/agent-search/measure/bench.sh [RUNS]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
BASE="${BASE:-$HOME/.claude/jobs/30494ad5/tmp/bench/base/target/release/ctsq}"
NEW="${NEW:-$repo/target/release/ctsq}"
exec python3 -I "$here/bench.py" "$BASE" "$NEW" "${1:-5}"
