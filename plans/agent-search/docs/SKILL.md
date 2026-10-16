---
name: ctsq
description: CSS-style structural code search via tree-sitter. Replaces treequery. Use for finding function definitions, call sites, class members, cross-file patterns, structural file outlines, call trees, call graphs, import graphs, and definition lookup in C++, Python, JS/TS, Rust, Go, Ruby, and more. Start with ctsq explain when the question is what a symbol is or does. Includes query-by-example (ctsq example) to derive a query from a code snippet when you don't know the syntax.
---

# ctsq — Cascading TreeSitter Queries

Replaces `treequery` entirely. Use `ctsq` for all structural code searches.

## Subcommands

```
ctsq explain <sym> [path]          what a symbol is and how it is used (start here)
ctsq -q '<query>' [FILE ...]       search (default)
ctsq files -q A -q B <path>        files where every one of the queries matches
ctsq example '<code>' -l <lang>    derive queries from an example snippet (query-by-example)
ctsq tree <file>                   structural outline of a file
ctsq callers <fn> <path>           who calls this function (walk up the call chain)
ctsq callees <fn> <path>           what this function calls (walk down the call chain)
ctsq callgraph <path>              full call graph (DOT; --format edges for plain pairs)
ctsq def <name> <path>             resolve a name to its definition location(s)
ctsq refs <name> <path>            all usage sites of a name (complement of def)
ctsq imports <file> <path>         what this file imports (forward dependency tree)
ctsq importers <file> <path>       what files import this file (reverse dependents tree)
ctsq importgraph <path>            full import graph (DOT; --format edges for plain pairs)
ctsq types [lang ...]              abstract type → tree-sitter node mapping, per language
```

### explain

**Reach for this first when the question is "what is this symbol" or "what does
it do".** One call gives the definition body, the callers, the callees, and a
count of the references left over, so there is no follow-up read of the file and
no second query.

```bash
ctsq explain NewServer src/
# SYMBOL NewServer (1 definition)
#
# DEFINITION src/server.go:14  function
# func NewServer(host string, port int) *Server {
# 	return &Server{host: host, port: port}
# }
#
# CALLERS (1 level)
#   src/main.go:19  main()  [main.go:18]  s := NewServer("localhost", 8080)
#
# CALLEES (1 level)
#   (none)
#
# REFERENCES (0 more)
```

Sections, in order:

1. `DEFINITION`: `file:line`, the kind, and the full body. Every definition
   prints when a name resolves to several, ambiguous ones marked `?`.
2. `CALLERS`: per call site, where the call is written, the function it sits
   in, where that function is defined, and the call line.
3. `CALLEES`: same shape, downward. Unresolved targets show `[external]`.
4. `REFERENCES`: a per-file count of the remaining `id#Symbol` matches, so
   you can judge whether another call is worth it.

Nothing resolves? It says so and lists the plain `id#Symbol` matches instead.

```bash
ctsq explain Server.Start src/       # qualified paths and method exprs, as with def
ctsq explain Config src/ --depth 2   # two levels of callers and callees
ctsq explain BigFn src/ --max-lines 20   # cap the body, "... (N more lines)" marks the cut
ctsq explain Config src/ --no-body   # signatures only
```

- `path` defaults to the current directory
- classes, structs and top-level variables resolve too, via the `tree` outline
- output is sorted by file then line, so runs are byte-identical

### tree

Prints a hierarchical outline of a file — classes with their methods, top-level
functions, variables. The header gives the file's size, so you can decide
whether to Read it whole. Each entry prints its signature line and how many
lines it spans.

```bash
ctsq tree src/winsys.cpp
# src/winsys.cpp  412 lines  ~4.1k tokens  3 entries
# ├── class DialogsSystem  :14  (196 lines)
# │   ├── fn ProcessMessages(HWND hwnd, UINT msg)  :22  (66 lines)
# │   └── fn IntersectWindows(Rect a, Rect b)  :88  (52 lines)
# ├── fn PushWindow(Window *w)  :210  (34 lines)
# └── var bActive  :5  (1 line)
```

Use it on a file you are **not** about to edit. For a file you are about to
edit, Read it whole instead.

Supports C, C++, Python, JavaScript, TypeScript, Rust, Go, Ruby. Language inferred from extension; override with `--lang`.

