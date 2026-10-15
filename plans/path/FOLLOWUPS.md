# ctsq path follow-ups

Minor findings from the mid-build adversarial review (2026-10-15) that were not fixed in this build.

- [ ] `--body` lines on call routes have no `path:line` prefix (decision 06), and `--body` is ignored on flow queries. `src/semantic/print.rs`
- [ ] A sibling module reached through a `replace` directive is walked into with an end selector but tagged `outside` in open-end mode. Running from a subdirectory tags in-module calls as `outside`. Apply one module-path rule in both modes, label unscanned in-module code "not scanned". `src/semantic/flows.rs` `is_outside_call`
- [ ] `select { case ch <- x: }` gives no `stopped: shared channel` row, a plain `ch <- x` does. `src/flow/engine.rs` send handling
- [ ] Local maps stop as `stopped: shared map` with no readers listed. Globals list theirs (decision 12)
- [ ] A result with 0 routes still prints the root row. Multi-line calls show only their first line, hiding the tracked argument
- [ ] Nested `if` depth 100/200/300 takes 0.4/2.4/7.4 s on a debug build (roughly O(n^2.7)). A 5000-line file takes 0.3 s
- [ ] Call route rows drift out of column alignment at deeper indents (`print.rs` pads before indenting)
- [ ] `c := b` copies have no resolved type, so `field#Contact.Phone` still matches `c.Phone` when the owner is unknown (fixer A gap b)
- [ ] Pointer detection reads source text (`*` prefix, `&`, `new(`) instead of mechanics keys for unary and result nodes (fixer A gap c)
- [ ] Call routes: a start with some routes inside `--max` and longer ones beyond it is not flagged as cut (fixer B)
- [ ] The flow effect memo key has no call depth, so an effect computed at a cut depth can be reused at a shallower one (fixer B, pre-existing)
- [ ] The upward walk (`src/flow/upward.rs`) names tree-sitter-go node kinds in a const list instead of reading Mechanics (decision 20). Field reads only go back to stores in the same function or the composite literal, not into callers (flow-up)
- [ ] Constructed types are recorded through `Resolver::collect_impls` with a sentinel trait name, which re-parses every Go file once more. Give `DefIndex` a real field or cache it on `TreeStore` (widening)
- [ ] A template file entry repeats its first line as two rows (`{{template "layout.html.tmpl" .}}  moved` then the same line `exact, via layout.html.tmpl`). Open-end mode does not tag the `Execute` call itself as `outside` (templates-flow)
