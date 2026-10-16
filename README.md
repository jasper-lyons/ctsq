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
# What is this symbol and how is it used? Definition body, callers, callees,
# and a count of what is left, in one call
ctsq explain NewServer fixtures/
SYMBOL NewServer (1 definition)

DEFINITION fixtures/example.go:14  function
func NewServer(host string, port int) *Server {
	return &Server{host: host, port: port}
}

CALLERS (1 level)
  fixtures/example.go:19  main()  [example.go:18]  s := NewServer("localhost", 8080)

CALLEES (1 level)
  (none)

REFERENCES (0 more)

# Find all uses of a symbol — like grep but structure-aware.
# Each hit prints the whole source line, the way grep -n does.
ctsq -q 'id#malloc' fixtures/
fixtures/example.c:6: char *buf = malloc(ARRAY_SIZE);
fixtures/example.cpp:7: data_ = malloc(size);
fixtures/example.cpp:15: char *buf = (char *)malloc(ARRAY_SIZE);
fixtures/example.py:2: result = malloc(64)

# Find function definitions named "process"
ctsq -q '*function#process' fixtures/
fixtures/example.c:4: void process(int ARRAY_SIZE) {

# Find calls to malloc inside a function named "process"
ctsq -q '(*function#process @f).body((&function#malloc @call))' fixtures/
fixtures/example.c:@f:4: "void process(int ARRAY_SIZE) {", @call:6: "malloc(ARRAY_SIZE)"

# Structural outline of a file (classes, functions, variables)
ctsq tree fixtures/example.go
fixtures/example.go  21 lines  ~70 tokens  4 entries
├── type Server struct {  :5
├── func (s *Server) Start() {  :10
├── func NewServer(host string, port int) *Server {  :14
└── func main() {  :18

# Files where several names all appear — grep -rl A | xargs grep -l B
ctsq files -q 'id#NewServer' -q 'id#host' fixtures/
fixtures/example.go  2 3

# Who calls "helper" (traces up the call chain)
ctsq callers helper fixtures/
helper()
├── main()  [example.js:1]
└── main()  [example.js:10]
```

## Commands

### `explain SYMBOL [PATH]`: what a symbol is and how it is used

The first thing to reach for when the question is "what is this and what uses
it". One call replaces the usual sequence of `def`, then reading the file, then
`callers`, then `refs`.

It is composition, not new analysis: `def` resolution for the definition sites,
the `tree` outline for names the call graph has no entry for (classes, structs,
top-level variables), the call graph behind `callers` and `callees`, and an
`id#Symbol` scan for whatever is left. Those commands are unchanged.

```sh
ctsq explain NewServer fixtures/
SYMBOL NewServer (1 definition)

DEFINITION fixtures/example.go:14  function
func NewServer(host string, port int) *Server {
	return &Server{host: host, port: port}
}

CALLERS (1 level)
  fixtures/example.go:19  main()  [example.go:18]  s := NewServer("localhost", 8080)

CALLEES (1 level)
  (none)

REFERENCES (0 more)
```

Sections, in order:

1. **DEFINITION**: `file:line`, the kind of definition, and the full body
   text. Several definitions all print, each with its own body. Ambiguous
   resolutions are marked `?`, as in `def`.
2. **CALLERS**: one row per call site, giving where the call is written, the
   function it sits in, where that function is defined, and the call line
   itself.
3. **CALLEES**: the same shape, for what the symbol calls. Unresolved
   targets show as `[external]`.
4. **REFERENCES**: a per-file count of the `id#Symbol` matches not already
   shown above, so you can tell whether digging further is worth a call.

If the symbol resolves to no definition at all, the report says so and lists
the plain `id#Symbol` matches instead.

**Flags:**
- `--depth N` sets the levels of callers and callees to walk (default 1)
- `--max-lines N` caps body lines per definition, truncating with a
  `... (N more lines)` marker (default 80)
- `--no-body` prints signatures only, on a `sig:` line
- `--lang L` forces a language, as everywhere else

`PATH` defaults to the current directory. Output is sorted by file then line,
so two runs give the same bytes.

### `-q QUERY [FILES…]` — structural search

Search files or directories with an abstract query. Language is inferred from file extensions; override with `-l`.

```sh
ctsq -q 'QUERY' file.rs
ctsq -q 'QUERY' src/            # all supported files under src/
ctsq -l rust -q 'QUERY' src/   # force Rust
ctsq -q 'QUERY' a.rs b.rs      # explicit file list
```

**Stdin — file paths** (language detected per file from extension):

```sh
find . -name '*.rs' | ctsq -q 'id#Symbol'
find . -name '*.py' | ctsq -l python -q '*function'
```

**Stdin — source content** (pass `-` as the path; `--lang` required):

```sh
cat file.ts | ctsq -l typescript -q '*function' -
echo 'fn foo() {}' | ctsq -l rust -q '*function' -
```

**Output.** Each match prints as `path:LINE: <the whole source line, trimmed>`, the same rendering `explain` uses for its REFERENCES list:

```sh
ctsq -q 'id#NewServer' fixtures/example.go
fixtures/example.go:14: func NewServer(host string, port int) *Server {
fixtures/example.go:19: 	s := NewServer("localhost", 8080)
```

**Context (`-C N`).** Print N source lines either side of each match, the way `grep -C` does. Matched lines keep the `path:LINE:` form, context lines use `path-LINE-` with dashes, and groups are separated by a `--` line. Windows that overlap or touch merge into one group, and lines keep their own indentation here, because the point of asking for context is to read the block as it sits in the file:

```sh
ctsq -q 'id#NewServer' -C 2 fixtures/example.go
fixtures/example.go-12- }
fixtures/example.go-13-
fixtures/example.go:14: func NewServer(host string, port int) *Server {
fixtures/example.go-15- 	return &Server{host: host, port: port}
fixtures/example.go-16- }
fixtures/example.go-17-
fixtures/example.go-18- func main() {
fixtures/example.go:19: 	s := NewServer("localhost", 8080)
fixtures/example.go-20- 	s.Start()
fixtures/example.go-21- }
```

`-C` is rejected with exit 2 alongside `--node`, which already prints the whole node.

A match with two or more captures, and a multi-line match under `--body`, keep the node-text form instead, because one source line cannot carry them. Those get no context. They still open with the file they came from, as `path:11-19: …` for a multi-line body and `path:@f:4: …, @call:6: …` for captures, so there is never a match without a file to open. Source read from stdin says `<stdin>`. Nothing matched prints a miss report on stdout and exits 1. For a lone `id#name` the report reruns the name over string literals, JSX text and comments, prints a `found as` block per kind and a `try:` line to run:

```sh
ctsq -q 'id#localhost' fixtures/example.go
no identifier matches id#localhost in fixtures/example.go (1 file)
found as string literal:
fixtures/example.go:19: s := NewServer("localhost", 8080)
try: ctsq -q 'string#/localhost/' fixtures/example.go
```

A lone `*kind#name` that matches nothing says which other kinds define the name:

```sh
ctsq -q '*type#NewServer' fixtures/example.go
no definition matches *type#NewServer in fixtures/example.go (1 file)
NewServer is defined as a function here: fixtures/example.go:14
try: ctsq -q '*function#NewServer' fixtures/example.go
```

**Query errors.** A query that does not parse prints the query with a caret under the fault and a hint, and exits 2:

```sh
ctsq -q 'id#(' fixtures/example.go
query error: expected a name after # at position 3
  id#(
     ^
a name is an identifier, "quoted text" or /regex/, as in id#foo, id#"a b" or id#/a|b/
```

**Paths.** A path that does not exist prints `no such file: X` on stderr (with `(did you mean Y)` when a close name exists) and the other paths still run.

**Scope (`--scope`).** Under each match, print the nearest named scope holding it as `  in path:START-END  signature`, then a count line. With `--body` the scope's body follows. `def`, `refs`, `callers`, `callees` and `files` take `--scope` too.

```sh
ctsq -q '*function#NewServer' --scope fixtures/example.go
fixtures/example.go:14: func NewServer(host string, port int) *Server {
1 match
```

**Flags:**
- `-C N`, `--context N`: print N source lines either side of each match, like `grep -C`
- `--scope`: print the function or class holding each match under it
- `--show-query` — print the compiled TreeSitter S-expression
- `--body` — show the full body of matched nodes (default: first line only)
- `--node` — print the matched node's own text instead of the source line (the pre-0.2 form)
- `--no-ignore` — ignore `.gitignore` / `.ignore` files
- `--tests`: show test files and test scopes (hidden by default, see below)
- `--exclude GLOB`: hide paths matching GLOB, judged relative to the search root (repeatable)

### Hidden tests and the footer

Test files and test scopes (Rust `mod tests` and `#[cfg(test)]` items, Go `_test.go` files and the like) are hidden by default in `tree`, `-q`, `def`, `refs`, `callers`, `callees`, `explain`, `files` and `find`. Which paths and scopes count as tests is set per language in the `[tests]` table of `config/languages/*.toml`, and `ctsq config dump go` shows the merged result. A file named directly on the command line is never hidden.

When something was hidden, the output ends with a count line and a `hidden:` line naming each count and the flag that shows it. When everything was hidden the count reads 0 and the exit is 1.

```sh
ctsq refs HiddenLimit fixtures/hidden
fixtures/hidden/limit.go:5: "HiddenLimit"
fixtures/hidden/limit.go:7: "HiddenLimit"
2 references
hidden: 2 in tests (--tests)

ctsq -q 'id#HiddenLimit' fixtures/hidden --exclude 'limit*'
0 matches
hidden: 4 in excluded paths (drop --exclude)
```

`--tests` shows the test hits again. `--exclude GLOB` hides more paths and can be given more than once.

### `tree [PATHS…]` — structural outline

Print a structural outline (classes → methods, top-level functions, variables).

Each file starts with a header line: path, line count, rough token count (bytes over four), and how many entries follow. Each entry prints its declaration line, cut at 100 characters, then the line it starts on, then a `(N lines)` span for anything over ten lines.

```sh
ctsq tree fixtures/example.go
fixtures/example.go  21 lines  ~70 tokens  4 entries
├── type Server struct {  :5
├── func (s *Server) Start() {  :10
├── func NewServer(host string, port int) *Server {  :14
└── func main() {  :18
```

There is no `--body` here: `tree` is the outline. For one entry's body use `ctsq -q '*function#Name' --body FILE`.

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

Resolution runs the two steps `explain` runs, in the same order: the call-graph definition index first, then the `tree` outline for the definitions the index cannot hold, such as a struct, a class or a type alias. Nothing found prints a miss report on stdout, with up to three names that look close, and exits 1:

```sh
ctsq def NewServe fixtures/
no definition for 'NewServe' in fixtures/ (28 files scanned)
similar names: NewServer
```

A name the glossary maps (see Named queries and the glossary) also searches the code names it maps to, and a last line says so.

`--scope` adds the holding function under each definition:

```sh
ctsq def saveAssetNow fixtures/scope --scope
fixtures/scope/Asset.tsx:4: const saveAssetNow = async () => {  ?
  in fixtures/scope/Asset.tsx:3-8  export const AssetEditor = (props: Props) => {
fixtures/scope/Asset.tsx:11: const saveAssetNow = () => props.asset  ?
  in fixtures/scope/Asset.tsx:10-14  export function BrandAssetPanel(props: Props) {
2 definitions
```

### `refs NAME PATH`: usage sites

Every place a name is used, the complement of `def`. It takes `--scope`, `--tests` and `--exclude` like the commands above.

```sh
ctsq refs NewServer fixtures/example.go
fixtures/example.go:14: "NewServer"
fixtures/example.go:19: "NewServer"
```

### `scopeof PATH:LINE[:COL]…`: from a line to its definition

`def` goes from a name to a location, `scopeof` goes from a location to the function or class holding it. It takes the `path:line` that compilers, stack traces and `grep` print.

```sh
ctsq scopeof fixtures/example.go:19
fixtures/example.go:19  in  fixtures/example.go:18-21  func main() {
func main() {
	s := NewServer("localhost", 8080)
	s.Start()
}
```

**Flags:**
- `--up N`: climb N enclosing named scopes (method, then class, then module)
- `--siblings`: after the body, list the other scopes in the same parent, signature only
- `--callers`: after the body, list the callers of the scope, one level
- `--block`: return the innermost block (an `if` or loop body) instead of the named scope
- `--budget N`: a scope longer than N lines prints a window of N lines centred on the line (default 150)

A line outside every named scope prints `path:LINE  at top level` and a window of the file. Closures are not named scopes, so a line inside one returns its named parent. A missing file or a line past the end goes to stderr and the other coordinates still run. Exit 0 when any coordinate resolved, 1 when none did, 2 when an argument is not a coordinate. Test hiding does not apply.

### `files -q QUERY [-q QUERY…] [PATH]` — cross-file co-occurrence

Files where every one of several queries matches at least once. This is the shape `grep -rl A | xargs grep -l B` had and a single query cannot express.

```sh
ctsq files -q 'id#NewServer' -q 'id#host' fixtures/
fixtures/example.go  2 3
```

The trailing column is the per-query match count, in the order the `-q` flags were given. Repeat `-q` once per query. Nothing matching every query prints a miss report on stdout and exits 1.

### `imports FILE PATH` — import chain (down)

What files a file imports, traced down the dependency tree (the file-level
analogue of `callees`). `PATH` scopes resolution.

```sh
ctsq imports src/main.rs src/
src/main.rs
├── clap  (external)
├── mod.rs  [src/compiler/lang/mod.rs]
└── std  (external)
```

### `importers FILE PATH` — import chain (up)

What files import a file, traced up the dependents tree (the analogue of
`callers`).

```sh
ctsq importers src/files.rs src/
src/files.rs
├── calltree.rs  [src/calltree.rs]
└── importgraph.rs  [src/importgraph.rs]
```

### `importgraph PATH` — full import graph

Build a file-level import graph for a path.

```sh
ctsq importgraph src/                   # DOT format (pipe to dot -Tpng)
ctsq importgraph src/ --format edges    # plain "source -> target" pairs
```

Resolution is per language: C/C++ `#include`, JS/TS relative paths (with
extension and `index` probing, plus `require`, dynamic `import()`, and
`export … from`), Python dotted and relative imports, Ruby `require` /
`require_relative` / `load` calls, and Rust `use` paths.
JS/TS path aliases (`~/…`, `@/…`, `@app/…`) resolve through the nearest
`tsconfig.json` / `jsconfig.json` (`baseUrl` + `paths`, following `extends`);
projects without one fall back to matching the specifier's path tail.
A Go import names a package (a directory), so Go edges point at the specific
member file whose symbols are used, tagged with the package. Externals and
unresolved specifiers appear as `(external)` leaves; ambiguous resolutions are
marked with `?`.

### `example SNIPPET` — query-by-example

Don't know the query? Paste the code you're looking for and ctsq derives it,
by inverting the same abstract-type mappings the compiler uses. Candidates are
printed from most specific to most general, and each one is self-verified
against the snippet before it's shown — every line is a query that provably
matches your example.

```sh
$ ctsq example 'u.sendEmail()' -l js
&function#sendEmail  # calls to sendEmail
function#sendEmail   # definitions of, or calls to, sendEmail
&function            # any function call

$ ctsq example 'def greet(name): pass' -l python
*function#greet  # definitions of greet
function#greet   # definitions of, or calls to, greet
*function        # any function definition
```

The first candidate is a **structural query**: raw node kinds with field
accesses that pin every named part of the snippet literally. Mark the parts
you *don't* care about with `$UPPERCASE` placeholders and they become
unconstrained, while everything unmarked stays literal:

```sh
$ ctsq example '$X.sendEmail(msg)' -l js
call_expression.function(member_expression.property(property_identifier#sendEmail)).arguments(arguments > (identifier#msg))  # this code shape, for any $X
&function#sendEmail                                                                                                          # calls to sendEmail
…
```

Here any receiver matches (`$X`), but the method name and the `msg` argument
are still required. Placeholders inside string literals are left alone
(`log("$X")` searches for the literal text `$X`).

`--lang` is required (a snippet has no file extension). Use `-` to read the
snippet from stdin. Bare expressions are auto-wrapped in a scratch function
for languages that reject them at top level (C, C++, Go). Constructs with no
abstract mapping fall back to raw tree-sitter passthrough queries
(`try {} catch (e) {}` → `try_statement`). `--debug` shows candidates that
failed self-verification and which node the snippet was classified from.

## Named queries and the glossary

A named query answers a question whose shape is fixed per framework, such as which component serves a route. `ctsq find` with no arguments lists the named queries that apply in the current directory and why, and `ctsq find QUERY [KEY]` runs one. The key is a case-insensitive substring, or a regex when written `/…/`.

```sh
cd fixtures/queries/go-chi
ctsq find route /users
main.go:20  /users       listUsers   main.go:9
main.go:21  /users       createUser  main.go:11
main.go:22  /users/{id}  Show        main.go:15
main.go:24  /users/{id}  createUser  main.go:11
4 routes (chi, from built-in queries/go-chi.toml)
```

Each row is the file and line of the match, the key, the target, and the target's definition (`?` when `def` finds none). A count line per framework names the file the query came from. Exit 0 when anything matched, 1 when nothing did, 2 for an unknown query or a query file that does not compile.

Built-in packs, in `config/queries/`:

| Query | Frameworks and languages |
|-------|--------------------------|
| `route` | Solid, React Router, Next.js app router, Go `net/http` and chi, Rails |
| `text`, `label`, `component` | JSX and TSX: the component rendering some text between tags, the component rendering a label, title, placeholder or alt prop, and the components themselves |
| `error` | Go, TypeScript and JavaScript, Ruby: the function emitting an error message. The key is matched as a format string, so `%s`, `%v`, `%d`, `%w` and `${…}` match any text and a message copied from a log finds its source |

Queries are TOML files looked up in `.ctsq/queries/` in the repo, then `$XDG_CONFIG_HOME/ctsq/queries`, then `$XDG_CACHE_HOME/ctsq/queries`, then the built-in packs. `ctsq config path` lists the directories. A repo file with the same framework and query name replaces the built-in one. Each file has these fields:

- `framework`, `langs`: the name shown in output and the languages it applies to
- `detect`: when it applies, by `import`, `file` or `dependency`
- `[query]` table, named for the query (such as `[route]`): `match` is a selector or a list of them with `@key` and `@target` captures, `target` is `def`, `enclosing`, `self` or `default-export`, and the optional `rewrite`, `key_rewrite`, `paths` and `key_from_path` shape the key and target

An example, saved as `.ctsq/queries/handlers.toml` in a Go repo:

```toml
# Package-level http.HandleFunc("/path", handler) calls.
framework = "plain-http"
langs     = ["go"]
detect    = { file = ["main.go"] }

[handler]
match = [
  "(call_expression.function(selector_expression.operand(identifier#http).field(field_identifier#HandleFunc))) > (argument_list > (interpreted_string_literal @key) + (identifier @target))",
]
target = "def"
```

```sh
ctsq find handler
main.go:30  /  index  main.go:17
1 handler (plain-http, from .ctsq/queries/handlers.toml)
```

**Glossary.** A repo file `.ctsq/glossary.toml` maps product words to code names:

```toml
"pipeline page" = ["Pipeline"]
"supplies"      = ["/resource|stock/"]
```

`find`, `def` and the `-q` miss reports also search the code names for a word the glossary holds, and a last line starting with the glossary's `path:line` says so:

```sh
cd fixtures/glossary
ctsq def 'pipeline page' .
./src/pipeline.go:5: func Pipeline() error {
.ctsq/glossary.toml:2: "pipeline page" also searched as Pipeline
```

## Query language

### Abstract node types

| Type | Matches |
|------|---------|
| `id` | any identifier, including property and field identifiers and JSX attribute names |
| `function` | function definitions and/or calls, including function expressions bound to a `const`, `let` or `var` |
| `method` | alias of `function` |
| `call` | calls by the last segment of the callee: `call#Println` finds `fmt.Println(…)` and `call#toBe` finds `a.toBe()` |
| `string` | string literals |
| `comment` | comments |
| `class` | class / struct definitions |
| `var` | variable declarations and/or uses |
| `variable` (alias `const`) | the declaration statement a name is bound by (`*variable`) and its uses (`&variable`) |
| `param` | function parameters |
| `type` | type declarations (`*type`) and type annotations (`&type`) |
| `import` | import / use statements |
| `literal` | string, number, boolean literals |
| `block` | block / body nodes |
| `if` `for` `while` `switch` | control-flow nodes |

Unknown identifiers pass through to TreeSitter verbatim as concrete node names. An identifier that is neither an abstract type nor a real node type in the language is rejected before anything runs:

```sh
ctsq -q '*bogus#X' fixtures/typescript/example.ts
unknown kind 'bogus' for typescript, valid kinds: id, function, call, class, module, type, var, variable, const, param, import, literal, block, if, for, while, switch, op, field, method, comment
```

An unknown field name after a `.` (as in `.bodyy(…)`) is a caret error like the parse errors above.

`variable` and `const` are the same kind under two names. They cover JavaScript, TypeScript, JSX and TSX (`lexical_declaration`, `variable_declaration`), Go (`var`, `const` and `:=`) and Rust (`let`, `const`, `static`). C, C++ and Python have no mapping: a C declaration mixes variables, prototypes and pointer declarators in one node, so `*variable#X` there would need real disambiguation rather than a table entry.

### Sigils (scope)

| Sigil | Meaning |
|-------|---------|
| `*type` | definitions only |
| `&type` | references / calls only |
| _(none)_ | both (widest net) |

The sigil changes which node the abstract `type` reaches. `*type#Name` matches the declaration the name is introduced by, so `--body` prints the whole thing: `type_spec` and `type_alias` in Go, `type_alias_declaration` and `interface_declaration` in TypeScript, `struct_item` / `enum_item` / `trait_item` / `union_item` / `type_item` in Rust, and the tagged specifiers plus `typedef` in C and C++. `&type#Name` matches the bare `type_identifier`, which is the annotation site.

`variable` works the same way. `*variable#Name` matches the statement the name is bound by, so `--body` on a TypeScript `const Name = { … }` prints the whole object. `&variable#Name` matches the bare identifier, which is every use.

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
A + B  adjacent    B is the next sibling of A
A ~ B  sibling     B is a later sibling of A in the same parent
```

### Captures and field access

Wrap a selector in `()` to capture it or access a specific field:

```
(&function#malloc @call)               capture the call site as @call
(*function#init @f).body(id#CONST)    match CONST inside init's body
(*function @f).body()                 the body node itself
selector.params(var#SIZE @v)          match SIZE in the params field
selector.field(@cap)                  bare @cap as a field argument: capture whatever the field holds
selector.field(kind @cap)             capture the field's node when it is of that kind
```

### Examples

```sh
# grep -n "ARRAY_SIZE" example.c
ctsq -q 'id#ARRAY_SIZE' fixtures/example.c
fixtures/example.c:4: void process(int ARRAY_SIZE) {
fixtures/example.c:6: char *buf = malloc(ARRAY_SIZE);

# grep -n "ARRAY_SIZE\|malloc" example.c
ctsq -q 'id#/ARRAY_SIZE|malloc/' fixtures/example.c
fixtures/example.c:4: void process(int ARRAY_SIZE) {
fixtures/example.c:6: char *buf = malloc(ARRAY_SIZE);

# grep -rn "malloc" src/ --include=*.cpp
ctsq -l cpp -q 'id#malloc' fixtures/
fixtures/example.cpp:7: data_ = malloc(size);
fixtures/example.cpp:15: char *buf = (char *)malloc(ARRAY_SIZE);

# all definitions of "process"
ctsq -q '*function#process' fixtures/example.c
fixtures/example.c:4: void process(int ARRAY_SIZE) {

# the match with a line either side of it, like grep -C 1
ctsq -q '*function#NewServer' -C 1 fixtures/example.go
fixtures/example.go-13-
fixtures/example.go:14: func NewServer(host string, port int) *Server {
fixtures/example.go-15- 	return &Server{host: host, port: port}

# the whole declaration a type name is introduced by
ctsq -q '*type#Server' --body fixtures/example.go
fixtures/example.go:5-8: Server struct {
	host string
	port int
}

# the whole statement a variable name is bound by
ctsq -q '*variable#defaults' --body fixtures/typescript/example.ts
fixtures/typescript/example.ts:38-41: const defaults = {
  retries: 3,
  label: "ctsq",
};

# functions that reference ARRAY_SIZE
ctsq -q '(*function @f).body(id#ARRAY_SIZE)' fixtures/example.c
fixtures/example.c:4: void process(int ARRAY_SIZE) {

# calls to malloc inside any function definition
ctsq -q '(*function @f).body((&function#malloc @call))' fixtures/example.c
fixtures/example.c:@f:4: "void process(int ARRAY_SIZE) {", @call:6: "malloc(ARRAY_SIZE)"

# not sure what to write? derive the query from example code
ctsq example 'malloc(n)' -l c
&function#malloc  # calls to malloc
function#malloc   # definitions of, or calls to, malloc
&function         # any function call
```

## Supported languages

| Language | Extensions |
|----------|-----------|
| C | `.c` `.h` |
| C++ | `.cpp` `.cc` `.cxx` `.hpp` `.hxx` |
| JavaScript | `.js` `.mjs` `.cjs` `.jsx` |
| TypeScript | `.ts` `.tsx` |
| Python | `.py` |
| Ruby | `.rb` `.rake` `.gemspec` |
| Rust | `.rs` |
| Go | `.go` |

`-l ts` includes `.tsx` and `-l js` includes `.jsx`, and each file is parsed by its own grammar. A script with no extension is detected by its shebang line (Ruby, Python and Node), so `ctsq -q '*function' fixtures/shebang` reads the scripts there.

## Call resolution

For **Rust** and **Go**, call sites resolve through imports/aliases, qualified paths, bound function values, and method receivers typed via params, annotations, literals, constructors, and call return types. Dynamic dispatch expands: a call through a Go interface or a Rust `dyn Trait` / `T: Trait` / `impl Trait` receiver gets one edge per implementing type. Ambiguous calls resolve to the best-ranked candidate (same file > same dir > imported) and are marked with `?`.

Other languages use name-only matching: unique names resolve exactly; ambiguous ones stay bare.