### callers / callees / callgraph

Traces call chains with cycle detection. Prints `(cycle)` instead of recursing on already-visited functions.

```bash
# Who calls IntersectWindows? (depth 3 by default)
ctsq callers IntersectWindows src/

# What does Loading() call, and what do those call?
ctsq callees Loading src/

# Limit depth for large codebases
ctsq callers ProcessMessages src/ --depth 2

# Whole-project graph: DOT by default, plain pairs with --format edges
ctsq callgraph src/ --format edges
```

Output format:
```
IntersectWindows()
├── MakeClipping()   [GVClass.cpp:38]
├── DrawRect()       [DrawForms.cpp:85] ?
│   └── ...
└── DrawHeader()     [DrawForms.cpp:360]
```

`path` can be a single file or a directory (searched recursively). Language inferred from extension; override with `--lang`.

**Call resolution (Rust and Go):** call sites resolve to real definitions, not
just matching names — through imports and aliases, qualified paths
(`util::helper()`, `pkg.Fn()`), bound function values (`f := helper; f()`),
and method receivers typed via parameters, annotations, literals,
constructors, and call return types (`x := NewServer()` / `let x = make()`).
Dynamic dispatch expands: a call through a Go interface or a Rust
`dyn Trait` / `T: Trait` / `impl Trait` receiver gets one edge per
implementing type in the project. Exact resolutions print plain; best-guess
resolutions are marked `?` (dashed edges in DOT). Other languages match by
name only: project-wide unique names resolve, everything else stays a bare
name.

### def

Where is this defined? Same resolver as `explain`, so anything `explain` finds
`def` finds, including Go structs and other types. It prints the location only.
When you want the body, the callers and the references too, `explain` costs the
same one call.

```bash
ctsq def helper src/             # bare name, lists all candidates (? if ambiguous)
ctsq def util::helper src/       # Rust path — narrows by module suffix
ctsq def Foo::method src/        # method on a specific type
ctsq def util.Helper src/        # Go package function
ctsq def Server.Run src/         # Go method (uppercase qualifier = type)
```

### files — co-occurrence across a tree

Which files mention **both** of these? Pass `-q` more than once and only files
where every query matches are listed. This is the search grep cannot express in
one call.

```bash
ctsq files -q 'id#itemCols' -q 'id#Tx' backend/internal/store/
# backend/internal/store/libraryitems.go
# backend/internal/store/assets.go
```

Narrow to one file afterwards with `ctsq explain` or `ctsq -q`. Language
inferred from extension; override with `--lang`.

### example — snippet → query

Don't know how to write the query? Paste the code you're looking for and ctsq
derives it. Candidates run most-literal → most-general, and each is verified
against the snippet before printing — every line can be copy-pasted into `-q`.

```bash
ctsq example 'u.sendEmail(msg)' -l js
# call_expression.function(member_expression.object(identifier#u).property(property_identifier#sendEmail)).arguments(arguments > (identifier#msg))  # this exact code shape
# &function#sendEmail   # calls to sendEmail
# function#sendEmail    # definitions of, or calls to, sendEmail
# &function             # any function call
```

Mark the parts to generalize with `$UPPERCASE` placeholders — everything
unmarked stays literal:

```bash
ctsq example '$X.sendEmail(msg)' -l js    # any receiver; method + arg still pinned
ctsq example 'function $F() {}' -l js     # → *function (any function definition)
```

- `--lang` is required (a snippet has no file extension); `-` reads the snippet from stdin
- placeholders inside string literals stay literal: `log("$X")` searches for the text `$X`
- bare expressions auto-wrap in a scratch function for C, C++, Go
- unmapped constructs fall back to raw node kinds: `try {} catch (e) {}` → `try_statement`
- `--debug` shows rejected candidates and which node the snippet was classified from

---

## Search (`-q`)

```
ctsq -l <lang> -q '<query>' [FILE ...]
```

Each match prints as `path:LINE: <the full source line>`, so a match is readable
without opening the file.

```bash
ctsq -q 'id#itemCols' backend/internal/store/libraryitems.go
# backend/internal/store/libraryitems.go:110: var itemCols = []string{"id", "name", "kind"}
# backend/internal/store/libraryitems.go:344: 	q := "SELECT " + strings.Join(itemCols, ", ")
```

