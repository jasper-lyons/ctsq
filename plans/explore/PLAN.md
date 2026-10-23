# explore: a terminal browser for call chains and data flow

## Goal

Jasper explores code today in an ack loop: search a keyword, read, search again. `explore` replaces that loop with a full-screen terminal program that follows links instead of keywords: definitions and usages, call chains, and data flow across function calls, shown as trees. It lives in the ctsq repo as a separate program and gets its answers from ctsq. Requirements were gathered in an interview with Jasper on 2026-10-22 and 2026-10-23, and tested against Mavexa, which is the test project.

## Scenarios

### S1. Change the padding around a label (first version)

On `http://localhost:3000/w/ws_101da8083e228c9f/assets` Jasper wants to change the padding around the "Upcoming" heading. Jasper pastes the URL, then runs an explicit text search for "Upcoming". The tree shows:

```
/w/:workspaceID/assets            frontend/src/routesConfig.ts:98
└── AssetsRoute                   frontend/src/routes/assets.tsx:5
    └── AssetsIndex               frontend/src/components/app/AssetsIndex.tsx
        └── 'upcoming'            { heading: 'upcoming', rows: groups().upcoming }
            └── ↻ for group       the loop over the groups
                └── <h2 class="mb-1 mt-4 ...">{group.heading}</h2>
```

The text search ignores case, because CSS can change case on screen. Checked on 2026-10-22: `ctsq find route` resolves the URL, `ctsq find text 'Upcoming'` finds nothing because the word is a data value and not JSX text, and `ctsq path "string#... ==>"` cannot start from a string literal (`the start selector matched no variable, parameter or field`). Both gaps are first-version work.

### S2. A value is wrong across frontend and backend (later)

An event shows under "past" when it should be "upcoming". Jasper starts on `eventStartsAt` in `AssetsIndex.tsx` and walks backward. The real chain in Mavexa, every link of which is in the code:

| Step | Code | What the tool does |
|---|---|---|
| 1 | `listWorkspaceAssets` returns `get<WorkspaceAsset[]>(ws('/assets' + qs))` (`frontend/src/lib/api.ts:2181`) | follow the URL string into helpers |
| 2 | `ws()` returns `` `/api/workspaces/${id}${path}` `` (`frontend/src/lib/workspaceContext.ts:11`) | join template pieces, `${...}` is a wildcard |
| 3 | `get` → `apiFetch` → `fetch(path, init)`, method defaults to `GET` | recognise the request exit and its method |
| 4 | `/api/workspaces/*/assets` matches `GET /api/workspaces/{workspaceID}/assets` (`backend/internal/api/handler.go:426`) | `find route` |
| 5 | `Handler.listWorkspaceAssets` passes `[]WorkspaceAsset` to `writeJSON` | recognise the response exit and its type |
| 6 | TS `eventStartsAt` ↔ Go `EventStartsAt` by `` `json:"eventStartsAt,omitempty"` `` (`backend/internal/store/events.go:916`) | match fields by JSON tag |
| 7 | `EventStartsAt` is scanned from `e.starts_at` in `WorkspaceStore.ListWorkspaceAssets` (`events.go:939`) | data flow within Go, mostly `ctsq path` today |

Guessed hops are marked and collapsed. Most real work crosses frontend and backend, so this is the next scenario after S1.

### S3. From code to spec and back (stretch)

`AssetsIndex.tsx` cites "spec 11, 12" in a comment. Jump from the comment to the spec, follow the spec's links to other docs, and list the code that cites it. Parked as a stretch (Jasper, 2026-10-23).

## Decisions

### 01. A separate program in the ctsq repo, browser only

`explore` is its own binary in this repo and calls ctsq's library for every answer. There is no printing mode: `ctsq path`, `callers` and `callees` already print trees. Why: Jasper, 2026-10-23. The data-flow and resolution gaps belong in ctsq, and the browsing does not.

### 02. Layout: tree pane, code pane, history pane

The tree pane on the left holds the current tree. The code pane on the right shows the source of the selected row. The history pane is toggled on and off. Focus moves between panes with global keys (Decision 07).

### 03. A new tree replaces the current one

Callers, callees, flow and usages each build a new tree in the tree pane and add a history node. Back returns to the previous tree. There are no tabs. Why: Jasper, 2026-10-23.

### 04. Trees list branches, not conditions

