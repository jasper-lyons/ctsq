# ctsq for agents: fewer calls per answer

Planned 2026-10-16 from 60 days of Claude Code session logs (1,723 sessions, 30,296 code-search calls). Evidence is summarised at the end. The scripts that produced it are in `measure/`.

## Goal

An agent searching code should reach the definition it needs in one or two ctsq calls, without falling back to grep, `sed -n` windows or whole-file reads.

Three measures, re-run with `measure/` after each package and recorded in `DELIVERY.md`:

| Measure | Baseline |
|---|---|
| ctsq calls followed by a Read of a file the call had just named | 1,437 of 15,475 |
| `-q` queries that error or match nothing | 10% error, 11% empty |
| Share of code characters agents take in through whole-file reads | 44% |

Example calls this plan must make work:

```
ctsq def saveAssetNow --scope frontend/src
ctsq scopeof internal/editturn/editturn.go:54 backend/x.go:120
ctsq -q 'id#/Still reading your files/' frontend/src     # misses as id, reports the literal hits
ctsq -q '*function#/^(BrandMark|Topbar)$/' --body src/    # arrow consts in TSX count as functions
ctsq def LoadUnits LoadUnits3 GameKeyCheck src/           # several names, one call
ctsq find route /library
ctsq find text "Live preview"
ctsq find error "no JSON object in reply"
ctsq tree backend/internal/pagedoc/                       # tests hidden, footer says how many
```

## Decisions

### 01. Adoption is fixed with complete examples, not new entry points

Agents under-use what ctsq already has. `explain` is 6% of calls although the docs name it first. `files` and `callers` are about 1% each. The docs show syntax, not a worked task. Every agent-facing doc (the ctsq skill, `ctsq --help`, the ctsq block in `~/.claude/CLAUDE.md`) gets complete examples, one per goal: the question, the call, a trimmed output, the next call. Why: agents copy examples. Beat: changing `tree` on directories into a package digest so agents land on it by habit.

### 02. Tests are hidden by default

`tree`, `-q`, `def`, `refs`, `callers`, `callees`, `explain`, `files` and `find` skip test files and test scopes unless `--tests` is passed. Every hidden hit is counted in the footer (decision 03). Why: 894 greps on ctsq output removed paths or tests, and 34 greps on directory trees did the same. The footer keeps "find every reference before deleting" safe, because the agent sees `4 hits in tests hidden`. What counts as a test is set per language in the language TOML (see Schema).

### 03. Every cut says what it hid and how to get it

Any output that drops something (tests, matches over `--max`, excluded paths) ends with footer lines, one per reason, naming the count and the flag that restores it. Why: about 8,700 ctsq calls pipe into `head`, which cuts silently. 254 of 404 `head` cuts on directory trees kept 40 lines or fewer, and 42 were followed by the same `tree` again. A footer lets the agent ask for more on purpose, and teaches the flags at the moment they are needed.

### 04. A miss explains itself

When an `id#` query matches nothing, ctsq runs the same name filter against literals and comments and reports where it does occur. When a kind or name matches nothing in a file but exists under another kind (`*function#X` where X is a `*variable`), it says so. Query errors print the query with a caret under the fault and a suggested fix, never a raw tree-sitter message. Why: `id#` used on text was the commonest wrong first move (8 of 13 wrong openings in a 120-episode sample), and query parse errors were the top error ctsq caused itself.

### 05. Scope is a flag on every command that returns hits

`--scope` prints, under each hit, the nearest named scope that holds it: its signature and its `path:START-END` span. `--scope --body` adds the scope's body. Works on `def`, `refs`, `callers`, `callees`, `files` and `-q`. Why: a name can be defined in many scopes (a local const inside several components), and agents read a window around nearly every hit to learn what function it sits in. Signature and span by default because a body per hit recreates the large outputs this plan removes. Beat: printing the scope on every hit by default (too much output for plain locate queries).

### 06. `scopeof` takes coordinates only

`ctsq scopeof path:line [path:line ...]` returns the nearest named scope (function, method, class, type, or a const bound to a function) holding each line, with its body. Options:

- `--up N` climbs N enclosing named scopes (method, then class, then module)
- `--siblings` lists the neighbouring definitions in the same parent, signature only
- `--callers` adds the callers of the scope, one level
- `--block` returns the innermost block (an `if` or loop body) instead of the named scope
- `--budget N` (default 150 lines): a scope over budget prints its signature, then a window of the budget's size centred on the line