- `-l` — language: `c`, `cpp`, `javascript`, `typescript`, `tsx`, `python`, `rust`, `go` (inferred from extension if omitted)
- `-q` — query string
- `-C N` / `--context N`: N source lines either side of each match, like `grep -C`
- `--body` — include full body text of matched nodes (default: first line only)
- `--node` — print the matched node text only, the old default
- `--show-query` — print the compiled tree-sitter S-expression
- Multiple files or directories accepted

Every line of output starts with the file it came from, including a multi-line
`--body` match (`path:START-END: <text>`) and a multi-capture match
(`path:@f:4: …, @call:6: …`). Source piped in on stdin prints as `<stdin>`.
Never open a file just to find out where a match was.

`-C N` prints the block around each match instead of the one line, which is the
call to make when you would otherwise read the file afterwards. Matched lines
keep the `path:LINE:` colon form, context lines use `path-LINE-` with dashes,
groups are separated by `--`, windows that overlap or touch merge, and lines
keep their real indentation.

```bash
ctsq -q 'id#useAuth' -C 2 frontend/src/lib/auth.tsx
# lib/auth.tsx-51- };
# lib/auth.tsx-52-
# lib/auth.tsx:53: export function useAuth(): AuthValue {
# lib/auth.tsx-54-   const value = useContext(AuthContext);
# lib/auth.tsx-55-   if (!value) throw new Error('useAuth must be used inside AuthProvider');
# --
# lib/auth.tsx-62- export const RequireAuth: ParentComponent = (props) => {
# lib/auth.tsx:63:   const auth = useAuth();
# lib/auth.tsx-64-   const navigate = useNavigate();
```

`-C` is rejected with `--node`, which already prints the whole node.

## Abstract types

```
id        any identifier — the grep replacement (see below)
module    namespace (C++), mod (Rust) — supports .body() scoping
variable  a variable declaration, with const as another name for it
function  class  var  param  type  import  literal  block
if  for  while  switch
```

Unknown identifiers pass through as raw tree-sitter node types (e.g. `translation_unit`).
An identifier that is neither an abstract type nor a real node type in the
language is rejected before any file is read:

```bash
ctsq -q '*bogus#X' file.ts
# unknown kind 'bogus' for typescript, valid kinds: id, function, call, class, module, type, var, variable, const, param, import, literal, block, if, for, while, switch, op
```

`*type#Name --body` prints a type definition in full, Go structs and type
aliases included, the same way `*class#Name --body` does for a class. Use
`*type#` for a Go struct rather than hunting for the right `class#` spelling.

`*variable#Name --body` does the same for a value: it prints the whole statement
the name is bound by, so a long `const` object comes back in one call.
`&variable#Name` stays on the uses. `const` is an alias, so `*const#Name` and
`*variable#Name` are the same query.

```bash
ctsq -q '*variable#ASSET_META' --body frontend/src/lib/data.ts
# lib/data.ts:11-19: const ASSET_META: Record<
#   AssetKind,
#   { label: string; short: string; icon: string; channel: 'page' | 'email' }
# > = {
#   landing: { label: 'Landing page', short: 'Landing', icon: 'ph-browser', channel: 'page' },
#   ...
# };
```

`variable` and `const` cover JavaScript, JSX, TypeScript, TSX (`const`, `let`,
`var`), Go (`var`, `const`, `:=`) and Rust (`let`, `const`, `static`). C, C++
and Python have no mapping and reject the kind.

## Replacing grep

| grep command | ctsq equivalent |
|---|---|
| `grep -n "Symbol" file.cpp` | `ctsq -q 'id#Symbol' file.cpp` |
| `grep -n -C 2 "Symbol" file.cpp` | `ctsq -q 'id#Symbol' -C 2 file.cpp` |
| `grep -n "A\|B\|C" file.cpp` | `ctsq -q 'id#/A|B|C/' file.cpp` |
| `grep -rn "Symbol" src/ --include=*.cpp` | `ctsq -l cpp -q 'id#Symbol' src/` |
| `grep -n "Symbol" a.cpp b.cpp` | `ctsq -q 'id#Symbol' a.cpp b.cpp` |
| `grep -n "void FuncName" file.cpp` | `ctsq -q '*function#FuncName' file.cpp` |
| `grep -rl A src/ \| xargs grep -l B` | `ctsq files -q 'id#A' -q 'id#B' src/` |