A data-flow fork shows each branch as its own child row. The row where the fork happens is marked `⑂`, and loops are marked `↻`. Condition text is not shown. In a call tree, each separate call is a branch. The example is illustrative; its line numbers are not real.

```
⑂ AssetsIndex.groups              AssetsIndex.tsx:88
├── upcoming.filter(...)          AssetsIndex.tsx:91
└── past.filter(...)              AssetsIndex.tsx:95
```

Why: Jasper does not need the conditions, only the possible routes (2026-10-23). Child rows rather than a collapsed marker, because data flow forks often and `a`/`h` folds one away with a single key.

### 05. History is a tree, per session, labelled by query

Every step adds a node: a search, a new root, callers, flow. Going back and taking a different step starts a branch, like Neovim's undo tree. Nothing is lost. Each node is labelled with its query, for example `callers listWorkspaceAssets`. History lasts for the session only, and there are no pins. Why: Jasper, 2026-10-23. Jasper called history "a key part of the exploration".

### 06. Guessed and unresolved hops

A hop ctsq could not resolve exactly is marked `?`. Routes that contain one sort last and start collapsed. In callers and callees trees, a call ctsq cannot resolve prints `unresolved Name` and the branch stops there. `path A --> B` still shows only routes that reach `B`.

### 07. Keys, in three groups

**Global** (any pane)

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | next / previous pane |
| `Ctrl-h` / `Ctrl-l` | pane to the left / right |
| `H` | show or hide the history pane |
| `:` | start point: URL, visible text, `file:line`, symbol. A start beginning with `/` is the explicit text search across the repo, for example `:/Upcoming` |
| `Ctrl-o` / `Ctrl-i` | back / forward through history |
| `?` | keys for the focused pane |
| `q` | quit |

**Tree pane**: Vim keys and left-hand keys both work.

| Vim | Left hand | Action |
|---|---|---|
| `j` / `k` | `s` / `w` | down / up |
| `l` / `h` | `d` / `a` | expand / collapse |
| `Enter` | `e` | make the row the root of a new tree |
| `c` / `C` | | callers / callees |
| `f` / `F` | | flow forward from / backward to this value |
| `u` | | usages |
| `/` | | filter the rows |
| `za` | | expand or collapse all `?` routes |

**Code pane**

| Key | Action |
|---|---|
| `j` / `k`, `Ctrl-d` / `Ctrl-u` | scroll |
| `w` / `b` | cursor to the next / previous identifier |
| `c` / `C` / `f` / `F` / `u` | as in the tree, for the identifier under the cursor |
| `gd` | go to definition |
| `/` | search within the pane |

The code pane shows the whole function holding the selected row, scrolled to the row, and takes Vim keys only.

**History pane**: `j` / `k` move, `Enter` returns to that node's tree.

Why: `wasd` for the left hand and `f` for flow (Jasper, 2026-10-23). `/` filters or searches the focused pane, as in Vim, and the repo-wide text search is a `:` start (Jasper, 2026-10-23).

### 08. One model for every code boundary

A boundary is any point where code leaves the source ctsq reads: a standard library, browser APIs, a third-party package, the network between frontend and backend, or the database. Every boundary is handled the same way, with no name-only fallback anywhere.

- **Library boundaries** (Go standard library, `lib.dom.d.ts`, `node_modules` `.d.ts`, Go modules): ctsq reads declarations only, from packages the code imports, when a hop needs them. A hop into one prints `external net/http.Header.Set` with its signature and the route stops. Return types carry forward, so `req, _ := http.NewRequest(...)` types `req`.
- **Process boundaries** (network, database): a repo file `.ctsq/boundaries.toml` declares where a request leaves the frontend (`fetch`: which argument is the URL, where the method comes from), where a response leaves the backend (`writeJSON`: which argument is the body), where a request body enters (`json.Decoder.Decode`), and any proxy prefix rewrites. ctsq links the two sides: URL to route through `find route`, and TS fields to Go fields through `json` tags. Everything else is inferred.

Why: found on 2026-10-23. `ctsq path 'function#listWorkspaceAssets --> function#/Query|Select/'` printed a false route `writeJSON` → `Footer.Set` → `assetRef.write` → `UpdateAssetDocument`. Two faults caused it. `w.Header().Set` (`backend/internal/api/handler.go:1195`) has an outside receiver type, so ctsq fell back to the only project method named `Set` and printed it as exact. The local closure `write(...)` at `backend/internal/designsystem/footer.go:287` was matched by name to the method `assetRef.write`. Jasper asked for one standard approach to all boundaries and no fallback.

