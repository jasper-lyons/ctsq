# explore stage C: data flow in TypeScript

## Goal

Data flow (`f`/`F` in the browser, `ctsq path` with `==>` and `<==`) works in TS, TSX, JS and JSX, and can start from a string literal. Done means scenario S1 of `plans/explore/PLAN.md` works end to end: from the string `'upcoming'` to the `<h2>` that renders it. It also means flowing backward from `a.eventStartsAt` in `AssetsIndex.tsx` reaches `api.ts`. This is Decision 11's stage C, and `PLAN.md`'s Decisions and Conventions apply.

## Baseline (2026-10-24)

The flow engine has two steps since A3b:
- **Setup:** `DownSetup::new` (`src/semantic/flows.rs:336`) and `UpSetup::new` (`:436`), both loading `config::load(root,"go")`.
- **Per start:** `Walk::tree_at` (`:965`) and `UpWalk::tree_at` (`:522`). The browser enters through `api.rs:372` `flow()`.

Places that refuse non-Go files:
- `locate` (`flows.rs:306`), which the browser hits
- `evaluate_flows` (`:130`), `evaluate_upward` (`:225`)
- `field_users` (`:586`), `fn_value_effect` (`:1212`)
- `objects.rs:155,230`, `values.rs:199`, `upward.rs:610`, `widen.rs:61-86`, `template.rs:184,341`

The language-neutral parts:
- The Walker (`src/flow/engine.rs:451`) tracks a variable → hop map and field paths, with one hop per statement (`hop()` `:726`).
- Node kinds, field names and tokens come from `config/languages/*.toml` through `config/mechanics.rs:47` `Nodes`, where every key is required. `typescript.toml` and `tsx.toml` hold only `[tests]`.

The Go-specific parts:
- `engine.rs:11` imports `binding_type`, `go_imports`, `go_result_names` and `go_result_types`
- error results (`:1313`) and pointer aliases (`:193`)
- `composite` (`:1738`) keeps keys only for struct-typed literals
- `literals` (`:942`) binds closure parameters only for direct calls
- `into_callee` (`flows.rs:998`) uses `go_signatures`, and `callee_effect` (`:1051`) uses gomod
- `upward.rs` hardcodes Go kinds (`:28-61`, about 120 strings)
- the library models are `config/models/go/stdlib@1.23.toml`, read by `ModelOutside` in `src/flow/models.rs`

`start_at` (`engine.rs:69`) accepts only `field_name` and `name`, so a string literal cannot start a flow.

### S1, forward, in Mavexa `frontend/src/components/app/AssetsIndex.tsx`

| Hop | Line | Needs |
|---|---|---|
| `'upcoming'` | `:334:30` | value start |
| `{ heading: 'upcoming', ... }` | `:334` | object literal path `heading`, no type |
| `[ ... ]` | `:333-337` | array literal carries paths |
| `<For each={...}>` | `:332-333` | JSX as a call, ↻ |
| `(group) =>` | `:339` | `For` model binds the element to parameter 0 |
| `group.heading` | `:341` | field read strips the path |
| `{...}` inside `<h2>` | `:341` | `rendered` sink, hop below statement level |

`group.rows` (`:340`) and `<For each={group.rows}>` (`:343`) must not appear, because their path differs. S1 stays inside the `AssetsIndex` arrow function (`:100-372`), so it does not need B3.

### Backward, from `a.eventStartsAt` (`AssetsIndex.tsx:60`)

The chain runs:
1. callback parameter of `rows.filter` (`:60`)
2. the parameter `rows` of `groupAssets` (`:58`)
3. the caller `groupAssets(shown())` (`:132`)
4. `shown`, a `createMemo` accessor (`:124`)
5. `returned()` (`:122`), with `assets.latest ?? []`
6. `[assets] = createResource(..., async q => {...return rows})` (`:117-120`)
7. `rows = await listWorkspaceAssets(...)` (`:118`)
8. `api.ts:2183` `return get<...>(ws(...))`
9. `get` (`:436`), `res.json()` (`:441`)
10. `apiFetch` (`:422`)
11. `fetch` (`:427`), where the route stops as external

The frontend↔backend boundary is outside stage C.

## Decisions

### C01. Rendering is a sink

A value inside a `{...}` container under a lowercase (intrinsic) JSX element is tagged `rendered`. So is a value in an attribute of an intrinsic element (`class=`, `href=`). Inside JSX there is one hop per `{...}` container or attribute, not one per statement. Why: S1 ends at the `<h2>`, and the whole `return (...)` is one statement. Adopted by the orchestrator on 2026-10-24 as the mapper recommended.

### C02. JSX is a call

`<Comp a={x}>{child}</Comp>` is a call to `Comp`, with the attributes as named arguments and the children as an argument called `children`. A lowercase tag is not a call. Its expressions are sinks (C01).

### C03. Solid models: the core primitives, stores, `Index` and `Switch`

Models cover:
- `For`, `Index`, `Show`, `Switch`/`Match`
- `createSignal`, `createMemo`, `createResource`
- `createStore`/`setStore`
- the array methods `map`, `filter`, `sort`, `find`, `flat`, `flatMap`
- `Promise`/`await` and `Response.json`

They live as TOML under `config/models/ts/` (`builtins.toml`, `solid-js.toml`). The model syntax gains a slot that binds a receiver's or argument's elements to a callback's parameter, such as `receiver[*] -> arg0.param0`. Why: Jasper chose core plus stores, `Index` and `Switch` (2026-10-24). `on()` and contexts come later.

### C04. A callback bound by an iteration model is a loop

A hop inside a callback bound by `For`, `Index` or an array method is marked `↻`, as well as hops inside syntactic loops. This amends the Schema's Loop rule in `PLAN.md`, which derived Loop from syntax only.

