# ctsq — Cascading TreeSitter Queries

**Project location:** `/Users/jasperlyons/workspace/projects/ctsq/`

# Abstract TreeSitter Query Language

## Context

TreeSitter queries use grammar-specific node names (`function_declaration` in JS, `function_definition` in Python, `function_item` in Rust). This makes it impossible to write a single query that works across languages. The goal is an abstract query language that:

1. Uses a fixed set of semantic node types that are language-agnostic
2. Compiles down to correct TreeSitter S-expression queries per target language
3. Has a terse, CSS-inspired syntax (less verbose than raw S-expressions)
4. Casts a wide net by default — unknown names pass through as concrete node names

---

## Query Language Grammar

```
query        := selector (combinator selector)*
selector     := atom field_access*
atom         := sigil? node_type? name_match?          -- bare, no capture
              | '(' query capture? ')'                  -- node context, capturable

field_access := '.' field_name '(' query? ')'

sigil        := '*'                                     -- definition only
              | '&'                                     -- reference/call only
                                                        -- no sigil = both
node_type    := identifier                              -- abstract (known) or concrete (pass-through)
name_match   := '#' bare_word                           -- exact text match (shorthand)
              | '#' '"' text '"'                        -- exact text match (explicit)
              | '#' '/' regex '/'                       -- regex text match
capture      := '@' capture_name

combinator   := ' '   -- descendant
              | '>'   -- direct child
              | '+'   -- strict adjacent sibling
              | '~'   -- ordered sibling (anywhere after, same parent)
```

### Key rules

- **`()` always provides a node** — never pure syntactic grouping
- **Capture requires `()`** — `@capture` only appears inside `()`
- **No sigil = both** — matches definitions and references, widest net
- **Pass-through** — if a node type is not in the abstract vocabulary, it is passed to TreeSitter verbatim as a concrete node name
- **No sub-types** — use concrete node names (via pass-through) when you need that specificity

---

## Attributes, dot shorthand and semantic combinators

```
atom         := sigil? node_type? name_match? attr*
attr         := '[' key op value ']'
op           := '=' | '^=' | '*=' | '$='                -- equals, starts, contains, ends
value        := bare_word | '"' text '"' | '/' regex '/'  -- /regex/ only with '='
selector     := (atom | '(' query capture? ')') (field_access | ':not(' selector ')')*
combinator   := ... | '->' '-->' '<-' '<--'             -- call graph
              | '=>' '==>' '<=' '<=='                   -- data flow
```

- Attributes are carried on `Atom.attrs` and ignored by the compiler. They are filtered after tree-sitter matching. A value containing `/` must be quoted: `[from="log/slog"]`.
- Dot shorthand: in `#A.B`, when the token after `B` is not `(`, the atom gets name `B` and the attribute `[qual=A]`. `#log.body(...)` stays field access.
- Semantic combinators and `:not(...)` parse anywhere, but `compiler::validate` rejects them everywhere with an error that starts with `compiler::SEMANTIC_QUERY_PREFIX` (test with `is_semantic_query_error`). `compiler::has_semantic` answers the same question for the `path` subcommand.
- An unknown character in a query is a parse error.

## Attributes evaluation

`src/semantic/attrs.rs` filters `-q` rows after `runner::run_rows`. It reparses the file and finds each row's primary node by `start_byte` and `end_byte`.

- Only the last selector may carry attributes, because a `-q` row only holds the last selector's node. Attributes on any other selector, inside a group, a field inner query or a `:not` exit 2 with `attributes are only supported on the last selector in -q`.
- From the primary node the stage finds the name identifier (the called name of a call, the member of a member access, the `name` field of a declaration) and its qualifier: the Go `selector_expression` operand, the JS/TS `member_expression` object, the Python `attribute` object, the Rust scoped path. The qualifier's leftmost identifier is its root.
- `from` is the import origin of the qualifier root, or of the name itself when it has no qualifier. Go reads `resolve::go::go_imports` (local name to full import path). JS/TS, Python and Rust read `RawImport.bindings` from `resolve::imports`. C, C++ and Ruby exit 2 with `attribute from is not supported for <lang>`.
- `in` is the nearest enclosing struct, class, impl, interface, module or namespace, or a Go method's receiver type. A declaration does not enclose its own name. For a Go member access through a variable (`u.Email`), `in` is the variable's type from `resolve::go::binding_type`. For a chain (`b.Contact.Phone`) it is the type of the operand just before the name: the declared type of field `Contact` on b's type, from the struct declarations of the file (`resolve::go::go_field_type`, embedded fields promoted). A chain whose type is unknown has no `in`.
- `qual` (dot shorthand) holds when `in` matches the value, or when the qualifier root is bound by an import and the root or the whole qualifier text matches the value. So `function#slog.Info` matches `slog.Info` from `"log/slog"`, and `function#log.Printf` does not match `fmt.Printf`.
- Operators: `=` equals, `^=` starts with, `*=` contains, `$=` ends with, `=/re/` uses the `regex` crate. An unknown key or a bad regex exits 2.
- A row survives only when every attribute holds. A query left with no rows prints the usual miss report and exits 1.
- A semantic combinator or `:not` in `-q` exits 2 with `use: ctsq path '<query>'`.
- The runner drops rows with identical node text, so two matches of the same name on one line keep only the first before filtering.

## Examples