`id` finds every `identifier` node matching the name — function calls, variable uses, type references, anything, and prints each with its source line. Use `*function#Name` when you specifically want definitions.

Before reaching for `id#`, ask whether `ctsq explain Symbol path/` answers the
question. One call gives the definition body, the callers, the callees and the
remaining references.

## Sigils

```
*function   definitions only
&function   references/call sites only
function    both
```

The sigil also picks which node `type` and `variable` reach: `*` lands on the
declaration the name is introduced by, which is what `--body` needs, and `&`
stays on the bare identifier at each use.

## Name filters

```
type#Name         exact match
type#"my name"    quoted exact (allows spaces)
type#/regex/      regex match — use for OR: id#/malloc|calloc|realloc/
```

## Captures

Wrap in `()` and append `@label`:

```
(&function#malloc @c)
(*function#main @f)
```

## Field access

```
selector.body(inner)    inner anywhere in body
selector.params(inner)  inner anywhere in params
selector.body()         the body node itself
```

## Combinators

```
A B    B anywhere inside A (descendant)
A > B  B as direct child of A
A + B  adjacent siblings (emitted separately)
A ~ B  any siblings (emitted separately)
```

## Module scoping

`module` maps to C++ `namespace_definition` and Rust `mod_item`. Use `.body()` to scope a search inside a specific module.

```bash
# All functions inside namespace Core (C++)
ctsq -q '(module#Core @m).body((*function @f))' file.cpp

# Just the functions, without printing the namespace line
ctsq -q '(module#Core).body((*function @f))' file.cpp

# Call sites of malloc inside a specific namespace
ctsq -q '(module#Utils).body((&function#malloc @c))' file.cpp

# Functions referencing a constant inside a Rust mod
ctsq -q '(module#core).body((*function @f).body(id#MY_CONST))' lib.rs

# tree shows namespaces/mods as nested containers
ctsq tree src/engine.cpp
# ├── namespace Core  :1
# │   ├── fn Init  :3
# │   └── namespace Internal  :10
# │       └── fn Helper  :12
# └── fn main  :20
```

`module` is present in Ruby and Rust, absent in C, Python, JavaScript, Go — queries using it silently return no results in those languages.

## Common patterns

```bash
# What is this symbol and how is it used? Start here
ctsq explain IntersectWindows src/

# Don't know the query? Derive it from example code
ctsq example '$X.sendEmail(msg)' -l js

# Structural outline of a file
ctsq tree src/winsys.cpp

# Who calls a function across the codebase
ctsq callers IntersectWindows src/

# What a function calls (limit depth for large codebases)
ctsq callees Loading src/ --depth 2

# Files where two things both occur (grep cannot do this in one call)
ctsq files -q 'id#itemCols' -q 'id#Tx' backend/internal/store/

# A Go struct or type definition, full body
ctsq -q '*type#Server' --body backend/internal/api/server.go

# Find every occurrence of a symbol, each with its source line (replaces grep -n)
ctsq -q 'id#IntersectWindows' src/foo.cpp

# Multi-symbol OR search (replaces grep -n "A\|B\|C")
ctsq -q 'id#/malloc|calloc|realloc/' src/alloc.cpp

# All call sites of a function
ctsq -q '(&function#GetTickCount @c)' src/foo.cpp

# All functions whose body calls something
ctsq -q '(*function @f).body((&function#GetTickCount @c))' src/foo.cpp

# Functions that reference a constant — @f returns the function, results deduplicated
ctsq -q '(*function @f).body(id#MAX_SIZE)' src/foo.cpp

# Functions referencing any of several constants
ctsq -q '(*function @f).body(id#/MAX_SIZE|MIN_SIZE|BUF_LEN/)' src/foo.cpp

# Function definition, full body
ctsq -q '(*function#Update @f)' --body src/mapa.cpp

# Classes matching a regex
ctsq -q 'class#/Global/' --body src/globals.cpp

# Inspect the compiled S-expression
ctsq -l cpp --show-query -q '*function#main'
```

### Note on `.body()` with `id`

`.body(id#X)` emits one result per occurrence of `X` inside the function body. Results are deduplicated — each matching function appears once. Add `@f` on the outer function to get the function line; omit it to get each reference line.
