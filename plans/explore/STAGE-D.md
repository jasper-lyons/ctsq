# explore stage D: library declarations

## Goal

When a call leaves the repo for the Go standard library, a Go module, a browser API or an npm package, ctsq names the call and prints its signature, for example `external net/http.Header.Set  func (h Header) Set(key, value string)`. The return type is carried forward, so the next receiver gets a type too. This is Decision 08 of `plans/explore/PLAN.md` for library boundaries, and that plan's Decisions and Conventions apply. It is built in two steps. First come two standalone readers, D1 and D2, as new files that nothing calls yet, built while stages A and B finish. Then D3 wires them into resolution after A and B have merged.

## Baseline (2026-10-24)

- After stage A, a call on a receiver typed from outside the repo has no target (`resolve_method`, `src/resolve/go.rs:981-1021`). A package function from outside goes to `resolve_pkg_fn` (`go.rs:1106`), which looks at project functions only. Go types are bare strings (`Header`, not `net/http.Header`).
- `config/gomod.rs:110` `is_outside(module, import_path)` decides whether an import is outside the repo. A path whose first segment has no dot is the standard library.
- Environment:
  - GOROOT is `/opt/homebrew/Cellar/go/1.25.5/libexec` (go1.25.5) when `go env` runs with `GOTOOLCHAIN=local`. Without it, Mavexa's `go 1.25.7` line switches `go env` to a toolchain under the module cache. GOMODCACHE is `~/go/pkg/mod`.
  - `config/gomod.rs` parses only `module` and `go` lines. There is no reader for `require` or `replace`. Mavexa's `backend/go.mod:125` has a local-path `replace`.
  - Mavexa's backend imports 65 standard library packages and 71 third-party packages directly. 150 of its 256 modules are in the cache, and it has no `vendor/`.
  - Mavexa's `frontend/node_modules` exists. `lib.dom.d.ts` is at `node_modules/typescript/lib/` (39,429 lines). solid-js 1.9.13 uses an `exports` map with nested conditions. @solidjs/router 0.16.1 falls back to the top-level `types` field.
- Parse cost, with no cache: `ctsq tree` over `net/http/server.go` (4.1k lines) takes 0.016s, and over `lib.dom.d.ts` 0.063s. All of `net/http` is about 0.1s.
- tree-sitter parses `.d.ts` cleanly: bodiless signatures, overloads, `declare`, interfaces. `declare module`, `declare global` and `export =` are untested.

## Decisions

### D01. Declarations only, read on demand, kept in memory for one run

The readers parse declarations and skip bodies. Each package is parsed at most once per process and held in memory, never on disk. Why: Decision 10 rules out a cache, and the measured parse cost makes one unnecessary.

### D02. Go: the toolchain on PATH, vendor first, never download

