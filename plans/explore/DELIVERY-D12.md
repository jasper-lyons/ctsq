# Delivery: explore stage D, packages D1 and D2

Branch `explore-d12`, built on `explore-stage-d` (d40334d). Nothing calls the new code yet. D3 wires it in.

## What's been built

Two readers that answer "what is this name outside the repo, and what is its signature":

- **Go (`src/decls/go.rs`, D1).** Given an import path, it finds the package directory:
  - the standard library under GOROOT
  - a module through the `go.mod` requires, with `replace` applied, in the module cache with `!` escaping
  - the project's `vendor/`, which is checked first
  - GOROOT's own `src/vendor`

  It reads the package's declarations under the host's build constraints. It returns functions, methods, fields, types, vars and consts with their signatures and with result types qualified by package: `NewRequest` returns `*net/http.Request` and `error`. The roots come from `go env`, run once with `GOTOOLCHAIN=local`, so nothing is ever downloaded.
- **TypeScript (`src/decls/dts.rs`, D2).** Given an importing directory and a specifier, it finds the package's `.d.ts` entry: the `exports` conditions first, then `types`/`typings`, then `main`, then `@types/scope__name`.
  - `lookup` follows re-exports (`export {..} from`, `export *` with a cycle guard), `export =`, `declare module` and namespaces.
  - `member` walks `extends` chains, merging interface declarations.
  - `lib_dom` and `lookup_global` answer DOM globals.
- **Shared type (`src/decls/mod.rs`).** `TypeRef { pkg, name, pointer, elems }`. Its doc comment fixes how slices, maps, channels, variadics and anything unparsed are represented.

## How it affects a user

No CLI behaviour changes. `same-output.sh` shows 0 differences across 161 calls. The new fixtures under `fixtures/decls/go/` carry a `.ignore` file, so they stay out of the pinned call graph over `fixtures/`.

## Gate results (merged tip, run by the orchestrator)

| Gate | Result |
|---|---|
| `cargo build --release` | no warnings |
| `cargo test` | 877 passed, 0 failed, 7 ignored |
| `cargo test --release -- --ignored` | 7 passed, 0 failed |
| `check-examples.sh` | skill 27 checked, help 27 checked, no FAIL |
| `same-output.sh` (REF = d40334d, REFCACHE=0) | `same-output: 161 calls, 0 differences` |

The ignored tests:
- `decls::go::tests::real_goroot_client_do` passed. It finds `Client.Do` at `net/http/client.go:586` in the real GOROOT, checks the `Header.Set` signature, the `NewRequest` results and the `DefaultClient` type, and asserts that reading `net/http` takes under 200ms. The builder measured about 105ms alone and 133ms with other tests running in parallel.
- `decls::go::tests::detect_runs_go_env` passed.
- `decls::dts::tests::mavexa_solid_and_router` passed: `solid-js` `createSignal` and `@solidjs/router` `useNavigate` resolve from Mavexa's `frontend/node_modules`.
- `decls::dts::tests::mavexa_lib_dom_element_append_child` passed. It parses the real 39k-line `lib.dom.d.ts`, in about 100ms.
- The three existing `api::mavexa` tests passed.

## What D3 must know

- **Per-run stores.**
  - Go: `GoPackages::new(roots).get(import_path)` reads each package at most once.
  - TS: `dts.rs` keeps a process-wide map from file to parsed index, so each file is parsed once per process. It does not use `parsed.rs`, because that cache can evict entries.
- **"Not downloaded".** When the module is missing from the cache, `package_dir` returns None. To print `(not downloaded)`, D3 checks whether the import matches a require from `read_go_mod`.
- **Shortened locations (D05).** Go uses `short_location(&roots, file, line)`. TS uses `dts::short_path` and `DtsDecl::location()`.
- **What Go results hold.** A field's result is its type, a type declaration's is its underlying type, and a struct or interface leaves results empty.
  - Interface methods are `GoKind::Method`, with `recv` set to the interface.
  - Embedded fields are `Field` decls named by their bare type name. Their `TypeRef` is also in `PkgDecls.embedded`.
  - `method` and `field` promote one level through embedded types of the same package. An embedded type from another package is not followed: D3 takes the field's `TypeRef` and looks it up in that package.
- **Go `type_of` without a written type.** It infers the type from `&T{}`, `T{}`, a call to a function in the same package, literals, `errors.New`, `fmt.Errorf` and iota groups. Anything else returns None.
- **TS functions bound to a const.** `export declare const useNavigate: () => Navigator` comes back as `DtsKind::Var`, with ret as the text `() => Navigator` and `pkg` None. D3 has to parse that text to get the call's return type.
- **TS gaps.** Call, construct and index signatures and enums are skipped, so a callable interface such as router's `Navigator` has no members for its call form. A re-export under another name keeps the name in its source file: `lookup("memo")` returns `createMemo`.
- **TS `pkg`.** A file under `@types/scope__name` reports `@scope/name`. Files under `node_modules/typescript` (lib.dom) always get None.
- **Duplication left for D3 to remove (D06).**
  - `dts.rs` has its own JSON reader that keeps key order, because the one in `tsconfig.rs` is private.
  - `go.rs` has its own go.mod `require`/`replace` parser, and reuses only `gomod::find_go_mod`.
- **Out of scope.** Node built-ins (`fs`, `node:fs`) need ambient modules reached through triple-slash references in `@types/node`. tsconfig `lib`, `types` and `paths` are not read (D03).

## Process

- One plan review (architecture, Opus). Its 13 findings were all accepted and folded into STAGE-D.md (see `## Plan review (2026-10-24)` there) in be05c95. That commit also adds the shared `TypeRef` scaffold.
- D1 go-decls: Opus, 67 tool calls, commit 65a4794. D2 dts-decls: Opus, 43 tool calls, commit 9296cea. They ran in parallel from be05c95, D1 merged first by fast-forward, then D2 merged cleanly.
- No code review ran on D1 or D2 beyond the gates. D3's review should cover them along with the wiring.
