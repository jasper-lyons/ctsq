# ctsq — Cascading TreeSitter Queries

Cross-language structural code search via TreeSitter.

## The problem

TreeSitter is a great parsing library, but its query language uses grammar-specific node names: `function_declaration` in JavaScript, `function_definition` in Python, `function_item` in Rust. You can't write one query that works across languages.

ctsq solves this with a fixed set of abstract node types (`function`, `class`, `var`, …) that compile down to the correct TreeSitter S-expressions per language. One query, any language.

## Install

```sh
cargo build --release
make install          # installs to ~/.local/bin/ctsq
```

Requires Rust + Cargo.

## Quick start

```sh
# Find all uses of a symbol — like grep but structure-aware
ctsq -q 'id#malloc' fixtures/
fixtures/example.c:<match>:6: "malloc"
fixtures/example.cpp:<match>:7: "malloc"
fixtures/example.cpp:<match>:15: "malloc"
fixtures/example.py:<match>:2: "malloc"

# Find function definitions named "process"
ctsq -q '*function#process' fixtures/
fixtures/example.c:<match>:4: "void process(int ARRAY_SIZE) {"
fixtures/example.js:<match>:5: "() => {"

# Find calls to malloc inside a function named "process"
ctsq -q '(*function#process @f).body((&function#malloc @call))' fixtures/
fixtures/example.c:@f:4: "void process(int ARRAY_SIZE) {", @call:6: "malloc(ARRAY_SIZE)"

# Structural outline of a file (classes, functions, variables)
ctsq tree fixtures/example.go
fixtures/example.go
├── type Server  :5
├── fn Start  :10
├── fn NewServer  :14
└── fn main  :18

# Who calls "helper" (traces up the call chain)
ctsq callers helper fixtures/
helper()
├── main()  [example.js:1]
└── main()  [example.js:10]
```

## Commands

### `-q QUERY [FILES…]` — structural search

Search files or directories with an abstract query. Language is inferred from file extensions; override with `-l`.

```sh
ctsq -q 'QUERY' file.rs
ctsq -q 'QUERY' src/            # all supported files under src/
ctsq -l rust -q 'QUERY' src/   # force Rust
```

**Flags:**
- `--show-query` — print the compiled TreeSitter S-expression
- `--body` — show the full body of matched nodes (default: first line only)
- `--no-ignore` — ignore `.gitignore` / `.ignore` files

### `tree [PATHS…]` — structural outline

Print a structural outline (classes → methods, top-level functions, variables).

```sh
ctsq tree fixtures/example.go
fixtures/example.go
├── type Server  :5
├── fn Start  :10
├── fn NewServer  :14
└── fn main  :18
```

### `callers NAME PATH` — call chain (up)

Show what calls a given function, walking up the call chain.

```sh
ctsq callers helper fixtures/
helper()
├── main()  [example.js:1]
└── main()  [example.js:10]

ctsq callers Start fixtures/ --depth 5   # default depth: 3
Start()
└── main()  [example.go:18]
```

### `callees NAME PATH` — call chain (down)

Show what a function calls, walking down the call chain.

```sh
ctsq callees main fixtures/example.go
main()
├── NewServer()  [example.go:14]
└── Start()  [example.go:10]
    └── Println()
```

### `callgraph PATH` — full call graph

Build a call graph for all functions in a path.

```sh
ctsq callgraph fixtures/                   # DOT format (pipe to dot -Tpng)
ctsq callgraph fixtures/ --format edges    # plain "caller -> callee" pairs
example.c:10 -> process
example.c:4 -> malloc
example.go:18 -> example.go:10
example.go:18 -> example.go:14
example.js:1 -> example.js:5
…
```

### `def NAME PATH` — definition lookup

Resolve a name to its definition location(s).

```sh
ctsq def helper fixtures/
fixtures/example.js:5: const helper = () => {

ctsq def NewServer fixtures/
fixtures/example.go:14: func NewServer(host string, port int) *Server {
```

## Query language

### Abstract node types

| Type | Matches |
|------|---------|
| `id` | any identifier |
| `function` | function definitions and/or calls |
| `class` | class / struct definitions |
| `var` | variable declarations and/or uses |
| `param` | function parameters |
| `type` | type annotations |
| `import` | import / use statements |
| `literal` | string, number, boolean literals |
| `block` | block / body nodes |
| `if` `for` `while` `switch` | control-flow nodes |

Unknown identifiers pass through to TreeSitter verbatim as concrete node names.

### Sigils (scope)

| Sigil | Meaning |
|-------|---------|
| `*type` | definitions only |
| `&type` | references / calls only |
| _(none)_ | both (widest net) |

### Name matching

```
type#name        exact match
type#"my name"   exact match (allows spaces)
type#/regex/     regex match
```

### Combinators

```
A B    descendant  — B anywhere inside A
A > B  child       — B as a direct child of A
A + B  adjacent    — A and B are adjacent siblings
A ~ B  sibling     — A and B are ordered siblings (B anywhere after A, same parent)
```

### Captures and field access

Wrap a selector in `()` to capture it or access a specific field:

```
(&function#malloc @call)               capture the call site as @call
(*function#init @f).body(id#CONST)    match CONST inside init's body
(*function @f).body()                 the body node itself
selector.params(var#SIZE @v)          match SIZE in the params field
```

### Examples

```sh
# grep -n "ARRAY_SIZE" example.c
ctsq -q 'id#ARRAY_SIZE' fixtures/example.c
<match>:4: "ARRAY_SIZE"
<match>:6: "ARRAY_SIZE"

# grep -n "ARRAY_SIZE\|malloc" example.c
ctsq -q 'id#/ARRAY_SIZE|malloc/' fixtures/example.c
<match>:4: "ARRAY_SIZE"
<match>:6: "malloc"
<match>:6: "ARRAY_SIZE"

# grep -rn "malloc" src/ --include=*.cpp
ctsq -l cpp -q 'id#malloc' fixtures/
<match>:7: "malloc"
<match>:15: "malloc"

# all definitions of "process"
ctsq -q '*function#process' fixtures/example.c
<match>:4: "void process(int ARRAY_SIZE) {"

# functions that reference ARRAY_SIZE
ctsq -q '(*function @f).body(id#ARRAY_SIZE)' fixtures/example.c
@f:4: "void process(int ARRAY_SIZE) {"

# calls to malloc inside any function definition
ctsq -q '(*function @f).body((&function#malloc @call))' fixtures/example.c
@f:4: "void process(int ARRAY_SIZE) {", @call:6: "malloc(ARRAY_SIZE)"
```

## Supported languages

| Language | Extensions |
|----------|-----------|
| C | `.c` `.h` |
| C++ | `.cpp` `.cc` `.cxx` `.hpp` `.hxx` |
| JavaScript | `.js` `.mjs` `.cjs` `.jsx` |
| Python | `.py` |
| Rust | `.rs` |
| Go | `.go` |

## Call resolution

For **Rust** and **Go**, call sites resolve through imports/aliases, qualified paths, bound function values, and method receivers typed via params, annotations, literals, constructors, and call return types. Dynamic dispatch expands: a call through a Go interface or a Rust `dyn Trait` / `T: Trait` / `impl Trait` receiver gets one edge per implementing type. Ambiguous calls resolve to the best-ranked candidate (same file > same dir > imported) and are marked with `?`.

Other languages use name-only matching: unique names resolve exactly; ambiguous ones stay bare.
