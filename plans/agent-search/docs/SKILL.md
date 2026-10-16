---
name: ctsq
description: CSS-style structural code search via tree-sitter. Replaces treequery. Use for finding function definitions, call sites, class members, cross-file patterns, structural file outlines, call trees, call graphs, import graphs, and definition lookup in C++, Python, JS/TS, Rust, Go, Ruby, and more. Start with ctsq explain when the question is what a symbol is or does. Includes query-by-example (ctsq example) to derive a query from a code snippet when you don't know the syntax.
---

# ctsq: Cascading TreeSitter Queries

Use `ctsq` for all structural code searches. When you are unsure where to start, run `ctsq explain Name src/`. It returns the definition, its callers, its callees and the other uses in one call.

## Worked examples

Each example is a question, the call, a trimmed real output and the next call. Outputs come from this repo's `src/`. Copy the shape and swap in your own names.

### 1. Pull a named definition

Question: show me the function `fmt_tokens`.

```
ctsq -q '*function#fmt_tokens' --body src/tree.rs
src/tree.rs:111-117: fn fmt_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
...
```

`*` means definitions only and `--body` prints the whole thing, so no Read follows. Next: find who uses it (example 3).

### 2. Pull several bodies at once

Question: show me `fmt_tokens` and `count_items` together.

```
ctsq -q '*function#/^(fmt_tokens|count_items)$/' --body src/tree.rs
src/tree.rs:111-117: fn fmt_tokens(n: usize) -> String {
...
src/tree.rs:119-121: fn count_items(items: &[Item]) -> usize {
    items.iter().map(|i| 1 + count_items(&i.children)).sum()
}
```

`/regex/` after `#` matches names, so one call replaces one call per name. Anchor with `^` and `$` or it also matches longer names. Next: see what calls them (example 6).

### 3. Find where a name is used

Question: where is `fmt_tokens` used?

```
ctsq -q 'id#fmt_tokens' src/
src/tree.rs:70: fmt_tokens(source.len() / 4),
src/tree.rs:111: fn fmt_tokens(n: usize) -> String {
2 matches
hidden: 2 in tests (--tests)
```

Each hit is `path:LINE: the source line`, so you rarely need to open the file. Add `-C 3` for three lines of context. The last two lines say two more hits sit in tests, which are hidden by default (example 12). Next: pull the function around line 70 with `ctsq -q '*function#report' --body src/tree.rs`.

### 4. Find where some text appears

Question: where does the message "valid kinds" come from?

```
ctsq -q 'literal#/valid kinds/' src/compiler
src/compiler/mod.rs:216: "unknown kind '{}' for {}, valid kinds: {}",
1 match
hidden: 1 in tests (--tests)
```

`id#` only matches identifiers, so text inside a string needs `literal#`. Next: pull the function that line 216 sits in.

```
ctsq -q '*function#validate' --body src/compiler/mod.rs
src/compiler/mod.rs:195-223: pub fn validate(query: &Query, lang: &dyn Lang, lang_name: &str) -> Result<(), String> {
...
```

### 5. Outline a file, then pull one body

Question: what is in `src/tree.rs`, and what does `count_items` do?

```
ctsq tree src/tree.rs
src/tree.rs  796 lines  ~8.4k tokens  30 entries
├── pub struct Outline {  :5
├── pub fn report(path: &str, lang_name: Option<&str>, hider: &mut crate::files::Hider) -> Outline {  :19  (58 lines)
├── fn drop_scoped(items: &mut Vec<Item>, scopes: &[(usize, usize)]) -> usize {  :95  (13 lines)
├── fn fmt_tokens(n: usize) -> String {  :111
├── fn count_items(items: &[Item]) -> usize {  :119
...
30 entries
hidden: 13 in tests (--tests)
```

The header gives the file size, so you know whether a whole Read is sensible. The `mod tests` block is left out and counted on the last line. Next: pull only the entry you want.

```
ctsq -q '*function#count_items' --body src/tree.rs
src/tree.rs:119-121: fn count_items(items: &[Item]) -> usize {
    items.iter().map(|i| 1 + count_items(&i.children)).sum()
}
```

### 6. Who calls something

Question: what calls `count_items`?

```
ctsq callers count_items src/
count_items()
├── report()  [tree.rs:19]
...
└── count_items()  [tree.rs:119] (cycle)
11 callers
hidden: 12 in tests (--tests)
```