| Abstract query | Meaning |
|---|---|
| `function` | any function, definition or reference |
| `*function` | function definitions only |
| `&function` | function calls/references only |
| `(&function#sizeof @f)` | call to `sizeof`, captured as `@f` |
| `(&function#/GFILE\|gfile\|GFile/ @f)` | calls matching regex, captured |
| `(&function#malloc @f).params(var#ARRAY_SIZE @v)` | malloc call + ARRAY_SIZE arg, both captured |
| `(&function#malloc @f).params((var#ARRAY_SIZE @v) + #"<< 2")` | same, with text predicate on sibling scope |
| `(*function#main @def).body((&function#malloc @call))` | definition of main containing a malloc call |
| `arrow_function` | concrete TS node, passed through directly |

---

## Compilation Model

### Vocabulary resolution

The compiler checks each `node_type` against the fixed abstract vocabulary:
- **Known** → expand to one or more language-specific structural patterns
- **Unknown** → pass through verbatim as a concrete TS node name

This means users can always escape to raw TS node names without any special syntax.

The `field` kind names a struct, class or interface field. `*field` is the declaration, `&field` is an access (`x.name`), and bare `field` is both. The match lands on the declaration or the access expression, and the field name is read off it, so attribute filters such as `field#User.Email` resolve the owner the same way they do for `field_identifier`. Per language: Go `field_declaration` and `selector_expression`, JS and TS class fields (plus TS `property_signature`) and `member_expression`, Python `self.x = ...` assignments and `attribute`, Rust `field_declaration` and `field_expression`, C and C++ `field_declaration` and `field_expression`, Ruby instance variables (`@x`, quote the name in a query). Method calls are selectors and member expressions too, so `&field` also lists them.

### Mapping layer

The mapping is **not** a simple name substitution table. Each abstract type maps to **structural patterns** per language — because some variants require different tree shapes.

Example — JavaScript:

```
&function → [
  (call_expression function: (identifier)),
  (call_expression function: (member_expression)),
]

*function → [
  (function_declaration ...),
  (function_expression ...),
  (arrow_function ...),        -- name lives in parent variable_declarator, not this node
  (method_definition ...),
]
```

The arrow function case is structural: the name lives in a parent `variable_declarator`, not the `arrow_function` node itself. This cannot be expressed declaratively — these mappings are implemented in code.

**The mapping layer is fixed and built-in.** No user-extensible vocabulary. Users who need something outside the vocabulary use concrete node names directly (pass-through).

### Compiler pipeline

```
1. Parse     — query string → abstract AST
2. Resolve   — expand known abstract types to per-language structural patterns
              — unknown types pass through as concrete node names
3. Transform — apply name_match predicates, field accesses, captures
4. Emit      — produce TreeSitter S-expression query string
```

### Descendant depth

A descendant at the top level of a query (`function#handle variable#name`) matches the tail at any depth below the head. It compiles to a two-phase scoped query whose scope field is empty, meaning the whole head node. The head pattern also gets an explicit `@_root` capture, and the runner scopes to that node in preference to any user capture. The tail must be strictly below the head, so a head that matches its own tail pattern does not count. Each row sits on the tail match, not on the head. Only the first top-level descendant gets this treatment. Descendants inside groups, inside field inner queries, and after the first top-level one keep the older form, where the tail is wrapped in one `(_ ...)` node and so reaches one grammar level down.

### Name match compilation

| Abstract | TreeSitter predicate |
|---|---|
| `#sizeof` | `(#eq? @_n "sizeof")` |
| `#"sizeof"` | `(#eq? @_n "sizeof")` |
| `#/sizeof/` | `(#match? @_n "sizeof")` |

### Text-across-nodes (`#"..."` in subquery)

`#"text"` inside `.field(subquery)` compiles to a `#match?` predicate on the captured scope node:

```
.params(var#ARRAY_SIZE + #"<< 2")
→
arguments: (argument_list (identifier) @_v (#eq? @_v "ARRAY_SIZE")) @_p
(#match? @_p "<< 2")
```

### Multi-variant expansion

Abstract types that map to multiple node variants compile to a TS alternation:

```
(&function#sizeof @f)
→
[(call_expression function: (identifier) @_f (#eq? @_f "sizeof")) @f
 (call_expression function: (member_expression) ...)]
```

### Missing concepts

When an abstract type has no mapping in the target language (e.g. `class` in C), the query returns empty results silently. No error. Consistent with the wide-net principle — a query that doesn't apply to a language simply matches nothing.

Exception: `class` maps to `struct` in C. The vocabulary aims to map to the closest semantic equivalent per language, not just exact matches.

---

## Semantic Vocabulary (fixed)

Loosely aligned with LSP semantic token types:

| Abstract type | Concept | Example C equivalent |
|---|---|---|
| `function` | function / method | `function_definition` |
| `class` | class / struct / interface | `struct_specifier` |
| `var` | variable / identifier reference | `identifier` |
| `param` | function parameter | `parameter_declaration` |
| `type` | type annotation / reference | `type_identifier` |
| `import` | import / include statement | `preproc_include` |
| `call` | function call expression | `call_expression` |
| `op` | operator | `binary_expression` operator field |
| `literal` | any literal value | `number_literal`, `string_literal`, ... |
| `block` | statement block / body | `compound_statement` |

---

# Call → Definition Resolution