Accepts `path:line` and `path:line:col` as printed by ctsq, grep, compilers and stack traces. Help text pairs it with `def`: `def` goes from a name to a location, `scopeof` from a location to its definition. Why: "show me lines N to M" was the third most common question (50 of about 490 steps), and coordinates also arrive from grep, stack traces and test failures. `--siblings` and `--callers` each fix a wrong anchor seen in the logs (`LoadUnits` beside `LoadUnits3`, a key handler whose owning mode was not visible). Beat: `ctsq at` (does not say what comes back), `scopeof Name` (ambiguous, covered by decision 05).

### 07. Several names in one call, partial success exits 0

`def`, `refs`, `callers`, `callees` and `explain` accept several names. ctsq answers each in order, lists the names that did not resolve, and exits 0 if any resolved. For `-q`, a regex name filter already does this (`*function#/^(A|B)$/`), and the docs say so. Why: agents chain `ctsq def A; ctsq def B; ctsq def C`, and one miss made the chain look failed, so they retried calls that had worked. A body call is followed by another body call 36% of the time.

### 08. Kinds follow how agents name things

Aliases and wider matches, per language:

- `function` includes arrow functions and function expressions bound to a `const`, `let` or `var` (JS, TS, TSX)
- `method` is accepted in every language, as an alias of `function` where the language has no separate kind (Go, Ruby)
- `string` is an alias of `literal`
- `id` includes property identifiers, field identifiers and JSX attribute names
- `call` matches the last segment of a member call (`pkg.Func(...)`, `it(...)`)
- `-l ts` and `-l typescript` include `.tsx`, `-l js` includes `.jsx`
- `comment` is a kind in every language
- language is detected from the shebang line when a file has no extension
- C++ definitions behind export macros (`extern "C" __declspec(dllexport)`) parse as functions

Why: each appeared as a first-try miss in the samples. Together they account for most of the 25 to 40% first-try miss rates on "what does X do" and "where else is X used".

### 09. Named queries live in TOML, in the existing lookup order

A third kind of config file, beside language mechanics and models, defines named queries: a question with a name (`route`, `text`, `label`, `component`, `error`) and how to answer it for one framework. Lookup order is unchanged: repo `.ctsq/`, then `~/.config/ctsq`, then cache, then built-in. A repo file with the same query name and framework replaces the built-in one. Why: 17 of the 19 anchors in description-started searches were things a framework-aware query could find (UI text, labels, routes, components, config keys, error messages). Their shapes are fixed per framework, and per-repo overrides handle the rest (the Solid app in Mavexa keeps routes in one lazy-import array that a generic React Router query misses).

### 10. Named queries are written in ctsq selectors

The `match` of a named query is an ordinary `-q` selector with captures. `@key` marks what the user searches by, `@target` marks what the answer points at. Where the selector syntax cannot express a framework's shape, the syntax is extended, not bypassed. The first extension is field access on any tree-sitter field (`.key()`, `.value()`, `.arguments()`, `.function()`), generalising today's `.body()` and `.params()`. Why: one query language for people, agents and config. Every named query can be pasted into `-q` to debug it. Beat: a raw tree-sitter S-expression escape hatch.

### 11. `ctsq find <query> [key]`

`ctsq find route /library` prints each match as `path:line`, the key, and the target. With no key it lists every match, so an agent can scan all routes or labels for a product term it cannot name. Keys match as case-insensitive substrings, or as a regex when written `/.../`. `ctsq find` with no arguments lists the named queries available for the current repo, with the framework each one detected. Why: one subcommand for every named query keeps the help short, and the bare listing is how agents discover it.

### 12. A repo glossary covers renamed product terms

`.ctsq/glossary.toml` maps product words to code names. `find`, `def` and every miss report consult it: a key or name that matches a glossary term also searches its mapped names, and the output says it did. Why: searches took several hops exactly where a product term is named differently in code ("pipeline page" is `Pipeline`, "brief sync" is `sync_merged`). Repo only, written by people or agents, never inferred.

### 13. First framework packs

Built-in named queries ship for:

- TSX and JSX: `text` (JSX text), `label` (label-like string props), `component` (functions returning JSX)
- routes: React Router objects, Solid router config, Next.js `app/` file routes, Go `net/http` and chi, Rails `routes.rb`
- `error`: error constructors in Go (`errors.New`, `fmt.Errorf`), TS and JS (`new Error`, `throw`), Ruby (`raise`)

Why: the logs are dominated by Go, TSX and TS, then Ruby (file mentions: Go 15,381, TSX 8,173, TS 6,067, Ruby 1,472), across Mavexa (Go and Solid), emc-gigs (Rails) and the control scripts.

### 14. Error keys match through placeholders

An `error` key matches a format string with `%s`, `%v`, `%d`, `%w`, `%q` and `${...}` treated as wildcards. Concatenation (`"a " + x`) is not followed. Why: agents search with the rendered message from a log, and the code holds the format string.

### 15. Live queries, no persistent index

Named queries, glossary lookups and test detection run against the tree at call time, like everything else in ctsq (path plan decision 27). Revisit if `find` on the Mavexa repo takes over two seconds.

### 16. Every row starts with `path:line`

Inherited from path plan decision 06. Applies to `find`, `scopeof`, `--scope` rows and every footer line that names a location.

## Open questions

- Whether `--scope` should also apply to `tree` (an outline of only the scopes that hold matches for `--focus X`). Not in v1. Revisit after the docs package is measured.
- Whether the directory digest (exported only, types expanded, tests grouped under what they test) is still wanted once docs and test hiding land. Not in v1. Decide from the `tree` piping rate after docs-examples.

## Conventions

- Read `CLAUDE.md` and `DESIGN.md` in the repo root first. Use ctsq for code reading, mdq for markdown.
- Check current behaviour before changing it. Several findings come from logs and were not reproduced by hand (`-l ts` and `.tsx`, arrow consts, C++ macros, `call` on member calls). Write the failing test first, and drop a bullet that already works.
- Abstract vocabulary lives in `types::ABSTRACT_TYPES` (`src/types.rs:11`), validated by `compiler::validate` (`src/compiler/mod.rs:195`). New kinds and aliases go there.
- Language mappings live in `src/compiler/lang/` (one file per language, plus the extension map in `lang/mod.rs`). File discovery lives in `src/files.rs`.
- Report functions return `String` and are tested directly, as `tree::report` (`src/tree.rs:10`), `explain::report` (`src/explain.rs:41`) and `resolve::def_rows` (`src/resolve/mod.rs:286`). `tests/cli.rs` covers the process boundary only.
- Exit codes: 0 when anything matched, 1 when nothing did, 2 for usage errors. Decision 07 extends this to multi-name calls.
- Footer lines go through one shared function so every command words them the same way.
- No em dashes and no semicolons in docs, help text or output.
- `cargo test` passes and `make install` builds after every package.
- After each package, re-run `measure/` over sessions started after the package was installed, and record the three goal measures in `DELIVERY.md`.

## Schema

### Test detection: added to each language file, `languages/<lang>.toml`

```toml
[tests]
files  = ["*_test.go"]                      # globs on the path relative to the search root
dirs   = ["testdata"]                       # any path segment
scopes = []                                 # selectors whose matches are test scopes inside non-test files
```

Defaults per language:

| Language | files | dirs | scopes |
|---|---|---|---|
| Go | `*_test.go` | `testdata` | |
| TS, TSX, JS, JSX | `*.test.*`, `*.spec.*` | `__tests__`, `test`, `tests`, `e2e` | |
| Ruby | `*_test.rb`, `*_spec.rb` | `test`, `spec` | |
| Python | `test_*.py`, `*_test.py`, `conftest.py` | `tests` | |
| Rust | | `tests` | `mod#tests`, items under `#[cfg(test)]` |
| C, C++ | `*_test.*`, `test_*.*` | `test`, `tests` | |

### Named query file: `queries/<framework>.toml`

```toml
framework = "react-router"
langs     = ["tsx", "ts", "jsx", "js"]
detect    = { import = ["react-router", "react-router-dom"] }   # when to apply: an import, a file, or a dependency

[route]
match  = "(pair.key(id#path).value(literal @key)) ~ (pair.key(id#/^(element|Component|component)$/).value(@target))"
target = "def"            # def: resolve the @target name with `def`
                          # enclosing: the named scope holding the match
                          # self: the match itself
```

