# explore stage B: TypeScript calls resolved through imports

## Goal

Stage B gives TS, TSX, JS and JSX files a real resolver, so call trees in Mavexa's frontend follow imports instead of matching names. It is Decision 09 of `plans/explore/PLAN.md`, and that plan's Decisions and Conventions apply here. Done means a callees tree from `AssetsRoute` reaches `AssetsIndex`, then `listWorkspaceAssets` in `api.ts`, with no false edges and no name-only matches.

## Baseline (2026-10-23)

These counts come from `ctsq callgraph src --format edges`, run from Mavexa's `frontend/` with the installed ctsq:

| Edges | All | Outside tests |
|---|---|---|
| total | 12,399 | |
| resolved to a definition | 1,659 | 1,480 |
| JSX elements, never resolved | 3,986 | 3,801 |
| bare names, never resolved | 6,698 | 5,967 |

What goes wrong today, all in `fallback::NameResolver` (`src/resolve/fallback.rs`):
- JSX elements record the whole element text (`<Button variant="main" ...`) as the callee, so nothing matches. Multi-line JSX leaves junk callees such as `id}>()`. The patterns are at `src/compiler/lang/typescript.rs:259-320`, with JSX at `:290`.
- Generic calls keep their type arguments: `send<void>`, `get<StoredChatMsg[]>`. They never resolve, even within their own file (`frontend/src/lib/api.ts:436`, `:992`).
- Method calls keep only the property name, so `q.trim()` can match a project function named `trim`.
- Name-only matches print as exact and create false links. In `AssetsIndex.tsx`:
  - `on(...)` at `:105`, imported from solid-js, links to a test file's local `const on` (`Calendar.test.tsx:27`)
  - `q.trim()` at `:106` links to `brandTokens.ts:44`
  - the signal getter `query` at `:105` links to `calendar.ts:112`
- Imported names resolve only when they happen to be unique (`listWorkspaceAssets` → `api.ts:2181`). Components imported through a barrel file never resolve, for example `<Button` from `~/components/ui`, which reaches `primitives.tsx:65` through `export *`.

Import shapes in Mavexa: 1,001 local imports, 628 via `~/`, 369 relative and 4 via `~experiments/`. 14 are default imports, 5 are aliased (`{a as b}`) and none are namespace imports. There are 10 `export *` lines and 47 `export default X` statements.

## Decisions

### B01. One resolver for TS, TSX, JS and JSX

A new `TsResolver` in `src/resolve/ts.rs`, chosen by `resolver_for` (`src/resolve/mod.rs:507`) for all four languages, with a new `FileModel::Ts` variant (`mod.rs:185`). Why: they share import syntax and the import code in `src/resolve/imports.rs` already serves all four.

### B02. Resolution order, with no name fallback