`callers`, `callees`, `callgraph`, and `def` resolve call sites to definitions
via per-language resolvers in `src/resolve/` (a `Resolver` trait parallel to
the `Lang` trait). Resolvers walk tree-sitter CSTs directly — no round-trip
through the query engine's text output, which destroys receiver and path
structure.

## Architecture

Two phases over the whole job set, producing an in-memory `CallGraph`
(`src/resolve/graph.rs`) that the calltree commands traverse:

1. `collect_defs` — per file, index every definition as
   `Def { name, qualified, kind, ret, id: file:line }`. `qualified` is the
   module path (derived from the file path relative to the nearest `src/`,
   plus inline `mod`s, plus the impl type for methods); `ret` is the bare
   name of the (first) return type, feeding `x := f(); x.m()` inference.
2. `resolve_calls` — per file, classify each call site
   (`Bare` / `Path` / `Method{receiver}`) and resolve it against the full
   index.

Graph nodes are keyed by definition identity (`file:line`) or external name —
cycle detection in the tree walkers uses this key, so same-named functions in
different files no longer produce false cycles.

## Rust resolution rules (in order)

- **Qualified path** (`a::b::f()`, `Foo::method()`): expand the leading
  segment through the file's `use` map, then suffix-match against
  `Def.qualified`. A path that matches nothing is external — no name-only
  fallback (so `HashMap::new()` never grabs an unrelated local `new`).
- **Bare call** (`f()`): local `let f = func;` binding (followed
  transitively, depth-capped) → same-file free fn → explicit `use` (aliases
  included) → glob `use` → ranked fallback.
- **Method call** (`x.m()`, `self.m()`): receiver type from the enclosing
  `impl` (for `self`, falling back to the trait for `impl Trait for T`), a
  `let x: T` annotation, a `let x = f()` call binding — the callee's indexed
  return type (`-> Self` maps to the impl type), else the `T::assoc(..)`
  constructor heuristic — a `T { .. }` literal, or a typed parameter →
  method defined on that type → ranked fallback (methods preferred over
  free fns).
- **Trait dispatch (CHA)**: bodyless trait signatures index as
  `InterfaceMethod` defs and `impl Trait for Type` blocks are recorded as
  implements-relations (even when empty). When a receiver types to a trait —
  `&dyn Trait`, `Box/Rc/Arc<dyn Trait>`, a `T: Trait` / `where T: Trait`
  bound, or `impl Trait` in argument position — the call expands to one edge
  per implementing type: the override when the impl defines the method, else
  the trait's default. Single target exact, several each a guess, no impls →
  the signature line. `self.m()` does not expand — inside an impl, `Self` is
  statically known.

**Ranking:** when several candidates survive, locality orders them — same
file > same directory > imported module > anywhere — and the winner is a
*guess*: trees and `def` output mark it with `?`, DOT edges are dashed,
`--format edges` appends `?`. A single project-wide candidate is exact.
Zero candidates leave the call as a bare external name.

**Out of scope** (falls back to ranked name matching): blanket impls
(`impl<T: A> B for T`), supertrait methods, macro-generated code, `pub use`
re-export chains, exact block-scope shadowing (last `let` before the call
line wins).

## Go resolution rules (in order)

Same pipeline (`src/resolve/go.rs`), adapted to Go's shape. `qualified` is
the file's directory components plus the receiver type for methods —
packages are directories, so no inline-module handling. The wrinkle:
`pkg.Fn()` and `x.Method()` are the same syntax, so selector calls are
classified as methods with a `Var` receiver and disambiguated at resolve
time.

- **Bare call** (`f()`): local `f := func` binding (followed transitively,
  depth-capped) → same-file function → same-package (same-directory)
  function → dot imports → ranked fallback.
