#!/bin/bash
# Re-runs every worked example in SKILL.md and the EXAMPLES in `ctsq --help`
# against this repo with ./target/release/ctsq (build it first). Exit 0 when:
#   - every shown output line of each SKILL.md example appears in the real output
#     (lines that are only "..." are skipped, blank lines are skipped)
#   - every example call in the help EXAMPLES section exits 0
# The logic is in check-examples.py beside this file.
# Override the binary with CTSQ=/path/to/ctsq. Run from anywhere.
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/../../.." || exit 2
export CTSQ="${CTSQ:-$PWD/target/release/ctsq}"
exec python3 -I "$here/check-examples.py" "$here/SKILL.md"
