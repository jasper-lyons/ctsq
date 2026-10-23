#!/usr/bin/env bash
# Count how Mavexa's backend Go call edges resolve: by type, by the
# name-only fallback, or unresolved.
#
#   bash plans/explore/edges.sh [CTSQ_BINARY]
#
# The binary is $1, else $CTSQ, else `ctsq` on PATH. $MAVEXA overrides the
# Mavexa checkout. The script cds into $MAVEXA/backend, runs
# `ctsq callgraph --format edges .` with CTSQ_RESOLVE_DEBUG=1, and joins each
# edge with the resolver's stderr lines:
#
#   ctsq-resolve <TAB> class <TAB> caller-id <TAB> target-id|callee <TAB> file:line <TAB> callee
#
# An edge is one `caller -> target` line of the edges output, deduplicated by
# ctsq. It counts as by-type when any call site behind it resolved by type,
# else by-name, else unresolved. Edges with no debug line (a binary without
# the variable, or a language other than Go) count as other.
set -euo pipefail

bin="${1:-${CTSQ:-ctsq}}"
case "$bin" in
  */*) bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")" ;;
esac
mavexa="${MAVEXA:-$HOME/workspace/projects/mavexa/mavexa}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$mavexa/backend"
CTSQ_RESOLVE_DEBUG=1 "$bin" callgraph --format edges . >"$tmp/edges" 2>"$tmp/debug"

awk -F'\t' '
  FILENAME == ARGV[1] {
    if ($1 != "ctsq-resolve") next
    sites[$2]++
    r = ($2 == "by-type") ? 3 : ($2 == "by-name") ? 2 : 1
    key = $3 " -> " $4
    if (r > best[key]) best[key] = r
    next
  }
  {
    line = $0
    sub(/ \?$/, "", line)
    total++
    r = best[line]
    if (r == 3) t++
    else if (r == 2) n++
    else if (r == 1) u++
    else o++
  }
  END {
    printf "edges %d  by-type %d  by-name %d  unresolved %d  other %d\n", total, t, n, u, o
    printf "sites by-type %d  by-name %d  unresolved %d\n", sites["by-type"], sites["by-name"], sites["unresolved"]
  }
' "$tmp/debug" "$tmp/edges"