The tree reads upward through every level. `callees` goes down and `callgraph` shows both. Next: pull the nearest caller with `ctsq -q '*function#report' --body src/tree.rs`.

### 7. Files where several names occur

Question: which files mention both `DefIndex` and `CallGraph`?

```
ctsq files -q 'id#DefIndex' -q 'id#CallGraph' src/
src/flow/upward.rs  2 1
src/semantic/flows.rs  2 1
```

The trailing numbers are the match count for each `-q`, in order. This replaces `grep -rl A | xargs grep -l B`. Next: look at the hits in one file.

```
ctsq -q 'id#/^(DefIndex|CallGraph)$/' src/flow/upward.rs
src/flow/upward.rs:24: use crate::resolve::graph::CallGraph;
src/flow/upward.rs:25: use crate::resolve::{DefIndex, DefKind, TreeStore};
...
```

### 8. What is this symbol (the default)

Question: what is `count_items` and how is it used?

```
ctsq explain count_items src/
SYMBOL count_items (1 definition)

DEFINITION src/tree.rs:119  function
fn count_items(items: &[Item]) -> usize {
    items.iter().map(|i| 1 + count_items(&i.children)).sum()
}

CALLERS (1 level)
  src/tree.rs:49  report()  [tree.rs:19]  hidden.count(why, count_items(&items));
...
```

One call returns the body, callers, callees and the remaining references. Start here whenever the question is what something is or does. Next: follow a caller or callee with another `explain`.

### 9. Find calls to a method

Question: where is `strip_prefix` called, whatever it is called on?

```
ctsq -q 'call#strip_prefix' src/compiler
src/compiler/lang/mod.rs:197: let rest = first_line.strip_prefix("#!")?.trim();
```

`call#name` matches the last segment of the callee, so `x.strip_prefix(...)`, `a::b::f(...)` and `pkg.Func(...)` are all found by `f`, `Func` or `strip_prefix` alone. `method` is another name for `function`, `string` matches string literals and `comment#/text/` matches comments, in every language. Next: pull the caller around the hit with `--body`.

### 10. Read a script with no extension

Question: what is in `fixtures/shebang/brief`?

```
ctsq tree fixtures/shebang/brief
fixtures/shebang/brief  6 lines  ~23 tokens  1 entry
└── def run_brief(name)  :4
```

A file with no extension takes its language from the `#!` line (`ruby`, `python`, `python3`, `node`, directly or through `env`), so no `--lang` is needed, and a directory walk includes such files too. A shell script has no grammar, so `bash` and `sh` files are skipped. `-l ts` also takes `.tsx` files and `-l js` takes `.jsx`. Next: pull one body with `ctsq -q '*function#run_brief' --body fixtures/shebang/brief`.

### 11. A miss that explains itself

Question: where does the message "Cannot detect" come from? The first try used `id#`, which only matches identifiers.

```
ctsq -q 'id#/Cannot detect/' src/tree.rs
no identifier matches id#/Cannot detect/ in src/tree.rs (1 file)
found as string literal:
src/tree.rs:33: None => { eprintln!("Cannot detect language. Use --lang"); std::process::exit(1); }
try: ctsq -q 'string#/Cannot detect/' src/tree.rs
[exit 1]
```

When an `id#` query matches nothing, ctsq reruns the name as a substring (or your regex) over string literals, JSX text and comments in the same files and prints one `found as` block per kind it finds, then a `try:` line whose query matches those rows. A definition query that matches nothing, such as `*function#Server` when `Server` is a type, says `Server is defined as a type here: path:line` with a `try:` line for that kind. A path that does not exist prints `no such file: X (did you mean Y)` on stderr and the other paths still run. A query that cannot parse prints the query with a caret under the fault and exits 2. A miss exits 1, and the whole report above goes to stderr with stdout left empty, which is why the output here shows it with no result rows. Next: run the `try:` line.

### 12. Hidden tests, and how to show them

Question: where is `HiddenLimit` used, tests included? Before deleting a name you need every use.

```
ctsq refs HiddenLimit fixtures/hidden
fixtures/hidden/limit.go:5: "HiddenLimit"
fixtures/hidden/limit.go:7: "HiddenLimit"
2 references
hidden: 2 in tests (--tests)
```