For each call site, take the first of these that applies:
1. a local binding in an enclosing scope (parameter, `const`/`let`, destructuring such as `const [query, setQuery] = createSignal()`, a nested function) → local, no target
2. a definition in the same file
3. an import: a named import (through the alias to its original name), a default import (through the target file's `export default X`), or a barrel chain (`export *`, `export {x as y} from`, at most 8 files deep)
4. an import from a package (`solid-js`, `@solidjs/router`) → external, labelled `solid-js.createSignal`
5. anything else → unresolved

A method call `obj.m()` is unresolved unless `obj` resolves to an import namespace. Why: Decision 08 rules out name matching, and a method call cannot be resolved without types. The `External` label fills stage D's placeholder early for package imports, without a signature.

### B03. Call-site names are cleaned in the model

- A JSX element's callee is its tag name. A dotted tag such as `<Foo.Bar>` is a member call.
- Type arguments are stripped from the callee: `send<void>()` → `send`.
- `new X()` is recorded as a call to `X`.

Why: today's junk callees come from recording raw text. Fixing them in the model means `calltree.rs` and `explain.rs` need no change.

### B04. Calls stay attributed to the top-level definition

A call inside a nested arrow function still belongs to the outermost named definition, as it does today. Amended 2026-10-23 after plan review: "as it does today" is the rule. A call belongs to the innermost enclosing definition that `DefIndex` lists for the file (the `own: &[Def]` that `Resolver::model` receives, built by the recursive `tree::fn_locs_recursive`, `src/tree.rs:639`). Class methods are listed defs today, so `callees m` on a method keeps its own calls. "Outermost" applies only to nested functions that are not listed defs. Nested functions as their own call-graph nodes can wait. Why: it keeps B to resolution, and the browser shows the whole function anyway.

### B05. Left for later

These wait: namespace members (`api.fn()`, none in Mavexa), `this.m()` and class methods, object-literal methods, and the dotted-stem bug in specifier probing (`./api.client` probes `api.ts`, `imports.rs:504`, `tsconfig.rs:322`). These stay unresolved under Decision 08 because they need types: methods on values returned from library calls (`refetch` from `createResource`, `res.json()`), built-in methods (`.map`, `.trim`) and callbacks passed in props (`props.onClose()`).

## Open questions

None yet.

## Conventions

- `plans/explore/PLAN.md` `## Conventions` applies in full: gates, `REF=` and `REFCACHE=0`, never `make install`, Mavexa read-only.
- Before launching, the orchestrator runs one architecture-lens plan review of this file, as the orchestrate skill requires.
- B1 owns no file that stage A edits. B2 adds one line to `src/resolve/mod.rs`, which stage A's strict-calls and structure-out also own: the `mod ts;` line in the module list at `mod.rs:15-22`, which stage A does not touch, so the merge is trivial. Both may run while stage A is in progress, each cut from the `explore-stage-b` tip.
- No warnings before B3: everything B1 and B2 add is unread until B3 wires it, and the crate is binary-only, so `cargo build` would warn `dead_code`. B1 marks each new item `#[allow(dead_code)] // wired by B3`, as at `src/resolve/go.rs:1080`. B2 uses one `#![allow(dead_code)]` at the top of `ts.rs`. B3 removes all of them.
- Unit tests read fixtures through `env!("CARGO_MANIFEST_DIR")/fixtures/...` (as `src/cooccur.rs:182` does). No unit test reads a file outside the repo. B3 launches only after stage A has merged, from a tip that contains both stage A and B1/B2.

## Work packages

| Order | Package | Tier | Depends on | Can start |
|---|---|---|---|---|
| B1 | ts-imports | Sonnet | | now, alongside stage A |
| B2 | ts-model | Opus | | now, alongside stage A |
| B3 | ts-resolve | Opus | B1, B2, stage A merged | after stage A |
| B4 | review | Opus xhigh, read-only | B3 | |
| B4b | review-fixes | Opus | B4 | |

## Package: ts-imports

Files: `src/resolve/imports.rs`, new fixtures under `fixtures/ts-resolve/imports/`, and tests beside the code.

Add to the import model without changing anything that exists:
- `RawImport` (`imports.rs:19`) keeps `bindings` as it is and gains a list of `{local, imported, kind}`, where kind is named, default or namespace, and `imported` is the original name of an aliased `{a as b}`. Fill it in `js_import_bindings` (`:519`) and `js_require_bindings` (`:555`). Every construction goes through `RawImport::new` (`:42`): initialise the list empty there and add a builder beside `bind` (`:51`). The `push` closure at `:457` gains an argument. `const x = require(m)` is a namespace binding, and `const {a, b: c} = require(m)` gives named bindings.
- A per-file export table, `ExportTable`, built by `pub fn js_exports(tree, source) -> ExportTable` for one file. It holds the named exports (`export function f`, `export const x`, `export {a, b as c}`), the default export (its local name, or none for an anonymous `export default function () {}`), and the re-exports (`export * from S`, `export {x as y} from S`, `export {default} from S`, `export {default as X} from S`). Exports with no source need a new arm in the walk around `:463`. That arm must not push a `RawImport`, or the import graph changes.
- `pub fn resolve_export(r: &JsSpecResolver, table_of: impl Fn(&str) -> Option<&ExportTable>, file: &str, name: &str) -> Option<(String, String)>` returns the file and the local name that defines the export. Pass `"default"` as the name for a default import. It follows re-exports through `JsSpecResolver::resolve` (`:260`). The depth limit of 8 files and the visited set for cycles live inside the function. When `resolve` returns more than one candidate file, the result is `None`. B1 builds neither the project-wide resolver nor the tables; B3 builds them and caches them on `DefIndex`. Tests build them over the fixture directory.

Acceptance:
- the fixtures cover a named import, an aliased import, a default import, `export *` through two barrels, `export {x as y} from`, `export {default as X} from`, an anonymous default export, a `require` namespace and destructure, a cycle, and a `~/` alias through a tsconfig, with a unit test each
- `same-output.sh` at 0 differences against the `explore-stage-b` base, because the import graph and `find` must not change
- the other gates pass

## Package: ts-model

Files: new `src/resolve/ts.rs` (not yet registered in `resolver_for`, but declared as a module in `src/resolve/mod.rs` with one `mod ts;` line), new fixtures under `fixtures/ts-resolve/model/`, and tests beside the code.

Builds `ts::FileScope` from a parsed file. It doesn't resolve anything yet:
- the definitions are the `own: &[Def]` list `Resolver::model` receives (today's `DefIndex` entries for the file, from `tree::collect_function_locs`, `src/tree.rs:615`), one entry per def in that order, each with an export flag. `FileScope::build(tree, source, own: &[Def])` takes them as given and does not recompute them.
- each call site holds its def index (the innermost enclosing listed def, B04 as amended), its cleaned callee (B03) as bare, member (with the receiver text, type arguments stripped, `a?.b()` treated as `a.b()`) or JSX, its line, and the start and end byte of the node the TS `call` abstract type matches: `call_expression`, `jsx_self_closing_element` or `jsx_opening_element` (`src/compiler/lang/typescript.rs:259-320`), and `new_expression` for `new X()`. Callers look a site up by that exact byte span.
- local bindings as `Binding { name, kind, scope: Range<usize>, decl_byte: usize }`, where kind is param, const, let, destructure or fn, and `scope` is the byte range of the block or function that holds the binding. A binding encloses a call when the call's start byte lies inside `scope`. It covers parameters, `const`/`let`, array and object destructuring, and nested function declarations. Import bindings belong to B1, not here.
- `call_positions()` with the same shape as `go::FileScope::call_positions` (`src/resolve/go.rs:357`): `(Vec<def line>, HashMap<(start_byte, end_byte), (def idx, call idx)>)`, used at `src/resolve/scoped.rs:1306`, `:1339`

Acceptance:
- unit tests over the fixtures assert the exact call sites (name, kind, line, byte span) and bindings, covering: multi-line JSX with props, a dotted JSX tag, generic calls, a generic member call `api.get<T>()`, an optional call `a?.b()`, `new X()`, a destructured signal pair, a nested arrow function, a class method keeping its own calls, and one `.jsx` fixture parsed with the JavaScript grammar
- an excerpt of Mavexa's `frontend/src/.../AssetsIndex.tsx` (the signal pair at `:102`, `on(query, …)` and `q.trim()` at `:105-106`, the setter call at `:300`, `<Row a={a} />` at `:343`), copied into `fixtures/ts-resolve/model/AssetsIndex.tsx` with the surrounding structure kept. A test asserts that `<Row a={a} />` is a JSX call to `Row` and that `setQuery` is a local binding enclosing the `:300` call
- `same-output.sh` at 0 differences, since nothing is wired yet
- no warnings: `ts.rs` starts with `#![allow(dead_code)]` until B3 wires it, and B3 removes that line

## Package: ts-resolve

Files: `src/resolve/ts.rs`, `src/resolve/mod.rs` (`resolver_for`, `FileModel::Ts`, a cached project import table on `DefIndex` like `go_constructed` at `mod.rs:78`), `src/resolve/scoped.rs` (the TS arm of the `call_positions` reads), new `plans/explore/ts-edges.sh`, fixtures under `fixtures/ts-resolve/resolve/`, tests, and expected outputs in `tests/` and `plans/agent-search/docs/` that change.

Implements B01 and B02 on top of B1 and B2. `resolve_model` receives only the file, its model and the `DefIndex`, so the project's export tables and the `JsSpecResolver` are built once and cached on `DefIndex`.

Acceptance:
- `plans/explore/ts-edges.sh` runs the callgraph from Mavexa's `frontend/` and counts edges by outcome (local, same file, import, external, unresolved). The handoff gives the counts beside the baseline above.
- From Mavexa's `frontend/`:
  - `ctsq callees AssetsRoute src` reaches `AssetsIndex` (`AssetsIndex.tsx`)
  - `ctsq callees AssetsIndex src` includes `listWorkspaceAssets` → `api.ts:2181`, and `Button` → `primitives.tsx:65` through the barrel
  - in `api.ts`, `send<void>` resolves to its definition
  - `createSignal` is `external solid-js.createSignal`
  - the three false edges in Baseline are gone
- `same-output.sh`: differences only in TS, TSX, JS and JSX rows, and the handoff lists every class of difference with an example. Go and Rust rows show 0 differences.

## Package: review

Read-only, Opus at xhigh, in its own worktree. It commits any reproduction fixtures on its own branch. Try to refute:
1. No TS call is resolved by name alone.
2. A local binding always wins over a same-named import or definition.
3. Barrel chains resolve to the defining file, and cycles end.
4. Every Mavexa frontend edge that was resolved before B and is unresolved after B had no import, binding or same-file evidence.
5. Go and Rust output is unchanged.