Further fields:

- `detect` accepts `import` (any file imports one of these), `file` (a path exists, such as `config/routes.rb`), or `dependency` (named in `package.json`, `go.mod` or `Gemfile`). A query with no `detect` always applies to its `langs`.
- `rewrite = ["<regex>", "<replacement>"]` turns the `@target` text into a name before `def` runs. Rails uses it: `"library#index"` becomes `LibraryController#index`. Replacements support `$1` and the case filters `${1:pascal}` and `${1:camel}`.
- `key_from_path = "<regex>"` makes a file-based route: each file matching `paths` is a match, the key is built from the regex groups, and the target is the file's default export.

```toml
framework = "next-app"
langs     = ["tsx", "ts", "jsx", "js"]
detect    = { file = ["app/layout.tsx", "app/layout.jsx"] }

[route]
paths         = "app/**/page.{tsx,jsx,ts,js}"
key_from_path = "^app(/.*)?/page\\.\\w+$"
target        = "default-export"
```

The selectors above are the intended shape. The named-queries package verifies each against a fixture and extends the selector syntax where a shape does not compile (decision 10).

### Glossary file: `.ctsq/glossary.toml`

```toml
"pipeline page" = ["Pipeline"]
"brief sync"    = ["sync_merged", "cmd_sync"]
"supplies"      = ["/resource|stock/"]
```

Keys are matched case-insensitively as whole phrases. Values are names or regex name filters.

### Lookup order

Unchanged: repo `.ctsq/`, then `$XDG_CONFIG_HOME/ctsq`, then `$XDG_CACHE_HOME/ctsq`, then built-in. `queries/` sits beside `languages/` and `models/` in each. `ctsq config path` lists the queries directory. `ctsq config dump <lang>` shows the merged `[tests]` table.

## Endpoints

This is a CLI, so this section lists the command surface.

| Command or flag | Purpose | Package |
|---|---|---|
| `--tests` | include test files and test scopes | tests-and-footers |
| `--exclude GLOB` | skip paths matching GLOB, repeatable | tests-and-footers |
| `--max N` | stop after N matches, footer counts the rest | tests-and-footers |
| `def`, `refs`, `callers`, `callees`, `explain` with several names | one answer per name, misses listed, exit 0 if any resolved | misses |
| `--scope` | print the nearest named scope under each hit | scope |
| `ctsq scopeof path:line [...]` | the named scope holding each line | scope |
| `--up N`, `--siblings`, `--callers`, `--block`, `--budget N` | `scopeof` options | scope |
| `ctsq find` | list named queries available here | named-queries |
| `ctsq find <query> [key]` | run a named query, filter by key | named-queries |
| `ctsq config path` | also lists the queries directories | named-queries |

## Output format

Footer, after any output that dropped something. One line per reason, always last:

```
backend/internal/pagedoc/render.go:41: func Render(doc *Doc, opts Options) (string, error) {
backend/internal/pagedoc/render.go:88: func renderBlock(b Block) string {
2 matches
hidden: 6 in tests (--tests), 31 over --max 2 in 4 files (--max 40)
```

Miss with a fallback:

```
no identifier matches id#/Still reading your files/ in frontend/src (212 files)
found as string literal:
frontend/src/components/app/Chat.tsx:118: <p class="hint">Still reading your files…</p>
try: ctsq -q 'literal#/Still reading your files/' frontend/src
```

Query error:

```
query error: unexpected '/' at position 6
  id#/ui/page/
        ^
a '/' inside a regex name filter must be escaped: id#/ui\/page/
```

`--scope` rows, indented under each hit:

```
src/Asset.tsx:412:     const saveAssetNow = async () => {
  in src/Asset.tsx:380-655  export const AssetEditor = (props: Props) => {
src/Brand.tsx:233:     const saveAssetNow = () => persist(asset())
  in src/Brand.tsx:190-301  function BrandAssetPanel(props) {
2 definitions
```

`scopeof`:

```
internal/editturn/editturn.go:54  in  internal/editturn/editturn.go:31-88  func RunWithTools(ctx context.Context, t Turn) error {
<body, lines 31-88>
```

Over budget, the body is replaced by the signature, the window, and a footer such as `showing lines 380-529 of 31-412, --budget 400 for all`.

`find`:

```
frontend/src/routesConfig.ts:22    /library        Library    frontend/src/pages/Library.tsx:14
frontend/src/routesConfig.ts:23    /library/:id    Asset      frontend/src/components/app/Asset.tsx:380
2 routes (solid-routes, from .ctsq/queries/solid-routes.toml)
```

The last line names the framework and the file the query came from, so an agent can find and fix a query that misses.

Several names:

```
src/load.go:30: func LoadUnits(r io.Reader) ([]Unit, error) {
src/load.go:96: func LoadUnits3(r io.Reader) ([]Unit, error) {
not found: GameKeyCheck (similar: gameKeyCheck in src/input.c:220)
```

## Packages, in build order

Build on branch `agent-search-build`, one or more commits per package, in this order. Merging to `main` is Jasper's call. Docs come first so their effect can be measured alone.

### docs-examples

**Do.**
- Run `measure/` over the last 30 days and record the baseline in `DELIVERY.md`.
- Rewrite the ctsq skill (`control/.agents/skills/ctsq/SKILL.md`) around worked examples, one per goal: pull a named definition, pull several bodies at once with a regex name filter, find where a name is used, find where some text appears (`literal#`), outline a file then pull one body, who calls something, files where several names occur, what a symbol is (`explain`). Each shows the call, a trimmed real output and the next call.
- Rewrite the EXAMPLES section of `ctsq --help` the same way, with fewer examples and the same shape.
- Draft the matching change to the ctsq block of `~/.claude/CLAUDE.md` and give it to Jasper for approval. Do not edit that file directly.

**Acceptance.** Every example in the skill and help runs against this repo or a fixture and produces the shown output. `measure/` re-run after a week of use, recorded in `DELIVERY.md`.

### kinds

**Do.**
- One failing test per bullet in decision 08, using inline sources, then the change in `src/compiler/lang/*` and `types::ABSTRACT_TYPES`.
- Shebang detection in the extension map (`src/compiler/lang/mod.rs`): `ruby`, `python`, `python3`, `node`, `bash` and `sh` through `env` or a direct path.
- `ctsq types` lists the new aliases.

**Acceptance.** Tests pass. `ctsq -q '*function#Topbar' --body` finds a const arrow component in a TSX fixture. `ctsq tree scripts/brief` in the control repo outlines the Ruby script with no `--lang`.

### misses

**Do.**
- The miss fallback (decision 04): on zero matches for `id#`, rerun the name filter as `literal` and `comment` over the same files and print the found-as block. On zero matches for a definition query, check the other definition kinds for the same name.
- Caret query errors: carry byte positions through `query/parser.rs` errors, and map tree-sitter compile failures back to the selector that produced them.
- Regex name filters accept `\/` and quotes. Add parser tests for `/`, `"`, `,` and `|` inside filters.
- A path argument that does not exist prints `no such file: X (did you mean Y)` on stderr and the call continues with the other paths.
- Several names for `def`, `refs`, `callers`, `callees` and `explain` (decision 07), with the `not found` line and `similar:` suggestions from the existing near-name code in `def`.
- A miss report consults the glossary once it exists (the named-queries package adds this).

**Acceptance.** Report tests for each output in the Output format section. A query with a `/` inside a regex compiles. `ctsq def A B Missing` exits 0 and lists `Missing`.

### tests-and-footers

**Do.**
- `[tests]` table in the language mechanics schema (`src/config/mechanics.rs`), with the defaults above in the built-in files.
- Test path filtering in `src/files.rs`, shared by every command. Test scopes (Rust `mod tests`) filtered after matching.
- `--tests`, `--exclude GLOB`, `--max N` as global flags.
- One footer function used by every command (Conventions).
- `tree` hides test files and test scopes, with the footer.

**Acceptance.** `ctsq tree src/` on this repo no longer lists `mod tests` blocks and prints the hidden count. `ctsq refs X` on a fixture with test callers prints the hidden count, and `--tests` shows them.

### scope

**Do.**
- A nearest-named-scope resolver: given a file and a byte offset, walk up the tree to the first node that is a `function`, `class`, `type` or a const bound to a function, using the language's abstract mappings. Shared by `--scope` and `scopeof`.
- `--scope` on `def`, `refs`, `callers`, `callees`, `files` and `-q`, signature and span by default, body with `--body`.
- `scopeof` with `path:line` and `path:line:col` parsing, several coordinates, `--up`, `--siblings`, `--callers` (reusing `calltree`), `--block`, `--budget`.
- Help text pairing `def` and `scopeof`.