Test files and test scopes are hidden by default from `tree`, `-q`, `def`, `refs`, `callers`, `callees`, `explain` and `files`. Whatever a command hides is counted on `hidden:` lines after a count of what it showed, one line per reason (tests, excluded paths, over `--max-matches`), each with the flag that brings it back. Output that hid nothing has no such lines. What counts as a test is set per language: Go `*_test.go` and `testdata/`, JS and TS `*.test.*`, `*.spec.*`, `__tests__/`, `test/`, `tests/`, `e2e/`, Python `test_*.py`, `*_test.py`, `conftest.py`, `tests/`, Ruby `*_test.rb`, `*_spec.rb`, `test/`, `spec/`, C and C++ `*_test.*`, `test_*.*`, `test/`, `tests/`, and Rust `tests/`, `mod tests` and anything under `#[cfg(test)]`. Paths are judged from the path you search, so `ctsq -q X tests/` searches inside tests/ as usual, and a file you name directly is never hidden. `ctsq config dump go` shows the table, and a repo `.ctsq/languages/go.toml` with a `[tests]` table changes it. `--exclude GLOB` hides more paths the same way, counted as `in excluded paths`. When everything matched was hidden, stdout is empty and stderr reads `no match for Q outside tests in PATH (N files)` followed by the `hidden:` lines, and the exit is 1. Next: add `--tests`.

```
ctsq refs HiddenLimit fixtures/hidden --tests
fixtures/hidden/limit.go:5: "HiddenLimit"
fixtures/hidden/limit.go:7: "HiddenLimit"
fixtures/hidden/limit_test.go:3: "HiddenLimit"
fixtures/hidden/limit_test.go:5: "HiddenLimit"
```

### 13. What holds this hit

Question: `saveAssetNow` is defined inside two components. Which two?

```
ctsq def saveAssetNow fixtures/scope --scope
fixtures/scope/Asset.tsx:4: const saveAssetNow = async () => {  ?
  in fixtures/scope/Asset.tsx:3-8  export const AssetEditor = (props: Props) => {
fixtures/scope/Asset.tsx:11: const saveAssetNow = () => props.asset  ?
  in fixtures/scope/Asset.tsx:10-14  export function BrandAssetPanel(props: Props) {
2 definitions
```

`--scope` puts a row under each hit naming the nearest named scope that holds it, as `path:START-END` and the scope's first line, so no window read follows. A named scope is a function, method, class, type, Rust `impl` or `trait`, Ruby `module`, or an arrow function bound to a const. A closure is skipped and the walk goes on to its named parent. A hit at the top level has no row. It works on `def`, `refs`, `callers`, `callees`, `files` and `-q`, and the count line always prints. Add `--body` to print the scope's body under its row, indented four spaces. The same on a `-q` search:

```
ctsq -q 'id#hide_rows' --scope src/
src/main.rs:1023: hide_rows(&mut hider, file, &source, rows, true, &mut hidden)
  in src/main.rs:660-1102  fn main() {
src/main.rs:1107: fn hide_rows(
src/main.rs:1404: for r in &hide_rows(&mut hider, file, &source, rows, false, &mut name_hidden) {
  in src/main.rs:1363-1440  fn run_refs(
3 matches
```

Next: pull the scope with `ctsq -q '*function#run_refs' --body src/main.rs`.

### 14. From a line to its definition

Question: a stack trace says `src/scope.rs:395`. What function is that, and what holds it?

```
ctsq scopeof src/scope.rs:395 --budget 6
src/scope.rs:395  in  src/scope.rs:392-395  pub fn call_line(&mut self, path: &str, lang_name: &str, source: &[u8], line: usize) -> Option<Scope...
    pub fn call_line(&mut self, path: &str, lang_name: &str, source: &[u8], line: usize) -> Option<Scope> {
        let tree = self.tree(path, lang_name, source)?;
        nearest(path, lang_name, source, tree, line_start_byte(source, line))
    }
```

`def` goes from a name to a location, `scopeof` goes from a location to its definition. The heading names the line you asked about and the span of the nearest named scope holding it, then the scope's lines as they sit in the file. A line inside a closure returns the closure's named parent, and a function's own signature line returns that function. Give several coordinates in one call, `path:LINE:COL` to pin the byte. Ask for the owner of the method:

```
ctsq scopeof src/scope.rs:395 --up 1 --budget 6
src/scope.rs:395  in  src/scope.rs:372-402  impl Scopes {
    pub fn call_line(&mut self, path: &str, lang_name: &str, source: &[u8], line: usize) -> Option<Scope> {
        let tree = self.tree(path, lang_name, source)?;
        nearest(path, lang_name, source, tree, line_start_byte(source, line))
    }

    /// The scope holding a definition on `line`, stepping over a declaration
showing lines 392-397 of 372-402, --budget 31 for all
```

`--up N` climbs N enclosing scopes (past the outermost it stops there and says `only K enclosing scopes`). `--siblings` adds the other scopes in the same parent as `path:LINE: signature` rows, `--callers` adds one level of callers, `--block` returns the innermost block (an `if` or loop body) instead. A scope longer than `--budget` (default 150) prints a window of that many lines centred on your line, then a `showing lines` footer whose `--budget` value shows it all. A line outside every named scope prints `path:LINE  at top level` and a window of the file. A missing file or a line past the end goes to stderr and the other coordinates still run. Test hiding does not apply, since you named the file and line.

### 15. Which component serves this route

Question: which component renders the `/about` page? A route lives in a framework's own shape (a `<Route>` element, a route object), so a name search does not find it. Ask what named queries this repo answers:

```
ctsq find
built-in queries/react-router.toml:9:   route      react-router   (import react-router in fixtures/queries/react-router/src/App.tsx)
built-in queries/solid-routes.toml:9:   route      solid-routes   (import @solidjs/router in fixtures/queries/solid-mavexa/frontend/src/routesConfig.ts)
```

Each row names the file and line the query is defined at, the query, the framework, and why it applies here, in columns that line up. Then ask one, with a key:

```
ctsq find route about
fixtures/queries/react-router/src/main.tsx:6  /about       About    fixtures/queries/react-router/src/pages.tsx:5
fixtures/queries/solid/src/App.tsx:8          /about       About    fixtures/queries/solid/src/pages/About.tsx:1
fixtures/queries/solid/src/routes.ts:6        /about/team  Nowhere  ?
1 route (react-router, from built-in queries/react-router.toml)
2 routes (solid-routes, from built-in queries/solid-routes.toml)
hidden: 1 in tests (--tests)
```

Each match is the route's `path:LINE`, the key, the component and where `def` finds it (`?` when it finds nothing). The key is a case-insensitive substring, or a regex written `/.../`. With no key every route prints. The count line per framework names the file the query came from: when a query misses a repo's shape, copy that file to `.ctsq/queries/` in the repo, keep the `framework` name, and edit the `match` selectors, each of which also runs as a plain `-q` query. `ctsq config path` lists the queries directories. The search root is the current directory, and tests are hidden as everywhere else.

### 16. Which handler serves this route (Go, Rails, Next.js)

Question: which function handles `/users/{id}`? The same `find route` call answers it for a Go repo, because the built-in `chi` and `net-http` queries read `r.Get("/users/{id}", handler)` and `mux.HandleFunc("GET /x", h.method)`:

```
ctsq find route '/users/{id}'
fixtures/queries/go-chi/main.go:22  /users/{id}  Show        fixtures/queries/go-chi/main.go:15
fixtures/queries/go-chi/main.go:24  /users/{id}  createUser  fixtures/queries/go-chi/main.go:11
2 routes (chi, from built-in queries/go-chi.toml)
```

The target is the handler as `def` finds it, and `h.Show` gives `Show`. A Go 1.22 key keeps its method (`GET /api/healthz`). Rails reads `config/routes.rb`: `get '/about', to: 'static#about'` is rewritten to `StaticController#about` (the `rewrite` key, with the `${1:pascal}` case filter), then resolved to the controller file and the action inside it, `?` when nothing defines it. A `resources :gigs` line is one row, `/gigs`, aimed at `GigsController`. Next.js `app/**/page.tsx` files are routes whose key comes from the directory (`paths` and `key_from_path`) and whose target is the file's default export. A repo that names a module in its routes can use `target = "default-export"`, which resolves the module specifier with the tsconfig aliases and names the default export.

### 17. Which component renders this text

Question: which component shows "Welcome back"? `find text` reads the text between JSX tags and names the component or function that renders it. `find label` does the same for the string props a user reads (label, title, placeholder, alt, aria-label, aria-description, helperText, tooltip), and `find component` lists the functions that return JSX:

```
ctsq find text 'Welcome back'
fixtures/queries/jsx/src/Card.tsx:6  Welcome back  Card  fixtures/queries/jsx/src/Card.tsx:3
1 text (jsx-text, from built-in queries/jsx-text.toml)
```

