#!/usr/bin/env bash
# The three Mavexa checks of the api package (plans/explore/PLAN.md, Package: api).
# They are ignored unit tests in src/api.rs (module api::mavexa). The api takes its
# root as an argument, so the callees check passes <mavexa>/backend explicitly
# and nothing changes directory. MAVEXA overrides the Mavexa root.
# Mavexa is read-only. Exits non-zero when a check fails or Mavexa is missing.
set -euo pipefail
MAVEXA="${MAVEXA:-$HOME/workspace/projects/mavexa/mavexa}"
export MAVEXA
if [ ! -d "$MAVEXA" ]; then
  echo "check-mavexa: no Mavexa at $MAVEXA" >&2
  exit 1
fi
here="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$here"
out="$("${CARGO:-$HOME/.cargo/bin/cargo}" test --release --lib api::mavexa -- --ignored --nocapture 2>&1)" || {
  echo "$out"
  echo "check-mavexa: FAIL" >&2
  exit 1
}
echo "$out" | grep -E '^(start|callers|callees):|^test |^test result' || true
n="$(echo "$out" | grep -c '^test api::mavexa::.* ok$' || true)"
if [ "$n" -ne 3 ]; then
  echo "check-mavexa: FAIL, $n of 3 checks ran (is Mavexa present?)" >&2
  exit 1
fi
echo "check-mavexa: ok (3 checks)"