### 09. TypeScript call resolution moves into ctsq

ctsq resolves TS and TSX calls through imports, as it already does for Go and Rust, and does not use the TypeScript language server. Why: Jasper, 2026-10-23. Mavexa has 368 TS and TSX files in its frontend, and calls in them currently match by name only.

### 10. No cache, no speed target yet

Every tree is computed on demand. Slow is acceptable for now. Why: Jasper, 2026-10-23.

### 11. First version is built in stages A to D, each usable before the next

- A: the library split, the resolver fixes and the browser, with data flow on Go.
- B: TS calls resolved through imports.
- C: TS data flow, including starting from a string, which delivers S1.
- D: library declarations.

Why: ctsq's data flow is Go only (`evaluate_flows`, `src/semantic/flows.rs:87`, skips every non-Go file), so S1 needs a TS flow engine, the largest single piece of work. Stage A gives a usable browser on Mavexa's backend first. Jasper chose this order on 2026-10-23 over building TS data flow before the browser.

## Work in ctsq

1. Remove the name-only fallback in call resolution. Unresolved calls print `unresolved`. Name-resolved hops always carry `?`.
2. Local bindings (variables, closures) take priority over project names.
3. Library boundaries (Decision 08): Go standard library and module declarations, then TS `.d.ts`.
4. TS and TSX call resolution through imports (Decision 09).
5. `path` can start from a string literal: `string#/upcoming/i ==>` follows the value into the code that renders it.
6. Process boundaries and `.ctsq/boundaries.toml` (S2).

## First version

Stages A to D (Decision 11). The packages are in Work packages below. Left for later: process boundaries and S2, a command in mdq that lists a file's links with their sections (the browser does the following, and mdq stays a reader), S3, and opening a row in Neovim.

## Open questions

- What is the program called? `explore` is a working name.
- Neovim: quit and open at the line, or send it to a running Neovim started with `--listen`? On hold (Jasper, 2026-10-23).

## Conventions

- Read `CLAUDE.md`. ctsq for code, mdq for markdown. Never `make install`: it replaces the ctsq every session uses, and Jasper installs from the merged branch. This overrides the older line in `plans/path/PLAN.md` Conventions.
- Gates, every package:
  - `~/.cargo/bin/cargo build --release` with no warnings
  - `~/.cargo/bin/cargo test`
  - `CTSQ=target/release/ctsq bash plans/agent-search/docs/check-examples.sh`
  - `REF=<reference binary> NEW=target/release/ctsq REFCACHE=0 bash plans/parse-cache/same-output.sh`. Always pass `REF`: the script's default points at an old binary from another job. Quote its summary line, with the count, in the handoff.
- Reference binaries: the orchestrator builds the tip each package is cut from into `<job tmp>/ref-<package>/target/release/ctsq` and names that path in the brief. A package with "0 differences" is compared against its own base tip. A2a is compared against the A1 tip.
- Mavexa (`~/workspace/projects/mavexa/mavexa`) is read-only. `find` takes its root from the current directory, so run Mavexa checks from a script that `cd`s there.
- Unit tests sit beside the code, CLI tests go in `tests/cli.rs`, and fixtures go under `fixtures/`. The browser's tests drive the app state with key events and render it to ratatui's `TestBackend`.
- Precedence: where this guide is silent, the Decisions above decide. Where both are silent, the existing code's conventions decide.

## Schema

The browser reads every answer as one tree type, defined in `src/api.rs`:

```rust
pub struct Tree {
    pub query: String,        // the history label, e.g. "callers listWorkspaceAssets"
    pub roots: Vec<Node>,
    pub hidden: Vec<String>,  // the `hidden:` footer lines ctsq prints today, verbatim
}

pub struct Node {
    pub label: String,        // e.g. "Handler.listWorkspaceAssets", "'upcoming'", "/w/:workspaceID/assets"
    pub path: String,         // repo-relative; empty when the node has no location
    pub line: usize,          // 1-based; 0 when the node has no location
    pub col: usize,           // 1-based column of the named thing on the line; 0 when unknown
    pub text: String,         // the source line, trimmed
    pub mark: Mark,
    pub tags: Vec<String>,    // data-flow tags exactly as `ctsq path` prints them: exact, derived, conditional@12 ...
    pub guessed: bool,        // what ctsq prints as `?`
    pub children: Vec<Node>,
}

pub enum Mark { None, Fork, Loop, Unresolved, External(String) }

pub struct Scope { pub path: String, pub start: usize, pub end: usize, pub signature: String, pub body: String }

pub enum Direction { Forward, Backward }

pub struct ApiError(pub String); // the text ctsq would print on stderr
```

