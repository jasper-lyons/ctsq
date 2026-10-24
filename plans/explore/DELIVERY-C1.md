# Delivery: stage C, package C1 (ts-mech)

Branch `explore-c1`, cut from `explore-stage-c` at c1dd3e9. Commits: 83ba311 (STAGE-C.md amended from the plan review), 9b250fa (C1). Nothing changes for a user yet: no command reads the TS mechanics, the TS models or `ts_sig` until C2 and C3 wire them in.

## What's been built

- **TS, TSX and JS flow mechanics.** The three language files (`config/languages/typescript.toml`, `tsx.toml` and `javascript.toml`) now have `[nodes]`, `[fields]` and `[tokens]` sections. `Nodes` (`src/config/mechanics.rs`) gains twelve optional keys: `object`, `pair`, `array`, `jsx_element`, `jsx_self_closing`, `jsx_expression`, `jsx_attribute`, `jsx_text`, `pattern`, `await`, `spread` and `literal`. A key that is empty is left out of `config dump`. `typescript.toml` has no `jsx_*` keys, because the plain TypeScript grammar has no JSX. One test checks that every node kind named in the three files exists in its grammar, and that every field name is a real field.
- **TS models.** There are two new files, `config/models/ts/builtins.toml` and `config/models/ts/solid-js.toml`, with 9 models each. `config::model_dir` sends the languages typescript, tsx and javascript to the model directory `ts`, both for the built-in models and for models on disk.
- **The slot syntax from C03.** A pass string can now say `receiver[*] -> 0.param0`, meaning the receiver's elements flow into parameter 0 of the callback passed as argument 0. `Pass` has two new fields, `from_elements: bool` and `to_param: Option<usize>`, plus a new constructor `Pass::plain`. `Slot` itself is unchanged. Every existing Go model parses to the same passes as before.
- **`src/flow/ts_sig.rs`.** It reads a TS or JS parameter list and gives each name it binds, including names inside destructured parameters, as a path. Nothing calls it yet.

## Gate results (re-run by the orchestrator on the merged tip 9b250fa)

- `cargo build --release`: no warnings
- `cargo test`: 864 passed, 0 failed, 3 ignored
- `check-examples.sh`: skill 27 examples checked, help 27 examples checked
- `same-output.sh` (REF = c1dd3e9 built in the job tmp dir, REFCACHE=0): `same-output: 161 calls, 0 differences`
- `config/languages/go.toml` is byte-identical to c1dd3e9, and `ctsq config dump go` is identical to the reference binary's output (83 lines)
- `config dump tsx` and `config dump javascript` both list `<built-in>/models/ts/builtins.toml (9)` and `<built-in>/models/ts/solid-js.toml (9)`

## What C2 and C3 must know

- **Model keys.** A key is `<import source>.<name>`, for example `solid-js.For` or `solid-js/store.createStore`. A method call whose receiver type is unknown uses `<Global>.prototype.<method>`, for example `Array.prototype.map`. This is in C03.
- **Named JSX arguments use a new optional model key, `args`.** This is a deviation from the contract, and I accepted it. `Model.args: Vec<String>` lists a component's named arguments in index order, and passes refer to them by that index. For example, `solid-js.For` has `args = ["each", "children"]` and the pass `0[*] -> 1.param0`. C3 maps a JSX attribute `name` to `Slot::Arg(position of name in args)`, and maps the element's children to the position of `"children"`. Attributes that are not in `args` are not tracked. A model without `args` is positional, as before.
- **Where the models over-approximate.** The pass vocabulary cannot express four things. C3 should know them and must not treat these models as exact:
  - Nothing can target a callback's return value. So `createMemo`, `map` and `then` use `0 -> return` or `receiver -> return`, which says more flows than really does.
  - Nothing can target one element of a returned tuple. So `createSignal` and `createStore` are modelled as `0 -> return`. `setStore` has no model, because it is not an import and so has no key.
  - `Index` passes each item to its callback wrapped in an accessor (`item()`). It is modelled the same as `For`.
  - `createResource` puts its one-argument and two-argument forms in one entry: `[0 -> 1.param0, 0 -> return, 1 -> return]`.
- **`ts_sig` API.** `TsParam { name, position, path, rest, default, ty, start_byte, end_byte }` and `TsSignature { line, name, params, start_byte, end_byte }`. The functions are `ts_signatures(root, src)`, `ts_signature(func, src)` and `ts_params(formal_parameters, src)`.
  - `{b, c}` at position 1 gives `b` with path `[b]` and `c` with path `[c]`, both at position 1.
  - `{b: x}` gives the name `x` with path `[b]`.
  - Array patterns use index paths (`["0"]`, `["2"]`), and holes are counted.
  - `default` is true for `a = 1`, for `{b = 2}` and for `a?: T`.
  - The file has a file-level `allow(dead_code)` marked "wired by C3". Remove it when C3 calls the module.
- **Parameters: use `ts_sig`, not `nodes.param`.** In JS, `nodes.param` is `formal_parameters`, the whole list, because the JS grammar has no node per parameter. TS and TSX use `required_parameter` and `optional_parameter`.
- **Field names.** `fields.name` includes `pattern` (a TS parameter's name), `fields.params` includes `parameter` (an arrow function's single parameter written without parentheses), and `fields.function` includes `constructor` (for `new`).
- **`await` is a Rust keyword.** The Rust field is `await_`, with a serde rename. The TOML key is `await`.
- **`literal` holds TS kinds only.** Go's literal kinds were left to C2 (C05).

## Process

- Plan review: one architecture reviewer (Opus) returned ten findings. All ten were accepted and recorded in STAGE-C.md under `Plan review (2026-10-24, C1)`.
- Builder: one agent (Opus), 56 tool calls, one commit, merged fast-forward. Its worktree and branch have been removed.
