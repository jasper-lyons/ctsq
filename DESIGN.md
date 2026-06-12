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

## Verification

Proof-of-concept compiler targeting JavaScript and C:

1. `(&function#sizeof @f)` over C → matches all `sizeof` calls
2. `(*function#main @f)` over JS and Python → matches `main` definitions in both languages from one query
3. `(&function#malloc @f).params(var#ARRAY_SIZE @v)` over C → matches the specific call pattern
4. Same query over Python → empty result, no error
5. `arrow_function` over JS → passes through, matches `arrow_function` nodes directly