- **Selector call** (`x.F()`): local bindings and params first — they shadow
  package names. A typed receiver (`var x T`, `x := T{..}` / `&T{..}`, an
  `x := f()` call binding — the callee's indexed return type, with
  `x, err := f()` binding the first result, else the `NewT()` naming
  convention — a typed parameter, or the method's own receiver) → method on
  that type. Otherwise, if `x` is an imported package (aliases included),
  match functions by import path; defs only know their on-disk path while
  imports carry the module path (`example.com/demo/util` vs
  `/tmp/demo/util`), so candidates match on any shared path suffix and only
  the deepest overlap survives. An imported package that matches nothing is
  external — `fmt.Println()` never grabs an unrelated local `Println`.
- **Interface dispatch (CHA-lite)**: interface declarations index each
  signature as an `InterfaceMethod` def. When a receiver types to an
  interface, the call expands to one edge per project type whose method set
  covers all of the interface's methods — a single implementor is exact,
  several are each a guess, none points the edge at the signature line.
- Calls inside `go` / `defer` statements and func literals attribute to the
  enclosing named function; generic instantiations (`f[int]()`) unwrap.

**Out of scope** (falls back to ranked name matching): embedded
interfaces/structs (method sets are not inherited through embedding),
struct-field receivers (`s.client.Do()`), and plain reassignment after the
declaration.

Languages without a dedicated resolver use `fallback::NameResolver`, which
preserves the original behavior: project-wide unique names resolve,
everything else stays bare, never a guess.

**Future:** external per-project config will adjust resolution. The design
point for that is the ordered candidate-generation steps in each resolver's
`resolve_calls` and the `rank` locality weights — rules should plug in
there, not grow new code paths.

---

## Call walking

`ctsq path` evaluates the call combinators in `src/semantic/calls.rs` over the call graph. The data comes from three places.

- `resolve::TreeStore` reads and parses every file of a run once. `graph::build_from_store` hands each resolver its tree through `Resolver::collect_defs_tree` and `Resolver::resolve_calls_tree`. Resolvers that ignore the tree parse again, which is every resolver but Go's. `graph::build` keeps reading files itself and builds the same graph.
- `CallGraph.sites` keeps every call site (`resolve::CallSite`: caller, target, line, byte range, argument byte ranges). The edge list keeps one edge per caller, callee and target, so the flow engine reads sites, not edges. Only the Go resolver fills it.
- `resolve::go::go_signatures` lists the receiver and the parameters of each Go function, method and interface method, with 0-based positions that count every name and every unnamed parameter.

Evaluation, in order:

1. `semantic::split_query` cuts the query at its top-level semantic combinators into AST segments. A semantic combinator inside a group or a field inner query is an error. `:not()` is only allowed on the last selector of a segment, where it is taken off before the segment compiles.
2. `semantic::segment_defs` compiles and runs each segment like a `-q` query, filters attributes, then maps rows to definitions. A row maps to a definition when it overlaps the definition header, which runs from the start of the declaration to the start of its body. So `function#main` picks the declaration of `main`, and a call to `main` inside another body picks nothing. A file whose language rejects the segment is skipped, and the error is only returned when every file rejects it.
3. The `:not()` selectors are matched the same way. No route passes through a definition they match, start and end included.
4. All joins must read one way: `->` and `-->`, or `<-` and `<--`. A data-flow combinator is an error here. Upward queries are reversed into call order, so every route is printed caller first and the query's first selector matches the last hop.
5. For each start, legs are searched in order. `->` and `<-` take exactly one edge. `-->` and `<--` search breadth first over simple paths, shortest first, pruned by a reverse breadth-first distance to the leg's targets. A path stops at the first target it reaches. A middle segment is a waypoint that every route passes through.
6. Only edges that resolved to a definition are walked. External callees cannot be a step.
7. `max` (default 12) bounds the whole route in edges. Without `all`, at most `limit` (default 5) routes are kept per start, shortest first. The search stops after 200000 queued paths and sets `truncated`. A start with no route within `max` is searched again without the bound, and when that finds one `cut` counts it, printed as `N routes cut at --max <n>`.

Each hop carries its definition, the line where it calls the next hop, and the confidence of the edge that reached it. A guessed edge (ranked candidates, interface dispatch to several types) prints `?`.

## `ctsq path` output

`src/semantic/print.rs` prints call routes. `main.rs` runs `ctsq path '<query>' [paths...]` with `--max N` (default 12), `--all`, `--only <tag,...>` (stored, applied by the data-flow walk) and `--body`.

- Every row is `path:line`, padded, the definition name (`Owner.name` for methods), then `calls <next> at :<line>`. A guessed edge prints `?` after the callee. Two spaces per level, no box characters.
- Routes with the same start merge into one tree, so a shared prefix prints once and a row lists each callee it reaches. Upward queries print in the order written, the first selector first, and each row says `called by <caller> at :<line>`.
- `--body` prints each definition body under its row, one level deeper.
- The trailer is `N route(s)`, then notes for selectors that matched nothing, definitions removed by `:not` and a search stopped at its cap.
- `print::row(path, line, func, text, tags)` lays out one row, and data-flow trees reuse it.
- Exit codes: 0 routes found, 1 none, 2 usage or query error. A query with a data-flow combinator prints `data-flow walking is not available yet` and exits 2 until the flow walk lands.
- `fixtures/.ignore` hides `fixtures/path/` from directory scans of `fixtures/`, so the pinned call graph test keeps its output. Naming `fixtures/path` directly still works.

## Verification

Proof-of-concept compiler targeting JavaScript and C:

1. `(&function#sizeof @f)` over C → matches all `sizeof` calls
2. `(*function#main @f)` over JS and Python → matches `main` definitions in both languages from one query
3. `(&function#malloc @f).params(var#ARRAY_SIZE @v)` over C → matches the specific call pattern
4. Same query over Python → empty result, no error
5. `arrow_function` over JS → passes through, matches `arrow_function` nodes directly

# Config and models

Flow mechanics and outside-call models are data, not Rust. Both are TOML (decision 21).

## Lookup order

Highest priority first, and the first match wins:

1. repo `.ctsq/`
2. `$XDG_CONFIG_HOME/ctsq`, else `~/.config/ctsq`
3. `$XDG_CACHE_HOME/ctsq`, else `~/.cache/ctsq` (generated, safe to delete, nothing writes it in v1)
4. built-in defaults compiled in with `include_str!` from `config/`

Under each root: `languages/<lang>.toml` (mechanics) and `models/<lang>/*.toml` (models).

## Mechanics

`languages/<lang>.toml` has `[outside]` (`module_file`) and `[nodes]` (`assign`, `call`, `return`, `field`, `param`, `import`, `branch`, `loop`, `assert`, `composite`), each a list of tree-sitter node kinds. Layers merge per key, so a repo file that sets only `nodes.loop` leaves the rest as built in. The flow engine reads these keys and hard-codes no node names.

## Models

Each table name is a qualified name: `importpath.Name`, or `(*importpath.Type).Method` for methods. Fields:

- `pass`: list of `"from -> to"`. From is an argument index, `*` (every argument) or `receiver`. To is an argument index, `receiver` or `return`.
- `clear`: the value stops being tracked.
- `whole`: the call uses every field of its arguments.
- `source`: the named slot receives a fresh tracked value (`"return"`).
- `template = { data_arg = N }`: the call renders a template with that data.
- `template_load = { fs_arg, names_from, text_arg }`: the call loads templates.

A malformed file reports file, line and key and exits 2. Unknown fields are errors.

## Versions

A file named `name@1.23.toml` is versioned. The `go` line of the nearest `go.mod` picks the highest version not above it (`go 1.23.4` reads as 1.23), falls back to the lowest shipped when the module is older than all of them, and to the highest when there is no go.mod. This applies in every root, not only the built-in set.

## Outside the codebase

For Go, a call is outside when the import path is neither the module path from `go.mod` nor under it (`config::gomod::is_outside`). A call with no model is assumed to derive its return value from its arguments and is tagged `unmodelled`.

## Commands

- `ctsq config path` prints the four locations in lookup order.
- `ctsq config dump <lang>` prints the merged mechanics, the selected Go version and the model count per source.

# Flow engine

`src/flow` follows one value through one function. It reads node kinds from `[nodes]`, child field names from `[fields]` and token texts from `[tokens]` in the language's mechanics file, and names no language node itself.

## Mechanics keys it adds

- `[nodes]`: `function`, `block`, `case`, `default_case`, `name`, `field_name`, `field_decl`, `type_decl`, `range`, `list`, `loop_clause`, `index`, `declare` (assignments that declare their names in the enclosing block).
- `[fields]`: `left`, `right`, `operator`, `condition`, `initializer`, `update`, `consequence`, `alternative`, `body`, `alias`, `case_label`, `function`, `arguments`, `operand`, `field`, `name`.
- `[tokens]`: `plain_assign` (operators that replace the left side) `blank` (names that discard a value) and `declare` (the token that makes a range header declare its names, `:=`).

## API

- `start_at(&FlowCtx, start_byte, end_byte) -> Option<Start>` classifies the node at a byte range. A field name, in a struct declaration or a read, gives `StartKind::Field` with its owner type when known. A plain name gives `StartKind::Var`.
- `enclosing_function(root, &Mechanics, byte)` finds the function to walk.
- `analyse_function(&FlowCtx, func, &Start, is_end, &dyn Outside) -> FlowResult` walks it. `is_end` is a closure over call nodes. `Outside` answers for every other call that receives tracked data with `CallEffect::Unknown`, `Pass`, `Clear` or `Whole`. A closure `Fn(&CallSite) -> CallEffect` is an `Outside`, and `no_models` answers `Unknown` for everything.
- `FlowResult.root` is a tree of `Hop` (file, line, function, trimmed source line, tags, children). A hop's children read what it produced.
- `report(&[FlowResult]) -> String` prints the rows of the Output format, two spaces of indent per level, then the sink count and the routes that end at an unmodelled call.

## Rules (in order)

1. A parameter, or a variable bound outside the body, is tracked from the first statement. A variable bound inside the body is tracked after its binding statement. A variable read inside the body is tracked from that statement.
2. A field start tracks every read `x.Field` in the function. When the owner type is known and the operand has another type, the read is skipped. The operand's type is `binding_type` for a variable and, for a chain `x.A.Field`, the declared type of `A` on x's type (`flow::operand_type`), so `b.Contact.Phone` is a read of `Contact.Phone`, not `Booking.Phone`.
3. Assignment with a tracked right side tracks each name on the left: `exact` when the right side is just the tracked value, `derived` otherwise. Unequal left and right counts give `derived`. A range header binds one element, so it is `derived`.
4. Assignment with nothing tracked on the right drops each name on the left. An operator outside `plain_assign`, such as `+=`, keeps what the left side held. A plain store of an untracked value into `o.A.B` removes the paths of `o` that begin with `[A, B]`, and stops tracking `o` when none are left.
5. A store into `o.F` or `m[k]` tracks the object `o` or `m`. A field store adds the tag `field F`.
6. Conditions carry no data. A sink called inside a condition is still recorded.
7. After an if or switch, a name is tracked when any arm tracked it, tagged `conditional@<line>` of the condition when not every arm did. An if without else and a switch without a default arm count the untaken path as an arm.
8. A loop reruns its header and body until the tracked set stops changing. The set only grows inside a loop, so this ends. A safety cap of 32 passes plus 4 per name the function declares catches field paths that keep nesting (`o.Next = &o`), and a loop that hits it gets a hop tagged `stopped: loop cap`. The set after the loop joins the set before it, since the body may not run.
9. A call whose argument is tracked and that `is_end` accepts is a `sink`. Any other call that receives a tracked argument or receiver asks `Outside`. `Unknown` carries the data and tags `unmodelled`. `Pass` carries it. `Clear` stops it (decision 11). `Whole` carries it and also tracks the receiver object.
10. A call statement that receives tracked data is `moved`, and so is a return of a tracked value.
11. A hop's tags read in the order primary (`exact`, `derived`, `moved`), `field`, `conditional@`, `unmodelled`, `sink`. A hop's parent is the hop that produced the first tracked value it reads.
12. Names are tracked with block scopes. A `:=`, a `var`, a `:=` range header, the init clause of an if, switch or for, a type switch alias and a case body each declare in their own block, and the new binding shadows the outer one until the block ends, when the outer binding comes back as it was. A plain `=` assigns the nearest binding. Function literals are walked as `Function values and closures` describes.

## Models in the flow

`flow::ModelOutside::new(&LangConfig)` is the `Outside` that answers from model files. `flow::model_key(&LangConfig, &CallSite)` returns the first key with a model among `flow::candidate_keys`:

1. A package call gives `importpath.Func`, the import path taken from the file's imports.
2. A method on a typed receiver gives `(*importpath.Type).Method`, then `(importpath.Type).Method`. The engine reads the receiver's type with `resolve::go::binding_type_qualified`, which keeps the package part (`strings.Builder`), swaps the package name for its import path and puts the result in `CallSite::recv_path`.
3. A plain name gives `builtin.name`.

A model maps to an effect. `clear` gives `Clear`. Anything else gives `CallEffect::Model { pass, whole }`, which the engine applies entry by entry: an entry whose `from` slot holds tracked data moves it to the result, or to the variable an argument or the receiver is rooted at, so `json.Unmarshal(body, &form)` tracks `form`. An assignment whose only effect is such a move records a `moved` hop. A model with no `pass` and no `clear`, such as a source or a template model, moves nothing.

`whole` (decision 13): for a field start with a known owner type, an argument whose type is the owner counts as tracked when the call's model is `whole`, so `fmt.Printf("%v", u)` reaches an end predicate on `Printf` from `User.Email`. The engine lists such arguments in `CallSite::whole_args` and asks `Outside` about the call even when nothing else is tracked.

With no model, a call into the repo gives `CallEffect::Via(name)`: the result carries the data and the hop is tagged `via <name>`, placed after `field` and before `conditional@`. A call is in the repo when its callee is a plain name declared as a function in the same file (`CallSite::local`), its package lies inside the go.mod module, or its receiver type has no package part. Every other call gives `Unknown` and is tagged `unmodelled` (decision 18).

# Cross-function flow

`semantic::flows::evaluate_flows` runs the data-flow combinators of `ctsq path`. `=>` reads "flows in one step to", `==>` reads "eventually flows to". The backward forms `<=` and `<==`, a chain of more than one flow step, `:not()` and a mix with call combinators exit 2.

## Starts and ends

1. The query is cut with `split_query`. The head segment's rows are start points: each row is narrowed to the name inside it (a field declaration, field access or parameter declaration gives its name) and classified by `flow::start_in`. A variable start is walked in its enclosing function, once per function and name, from its first row.
2. A field start (`field#Booking.CustomerPhone`) is one group per field and owner type. The group's root row is the field declaration. Every function in the scanned files that reads or writes a field of that name is walked from the field, and their trees hang side by side under the declaration row. A selector whose operand has another known type (`flow::operand_type`) and a keyed element of a literal of another known type do not make a function a user.
3. With a tail segment, a call is a sink when one of the tail's rows covers the call's function name (`Printf` in `log.Printf`), so attribute filters such as `function#log.Printf` apply.
4. With no tail (the query ends in `=>` or `==>`), a call is an end when it goes outside the module (decision 16): a `pkg.F` call on an import outside the go.mod module path, or a method on a variable whose type comes from such a package. Those hops are tagged `outside` in place of `sink`. With no go.mod every import counts as outside.

The language config is loaded from the repo root of the first scanned path. The module path and Go version come from the go.mod nearest that path when there is one.

## Following calls

1. When a call receives tracked data, its byte range is looked up in `CallGraph.sites` for the current file. A site with an in-repo target is walked in the callee: the argument positions map to parameters through `go_signatures` (a variadic parameter takes every later argument, the receiver joins when it is tracked). The walk starts at the first such parameter and tracks the rest from the top of the body (`flow::analyse_function_with`). The callee's root row is its signature line, tagged `moved`, and `widened` too when an interface call has several targets.
2. The engine gets `CallEffect::Into { name, returns, writes, trees }`. The call's hop is tagged `via <name>`, the callee trees hang under it before the caller's later hops, and the call's result carries the data only when a return statement in the callee carries it. So a helper that drops the value ends the route. `returns` (`FlowResult::returns`) has one entry per result position, and in `a, b := f(x)` only the names at positions some return carries are tracked, so `_, err := two(x)` with `return s, nil` does not track `err`. A bare `return` hands back the named results as they stand.
3. `writes` are the pointer parameters (type `*T`) and the pointer receiver that hold tracked data when the callee returns, set inside it (`FlowResult::exits`, the union of the states at each return and the end of the body), with their field paths. The caller tracks the object the matching argument or receiver is rooted at from the call's hop: `fill(&b, x)` with `b.Phone = s` inside tracks `b` with `[Phone]`, and so does `b.SetPhone(x)` on a pointer receiver. The hop that last stored into a pointer parameter or receiver is kept in the printed callee tree like a return, so the `b.Phone = s` row shows under the call.
4. Recursion is guarded by a stack of (definition, tracked argument positions). A call already on the stack and every call under `=>` fall back to `CallEffect::Via`, which carries the data without walking. A call deeper than `--max` functions gives `CallEffect::Cut`, which carries the data like `Via` and tags the hop `stopped: max depth`, placed first on the row. Its hop is kept by pruning like any stop. Effects are memoised per call node and tracked arguments, so loop passes reuse them.
5. Calls with no in-repo target go to the models (`ModelOutside`). Conversions to predeclared types (`string(b)`, `[]byte(s)`) keep the data.

## Printing

1. Branches that never reach an end are pruned. A callee tree is kept where it reaches an end itself, and along the hops that lead to its returns when the call's hop is kept, so a route through a function that hands the value back shows that function. Under `=>` only ends one step below the start count.
2. `--only` keeps hops whose tags include one of the given tags, matched whole or by first word (`via` keeps `via normalise`, `conditional` keeps `conditional@4`), with their ancestors. A route then counts when a hop on it, its end included, carries a wanted tag. A group with no kept hop prints nothing, and when no route counts the trailer is `0 routes match --only <tags>`.
3. Rows use `print::row`, two spaces per level. The trailer is `N routes to sink` (or `N routes to outside` with no end), then `N routes cut at --max <n>` counting the printed `stopped: max depth` hops, then `N routes passed through unmodelled calls` counting kept routes with an `unmodelled` hop above their end, then the unmodelled-ends line counted before pruning.
4. Exit 0 when at least one end is found, 1 when none, 2 on errors.

# Object tracking

Decision 12 (follow the object) and 13 (reflection moves whole objects). Code in `src/flow/objects.rs` and the field, composite and store handling of `src/flow/engine.rs`.

## Field paths

1. Every tracked variable carries `Paths`: `None` when the variable holds the data itself, or a list of field paths such as `[["CustomerPhone"]]` when it is an object holding the data in those fields.
2. A store `o.A.B = x` tracks `o` with `[A, B]` put in front of `x`'s paths, joined with what `o` already held. A composite literal `T{F: x}`, `&T{F: x}` or positional `T{x}` gives an object with `[F]` in front of `x`'s paths. A map or slice literal keeps the element's paths.
3. A read `o.F` keeps the paths that begin with `F`, minus `F`. An empty remainder means the read is the data, `exact` when `o` was. A read of another field is not tracked. A read of a value tracked as itself stays tracked, as before.
4. The receiver of a method call is its operand, so `rv.Field(i)` is not a read of a field called `Field`.
5. Paths travel with calls: `CallSite.arg_paths` and `recv_paths` give each tracked argument's paths, the callee parameter starts with them (`Start.paths`, the `also` pairs of `analyse_function_with`), and a call whose callee returns tracked data gives its result the arguments' paths. Effects are memoised per paths too.
6. Any call that receives an object holding tracked data counts it as tracked, so an end call or a `whole` model (`fmt.Printf`, `(*encoding/json.Encoder).Encode`) fires on the object.
7. In a field start, a write of the start field (`T{F: x}` or `o.F = x` on the owner type) makes the object hold the field, and `field_users` also walks functions with a keyed element of that name.
8. Pointers alias inside a function (`pointer_aliases`). A parameter or variable of type `*T`, `p := &y` and `p := new(T)` are pointers. `p := &y` aliases `p` and `y`, and `c := b` aliases `c` and `b` when `b` is a pointer. A store through one (`c.F = x`, `*p = x`) also tracks every name it aliases with the same path. A store `*p = x` through a pointer of unknown origin (`p := slot()`), or through an expression with no root variable, is tagged `stopped: pointer write`.

## Embedded structs

`flow::Module` indexes every struct declaration of the scanned files from `type_decl` and `field_decl`: field names, base types, struct tags, and embedded fields (a field declaration with no name, named by its type). A path step that is an embedded struct name is skipped when the next read is a promoted field, so `ab.CustomerPhone` reads `ab.Booking.CustomerPhone`. A field start whose owner is `Booking` reads `ab.CustomerPhone` when `ab`'s type embeds `Booking` at any depth.

## Shared stops

1. Storing tracked data into a map element, a package-level variable (or its field), or sending it on a channel stops that branch. The row is tagged `stopped: shared map`, `stopped: shared global` or `stopped: shared channel` and nothing is tracked past it.
2. A map is a package-level variable whose type or value is a map, or a local declared with a map type, a map literal or `make(map...)`. A package-level variable is one declared at the top of a scanned file and not declared again in the function.
3. For a package-level variable, every other function that names it is listed as a child row (`path:line` and function name), and the stop row adds `N readers`. Pruning keeps stop rows with their readers.
4. Mechanics keys: `send`, `global`, `map_type`, `keyed`, `element`, `conversion`, `generic` nodes, and `type`, `tag`, `key`, `value`, `channel` fields.

## Generics

`F[T](a, b)` parses as a call of an index expression and `F[T](x)` as a conversion to a generic type. The engine drops the type arguments in both and calls `F`, and the Go resolver records the conversion form as a call site, so both resolve to the generic definition. The callee is walked by parameter name whatever `T` is.

## Struct tags

When a hop reads or writes a field with a struct tag, the row gets `tag <tag>`, as `tag json:"phone"`. The owner type comes from the operand's binding type, else from the one struct that declares the field name (or the tag every declaring struct agrees on).

## Constructor receivers

A method called on `pkg.NewT(...)` gets the receiver type `path.T`, so `json.NewDecoder(r.Body).Decode(&v)` finds the `Decoder` model and moves the body into `v`. With no end selector, a call on such a chain is outside when `pkg` is.

# Function values and closures

The cross-function walk follows calls whose callee is not a declared function name. `semantic::flows` resolves function-valued variables, `flow::engine` walks function literals.

## Function-valued variables

1. A call `send(x)` whose callee is a plain name that the enclosing top-level function declares (`f := helper`, `var f = helper`) counts the assignments to that name before the call, literals included. A parameter of function type has none and falls back to the call graph.
2. A name the function does not declare, and that is a package-level variable, counts every assignment in the scanned module: its `var` spec and each `name = value` inside functions that do not declare the name themselves. Files ending in `_test.go` are skipped, so a test that swaps the function does not count.
3. Exactly one assignment whose value names a function in the repo (`sendEmail`, or `pkg.SendEmail` when the name is unique) is walked like a direct call. The call's hop is tagged `moved, via <function>` and the callee's signature row hangs under it.
4. Otherwise the call's hop is tagged `stopped: function value` and the assignments are listed under it as rows, in file order. The result does not carry the data. A single local assignment that names no function (a method value, say) falls back to the call graph instead.

## Closures

1. A function literal is walked from the statement holding it when it is called in place (`func() { ... }()`, also under `go` and `defer`), passed as an argument of a call, or returned. A literal assigned to a variable is not walked.
2. The walk enters a literal only when its body reads a tracked variable of the enclosing scope, or when, called in place, it receives a tracked argument. Its own parameters shadow outer names.
3. The literal's line is a hop tagged `moved, closure`. Inside the body the captured variables and the bound parameters are tracked from that hop, so the body's hops hang under it. Outer variables the body assigns tracked data to (`out = x`) are tracked after the statement holding the literal, unless the literal is returned. Returns inside a literal are not returns of the enclosing function.
4. A returned literal makes the function return its captured data, so `Template(name, data)` returning a handler that passes `data` to `fmt.Fprintf` carries the caller's value into the handler body, and the caller's result carries it too.

## Interfaces

A method call whose call graph targets are all interface method signatures (no implementation in the scanned files) is tagged `stopped: interface`. A call with several implementations keeps one walk per target, tagged `widened`.

# Upward flow

`<=` (one step) and `<==` (eventually) walk from a value back to where it was created. `src/flow/upward.rs` holds the walk, `semantic::flows::evaluate_upward` runs it for `ctsq path`. Go only. The node kinds are tree-sitter-go names written once at the top of `upward.rs`, so the `from` attribute and value sets can use the same helpers without loading a config.

## Starts and ends

The head segment's rows are the values, one start per variable name per function, the first row in the function. A start that is where the name is assigned or declared prints that definition as the root row. A start at a use prints the use row with the reaching definitions under it. A query ending in `<=` or `<==` has no end and lists every origin. With an end selector (`variable#q <== function#os.Getenv`) only branches that reach a call the end matches are kept, and that call's row is tagged `source`.

## Rules (in order)

1. Reaching definitions: the nearest declaration of the name visible at the use (block scopes respected, closures see the functions around them, then package-level `var` and `const` in the file, then in the other files of the package), plus every plain assignment after it and before the use. An operator assignment (`+=`) also keeps the earlier value.
2. A literal, composite literal or function literal stops with `source literal`. So does a `var` with no value.
3. A parameter walks to the argument at its position in every in-repo caller (`CallGraph.sites`), each caller row tagged `moved`. A function with no in-repo callers stops with `source param`.
4. An in-repo call result walks into the callee's return expressions at the result position, tagged `derived, via <callee>` on the call row and `moved` on each return row. `return f()` for several results passes the position through.
5. An outside call reads its model in reverse. A `source` model stops with `source`. A `pass` from a slot to `return` continues from that argument or the receiver. A model with no pass to `return` stops with `source`. No model stops with `source unmodelled`.
6. A field read `o.F` goes back to the stores `o.F = x` before it in the same function, tagged `field F`, or to the keyed element `F` of the composite literal `o` was built from. Nothing found stops with `stopped: field F`. A package variable of an import (`os.Args`) is `source unmodelled`.
7. Conversions, `+`, index, slice and type assertions continue into their operands. Literal parts of a `+` add nothing.
8. Guards: a (def, parameter or result position) already being walked is not entered again, `--max` bounds the function crossings (`stopped: max depth` and the trailer `N routes cut at --max n`), and one walk builds at most 5000 hops.

## Printing

Rows print in reading order, the queried value first and its origins nested under it, with the shared row layout. The root keeps its tags. The trailer counts origins: `N routes from origins`, or with an end `N routes from source`. `<=` keeps one level under the root.

# Value sets

`flow::values::value_set(store, graph, file, node) -> ValueSet{values, globs, open, reasons}` answers which strings a Go expression can hold (plan decision 25). Each value and glob carries the file and line it was written at.

- A string literal adds one value, quotes removed.
- A name adds the value set of each reaching definition (the upward rule 1), so a constant adds its value and a local variable the union of its assignments.
- A parameter adds the union over every in-repo caller's argument, recursively. A parameter with no callers is open with the reason `parameter p of F has no callers`.
- A `+` concatenation multiplies out the pieces of its parts, an unknown part (open, or more than 16 alternatives) as `*`. A result with a `*` is a glob (`admin-*.html.tmpl`), one with none a value, and one that is only `*` makes the set open.
- Anything else makes the set open with the reason `<node kind> at file:line`.

# Receiver origins

`[from=...]` on a method called through a variable (`logger.info`, `l.Info`) uses the import of the call that created the variable when no import binds the name. Go walks the variable back through its reaching definitions in the file (`flow::upward::creating_import`), through chains such as `zap.NewExample().Sugar()`. JS and TS follow a declaration in the same file whose value is a call or `new` of an import (`const logger = pino()`), `require('pino')()` or a member call on such a value.