- `Fork` goes on a data-flow node with two or more children, which is where the value's routes split (Decision 04). It is not derived from `conditional@L`, which ctsq attaches after the merge point and also uses for loops (`src/flow/tests.rs:84-118`). Call trees do not use `Fork`: each child call is already a branch.
- `Loop` goes on a node whose line sits inside a `for`, `while` or range block of its own function, computed from the syntax tree.
- `Unresolved` goes on a callers or callees child call that has no target. `External(name)` waits for stage D.

## Endpoints

These are the library functions in `src/api.rs`. Each takes the search root and hides tests the way the CLI does. Each one builds its answer from the same structured results its CLI command prints from. It never re-implements the command.

| Function | Same answer as | Built from |
|---|---|---|
| `callers(root, name, depth) -> Result<Tree>` | `ctsq callers` | `calltree` rows (A3a) |
| `callees(root, name, depth) -> Result<Tree>` | `ctsq callees` | `calltree` rows (A3a) |
| `usages(root, name) -> Result<Tree>` | `ctsq refs` | refs rows, moved out of `run_refs` (`main.rs:1440`) in A3a |
| `definition(root, name) -> Result<Tree>` | `ctsq def` | `resolve::def_rows` |
| `scope_of(root, path, line) -> Result<Scope>` | `ctsq scopeof` | `scopeof` rows (A3a) |
| `flow(root, path, line, col, Direction) -> Result<Tree>` | `ctsq path 'X ==>'` / `'X <=='` | the per-start flow function (A3b) |
| `find(root, query, key) -> Result<Tree>` | `ctsq find <query> <key>` | `find` rows (A3a) |
| `text_search(root, needle) -> Result<Tree>` | `-q 'literal#/needle/i'`, plus JSX text and comments | the `-q` fetch, moved out of `main.rs:1136-1225` in A3a |
| `start(root, input) -> Result<Tree>` | | the `:` prompt. `/x` → `text_search("x")`. `http://` or `https://` → `find("route", input)`. `path:line` → that line's scope as one node. Anything else → `definition(input)` |

## UI contract

- **Layout.** The tree pane takes the left 45%, the code pane the rest. The history pane, when shown, takes the bottom third of the left column. A status line along the bottom shows the current query, the focused pane, and the `hidden:` counts. While `:` is active, the status line becomes the prompt.
- **Tree rows.** Each row is `<glyph> <label>` with `path:line` aligned to the right, then the tags, dimmed. Glyphs: `⑂` for Fork, `↻` for Loop, `✕` for Unresolved, `↗` for External. A guessed row ends in `?` and is dimmed. Routes containing a guessed row sort last and start collapsed (Decision 06). Everything else starts expanded two levels deep.
- **Code pane.** Shows `scope_of` for the selected row, scrolled so that row is centred and highlighted. The code-pane cursor sits on identifiers, and `c`/`C`/`f`/`F`/`u` act on the one under it. When `col` is 0, the first identifier on the line is used.
- **History pane.** An indented tree of query labels. The current node is highlighted. `Enter` restores that node's tree and its selection.
- **States.** While a query computes, the tree pane shows `computing <query>…`. It runs synchronously, and slow is acceptable (Decision 10). An empty result shows `no results for <query>`. An `ApiError` shows its text in the status line, and the previous tree stays.
- **Keys.** Exactly the tables in Decision 07. `?` shows the focused pane's table as an overlay.

## Work packages

