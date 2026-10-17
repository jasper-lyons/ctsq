#!/bin/bash
# usage: plans/perf/profile.sh [calls-file] [name-filter]   -> markdown table on stdout
# Builds a release binary with debug symbols into $PROF_OUT (default: ./target/prof-out), then
# times (median of 3), parse-stats and samples each call in calls.txt.
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
calls="${1:-$here/calls.txt}"
out="${PROF_OUT:-$repo/target/prof-out}"
mkdir -p "$out"
export CARGO_PROFILE_RELEASE_DEBUG=true
(cd "$repo" && ~/.cargo/bin/cargo build --release >&2) || exit 1
cp "$repo/target/release/ctsq" "$out/ctsq-prof"
rm -rf "$out/samples"
python3 -I "$here/profile.py" "$out/ctsq-prof" "$calls" "$out" "$2"