The first column is where the text is, the last is where the component starts. A key is a case-insensitive substring, or a regex when written `/.../`.

### 18. Which function emits this error message

Question: a log says `failed to load pipeline 12: open x: no such file`. Which function emits it? Paste the rendered message. The code holds the format string, so `find error` turns `%s %v %d %w %q %x %f %T %+v` and `${...}` into wildcards and matches the pattern anywhere in your text. A fragment of the literal text works too, in any case:

```
ctsq find error 'failed to load pipeline 12: open x: no such file'
fixtures/queries/errors/go/load.go:14  failed to load pipeline %s: %w  Load  fixtures/queries/errors/go/load.go:10
1 error (go-error, from built-in queries/error-go.toml)
```

The target is the function that emits the error. The packs read `errors.New` and `fmt.Errorf` in Go, `new Error`, subclasses ending in Error and `throw "..."` in TypeScript and JavaScript, and `raise` in Ruby. A message built by concatenation (`"a " + name`) is not followed.

### 19. A product word that is named differently in code

Question: where is "the pipeline page" handled, when the code calls it `Pipeline`? A repo can map product words to code names in `.ctsq/glossary.toml` at its root, keys matched case-insensitively as whole phrases and values either names or `/regex/` filters:

```
.ctsq/glossary.toml
"pipeline page" = ["Pipeline"]
"brief sync"    = ["sync_merged", "cmd_sync"]
"supplies"      = ["/resource|stock/"]
```

`find`, `def` and a missed `-q` search then also search the mapped names, and the last line says so (this block is not run by the example checker, because the repo root has no glossary, `cargo test` covers it on `fixtures/glossary`):

```
$ ctsq def 'pipeline page' src
src/pipeline.go:5: func Pipeline() error {
.ctsq/glossary.toml:2: "pipeline page" also searched as Pipeline
```

### 20. Several names in one call

Question: where are `fmt_tokens` and `count_items` defined, and is `Nothingness` anywhere? One call answers each name in order. `def`, `refs`, `callers`, `callees` and `explain` take names first and then paths, like `git log NAME -- PATH`.

```
ctsq def fmt_tokens count_items Nothingness src/
src/tree.rs:111: fn fmt_tokens(n: usize) -> String {
src/tree.rs:119: fn count_items(items: &[Item]) -> usize {
not found: Nothingness
[exit 0]
```

The names that did not resolve are listed on one `not found:` line on stderr, with `(similar: x in path:line)` after a name when a near name exists. The exit is 0 when any name resolved and 1 when none did, so a chain of lookups no longer looks failed because one name missed. The test footer, if any, counts all names once at the end. An argument that exists on disk is a path, an argument with a `/` that does not exist prints `no such file: X (did you mean Y)` and is dropped, and `--` makes everything after it a path. A name that is also a file or directory here, such as `src`, is a usage error (exit 2) until you write `ctsq def src -- lib/`. For `-q` a regex name filter already covers several names: `*function#/^(A|B)$/`. Next: `ctsq explain fmt_tokens count_items src/`.

### 21. Cap a long answer

Question: a name has dozens of hits and you only need a taste. `--max-matches N` prints at most N matches, rows or entries on `-q`, `def`, `refs`, `callers`, `callees`, `explain`, `files`, `find` and `tree`, so there is no need to pipe into `head`.

```
ctsq -q 'id#fmt_tokens' src/ --max-matches 1
src/tree.rs:70: fmt_tokens(source.len() / 4),
1 match
hidden: 2 in tests (--tests)
hidden: 1 over --max-matches 1 in 1 file (--max-matches 2)
```

The cut part is counted on its own `hidden:` line, with the number of files it sits in and the `--max-matches` value that shows every hit. A cut call still exits 0 because it printed rows. Next: rerun with the suggested number.

## Reference

## Subcommands