| Order | Package | Tier | Depends on |
|---|---|---|---|
| A1 | lib-split | Sonnet | |
| A2a | go-receivers | Opus | A1 |
| A2b | strict-calls | Opus | A2a |
| A3a | structure-out | Opus | A2b |
| A3b | flow-out | Opus | A3a |
| A3c | api | Sonnet | A3b |
| A4 | review | Opus xhigh, read-only | A3c |
| A4b | review-fixes | Opus | A4. Files are whatever the findings name. Holds every file until it merges |
| A5a | browser-core | Opus | A4b |
| A5b | browser-ui | Sonnet | A5a |
| A6 | final-review | Opus, read-only | A5b |
| B | ts-calls | detailed after A merges | A |
| C | ts-flow | detailed after A merges | B |
| D | library-boundaries | detailed after A merges | A |

The packages run in sequence. Most of them touch the resolver or the print paths, and the orderly route costs wall clock, which is not the scarce resource.

## Package: lib-split

Files: `Cargo.toml`, new `src/lib.rs`, `src/main.rs`, and visibility changes in any `src/` module.

Turn the single binary crate into a library crate named `ctsq` plus the `ctsq` binary. The module declarations at `src/main.rs:1-22` move to `src/lib.rs` as `pub mod`, and `main.rs` uses them through `ctsq::`. `Cargo.toml` gets an explicit `[lib]` and `[[bin]] name = "ctsq" path = "src/main.rs"`. Only A5a adds the `explore` bin. Widen visibility as far as the public signatures need, including the types they expose (`private_interfaces` and `private_bounds` lints). Do not move code or change behaviour. No module uses an item defined in `main.rs`, `build.rs`'s grammar links into both targets, and `tests/*.rs` spawn the binary (`tests/cli.rs:12`).

Acceptance:
- all four gates pass, with `same-output.sh` at 0 differences against the `408709e` reference
- no warnings, and `tests/` unchanged

## Package: go-receivers

Files: `src/resolve/go.rs`, new fixtures under `fixtures/resolve-strict/go/`, tests beside the code.

Gives Go method calls a receiver type in the cases that today reach the name fallback, so A2b can remove it without losing real edges:
- **Field-chain receivers** (`h.store.List()`, `s.db.Query()`). In `classify_callee_expr` (`go.rs:637-650`), anything that isn't a bare identifier becomes `Receiver::Opaque`. Type the chain through struct field types (`go_field_type`, `go.rs:1204`).
- **Call-result receivers** (`NewX().M()`, `x.Get().M()`), through the callee def's `ret`, the same way `var_type` (`go.rs:892`) types a Call binding.
- **Closures.** Add a `func_literal` arm to `classify_value` (`go.rs:585`) and a closure `Binding` variant (enum at `go.rs:309`). In `resolve_bare` step (a) (`go.rs:698`), a closure binding returns "local, no target" before steps (b) to (e).

The fallback stays in this package. Output changes only where a call now resolves through its type instead of its name.

Acceptance:
- Fixtures cover a field chain, a call result, a closure named like a project method, and a positive control: a same-package method on a typed variable still resolves. Each has a unit test.
- A script, `plans/explore/edges.sh`, counts how Mavexa's backend Go call edges resolve: by type, by name fallback, and unresolved. It runs `ctsq callgraph --format edges` from `backend/`, then classifies each edge with a debug flag or env var this package adds. The handoff gives the counts before and after. The by-name count must drop, and the total number of edges must not drop.
- `same-output.sh`: the handoff lists every differing call with its old and new target. Each new target must be justified by the receiver's type.

## Package: strict-calls

Files: `src/resolve/go.rs`, `src/resolve/rust.rs`, `src/resolve/fallback.rs`, `src/resolve/mod.rs`, `src/calltree.rs`, `src/explain.rs`, fixtures under `fixtures/resolve-strict/`, tests beside the code and in `tests/cli.rs`, `tests/path_flow-*.rs` expected outputs, and the expected outputs in `plans/agent-search/docs/`.

Implements Decision 08 for call resolution:
1. Remove the name-only fallback in Go: `resolve_bare` step (e) (`go.rs:737`) and `resolve_method` (`go.rs:769`). Do the same in Rust: `resolve_bare` (`rust.rs:681`) and `resolve_method` (`rust.rs:727`). `rank` (`go.rs:1259`, `rust.rs:896`) still ranks typed candidates.
2. `fallback::NameResolver::resolve_model` (`fallback.rs:39`) keeps its unique-name match until stage B, but marks it `Confidence::Guess` (`mod.rs:226`), so it prints `?`.
3. Callers and callees trees label a child call with no target `unresolved NAME` (`calltree.rs`, `explain.rs`). `path A --> B` is unchanged: it follows only routes that reach `B` (`calls.rs:308`, `:396-452`).
4. Flow output that changes because `site_targets` (`src/resolve/scoped.rs:1109`, `:1335`) lost a name-only target is accepted and listed. The flow engine's own name matching (`flows.rs:1041-1058`, `widen.rs:54`) stays until stage D.

