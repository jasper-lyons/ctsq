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
- `in` is the nearest enclosing struct, class, impl, interface, module or namespace, or a Go method's receiver type. A declaration does not enclose its own name. For a Go member access through a variable (`u.Email`), `in` is the variable's type from `resolve::go::binding_type`.
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
7. `max` (default 12) bounds the whole route in edges. Without `all`, at most `limit` (default 5) routes are kept per start, shortest first. The search stops after 200000 queued paths and sets `truncated`.

Each hop carries its definition, the line where it calls the next hop, and the confidence of the edge that reached it. A guessed edge (ranked candidates, interface dispatch to several types) prints `?`.

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
