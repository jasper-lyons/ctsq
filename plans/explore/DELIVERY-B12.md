# Stage B, packages B1 and B2: delivery note (2026-10-23)

Build branch `worktree-explore-b12`, cut from `explore-stage-b` at 7bde1e0. Tip: the merge of B2 (69388ea) plus this note. Nothing is wired into `resolver_for` yet, so ctsq's output is unchanged.

## What was built

- **Plan amendments (c75dfd4).** One architecture review of `STAGE-B.md`, scoped to B1 and B2, returned 11 findings. All were accepted, and one needed no change. The main ones:
  - `resolve_export` now takes the resolver and an export-table lookup as arguments, because it cannot work from one file alone.
  - B04 is clarified: a call belongs to the innermost def that `DefIndex` lists, as today, so class methods keep their own calls.
  - Call sites carry their exact byte span.
  - The Mavexa test reads an excerpt copied into fixtures, not the Mavexa checkout.
  - Conventions now say that B2's `mod ts;` line is in `mod.rs`, which stage A also owns.
- **B1 ts-imports (61a75f0), in `src/resolve/imports.rs`.**
  - Each import now records its local name, its original name and its kind (named, default or namespace) in `RawImport.import_bindings`.
  - `js_exports(tree, source) -> ExportTable` reads one file's exports.
  - `resolve_export(r, table_of, file, name)` follows re-exports to at most 8 files and stops on a cycle.
  - `js_parse(lang, source)` is a small public parse helper.
- **B2 ts-model (fcbdd1e), in the new `src/resolve/ts.rs`.**
  - `FileScope::build(tree, source, own)` gives one `TsDef` per listed def, each with an export flag and its `CallSite`s.
  - Each `CallSite` has a cleaned `Callee` (Bare, Member with its receiver, or Jsx), a line and a byte span.
  - `Binding`s record a scope byte range. `binding_for(name, byte)` returns the innermost binding of that name.
  - `call_positions()` has the same shape as Go's.

## Gate results, on the merged tip

| Gate | Result |
|---|---|
| `cargo build --release` | no warnings |
| `cargo test` | 826 passed, 0 failed (818 after B1, then 8 more from B2) |
| `check-examples.sh` | skill: 27 examples checked, help: 27 examples checked |
| `same-output.sh` (REF built from 7bde1e0, REFCACHE=0) | `same-output: 161 calls, 0 differences` |

I re-ran every gate myself: on B1's commit before merging it, and on the merged tip.

## Agents

| Agent | Package | Model | Tool calls |
|---|---|---|---|
| plan reviewer | architecture lens, B1 and B2 | Opus | 21 |
| builder | B1 ts-imports | Sonnet | 32 |
| builder | B2 ts-model | Opus | 60 |

## What B3 must know

- **Remove the warning allows.** B1 marks each new item `#[allow(dead_code)] // wired by B3`. B2 has `#![allow(dead_code)]` at the top of `ts.rs`. B3 removes all of them when it wires them.
- **Path keys.** `resolve_export` asks `table_of` for files by the display paths that `JsSpecResolver::resolve` returns. Key the cached export tables by those same paths.
- **Anonymous default exports.** `resolve_export` returns `Some((file, "default"))` for one, not `None`. Match the anonymous function by its position in that file.
- **Local re-exports stop.** A local `export {a}` where `a` was itself imported gives local name `a` in that file. The import is not followed, so B3 must chase it if it matters.
- **What the lookup ignores.**
  - `export * as ns from S` is ignored.
  - `export *` is never followed for `"default"`.
  - A specifier that resolves to more than one file gives `None`.
- **Intrinsic JSX tags.** A lowercase `Callee::Jsx` such as `li` or `div` is an intrinsic HTML tag and should not resolve.
- **Calls outside listed defs.** Calls at module top level or in class field initialisers are dropped. The model also skips `(f)()`, `f()()`, `super()`, `import()` and fragments `<>`.
- **JS exports are not defs.** `tree.rs` `extract_js` does not unwrap `export_statement`, unlike `extract_ts`. In `.js` and `.jsx` files, `export function Foo` and `export const Foo = () =>` are not listed defs, so they get no model entry. That is a pre-existing gap and outside B3's file list. Decide whether to fix it there or record it.
- **The fixture graph pin.**
  - `resolve/graph.rs` pins the call graph over all of `fixtures/` (`GRAPH_PIN`).
  - B2 added `fixtures/ts-resolve/model/.ignore` containing `*` to stay out of it. `fixtures/ts-resolve/imports/` has no `.ignore`, and today it leaves the pin unchanged.
  - Once the TS resolver is wired, both B1's fixtures and B3's new `fixtures/ts-resolve/resolve/` may change the pin. Either update `GRAPH_PIN` on purpose or add an `.ignore`.
- **Grammar detail.** In the JS 0.25 and TSX 0.23 grammars, a dotted JSX tag is a `member_expression`.
- **Branch to cut from.** B3 is cut from a tip with stage A merged. Merge this branch in first. The only file both sides touch is `src/resolve/mod.rs`, where B2 added one line, `pub mod ts;`, to the module list.

## Not done

- Pushing the build branch was refused by the session's permission classifier. The branch is committed locally only.