### C05. A flow can start from a value

`StartKind::Value` starts at the literal under a position: a string, a template string or a number. It works in every language with flow mechanics, including Go. A text-search hit's position starts it unchanged. A hit on visible text written directly in JSX (`jsx_text`) produces a one-row tree for its element, since it is already rendered.

### C06. Props flow into child components

The attribute `a` on `<Row a={...}>` becomes the path `[a]` on `Row`'s parameter 0, and `props.a` strips it. This needs B3, because the route has to resolve `Row` to its definition. Why: Jasper, 2026-10-24.

### C07. Backward flow is ported to TS

`upward.rs` reads its node kinds from mechanics instead of hardcoded Go strings, and gains the TS shapes above. It runs last in stage C. Why: Jasper, 2026-10-24. Without it, `F` does nothing on frontend code.

### C08. Go output does not change

Every package keeps `same-output.sh` at 0 differences on Go, apart from new start positions that only C05 makes possible.

## Open questions

None yet.

## Conventions

- `plans/explore/PLAN.md` `## Conventions` applies in full.
- The orchestrator runs one architecture-lens plan review of this file before the first launch, scoped to the packages being launched.
- C1 adds new files only, and new optional keys in `config/mechanics.rs`, so `go.toml` stays untouched. It may run while stage A is still in progress. C2 and later edit the flow engine, which stage A's review fixes may still touch, so they launch only after stage A merges. C4 and C5 also need B3.

## Work packages

| Order | Package | Tier | Depends on | Can start |
|---|---|---|---|---|
| C1 | ts-mech | Opus | | now |
| C2 | value-start | Opus | stage A merged | after A |
| C3 | ts-walk | Opus | C1, C2 | after A |
| C3r | review | Opus xhigh, read-only | C3 | |
| C4 | ts-cross | Opus | C3, B3 | after B |
| C5 | ts-up | Opus | C4 | after B |
| C6 | final-review | Opus, read-only | C5 | |

## Package: ts-mech

Files: the `[nodes]`, `[fields]` and `[tokens]` sections of `config/languages/typescript.toml`, `tsx.toml` and `javascript.toml`; `config/mechanics.rs` (new optional `Nodes` keys: object, pair, array, jsx_element, jsx_self_closing, jsx_expression, jsx_attribute, patterns, await, spread, literal kinds); new `config/models/ts/builtins.toml` and `config/models/ts/solid-js.toml`; the model parser for the C03 slot syntax (`src/config/models.rs`, `parse_file` `:117`); new `src/flow/ts_sig.rs` (a TS parameter list, with destructured parameters as paths), declared in `src/flow/mod.rs` with one line; and tests beside the code.

Acceptance:
- the TS, TSX and JS mechanics load, with every required key and the new optional ones present
- `go.toml` is unchanged, and loading it gives the same `Nodes` as before
- the models parse, and a test asserts the slots for `For`, `Index`, `createMemo`, `createResource`, `createStore` and `Array.map`
- `ts_sig` returns the parameters for `function f(a, {b, c}: P, ...rest)`, arrow functions, and `(props) =>`
- `same-output.sh` at 0 differences

## Package: value-start

Files: `src/flow/engine.rs` (`start_at`, `start_in`, `eval` seeding), `src/flow/mod.rs` (`StartKind`), `src/semantic/flows.rs` (`locate` accepts any language with flow mechanics), `src/api.rs` (text hits as flow starts, and C05's one-row `jsx_text` tree), fixtures, and tests.

Seeding: `eval` (`engine.rs:1616`) returns `carry: Some(0), exact` when a node's span equals the start span, the same mechanism `composite` uses for fields (`:1760`).

Acceptance:
- a Go fixture starts a flow from a string literal, through `ctsq path "string#/x/ ==>"` and through `api::flow` at its position
- `same-output.sh` at 0 differences

## Package: ts-walk

Files: `src/flow/engine.rs`, a new `src/flow/jsx.rs` if needed, the TS parts of `src/semantic/flows.rs`, fixtures under `fixtures/flow-ts/` (including a copy of `AssetsIndex.tsx` lines 326-350), and tests.

Covers the forward walk within one file:
- object literals without types, array literals, destructuring into paths
- callbacks bound through C03 models
- JSX as a call (C02), hops below statement level, the `rendered` sink (C01)
- implicit arrow returns, `await` and `??` as passes
- `fn_value_effect` for local accessor functions
- the C04 loop marks

Acceptance:
- on the fixture copy, and in a Mavexa check script `plans/explore/check-s1.sh`, `api::flow(AssetsIndex.tsx:334:30, Forward)` returns exactly the S1 hops in the Baseline table: `:332`/`:339` marked `↻`, ending at `:341` tagged `rendered`, with no rows at `:340` or `:343`
- `text_search("Upcoming")` includes the `:334` hit, and its position starts that same flow
- `same-output.sh`: 0 differences on Go

## Package: ts-cross

Files: `src/semantic/flows.rs` (`into_callee` and `signature` with `ts_sig` for TS, `callee_effect` without gomod for TS, `field_users` for TS), `src/flow/engine.rs` where calls are bound, fixtures and tests.

Acceptance:
- forward from `<Row a={a} />` at `AssetsIndex.tsx:343`, the flow reaches `props.a.eventName` rendered inside `Row` (around `:90`)
- a value passed to an imported function in another file follows into it through B3's targets

## Package: ts-up

Files: `src/flow/upward.rs`, the mechanics keys it needs, fixtures and tests.

Acceptance:
- backward from `a.eventStartsAt` at `AssetsIndex.tsx:60`, the walk reaches `api.ts:2183` and stops at `fetch` (`api.ts:427`) as external, with the hops of the Baseline backward chain
- Go backward output is unchanged