Acceptance:
- `fixtures/resolve-strict/` gains a Rust case and the two cases from Decision 08: `w.Header().Set(...)` beside a project method `Set`, and the `write` closure. Neither resolves to the project method. Positive controls still resolve.
- `plans/explore/edges.sh` on Mavexa: the total number of edges falls by at most the A2a "by name" count. The handoff lists every lost edge with the receiver expression and why it has no type.
- In Mavexa, `ctsq path 'function#listWorkspaceAssets --> function#/Query|Select/' internal`, run from `backend/`, has no route through `Footer.Set`, and still reaches `WorkspaceStore.ListWorkspaceAssets`.
- `same-output.sh` against the A2a tip: zero differences in Rust, JS and TS calls apart from added `?` marks. Every Go difference is listed with its old and new target.

## Package: structure-out

Files: `src/calltree.rs`, `src/scopeof.rs`, `src/find.rs`, `src/resolve/mod.rs` (`def_rows`), `src/main.rs`, new `src/refs.rs` and `src/query_run.rs`, and tests beside the code.

Split each print path into a builder that returns rows and a printer that formats them:
- `calltree::report_many` and `Walk` (`calltree.rs:83-142`)
- `scopeof::Report` (`scopeof.rs:66`)
- `find::Report` and `report_with` (`find.rs:26`, `:926`)
- `run_refs` (`main.rs:1440-1522`), which moves to `src/refs.rs`
- the `-q` fetch and `hide_rows` (`main.rs:1136-1225`), which move to `src/query_run.rs`

Builders return data and never print or exit. `main.rs` calls builder, then printer.

Acceptance:
- `same-output.sh` at 0 differences against the A2b tip
- each builder has a unit test on an existing fixture that asserts its rows

## Package: flow-out

Files: `src/semantic/flows.rs`, `src/flow/upward.rs`, `src/flow/mod.rs`, `src/flow/engine.rs` (visibility only), and tests beside the code.

Split `evaluate_flows` (`flows.rs:87`) and `evaluate_upward` (`:290`) into two steps:
- a setup step building the config, the Go module path, `Module`, `is_end` and the walk (`flows.rs:119-166`)
- a per-start function returning the pruned `flow::Hop` tree after `retag`, `prune` and `only` (`flows.rs:235-255`)

Forward starts go through `start_in` (`engine.rs:289`). Backward starts go through `upward::Up::start` (`flows.rs:330-346`). Add a position entry point that turns `path:line:col` into a byte offset and the start node. Field starts that merge several functions (`flows.rs:204-221`) keep their merge.

Acceptance:
- `same-output.sh` at 0 differences against the A3a tip
- a test starts from a position on an existing flow fixture, in both directions, and matches `ctsq path` for the same value

## Package: api

Files: new `src/api.rs`, `src/lib.rs` (one `pub mod api;` line), new `plans/explore/check-mavexa.sh`, and tests beside the code.

Implements the Schema and the Endpoints table on top of the A3a and A3b builders. Computes `Fork` and `Loop` as the Schema defines them.

Acceptance:
- one unit test per endpoint, on existing fixtures, comparing labels and `path:line` with the CLI output, both with tests hidden and with `--tests`
- a flow fixture with an if/else where the value goes down both branches: the node before the split carries `Fork`. A loop fixture: the node inside the loop carries `Loop`
- `check-mavexa.sh` passes:
  - `start(root, "http://localhost:3000/w/ws_101da8083e228c9f/assets")` returns the route `/w/:workspaceID/assets`
  - `callers(root, "listWorkspaceAssets", 3)` has no `Footer.Set` node
  - `callees(root, "Handler.listWorkspaceAssets", 2)`, run from `backend/`, contains `WorkspaceStore.ListWorkspaceAssets`

## Package: review

