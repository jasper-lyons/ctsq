# Handoff: path-setup

Branch worktree-agent-af72e8e25465c180c, cut from afea96e. All checks green at the last commit: cargo build (no warnings), cargo test, check-examples, same-output 0, differential 0, path-differential 0 (934 calls).

## Done
- `LazyModule` reads by binding use a bracket-aware Go token scan (scoped.rs `go_scan`, `binds_here`); struct-field reads (`is_embedded`, `any_field`) use `fields_here`. `objects.rs` `Reads::{All, Binding, Field}`.
- Every Go filter (declare, name, type, bind, field) is a per-file sorted word-id set (`GoDecls`, built once per file in `Words`). Profiling showed the per-name scans (`go_could_bind`, `word_lines`) were the top CPU cost.
- `segment_rows` skips files lacking an equality `qual`/`in` value or `New<value>` (semantic/mod.rs `attr_needles`).
- Email ==>: 2.19s / 793 parses / 254MB to 1.76s / 648 parses / 214MB. register param#r ==>: 1.36s to 1.18s.
- Tried and dropped: keeping LazyFlow's parses alive (`kept`). It removed repeat parses but raised `writeError param#msg <==` RSS from 65MB to 146MB.

## Not done: Email ==> is 1.76s, goal 1.5s
The profile is now spread out: tree-sitter parsing (~650 files), tree walking (`ts_tree_cursor_child_iterator_next`, from `named()` in the engine), malloc. The files parsed first come from: `widen::outside_impls_for` (~165, store.holding of interface method names), `LazyFlow::cover_named` (~150), the walk itself (~125).
Next steps:
1. `widen::outside_impls_for`: give it a Go filter (files that can declare a method of that name: `name_at` / func Name position) instead of `store.holding`.
2. `LazyFlow::cover_named` asks many names. List them with declarer counts (debug print in `cover_named`) and look for names a tighter kind could serve, as `ask_interface` did.
3. `from_utf8` still shows up in the profile (~60 samples). It comes from `holds_any` validating whole files. Check its callers.
