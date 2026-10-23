# explore stage D: library declarations

## Goal

When a call leaves the repo for the Go standard library, a Go module, a browser API or an npm package, ctsq names the call and prints its signature, for example `external net/http.Header.Set  func (h Header) Set(key, value string)`. The return type is carried forward, so the next receiver gets a type too. This is Decision 08 of `plans/explore/PLAN.md` for library boundaries, and that plan's Decisions and Conventions apply. It is built in two steps. First come two standalone readers, D1 and D2, as new files that nothing calls yet, built while stages A and B finish. Then D3 wires them into resolution after A and B have merged.

## Baseline (2026-10-24)

- After stage A, a call on a receiver typed from outside the repo has no target (`resolve_method`, `src/resolve/go.rs:981-1021`). A package function from outside goes to `resolve_pkg_fn` (`go.rs:1106`), which looks at project functions only. Go types are bare strings (`Header`, not `net/http.Header`).
- `config/gomod.rs:110` `is_outside(module, import_path)` decides whether an import is outside the repo. A path whose first segment has no dot is the standard library.
- Environment:
  - GOROOT is `/opt/homebrew/Cellar/go/1.25.5/libexec` (go1.25.5), and GOMODCACHE is `~/go/pkg/mod`.
  - Mavexa's backend imports 65 standard library packages and 71 third-party packages directly. 150 of its 256 modules are in the cache, and it has no `vendor/`.
  - Mavexa's `frontend/node_modules` exists. `lib.dom.d.ts` is at `node_modules/typescript/lib/` (39,429 lines). solid-js 1.9.13 uses an `exports` map with nested conditions. @solidjs/router 0.16.1 falls back to the top-level `types` field.
- Parse cost, with no cache: `ctsq tree` over `net/http/server.go` (4.1k lines) takes 0.016s, and over `lib.dom.d.ts` 0.063s. All of `net/http` is about 0.1s.
- tree-sitter parses `.d.ts` cleanly: bodiless signatures, overloads, `declare`, interfaces. `declare module`, `declare global` and `export =` are untested.

## Decisions

### D01. Declarations only, read on demand, kept in memory for one run

The readers parse declarations and skip bodies. Each package is parsed at most once per process and held in memory, never on disk. Why: Decision 10 rules out a cache, and the measured parse cost makes one unnecessary.

### D02. Go: the toolchain on PATH, vendor first, never download

- GOROOT and GOMODCACHE come from `go env`, once per run, so they match the `go` on PATH. Fixtures pass explicit roots instead.
- A project `vendor/` directory wins over the module cache.
- A module missing from the cache prints `external pkg.Name (not downloaded)` with no signature. ctsq never runs `go mod download`.
- Build constraints follow the host only: skip `_test.go`, keep only files whose `_GOOS`/`_GOARCH` suffix and `//go:build` line match the host.

Why: the defaults the mapper recommended, adopted by the orchestrator on 2026-10-24. Jasper can overrule any of them.

### D03. TypeScript: package resolution order, and lib.dom only when used

A package's `.d.ts` entry is found in this order:
1. the `types` condition in `exports["."]`, walking nested conditions
2. the top-level `types` or `typings` field
3. `main`, with its extension swapped for `.d.ts`
4. `@types/<name>`, where a scoped package `@scope/name` becomes `@types/scope__name`

Packages are looked up in `node_modules`, starting from the importing file's directory and moving upward. `lib.dom.d.ts` is read only when a global DOM name is referenced and `node_modules/typescript` exists. Otherwise the name stays unresolved. tsconfig `lib` and `types` settings are not read in this stage.

### D04. Results carry their package

Every declaration and every result type names the package it comes from (`TypeRef { pkg, name }`), so `http.NewRequest` returns `*net/http.Request`. Why: return types have to carry forward across the boundary. Bare names would collide with project types.

### D05. External locations print shortened

A declaration's location prints as `$GOROOT/src/net/http/header.go:42`, `$GOMODCACHE/...` or `node_modules/solid-js/types/reactive/signal.d.ts:10`. The full path stays inside the code, so the browser's code pane can open the file.

### D06. D1 and D2 are standalone and duplicate a little

D1 and D2 are new modules under `src/decls/` with their own small declaration parsers. They do not refactor `src/resolve/go.rs`, which stage A and B3 are still changing. D3 removes any duplication when it wires them in. Why: this lets D1 and D2 run in parallel with stage A, with no file overlap.

### D07. D3 waits for stages A and B

Wiring needs a qualified type representation in `Def.ret` and `var_type`, which cuts across code A and B3 are changing. It also needs a new external target variant beside `ResolvedCall.target` (`src/resolve/mod.rs:234`), the printing of `external pkg.Type.Method sig`, and the removal of the flow engine's own name matching (`src/semantic/flows.rs:1041-1058`, `src/flow/widen.rs:54`). D3's package section is written after A and B merge.

## Open questions

- Should tsconfig `lib`, `types` and `paths` steer the `.d.ts` reader? Not in this stage (D03).

## Conventions