Read-only, Opus at xhigh, in its own worktree. It commits any reproduction fixtures on its own branch and lists them in its handoff. Try to refute each of these claims:
1. No Go or Rust call is resolved by name alone. Find one that still is.
2. A local closure always wins over a project name of the same spelling.
3. Every Mavexa edge lost in A2b had no type evidence: no import, receiver type or lexical binding.
4. Every `api.rs` function agrees with its CLI command, both with tests hidden and with `--tests`.
5. On the fixtures under `fixtures/flow/` and `fixtures/resolve-strict/`, `Fork` appears on exactly the flow nodes with two or more children. `Loop` appears on exactly the nodes inside a loop of their own function.

Findings are typed `{file, line, issue, severity, fix}`. A4b fixes the blockers and majors.

## Package: browser-core

Files: new `src/bin/explore/` `app.rs`, `tree.rs`, `history.rs`, `keys.rs`, `Cargo.toml` (adds `ratatui`, `crossterm` and `[[bin]] name = "explore" path = "src/bin/explore/main.rs"`), a stub `src/bin/explore/main.rs`, and a test fixture repo under `fixtures/explore/`.

The app state is a plain struct: trees, selection, focus, history tree, prompt and filter. Key events change it, and it calls only `ctsq::api`. Implements Decisions 03 to 07 as state changes.

Acceptance:
- A table-driven test with one case per row of the Decision 07 tables, each asserting the state change. The test fails if a table row has no case.
- `Enter` then `Ctrl-o` restores the previous tree and its selection.
- Going back, then running a different query, creates a history branch, and the old branch stays.
- `/` filters tree rows. `:/text` runs `text_search`.
- A scripted session over `fixtures/explore/` asserts the exact row labels after each step: `:name`, `C`, `c`, `F` on a value, `Ctrl-o`.

## Package: browser-ui

Files: `src/bin/explore/main.rs`, new `src/bin/explore/code.rs` and `src/bin/explore/render.rs`, and `CLAUDE.md` (an `explore` entry under Architecture).

The terminal setup and the event loop, plus rendering of the state into the UI contract.

Acceptance:
- `TestBackend` snapshot tests for the layout, each glyph, a guessed row, the code pane on a selected row, the history pane, the help overlay, the empty state and the error state
- `cargo run --release --bin explore fixtures/explore` starts, and quits on `q`, in a scripted pty test
- The handoff includes a manual walk for Jasper on Mavexa's backend, in addition to the tests

## Package: final-review

Read-only, Opus. Runs the full gates. Checks drift three ways between code, this plan and `CLAUDE.md`. Walks the browser's keys against Decision 07.

## Package: ts-calls

Outline only. Stage B adds a TS resolver in `resolver_for` (`src/resolve/mod.rs:507`) that maps an imported name to its file through `JsSpecResolver::resolve` (`src/resolve/imports.rs:260`) and the import bindings (`imports.rs:19`, `js_import_bindings` `:519`). It is detailed after A merges.

## Package: ts-flow

Outline only. Stage C extends data flow to TS and TSX. Today `evaluate_flows` and `evaluate_upward` skip every non-Go file and load the Go config only (`flows.rs:87`, `:290`). It also lets a string literal be a start (`start_at`, `engine.rs:69`). Done means scenario S1 works end to end.

## Package: library-boundaries

Outline only. Stage D reads declarations from the Go standard library and modules, and from `.d.ts` files, on demand (Decision 08). The plug-in points are `DefIndex` (`src/resolve/mod.rs:60`), `resolve_pkg_fn` (`go.rs:834`), `method_on` (`go.rs:871`) and `var_type` (`go.rs:892`). It also removes the flow engine's own name matching (`flows.rs:1041-1058`, `widen.rs:54`).

## Plan review (2026-10-23)

Two reviewers read the plan before launch. One checked architecture against the code, the other checked the package split. All findings but one were accepted and are folded in above:
- The fallback goes only after field-chain and call-result typing (A2a), because most real Go method edges reach today's fallback through `Receiver::Opaque`.
- The structured-output refactors are their own packages (A3a, A3b).
- `Fork` is redefined.
- The browser is split in two.
- Gates name `REF` and `REFCACHE=0`.
- Acceptance lists every differing call.

Amended: "unresolved hops in `path`". `path A --> B` keeps following only routes that reach `B`, and unresolved calls show in callers and callees trees instead. Printing every dead end of an `A --> B` search would bury the routes.