**Acceptance.** `scopeof` on a line inside a nested closure returns the closure's named parent. `--up 1` from a method returns its class. Over budget prints the window. `def X --scope` on a name defined inside two components prints both parents.

### named-queries

**Do.**
- Generalised field access in `query/ast.rs` and `compiler/mod.rs`: any tree-sitter field name, not only `body` and `params`. Captures allowed on both sides of `~` and `+`.
- The `queries/` schema and loader in `src/config/`, the `detect` rules, `target` kinds, `rewrite`, `paths` and `key_from_path`.
- `ctsq find` (list) and `ctsq find <query> [key]`, with the output above.
- Glossary loading, used by `find`, `def` and miss reports.
- Error-key matching through placeholders (decision 14).
- Packs in this order, each with a fixture copied in shape from a real repo: Solid routes (Mavexa `routesConfig.ts`, written as a repo `.ctsq/` file in the fixture), React Router, Next.js `app/`, Go `net/http` and chi (Mavexa backend), Rails `routes.rb` (emc-gigs), TSX `text`, `label` and `component`, then `error` for Go, TS and Ruby.

**Acceptance.** Each pack's fixture test passes. Every `match` in the built-in packs also runs as a plain `-q` query. On the Mavexa repo, `ctsq find route /library` names the component, and `ctsq find error` on a message taken from a real log names the function that emits it.

## Evidence

Source: Claude Code session logs, 2026-08-17 to 2026-10-16. Logs, test output, git output and markdown were excluded from code search. JSON, YAML and SQL count as code. Counts of goals and questions come from subagents reading samples and are approximate.

**Volume.**
- 30,296 code-search calls in 4,651 episodes (848 main session, 3,803 subagent). An episode is an unbroken run of search calls ended by an edit or another action. Episodes of 5 or more calls hold 82% of calls.
- Code characters taken in: whole-file reads 44%, window reads 20%, ctsq 22%. Whole-file reads over 20k characters are 13% of whole reads and 46% of their characters.

**ctsq usage.**
- 15,475 calls, 93% from subagents. Raw `-q` 76%, `tree` 14%, `explain` 6%, `def` 2%, `callers` and `files` about 1% each.
- `id#` is the most used selector (9,434 uses). About 8,700 calls pipe into `head`, about 1,700 into `grep`, mostly to drop tests or paths.
- Errors: 10% of `-q` queries, 20% of `files` calls. Top causes are query parse errors, unknown kinds for a language, undetected languages and partial failures in chained commands.

**Goals.**
- Subagents mostly start with coordinates from their parent and read owned files or pull named definitions.
- Main sessions search from what the user sees or says: UI to code, explaining a wrong behaviour, capability questions, error sources.

**Questions in long episodes (about 490 labelled steps).**
- What does function X do: 85 steps, first try worked about 75%.
- Where else is X used: 62 steps, about 60%.
- Show lines N to M: 50 steps.
- Load a whole module: 48 steps.
- What is in this file: 35 steps, about 95%.
- Where does this string appear: 32 steps, about 50%.

**Description-started searches (30 sampled).**
- 20 anchored on the first call.
- One hop when the user's words appear verbatim in code, several when a product term is renamed.
- About half the misses before the anchor were tool friction.
- 17 of 19 anchors not taken from earlier context could be found by a framework-aware query.

**Directory `tree`.** 775 calls. Unpiped output had a median of 1,900 chars and none over 20k. 59% were piped to `head` or `grep`, 254 of 404 `head` cuts kept 40 lines or fewer, and 42 were followed by the same `tree` again.

### Re-measuring

```
python3 -I measure/extract_search.py 30 OUT_DIR       # writes OUT_DIR/events.jsonl
cd OUT_DIR
python3 -I <ctsq>/plans/agent-search/measure/ctsq_stats.py
python3 -I <ctsq>/plans/agent-search/measure/code_only.py
python3 -I <ctsq>/plans/agent-search/measure/code_episodes.py
```

`events.jsonl` holds command text and result heads from real sessions and can contain secrets. Keep it out of the repo.
