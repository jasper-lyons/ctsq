# Handoff: path-setup

Branch worktree-agent-ae75ad35f3687fd53, cut from 47c38ee. All checks green at the last commit: cargo build (no warnings), cargo test, check-examples, same-output 0, differential 0, path-differential 0 (934 calls, 18 routes added).

## Done
- Lazy `TreeStore` (resolve/mod.rs): parse on `get`, `holding(names)` by the shared word index, `paths`, `source`, `file_of`, `finder`. `segment_rows` checks the grammar and needles before parsing.
- Call routes (main.rs `scoped_call_routes`, calls.rs `scope_names`, `evaluate_calls_near`): scoped graph rooted at the end sets, `up = --max`; rebuilt at any depth only when a start has no route (the cut count needs it).
- Flow engine: `scoped::FlowGraph` (whole or lazy) replaces `&CallGraph` in flows.rs, upward.rs, values.rs, template.rs; `Module::lazy` reads per name; `widen::outside_impls_for` per interface; every `store.iter()` scan in the engine is `store.holding(&[name])`.
- Go lexical filters (scoped.rs `go_could_declare`, `go_could_name`, `go_could_type`, `go_could_bind`) pick which holders of a name can answer a read; `DefIndex::take_asked_split` and `ask_interface` tell the kinds apart; `constructed` asks nothing once yes.

## Not done: `field#signupRequest.Email ==>` is still ~2.1s (793 parses)
Measured in one run: before 2.11s / after 2.15s, RSS 276MB to 240MB. The walk itself enters about 100 functions (many field users of Email, tests included), each resolved file asks names whose declarers are many (`error` 298 files before the interface split, `Handler`, `WorkspaceStore`, `Time`, `Store`), and files the lazy graph loads are parsed again by the store (parse count above 740): `LazyFlow::load` drops the tree, the store re-parses it later.

Next steps, in order of expected gain:
1. Share parses between `LazyFlow` and the store: let `FlowGraph::lazy` take the store's `Rc<ParsedFile>` (or keep the Rc in `LazyFlow` for files the engine will read). Removes the ~250 repeat parses.
2. Find which `ask` sites still ask type names broadly (`binding_type`/receiver typing in go.rs) and give them their own kind with a tighter filter, as `ask_interface` did. Use `CTSQ_DBG`-style prints in `LazyFlow::cover*` to list the names and declarer counts.
3. `function#login --> function#writeError` is 1.17s with 523 parses: the reverse ball of writeError at depth 12 is most of the backend. Exactness needs every edge into a definition within 12 calls of writeError; only a cheaper per-file read would help.