- `plans/explore/PLAN.md` `## Conventions` applies in full: gates, `REF=` and `REFCACHE=0`, never `make install`, Mavexa read-only.
- Before launching, the orchestrator runs one architecture-lens plan review of this file, scoped to D1 and D2.
- D1 and D2 add only new files under `src/decls/` and their fixtures. The single shared line is `pub mod decls;` in `src/lib.rs`, owned by D1. D2 adds its module to `src/decls/mod.rs`, which D1 creates, so D2 merges after D1.
- Tests that read the real GOROOT or Mavexa's `node_modules` are marked `#[ignore]`, so the default `cargo test` stays hermetic. The handoff reports their results, run with `cargo test -- --ignored`.

## Work packages

| Order | Package | Tier | Depends on |
|---|---|---|---|
| D1 | go-decls | Opus | |
| D2 | dts-decls | Opus | D1's `src/decls/mod.rs` and `lib.rs` line only. It may run in parallel and merge second |
| D3 | wiring | detailed after A and B merge | A, B, D1, D2 |

## Package: go-decls

Files: new `src/decls/mod.rs`, `src/decls/go.rs`, `src/lib.rs` (one `pub mod decls;` line), fixtures under `fixtures/decls/go/`, and tests beside the code.

API:
- `pub struct GoRoots { goroot, modcache, go_mod: Option<PathBuf> }` and `GoRoots::detect(project_dir)`, which runs `go env` once and returns None when there is no `go`
- `pub fn package_dir(&GoRoots, import_path) -> Option<PathBuf>`:
  - the standard library is `GOROOT/src/<path>`
  - a module is found by longest-prefix match against the `go.mod` requires, giving `<cache>/<module>@<version>/<rest>`, with the cache's `!` escaping for uppercase letters
  - `vendor/` is checked first
- `pub fn read_package(dir) -> PkgDecls`, following D02's file rules
- lookups on `PkgDecls`: `func(name)`, `method(type, name)` (including methods promoted one level through an embedded struct), `field(type, name)`, `type_of(name)`
- `pub struct GoDecl { import_path, recv: Option<String>, name, kind, sig: String, params: Vec<(String, TypeRef)>, results: Vec<TypeRef>, file: PathBuf, line: usize }`
- `pub struct TypeRef { pkg: Option<String>, name: String, pointer: bool }`. `TypeRef` lives in `src/decls/mod.rs` and is shared with D2.

Fixtures:
- a small fake GOROOT with `net/http` (`header.go`, `request.go`, an `x_test.go`, an `x_windows.go`) and `fmt`
- a module cache holding `example.com/lib@v1.2.3` and one module whose path has an uppercase letter
- a `go.mod` with requires, and a `vendor/` variant

Acceptance:
- `Header.Set` has the signature `func (h Header) Set(key, value string)`
- `NewRequest` returns `[*net/http.Request, error]`
- `_test.go` and other-OS files are ignored
- a method promoted through an embedded struct is found
- `@v` paths and `!` escaping resolve
- `vendor/` wins over the cache
- function bodies are never parsed: a fixture whose bodies contain syntax errors still reads cleanly
- an ignored test against the real GOROOT finds `Client.Do` in `net/http` at its real line, and reads the package in under 200ms
- the gates pass, with `same-output.sh` at 0 differences

## Package: dts-decls

Files: new `src/decls/dts.rs`, one `pub mod dts;` line in `src/decls/mod.rs`, fixtures under `fixtures/decls/dts/`, and tests beside the code.

API:
- `pub fn resolve_package(from_dir, specifier) -> Option<PathBuf>`, following D03, with subpath exports
- `pub fn lookup(entry, name) -> Option<DtsDecl>`. It follows `export {..} from` and `export *` (at most 8 deep, with a visited set), `export =` and namespaces. Overloads are merged.
- `pub fn member(entry, type, name) -> Option<DtsDecl>`, which walks `extends` chains
- `pub fn lib_dom(node_modules) -> Option<PathBuf>`, and global lookups in it
- `pub struct DtsDecl { name, kind, sigs: Vec<String>, ret: Vec<TypeRef>, file: PathBuf, line: usize }`

Fixtures:
- a fake `node_modules` holding `solid-js` (an `exports` map with nested conditions, and a barrel that uses `export *` and `.js` specifiers that map to `.d.ts`)
- `@solidjs/router` (top-level `types` only)
- `@types/node`
- a small `typescript/lib/lib.dom.d.ts` with `Node` and an `Element extends Node`
- a `declare module` file and an `export =` file

Acceptance:
- `createSignal` from `solid-js` returns at least 2 overload signatures, reached through the barrel
- `@solidjs/router` resolves through `types`
- an `exports` condition wins over top-level `types`
- `Element.appendChild` is found through `Node`
- an `export *` cycle terminates
- `declare module` and `export =` resolve
- an ignored test against Mavexa's `frontend/node_modules` resolves `solid-js` `createSignal` and `@solidjs/router` `useNavigate`
- the gates pass, with `same-output.sh` at 0 differences
