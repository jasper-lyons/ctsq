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

A hop ctsq could not resolve exactly is marked `?`. Routes that contain one sort last and start collapsed. A call ctsq cannot resolve prints `unresolved Name` and the route stops there.

### 07. Keys, in three groups

**Global** (any pane)

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | next / previous pane |
| `Ctrl-h` / `Ctrl-l` | pane to the left / right |
| `H` | show or hide the history pane |
| `:` | start point: URL, visible text, `file:line`, symbol |
| `/` | explicit text search across the repo |
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
| `za` | | expand or collapse all `?` routes |

**Code pane**

| Key | Action |
|---|---|
| `j` / `k`, `Ctrl-d` / `Ctrl-u` | scroll |
| `w` / `b` | cursor to the next / previous identifier |
| `c` / `C` / `f` / `F` / `u` | as in the tree, for the identifier under the cursor |
| `gd` | go to definition |

**History pane**: `j` / `k` move, `Enter` returns to that node's tree.

Why: `wasd` for the left hand and `f` for flow (Jasper, 2026-10-23). `/` is the text search Jasper already uses.

### 08. One model for every code boundary

A boundary is any point where code leaves the source ctsq reads: a standard library, browser APIs, a third-party package, the network between frontend and backend, or the database. Every boundary is handled the same way, with no name-only fallback anywhere.

- **Library boundaries** (Go standard library, `lib.dom.d.ts`, `node_modules` `.d.ts`, Go modules): ctsq reads declarations only, from packages the code imports, when a hop needs them. A hop into one prints `external net/http.Header.Set` with its signature and the route stops. Return types carry forward, so `req, _ := http.NewRequest(...)` types `req`.
- **Process boundaries** (network, database): a repo file `.ctsq/boundaries.toml` declares where a request leaves the frontend (`fetch`: which argument is the URL, where the method comes from), where a response leaves the backend (`writeJSON`: which argument is the body), where a request body enters (`json.Decoder.Decode`), and any proxy prefix rewrites. ctsq links the two sides: URL to route through `find route`, and TS fields to Go fields through `json` tags. Everything else is inferred.

Why: found on 2026-10-23. `ctsq path 'function#listWorkspaceAssets --> function#/Query|Select/'` printed a false route `writeJSON` → `Footer.Set` → `assetRef.write` → `UpdateAssetDocument`. Two faults caused it. `w.Header().Set` (`backend/internal/api/handler.go:1195`) has an outside receiver type, so ctsq fell back to the only project method named `Set` and printed it as exact. The local closure `write(...)` at `backend/internal/designsystem/footer.go:287` was matched by name to the method `assetRef.write`. Jasper asked for one standard approach to all boundaries and no fallback.

### 09. TypeScript call resolution moves into ctsq

ctsq resolves TS and TSX calls through imports, as it already does for Go and Rust, and does not use the TypeScript language server. Why: Jasper, 2026-10-23. Mavexa has 368 TS and TSX files in its frontend, and calls in them currently match by name only.

### 10. No cache, no speed target yet

Every tree is computed on demand. Slow is acceptable for now. Why: Jasper, 2026-10-23.

## Work in ctsq

1. Remove the name-only fallback in call resolution. Unresolved calls print `unresolved`. Name-resolved hops always carry `?`.
2. Local bindings (variables, closures) take priority over project names.
3. Library boundaries (Decision 08): Go standard library and module declarations, then TS `.d.ts`.
4. TS and TSX call resolution through imports (Decision 09).
5. `path` can start from a string literal: `string#/upcoming/i ==>` follows the value into the code that renders it.
6. Process boundaries and `.ctsq/boundaries.toml` (S2).

## First version

- The `explore` browser: three panes, the keys in Decision 07, the history tree, and Decisions 03 to 06.
- S1 working end to end on Mavexa.
- ctsq work items 1 to 5.

Later: process boundaries and S2, a command in mdq that lists a file's links with their sections (the browser does the following, mdq stays a reader), S3, and opening a row in Neovim.

## Open questions

- Which key filters the rows of the tree, now that `/` is the text search?
- Does the code pane also take `wasd`, or Vim keys only?
- What is the program called? `explore` is a working name.
- How does the code pane show context: the whole function holding the row, or a fixed window?
- Neovim: quit and open at the line, or send it to a running Neovim started with `--listen`? On hold (Jasper, 2026-10-23).