- GOROOT and GOMODCACHE come from `go env`, once per run, so they match the `go` on PATH. Fixtures pass explicit roots instead. (Plan review, 2026-10-24: `go env` runs with `GOTOOLCHAIN=local`, because otherwise a project's `go` line can make it download a toolchain.)
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
- D1 and D2 add only new files under `src/decls/` and their fixtures. The orchestrator commits the shared scaffold before either launches: `pub mod decls;` in `src/lib.rs`, and `src/decls/mod.rs` holding `TypeRef` (the struct and its doc comment are the type's contract). Neither package changes `TypeRef`. A change it needs is a blocker. In `mod.rs`, D1 inserts `pub mod go;` directly after the module doc comment at the top. D2 appends `pub mod dts;` as the file's last line. Those two edits are far apart, so the merges stay clean. D1 merges first.
- Every struct and enum field in the API sections below is `pub`. Derive `Clone, Debug, PartialEq, Eq` where the fields allow it.
- Performance bounds are measured with `cargo test --release -- --ignored`.
- Tests that read the real GOROOT or Mavexa's `node_modules` are marked `#[ignore]`, so the default `cargo test` stays hermetic. The handoff reports their results, run with `cargo test -- --ignored`.

## Work packages

| Order | Package | Tier | Depends on |
|---|---|---|---|
| D1 | go-decls | Opus | |
| D2 | dts-decls | Opus | D1's `src/decls/mod.rs` and `lib.rs` line only. It may run in parallel and merge second |
| D3 | wiring | detailed after A and B merge | A, B, D1, D2 |

## Package: go-decls

Files: new `src/decls/go.rs`, the `pub mod go;` line in `src/decls/mod.rs` (see Conventions), fixtures under `fixtures/decls/go/`, and tests beside the code.

API:
- `pub struct GoRoots { goroot, modcache, go_mod: Option<PathBuf>, goos, goarch, cgo: bool, go_version }` and `GoRoots::detect(project_dir)`. It runs `go env` once, with `GOTOOLCHAIN=local`, and returns None when there is no `go`. `go_mod` is the project's `go.mod`, found with `config::gomod::find_go_mod`. `vendor/` is the directory beside it.
- go.mod reading: D1 writes its own `require` and `replace` parser in `go.rs`, covering block and single-line forms and `// indirect`. A `replace` to a local path resolves relative to go.mod's directory. A `replace` to another module swaps the module path and version used for the cache.
- `pub fn package_dir(&GoRoots, import_path) -> Option<PathBuf>`:
  - the standard library is `GOROOT/src/<path>`
  - a module is found by longest-prefix match against the `go.mod` requires, giving `<cache>/<module>@<version>/<rest>`, with the cache's `!` escaping for uppercase letters
  - `vendor/` is checked first
- `pub fn read_package(dir, &GoRoots) -> PkgDecls`, following D02's file rules. Build constraints: the true tags are `goos`, `goarch`, `unix` when goos is a unix one, `gc`, `go1.1` through `go_version`, and `cgo` when CGO_ENABLED=1. Every other tag is false, so `ignore` and `nethttpomithttp2` are false. `//go:build` lines are evaluated in full, with `&&`, `||`, `!` and parentheses. A file that imports `"C"` is read like any other file.
- Qualifying types: a selector type such as `io.Reader` is mapped through its file's imports. The package name is the import's alias if it has one. Otherwise it is the `package` clause of the target package, read lazily. If that package cannot be found, it is the last path segment with any `/vN` or `.vN` suffix stripped. An unqualified named type gets the package's own import path. Builtins and type parameters get `pkg` None.
- `pub enum GoKind { Func, Method, Field, Type, Var, Const }`
- lookups on `PkgDecls`:
  - `func(name)`
  - `method(type, name)`, including methods promoted one level through an embedded struct field or an embedded interface in the same package. An embedded type from another package is not followed: the lookup returns None, and `field(type, TypeName)` returns the embedded field
  - `field(type, name)`
  - `type_decl(name)`: the `type` declaration itself
  - `type_of(name)`: the `TypeRef` of a package-level var or const (for example, `DefaultClient` is `*net/http.Client`)
- `pub struct GoDecl { import_path, recv: Option<String>, name, kind: GoKind, sig: String, params: Vec<(String, TypeRef)>, results: Vec<TypeRef>, file: PathBuf, line: usize }`
- `TypeRef` is the scaffold's, in `src/decls/mod.rs`.

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
- a declaration that comes after a function whose body has a brace-balanced syntax error is still read
- `//go:build ignore` and `nethttpomithttp2` files are skipped, and a `!js` file is kept
- `io.Reader` in a param qualifies to `io`, an aliased import qualifies to its path, and `map[string][]string` becomes the composite form in the `TypeRef` doc comment
- a local-path `replace` resolves
- an ignored test against the real GOROOT finds `Client.Do` in `net/http` at `client.go:586`, and reads the package in under 200ms with `--release`
- the gates pass, with `same-output.sh` at 0 differences

## Package: dts-decls

Files: new `src/decls/dts.rs`, the `pub mod dts;` line appended to `src/decls/mod.rs` (see Conventions), fixtures under `fixtures/decls/dts/`, and tests beside the code. Parse with `tree_sitter_typescript::LANGUAGE_TYPESCRIPT`, as `src/compiler/lang/typescript.rs:13` does.

API:
- `pub fn resolve_package(from_dir, specifier) -> Option<PathBuf>`, following D03, with subpath exports. Those cover exact keys and patterns with a single `*`. The condition walk order is `types`, `import`, `default`, then the remaining conditions in object order.
- Relative specifiers: `./x.js` maps to `./x.d.ts`, then `./x/index.d.ts`. An extensionless specifier is tried the same way.
- `pub fn lookup(entry, name) -> Option<DtsDecl>`. It follows `export {..} from` and `export *` (at most 8 deep, with a visited set), `export =`, `declare module "X"` and namespaces. Overloads merge only when they share a name in the same file.
- `pub fn member(entry, type, name) -> Option<DtsDecl>`:
  - it merges every `interface X` declaration of the name, and prefers the interface over a `declare var X` of the same name
  - it follows a type alias to an object type or an intersection, and returns None for any other alias shape
  - it walks `extends` chains, for interfaces and for classes (`extends` and `implements`)
- `pub fn lib_dom(node_modules) -> Option<PathBuf>`, and global lookups in it
- `pub enum DtsKind { Function, Class, Interface, TypeAlias, Var, Namespace, Module, Method, Property }`
- `pub struct DtsDecl { name, kind: DtsKind, sigs: Vec<String>, ret: Vec<TypeRef>, file: PathBuf, line: usize }`. `ret[i]` is the return type of `sigs[i]`. For a property or var, it is the declared type. A `TypeRef`'s `pkg` is the bare specifier of the package the type is declared in (`solid-js`), or None for lib.dom globals and builtins.

Fixtures:
- a fake `node_modules` holding `solid-js` (an `exports` map with nested conditions, and a barrel that uses `export *` and `.js` specifiers that map to `.d.ts`)
- `@solidjs/router` (top-level `types` only)
- `@types/scope__name`, for a scoped package with no types of its own (D03 rule 4). Node built-ins such as `fs` and `node:fs` are out of scope, because `@types/node` declares them as ambient modules reached through triple-slash references.
- a small `typescript/lib/lib.dom.d.ts` with `Node` and an `Element extends Node`
- a `declare module` file and an `export =` file

Acceptance:
- `createSignal` from `solid-js` returns at least 2 overload signatures, reached through the barrel
- `@solidjs/router` resolves through `types`
- an `exports` condition wins over top-level `types`
- `Element.appendChild` is found through `Node`
- an `export *` cycle terminates
- `declare module` and `export =` resolve
- `@scope/name` resolves through `@types/scope__name`
- a subpath export (`solid-js/web` or similar in the fixture) resolves
- an ignored test against Mavexa's `frontend/node_modules` resolves `solid-js` `createSignal` and `@solidjs/router` `useNavigate`
- the gates pass, with `same-output.sh` at 0 differences

## Plan review (2026-10-24)

One architecture-lens reviewer (Opus) checked D1 and D2 against d40334d and the environment. All 13 findings were accepted and are folded in above:
1. `go env` can switch toolchains (Mavexa says `go 1.25.7`): it now runs with `GOTOOLCHAIN=local` (D02, Baseline).
2. No go.mod `require`/`replace` reader exists: D1 writes one, local `replace` included.
3. Build tags were undefined: the true set is named, and evaluation is in full.
4. `TypeRef` could not carry `[]byte` or `map[string][]string`: `elems` added, and the composite forms are fixed in the scaffold's doc comment.
5. Selector types such as `io.Reader` had no qualification rule: the rule is named.
6. `kind` and `type_of` were vague: `GoKind` was added, plus `type_decl` and the rules for promotion.
7. "Bodies never parsed" cannot be tested, since tree-sitter parses whole files: reworded.
8. 200ms in a debug test build is unrealistic: bounds are measured with `--release`.
9. D2 could not compile without D1's `mod.rs`: the orchestrator commits a shared scaffold, and the two `pub mod` lines go at opposite ends of the file.
10. and 11. How `sigs` and `ret` relate, `pkg` for TS, `DtsKind`, and alias and merged-interface handling in `member`.
12. `@types/node` cannot be reached by D03's rules: replaced by an `@types/scope__name` fixture, and Node built-ins are out of scope.
13. Specifier mapping, subpath patterns and condition order are named.
