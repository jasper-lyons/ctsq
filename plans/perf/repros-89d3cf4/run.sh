#!/usr/bin/env bash
# Repros found by the 89d3cf4 review where ./target/release/ctsq differs from
# the reference binary (decision 01). Prints one DIFF line per differing call.
#   named-result-line: path flow (lazy FlowGraph) misses a type constructed only
#     by a func whose named results sit on their own lines (`l *Lazy,`), so
#     interface dispatch drops Lazy.Run. declare_at has no rule for that position.
#   newt-convention: `func NewLazy() any` makes Lazy constructed (go.rs
#     constructed_types, NewT convention), but the file never holds the word
#     Lazy, so the scoped graph and the lazy flow never load it. Breaks
#     callers, callees, explain, path --> and path ==>.
#   semicolon-decl: `var x = 1; func OneLine(...)` and `type T struct{}; func (T) Run`
#     on one line: name_at/declare_at only see a func at line start (non-gofmt).
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
REF="${REF:-$HOME/.claude/jobs/30494ad5/tmp/pc-ref/target/release/ctsq}"
NEW="${NEW:-$repo/target/release/ctsq}"
check() {
  local dir="$1"; shift
  a="$(cd "$here/$dir" && "$REF" "$@" 2>&1; echo "exit $?")"
  b="$(cd "$here/$dir" && "$NEW" "$@" 2>&1; echo "exit $?")"
  [ "$a" = "$b" ] || echo "DIFF $dir: ctsq $*"
}
check named-result-line path 'function#Disp param#s ==> function#Sink' .
check newt-convention path 'function#Disp param#s ==> function#Sink' .
check newt-convention path 'function#Disp --> function#Sink' .
check newt-convention callers Sink .
check newt-convention callers Run .
check newt-convention callees Disp .
check newt-convention explain Disp .
check semicolon-decl path 'function#Entry param#s ==> function#Sink' .
check semicolon-decl path 'function#Disp param#s ==> function#Sink' .