```
ctsq explain <sym>... [path]...    what a symbol is and how it is used (start here)
ctsq -q '<query>' [FILE ...]       search (default)
ctsq files -q A -q B <path>        files where every one of the queries matches
ctsq example '<code>' -l <lang>    derive queries from an example snippet (query-by-example)
ctsq tree <file>                   structural outline of a file
ctsq callers <fn>... [path]...     who calls this function (walk up the call chain)
ctsq callees <fn>... [path]...     what this function calls (walk down the call chain)
ctsq callgraph <path>              full call graph (DOT; --format edges for plain pairs)
ctsq def <name>... [path]...      resolve a name to its definition location(s)
ctsq refs <name>... [path]...      all usage sites of a name (complement of def)
ctsq scopeof <path:line> ...       the function or class holding each line, with its body (reverse of def)
ctsq find [<query> [key]]          list the named queries (route, text, error ...) that apply here, or run one
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

Test files and test scopes (Rust `mod tests`, `#[cfg(test)]` items) are left
out, and the run ends with a count of the entries shown and a `hidden:` line
counting the entries left out. `--tests` keeps them.

Supports C, C++, Python, JavaScript, TypeScript, Rust, Go, Ruby. Language inferred from extension; override with `--lang`.

### callers / callees / callgraph

Traces call chains with cycle detection. Prints `(cycle)` instead of recursing on already-visited functions.

```bash
# Who calls IntersectWindows? (depth 3 by default)
ctsq callers IntersectWindows src/

# What does Loading() call, and what do those call?
ctsq callees Loading src/

# Limit depth for large codebases
ctsq callers ProcessMessages src/ --max-depth 2

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

### scopeof

From a `path:line` (or `path:line:col`) to the function or class holding it,
with its body. The reverse of `def`.

```bash
ctsq scopeof src/footer.rs:30
# src/footer.rs:30  in  src/footer.rs:26-32  pub struct Hidden {
```

- `--up N` climbs N enclosing scopes, `--siblings` lists the other scopes in the
  same parent, `--callers` lists callers one level, `--block` returns the
  innermost block instead of the named scope
- `--budget N` (default 150) shows a window of N lines for a longer scope
- several coordinates in one call, a bad one goes to stderr and the rest run
- exit 0 when any resolved, 1 when none did, 2 for an argument that is not a coordinate

### find

Named queries for questions whose shape is fixed per framework. `ctsq find`
lists the ones that apply in the current directory and why, `ctsq find <query>
[key]` runs one. The key is a case-insensitive substring, or `/regex/`.

```bash
ctsq find
ctsq find route /users
ctsq find text 'Welcome back'
ctsq find error 'failed to load pipeline 12: open x'
```

- built-in: `route` (Solid, React Router, Next.js app, Go net/http and chi,
  Rails), `text`, `label`, `component` (TSX and JSX), `error` (Go, TS/JS, Ruby)
- an error key matches through printf placeholders (`%s`, `%v`, `%w`, `${...}`),
  so a message copied from a log finds its source
- query files live in `.ctsq/queries/`, then `$XDG_CONFIG_HOME/ctsq/queries`,
  then `$XDG_CACHE_HOME/ctsq/queries`, then built-in (`ctsq config path`)
- `.ctsq/glossary.toml` maps product words to code names, used by `find`, `def`
  and `-q` miss reports, each saying so in a line starting with its `path:line`
- exit 0 when anything matched, 1 when nothing did, 2 for an unknown query

### --scope

On `-q`, `def`, `refs`, `callers`, `callees` and `files`. Prints
`  in path:START-END  signature` under each hit and a count line. With `--body`
the scope's body follows.

```bash
ctsq -q 'id#hide_rows' --scope src/
# src/main.rs:998: hide_rows(&mut hider, file, &source, rows, true, &mut hidden)
#   in src/main.rs:645-1069  fn main() {
```

### --tests and --exclude

Test files and test scopes are hidden in `tree`, `-q`, `def`, `refs`,
`callers`, `callees`, `explain`, `files` and `find`. Output that hid something
ends with a count line and
`hidden: N in tests (--tests), N in excluded paths (drop --exclude)`. When
everything was hidden the count reads 0 and the exit is 1.

- `--tests` shows them again
- `--exclude GLOB` hides paths matching GLOB, judged from the search root, and
  can be repeated
- a file named directly is never hidden

### files: co-occurrence across a tree

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

### example: snippet to query

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
call      a call, matched by the last segment of the callee (call#Println finds fmt.Println)
method    another name for function, in every language
string    the string literals only (literal also takes numbers)
comment   a comment, in every language
```

`function` also covers arrow functions and function expressions bound to a
`const`, `let` or `var`, and `id` also matches property, field and JSX attribute
names.

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
ctsq callees Loading src/ --max-depth 2

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
