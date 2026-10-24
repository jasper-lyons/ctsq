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

They live as TOML under `config/models/ts/` (`builtins.toml`, `solid-js.toml`). The model syntax gains a slot that binds a receiver's or argument's elements to a callback's parameter, such as `receiver[*] -> 0.param0` (amended 2026-10-24 by the C1 plan review: the existing syntax has bare indices, not `arg0`). Model keys are `<import source>.<name>` for imported functions (`solid-js.For`, `solid-js/store.createStore`) and `<Global>.prototype.<method>` for receiver methods whose receiver type is unknown (`Array.prototype.map`, `Promise.prototype.then`, `Response.prototype.json`). Why: Jasper chose core plus stores, `Index` and `Switch` (2026-10-24). `on()` and contexts come later.

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

Files:
- the `[nodes]`, `[fields]` and `[tokens]` sections of `config/languages/typescript.toml`, `tsx.toml` and `javascript.toml`. Those files hold only `[tests]` today; `go.toml` `:5`, `:39`, `:63` is the shape to copy. Every value is a real grammar kind. The plain `typescript` grammar has no `jsx_*` kinds, so `typescript.toml` leaves the JSX keys out.
- `src/config/mechanics.rs`: new optional `Nodes` keys, with these exact key names, each `Vec<String>`, `#[serde(default, skip_serializing_if = "Vec::is_empty")]` so `ctsq config dump go` prints exactly what it prints today:
  - `object` (`object`), `pair` (`pair`), `array` (`array`)
  - `jsx_element` (`jsx_element`), `jsx_self_closing` (`jsx_self_closing_element`), `jsx_expression` (`jsx_expression`), `jsx_attribute` (`jsx_attribute`), `jsx_text` (`jsx_text`)
  - `pattern` (`object_pattern`, `array_pattern`, `assignment_pattern`, `rest_pattern`, `pair_pattern`, `object_assignment_pattern`, `shorthand_property_identifier_pattern`)
  - `await` (`await_expression`), `spread` (`spread_element`)
  - `literal` (`string`, `template_string`, `number`; Go's literal kinds are left to C2, not added here)
  `Nodes` is `mechanics.rs:47`, `Fields` `:117`, `Tokens` `:171`. No key is required today (every struct is `#[serde(default)]`), and none becomes required.
- `src/config/models.rs`: the C03 slot syntax. `Slot`'s variants (`:10`) stay unchanged, because engine.rs and upward.rs match on them exhaustively and C1 does not own those files. The element and parameter binding go in new fields on `Pass` (`:44`): `from_elements: bool` (the `[*]` suffix on the source) and `to_param: Option<usize>` (the `.paramN` suffix on the target). `Pass::parse` (`:50`) accepts the suffixes; every existing model string parses to the same `Pass` with the new fields at their defaults. `parse_file` is `:117`.
- `src/config/mod.rs`: `BUILTIN_MODELS` (`:116`) gains the two new files, and the model lookup (`load_with`, `:280-313`) maps the languages `typescript`, `tsx` and `javascript` to the model directory `ts`, for built-in and for disk models (`.ctsq/models/ts/`, XDG). Additive only: Go's lookup is unchanged.
- new `config/models/ts/builtins.toml` and `config/models/ts/solid-js.toml`, keyed as C03 says.
- new `src/flow/ts_sig.rs`: a TS or JS parameter list, with destructured parameters as paths. Mirror `GoParam`/`GoSignature`/`go_signatures` (`src/resolve/go.rs:1357`, `:1373`, `:1383`). JS `formal_parameters` holds bare patterns; TS and TSX wrap them in `required_parameter`/`optional_parameter` with `pattern`, `type` and `value` fields. Declared in `src/flow/mod.rs` with one line, `pub mod ts_sig;` like its neighbours (`:14-17`). Nothing calls it until C3, so follow the repo's `#[allow(dead_code)] // wired by C3` pattern (`src/resolve/go.rs:1354`) where the build would warn.
- tests beside the code.

Acceptance:
- a test loads each of the three language files through the config loader and asserts these lists are non-empty: call, assign, function, param, object, pair, array, pattern, await, spread, literal, and for TSX and JS also jsx_element, jsx_self_closing, jsx_expression, jsx_attribute
- a test asserts every kind named in the three TOMLs exists in its grammar (`Language::id_for_node_kind`, named, non-zero)
- `config/languages/go.toml` is byte-identical to base, and `ctsq config dump go` prints the same as the reference binary
- every existing model in `config/models/` parses to the same passes as before (new `Pass` fields at their defaults)
- the TS models parse, and a test asserts the slots for `solid-js.For`, `solid-js.Index`, `solid-js.createMemo`, `solid-js.createResource`, `solid-js/store.createStore` and `Array.prototype.map`
- `ctsq config dump tsx` lists `<built-in>/models/ts/solid-js.toml` with a non-zero model count, and `config dump javascript` lists `builtins.toml`
- `ts_sig` returns the parameters for `function f(a, {b, c}: P, ...rest)` in TS, the same list in plain JS (`function f(a, {b, c}, ...rest)`), arrow functions, `(props) =>`, and a default value `(a = 1)`
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

## Plan review (2026-10-24, C1)

One architecture-lens reviewer (Opus) read this file scoped to C1 against the live code. All ten findings accepted; Package: ts-mech and C03's example were amended in the same commit.

- F1 models in `config/models/ts/` never load: the lookup filters on the language name (`typescript`, `tsx`, `javascript`) and built-in models are `include_str!`'d. Fixed by adding `src/config/mod.rs` to C1 with a `ts` directory alias.
- F2 `arg0` does not exist and a new `Slot` variant breaks matches in engine.rs and upward.rs. Fixed: `0.param0`, new `Pass` fields, `Slot` unchanged.
- F3 no TS model key convention. Fixed in C03: `<import source>.<name>` and `<Global>.prototype.<method>`.
- F4 "every required key" cannot fail: no key is required. Replaced with named non-empty lists.
- F5 `Nodes` has no `PartialEq` and new fields would change `config dump go`. Fixed with `skip_serializing_if` and a dump comparison.
- F6 an uncalled `ts_sig` warns. Fixed with the repo's `allow(dead_code)` pattern.
- F7 `patterns` and `literal kinds` were not key names. Named.
- F8 shorthand kind names differ from the grammar, and nothing checks mechanics kinds. Real kinds listed, grammar test added.
- F9 JS and TS build parameter lists differently. JS case added to acceptance.
- F10 `same-output.sh` does not load TS mechanics, so 0 differences is reachable. No change.
