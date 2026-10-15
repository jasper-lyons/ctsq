# ctsq path follow-ups

Minor findings from the mid-build adversarial review (2026-10-15) that were not fixed in this build.

- [ ] `--body` lines on call routes have no `path:line` prefix (decision 06), and `--body` is ignored on flow queries. `src/semantic/print.rs`
- [ ] A sibling module reached through a `replace` directive is walked into with an end selector but tagged `outside` in open-end mode. Running from a subdirectory tags in-module calls as `outside`. Apply one module-path rule in both modes, label unscanned in-module code "not scanned". `src/semantic/flows.rs` `is_outside_call`
- [ ] `select { case ch <- x: }` gives no `stopped: shared channel` row, a plain `ch <- x` does. `src/flow/engine.rs` send handling
- [ ] Local maps stop as `stopped: shared map` with no readers listed. Globals list theirs (decision 12)
- [ ] A result with 0 routes still prints the root row. Multi-line calls show only their first line, hiding the tracked argument
- [ ] Nested `if` depth 100/200/300 takes 0.4/2.4/7.4 s on a debug build (roughly O(n^2.7)). A 5000-line file takes 0.3 s
- [ ] Call route rows drift out of column alignment at deeper indents (`print.rs` pads before indenting)
