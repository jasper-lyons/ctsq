# ctsq path: walking calls and data flow

One plan for the whole feature. It holds the spec (decisions and their reasons) and the implementation guide (conventions, formats, work packages). Agents fetch it a section at a time with `mdq section plans/path/PLAN.md '<Heading>'`. Headings are an interface, do not rename them.

Precedence: where a package section is silent, `## Decisions` decides. Where both are silent, the existing code's conventions are the contract.

## Goal

Answer "where does this data leave my codebase?" from the command line, in a form an LLM can follow by file and line. Secondary: answer "how does A reach B?" over the call graph.

Example queries this plan must make work:

```
ctsq path 'function#parseConfig --> function#validate'
ctsq path 'function#handle variable#name ==> function#/Printf|Log/'
ctsq path 'field#Booking.CustomerPhone ==> function#log.Printf'
ctsq path 'field#User.Username ==> template_action'
```

## Decisions

### 01. Three query contexts, each with its own combinators

| Context | Directly, down | Eventually, down | Directly, up | Eventually, up |
|---|---|---|---|---|
| AST (where code is written) | `>` | space | `:has()` (later) | `:has()` (later) |
| Call graph | `->` | `-->` | `<-` | `<--` |
| Data flow | `=>` | `==>` | `<=` | `<==` |

Why: one query often mixes contexts, for example "the variable `name` written inside `handle`, flowing to `Printf`". Distinct shapes make the context visible. Beat: subcommand per context (cannot mix contexts in one query), and `<`/`<<` for upward calls (replaced to keep the arrow family consistent).

### 02. Syntax stays as close to CSS as possible

Attributes use CSS form `[name=value]` (values containing `/` are quoted: `[from="log/slog"]`) with `^=` (starts with), `*=` (contains), `$=` (ends with) and `=/regex/`. Avoiding routes uses `:not(selector)`. Why: Jasper's rule. Beat: flags like `--via` and `--avoid`.

### 03. Attributes exist for every language

The selector parser accepts attributes for all languages. Each language supplies what an attribute means. Why: one query language across languages.

### 04. `from` and `in` are the v1 attributes

- `from`: the origin string of the import the name came through, as written in the source (`"log"`, `"log/slog"`, `'pino'`).
- `in`: the enclosing declaration that owns the name (struct, class, impl, module, interface).

Why: qualifying names without language-specific syntax. Beat: package syntax per language (Jasper rejected it as Go specific).

### 05. Dot shorthand inside a name

`function#log.Printf` means `function#Printf` where `in=log` holds or the import's local name (alias, else last path segment) is `log`. So `slog.Info` matches an import of `"log/slog"`. `field#User.Email` reads the same way. A dot followed by an identifier and `(` stays field access (`.body(...)`). A dot inside a `#name` with no following `(` is the qualifier. The query always uses a dot, whatever separator the language uses. Why: short to type, one rule for every language.

### 06. Every output row carries file and line numbers

Every row of every new output starts with `path:line`. Why: that is how LLMs traverse files. Not negotiable.

### 07. Output is an indented tree, plain spaces

Two spaces per level, no box-drawing characters. Only branches that reach the end selector are printed. Why: box characters cost tokens on every row, and pruned trees stay short.

### 08. Follow values, not only calls

The data-flow walk tracks values through assignment, arguments, returns, fields and outside calls. Why: Jasper's real question is where data ends up, which calls alone cannot answer.

### 09. Tags on every hop

| Tag | Meaning |
|---|---|
| `source` | where the walk started, or where data enters from outside |
| `exact` | the same value, moved or copied unchanged |
| `derived` | built from a tracked value |
| `moved` | passed into a call or returned |
| `field <Name>` | stored into a field, the walk now follows the object |
| `conditional@<line>` | only tracked on some branches, line of the condition |
| `widened` | reached through an interface, one of several implementations |
| `unmodelled` | passed through an outside call with no model, assumed to carry the data |
| `via <function>` | passed through a named function (records sanitisers such as `NormalizePhoneNumber`) |
| `sink` | matched the end selector |
| `rendered` | printed by a template action |
| `outside` | handed to code outside the module with no further route |
| `stopped: <reason>` | the walk would have to guess, it stops and lists choices |

Why: shows how sure each hop is. Unmodelled calls are a kind of taint (Jasper's framing).

### 10. Derived values, explicit flow only

Per function, walk statements in order with a tracked set:

- `x := expr` using a tracked value adds `x`, `exact` if expr is just that value, `derived` otherwise
- `x = expr` using nothing tracked removes `x`
- after `if`/`else` or `switch`, a variable is tracked if any branch tracked it, tagged `conditional@<line>` when not every branch did
- loops rerun their body until the set stops changing (dataflow fixed point)
- conditions do not carry data (`if body.Admin { role = "admin" }` does not track `role`)

Why: complete answers with visible uncertainty. Beat: implicit flow (tracks almost everything).

### 11. Models can clear taint

A model can declare that a function's output is about the data but is not the data (`len`, `==`, `strconv.Itoa` on counts). Why: stops derived values multiplying.

### 12. Fields: follow the object

When a tracked value is stored in `o.F`, track `o` with field path `F`, and continue wherever `o` goes, reporting reads of `.F`. When `o` reaches something shared (map, package-level variable, channel), stop with `stopped: shared <kind>` and list the readers of the field. Beat: following every reader of the field name (false routes), always stopping (too noisy).

### 13. Reflection moves whole objects

Calls modelled as `whole` (for example `fmt.Printf` with `%v`, reflection-based DB helpers) carry every field of the argument. Why: the booking app trace showed DB writes and logs move whole structs by reflection.

### 14. Interfaces widen, limited to constructed types

At a call through an interface, the possible types become every implementation that the program actually constructs somewhere (composite literal, `&T{}`, `new(T)`, constructor return types). This is rapid type analysis. The walk follows all of them, tagged `widened`. Type assertions and type switches narrow the set. Implementations outside the module are grouped in output as `widened: N in repo, M outside`, `--expand` lists them. Beat: stop and ask at every interface (earlier idea, replaced by Jasper).

### 15. Sources and sinks are named in the query

There are no pre-classified sink kinds. The end selector says what counts, for example `function#log.Printf`. Models only describe pass-through, clear, whole, and template loading. Why: Jasper rejected pre-captured kinds. An unanchored regex also matches `Sprintf`. The walk passes through such calls as `derived` unless they match the end selector, so anchor or qualify names.

### 16. "Outside the codebase" is decided per language

Go: a call into a package whose import path is not under the module path in `go.mod`. JS/TS (later): imports resolving into `node_modules` or `node:` built-ins. When a walk has no end selector, every outside call that receives a tracked value is a leaf tagged `outside`.

### 17. v1 stops at the database

A database write is an outside call, so the walk stops there. Values read back from the database are a separate query by the user, starting from the read. Why: Jasper's decision, keeps v1 tractable. The schema loop (column written, read elsewhere, renamed by a view) is out of scope.

### 18. Unmodelled outside calls continue

With no model, assume the return value is derived from the arguments and tag `unmodelled`. Why: never silently drop a route. The trailer reports how many routes ended at or passed through unmodelled calls, so "0 routes" is honest.

### 19. Tree-sitter only, no language type checker

All analysis uses tree-sitter plus ctsq's own resolvers. Why: translates across languages. Beat: Go's `go/packages` and SSA (precise but Go only). Types come from declarations, composite literals, constructor and return types (the existing Go `Binding` table in `src/resolve/go.rs`).

### 20. Configurable languages and models

- Flow mechanics per language (which nodes are assignment, call, return, field read and write, parameter, import, branch, loop, type assertion) are data files. Built-in defaults are compiled in with `include_str!`, and a file in config overrides them.
- Query compilation mappings (`src/compiler/lang/*.rs`) stay in Rust. Moving them to data is out of scope.
- Locations, highest priority first: repo `.ctsq/models/`, `~/.config/ctsq/` (`languages/`, `models/`), `~/.cache/ctsq/models/` (generated, safe to delete, nothing writes it in v1).
- Standard library models are versioned by file name, `stdlib@1.23.toml`, chosen from the `go` line in `go.mod`, falling back to the nearest lower version.

### 21. Config format is TOML

Why: the `toml` crate is maintained and Rust-native. Beat: YAML (`serde_yaml` is archived). The earlier sketches in YAML translate directly.

### 22. Semantic combinators evaluate after tree-sitter matching

Tree-sitter cannot see across functions or files, or know import origins. The query AST is split into AST segments (compiled as today) joined by semantic combinators, evaluated in a new stage that joins runner matches against the call graph or the flow engine. Attributes are also filtered in this stage. Semantic combinators are only allowed at the top level of a query, never inside groups or field inner queries. The last selector of each AST segment supplies the start or end node. Why: the compiler only emits S-expressions (see `src/compiler/mod.rs:272`).

### 23. Semantic combinators live in the `path` subcommand

`ctsq -q` rejects a query containing a call or data-flow combinator with exit 2 and the hint `use: ctsq path '<query>'`. Attributes and the dot shorthand work in both `-q` and `path`. Why: keeps the `-q` output contract (`path:LINE: line`) unchanged.

### 24. Go templates are parsed

Go `text/template` and `html/template` files are a language. `.` is the data, `range` and `with` rebind `.`, `{{template "x" .}}` and `{{block}}` are calls into `{{define}}`, `{{$x := ...}}` is assignment, any action that prints a value is tagged `rendered`. Templates loaded from Go string variables (inline email templates) are parsed too. Why: Jasper wants rendering answered down to the template line.

### 25. Template names resolve by value set, bounded by go:embed

To link `Execute`/`ParseFS`/`ParseFiles`/`ParseGlob` to files, walk the name argument upward collecting values: literal adds one value, constant adds its value, parameter adds the union over every caller, concatenation becomes a glob, anything else marks the set open. The `//go:embed` patterns on the `embed.FS` give the finite list of files, and every value or glob is matched against it. Why: the booking app trace found a closed set of 11 names over 12 embedded files.

### 26. Go first, then JS/TS

Data flow, models and templates are Go only in v1. JS/TS flow is a later plan. Attributes (`from`, `in`) and call combinators work for every language that has the needed resolution (see Package: attributes).

### 27. No persistent index in v1

`path` parses every file once per run into an in-memory store shared by the call graph and the flow engine. Why: today graph build parses files twice and no cache exists. A disk cache is a later plan.

### 28. Test fixtures are synthetic

The acceptance fixture is a small synthetic Go app written for ctsq that reproduces the hard hops from the booking app trace. The real booking app is used only as a manual acceptance run. Why: the real repo contains personal data (a real phone number at `cmd/book/main.go:200`).

## Open questions

- Should derived hops show by default, or only `exact` with `--derived` to widen? v1 shows all tags and provides `--only <tag,...>`. Revisit after Jasper has used it.
- `from` semantics for Ruby, C and C++ (no per-identifier import binding). v1 reports `attribute from is not supported for <lang>` with exit 2.
- Matching ends against a folder or a pattern (`src/api/ ==> ...`). Possibly useful, possibly expensive. Not in v1.
- Generating models by parsing library source into `~/.cache/ctsq`. Not in v1.
- Following data through the database schema (decision 17) and through client-side JS in templates. Not in v1.

## Conventions

- Read `CLAUDE.md` and `DESIGN.md` in the repo root before starting. Use ctsq itself for code reading (`ctsq explain`, `ctsq tree`, `ctsq -q '*function#X' --body`), mdq for markdown.
- Abstract vocabulary lives in `types::ABSTRACT_TYPES` (`src/types.rs:11`). New abstract kinds (`field`, `template_action`) are added there and validated by `compiler::validate` (`src/compiler/mod.rs:155`).
- Unknown kinds fail in `validate` with exit 1 before any file is read. Usage errors exit 2. A query with no results prints a report and exits 1.
- `-q` output stays `path:LINE: <trimmed source line>`. Nothing in this plan changes it.
- Write report functions that return `String` and test them directly (the pattern of `explain::report`, `tree::report`, `resolve::def_rows`). `tests/cli.rs` only covers the process boundary.
- Unit tests live in `mod tests` beside the code. Go flow tests use inline source strings, like the helpers at `src/resolve/go.rs:721`.
- Graph nodes are keyed by `file:line` identity (`NodeKey`, `DefId`), never by name.
- Call targets keep `Confidence::Exact` or `Guess`. A guessed hop prints `?` after the tag.
- Document new Go semantics in `DESIGN.md` under "Go resolution rules (in order)", and add a `DESIGN.md` section per new subsystem (attributes, path, flow, models, templates).
- No em dashes and no semicolons in docs or help text.
- Every output row begins with `path:line` (decision 06). Indent with two spaces (decision 07).
- `cargo test` passes and `make install` builds after every package.

## Schema

Config and model files, TOML (decision 21).

### Language mechanics file: `languages/go.toml`

```toml
[outside]
module_file = "go.mod"          # module path read from its `module` line

[nodes]
assign      = ["short_var_declaration", "assignment_statement", "var_spec"]
call        = ["call_expression"]
return      = ["return_statement"]
field       = ["selector_expression"]
param       = ["parameter_declaration"]
import      = ["import_spec"]
branch      = ["if_statement", "expression_switch_statement", "type_switch_statement", "select_statement"]
loop        = ["for_statement"]
assert      = ["type_assertion_expression"]
composite   = ["composite_literal"]
```

Exact keys are owned by Package: models, which may add keys. The flow engine reads only these keys, never hard-codes node names.

### Model file: `models/go/stdlib@1.23.toml`

```toml
["strings.ToUpper"]
pass = ["0 -> return"]

["encoding/json.Marshal"]
pass = ["0 -> return"]

["encoding/json.Unmarshal"]
pass = ["0 -> 1"]               # bytes flow into the pointed-to value

["fmt.Sprintf"]
pass = ["* -> return"]          # every argument
whole = true                    # %v formats every field

["fmt.Printf"]
whole = true

["builtin.len"]
clear = true

["os.Getenv"]
source = "return"

["(*html/template.Template).Execute"]
template = { data_arg = 1 }

["(*html/template.Template).ParseFS"]
template_load = { fs_arg = 0, names_from = 1 }
```

Keys are the qualified name: import path, then `.Name`, or `(*importpath.Type).Method` for methods. Fields: `pass` (list of `from -> to` where from and to are an argument index, `*`, `receiver` or `return`), `clear`, `whole`, `source`, `template`, `template_load`.

### Lookup order

1. repo `.ctsq/models/<lang>/*.toml`
2. `~/.config/ctsq/models/<lang>/*.toml`
3. `~/.cache/ctsq/models/<lang>/*.toml`
4. built-in defaults compiled into the binary

First match for a qualified name wins. Language mechanics use the same order with `languages/<lang>.toml`.

## Endpoints

This is a CLI, so this section lists the command surface.

| Command | Purpose | Package |
|---|---|---|
| `ctsq -q '<query>'` | unchanged, plus attributes and dot shorthand | attributes-syntax, attributes-eval |
| `ctsq path '<query>' [paths...]` | evaluate a query with call or data-flow combinators | path-cli, flow-calls |
| `--max N` | longest route in hops, default 12 | path-cli |
| `--all` | every route, default is the 5 shortest per start | path-cli |
| `--only <tag,...>` | keep only hops carrying these tags | path-cli declares, flow-calls applies |
| `--expand` | list grouped outside implementations | widening |
| `--body` | print each hop's function body under the row | path-cli |
| `ctsq config path` | print the config and model directories in lookup order | models |
| `ctsq config dump <lang>` | print the merged mechanics file and model count for a language | models |

## Output format

Call route (`-->`):

```
cmd/app/main.go:12      main
  src/load.go:30          loadAll        calls parseConfig at :44
    src/config.go:41        parseConfig    calls validate at :71
      src/config.go:90        validate
1 route
```

Data flow (`==>`), grouped by source when the start matches several places:

```
src/user.go:14          field User.Email
  src/auth.go:33          login         email := u.Email                     exact
    src/auth.go:40          login         log.Printf("login %s", email)        sink
  src/signup.go:22        register      sendWelcome(u)                       moved, field Email
    src/mail.go:18          sendWelcome   slog.Info("sent", "to", u.Email)     sink
2 routes to sink
1 route ends at an unmodelled call: src/mail.go:25 mailer.Send
```

Row shape: `path:line`, padded, function name, trimmed source line, tags comma-separated. Stops print the choices indented under the row:

```
src/order.go:14         parseOrder    o.Items = decode(data)               stopped: shared map, 4 readers
  src/store.go:51         saveOrder
  src/notify.go:30        emailReceipt
```

## Work packages

`Cargo.toml` and `Cargo.lock` are owned by the orchestrator. `regex`, `serde` (derive) and `toml` are already added. A package that needs another crate reports a blocker, except templates-grammar, which may edit `[dependencies]` and `[build-dependencies]` for its grammar.

The crate is a binary only, so every `mod` line lives in `src/main.rs`. The orchestrator has already added `mod config;`, `mod semantic;` and `mod flow;` with empty stubs (`src/config/mod.rs`, `src/semantic/mod.rs`, `src/flow/mod.rs`). No package adds a `mod` line to `main.rs` for these. Submodules are declared inside the owning folder's `mod.rs`.

Tests: there is no lib crate, so `tests/*.rs` can only drive the `ctsq` binary. Engine packages before path-cli test with `#[cfg(test)]` modules inside their own files. From flow-calls on, each flow package also owns `fixtures/flow/<package>/` and `tests/path_<package>.rs`. Acceptance owns `fixtures/flow/booking/` and `tests/path.rs`.

| Stage | Package | Owns | Depends on | Tier |
|---|---|---|---|---|
| 1 | descendant | `src/compiler/mod.rs` (descendant compile), `src/runner.rs`, `src/cooccur.rs` (only if counts need adjusting) | none | Sonnet |
| 1 | models | `src/config/**`, `config/**`, `src/main.rs` (the `Config` subcommand variant and its dispatch arm only) | none | Sonnet |
| 2 | attributes-syntax | `src/query/**`, `src/compiler/mod.rs` | descendant | Sonnet |
| 3 | attributes-eval | `src/semantic/mod.rs`, `src/semantic/attrs.rs`, `src/resolve/imports.rs` (additive), `src/resolve/go.rs` (additive pub exports), `src/main.rs` (attribute filter call, `-q` exit 2 hint) | attributes-syntax, models | Opus |
| 4 | call-walk | `src/semantic/calls.rs`, `src/semantic/mod.rs`, `src/resolve/graph.rs`, `src/resolve/mod.rs` (additive), `src/resolve/go.rs` (additive) | attributes-eval | Opus |
| 4 | field-kind | `src/types.rs`, `src/compiler/lang/*.rs` (adding the `field` kind only), `tests/field.rs` | attributes-eval | Sonnet |
| 4 | flow-engine | `src/flow/**`, `src/config/mechanics.rs` (additive fields) | attributes-eval | Opus |
| 5 | path-cli | `src/semantic/print.rs`, `src/main.rs` (`path` subcommand and flags), `tests/path_cli.rs`, `fixtures/path/` | call-walk | Sonnet |
| 5 | flow-models | `src/flow/**` | flow-engine | Opus |
| 6 | flow-calls | `src/flow/**`, `src/semantic/mod.rs`, `src/semantic/flows.rs`, `src/semantic/print.rs`, `src/resolve/go.rs` (additive) | path-cli, flow-models | Opus |
| 7 | flow-objects | `src/flow/**` | flow-calls | Opus |
| 8 | flow-funcs | `src/flow/**`, `src/resolve/go.rs` (additive) | flow-objects | Opus |
| 9 | adversarial review and fixer | read-only, then fixes by name | flow-funcs | Opus |
| 10 | flow-up | `src/flow/**`, `src/semantic/mod.rs`, `src/semantic/flows.rs`, `src/semantic/attrs.rs` | review fixes | Opus |
| 11 | widening | `src/flow/**`, `src/resolve/go.rs`, `src/semantic/print.rs`, `src/semantic/calls.rs`, `src/main.rs` (`--expand`) | flow-up | Sonnet |
| 12 | templates-grammar | `src/compiler/lang/gotmpl.rs`, `src/compiler/lang/mod.rs`, `src/types.rs`, `build.rs`, `vendor/`, `Cargo.toml` (dependency sections) | widening | Opus |
| 13 | templates-flow | `src/flow/**`, `src/semantic/flows.rs` | templates-grammar | Opus |
| 14 | acceptance | `fixtures/flow/booking/`, `tests/path.rs`, `DESIGN.md` (final consistency) | templates-flow | Sonnet |

Every package also owns the `DESIGN.md` section for its subsystem (additive). Stages run in order. Packages in the same stage run in parallel.

## Package: descendant

**Problem.** `compile_query` (`src/compiler/mod.rs:272`, descendant at `:297`) compiles descendant through one `(_ ...)` wildcard, so `function#handle variable#name` only finds `name` one level below the function.

**Do.**
- At the top level of a query only, compile `head tail` (descendant) as `CompiledQuery::Scoped{outer, scope_field: "", inner}`. `child_by_field_name("")` falls back to the whole node in `runner.rs`. Do not add a `CompiledQuery` variant: it is matched exhaustively in `src/example.rs:700,719,733` and `src/main.rs:455,610`.
- When compiling the outer of such a descendant, add an explicit `@_root` capture on the head node and make the Scoped runner (`runner.rs:186-199`) prefer it, so `(&function#x @c) id#y` scopes to the head node, not the identifier.
- Nested descendants (inside groups, field inner queries, and any descendant after the first top-level one) keep the `(_ ...)` form. Document this in `DESIGN.md`.
- `MatchRow` (`runner.rs:15-19`) gains `start_byte`, `end_byte` and `kind` of the primary node. Constructors are at `runner.rs:70` and `:138`. Later packages rely on these to find the matched node.
- Row semantics change: a top-level descendant prints one row per inner match at the inner match's line. Leave `+` and `~` alone.

**Acceptance.**
- A Go fixture with a variable three blocks deep in a function is found by `function#handle variable#name`, on the variable's line.
- `cargo test` passes. Check `ctsq files` (cooccur) output on `fixtures/` before and after and state any count change in the handoff.
- Unit tests for `@_root` preference and the new `MatchRow` fields.

## Package: models

**Do.**
- `src/config/` loads language mechanics and model files in the lookup order from `## Schema`, merges built-in defaults (`config/languages/go.toml`, `config/models/go/stdlib@<ver>.toml` via `include_str!`), and picks the stdlib version from the `go` line in `go.mod` with nearest-lower fallback.
- `src/config/mechanics.rs` defines the mechanics struct with a field for every key in `## Schema` (`outside`, `nodes.*`), each `#[serde(default)]`, so later packages can add keys.
- `src/config/gomod.rs`: `module_path(root) -> Option<String>` and `go_version(root)`, finding the nearest `go.mod` upward from a path. Decision 16 relies on this.
- Ship a Go stdlib model covering at least: `strings` (pass), `strconv` (Itoa clears, Quote passes), `fmt` (Sprintf passes whole, Printf/Fprintf/Println whole), `log` and `log/slog` (whole), `encoding/json` (Marshal, Unmarshal, `(*Encoder).Encode` whole), `bytes` and `bufio` buffers pass, `os.Getenv` source, `builtin.len` clear, `errors.New` and `fmt.Errorf` pass, `html/template` and `text/template` Execute, ExecuteTemplate, Parse, ParseFS, ParseFiles, ParseGlob, `(*regexp.Regexp).ReplaceAllString` pass, `database/sql` Exec and Query.
- `ctsq config path` and `ctsq config dump <lang>` (see `## Endpoints`). In `main.rs`, only the `Config` subcommand variant and its dispatch arm.
- Malformed files report the file, line and key, exit 2.

**Acceptance.** Unit tests: repo `.ctsq/models/go/app.toml` overrides a built-in model for the same name, `go 1.23.4` selects `stdlib@1.23`, `go 1.25` with only 1.23 shipped selects 1.23, every `pass` form parses, `module_path` finds the module line. `ctsq config path` prints the four locations.

## Package: attributes-syntax

**Lexer** (`src/query/parser.rs`, `Lexer::next_token` :31, one-char fallback near :74).
- Real tokens for `[`, `]`, `=`, `^=`, `*=` (lookahead, so `*` alone stays the Def sigil), `$=`, `:`, and the combinators `->`, `-->`, `<-`, `<--`, `=>`, `==>`, `<=`, `<==`.
- Unknown characters stop lexing as one-char `Ident`, they are an error.
- Attribute values are an Ident, a quoted string, or `/regex/`. A value containing `/` must be quoted: `[from="log/slog"]` (a bare `/` starts a regex).
- Identifiers stay `[A-Za-z0-9_]`. The abstract kind for template output is `template_action` (underscore).

**Parser.**
- `parse_atom` (:219): after the name match, zero or more `[key op value]` into `Atom.attrs: Vec<AttrMatch>`, `AttrMatch{key, op: Eq|Prefix|Contains|Suffix|Regex, value}`.
- Dot shorthand (decision 05): inside `#name`, `A.B` where the token after `B` is not `(` becomes name `B` plus `AttrMatch{key: "qual", op: Eq, value: A}`. `.body(` keeps working.
- `:not(<selector>)` parses onto `Selector` as a new field. Update the struct literal in `try_compile_scoped` (`src/compiler/mod.rs:226`).
- `parse_query` (:124) and its lookahead (:135-149) accept the new combinators as `Combinator::{Calls, CallsEventually, CalledBy, CalledByEventually, Flows, FlowsEventually, FlowsFrom, FlowsFromEventually}`.

**Compiler.** `compiler::validate` rejects any semantic combinator or `:not` anywhere in a query, including inside groups and field inner queries, with a distinct error kind `SemanticQuery`. Every entry point (`main.rs`, `cooccur.rs:21`, `explain.rs:392`, `resolve/fallback.rs:40`, `example.rs:695`) then fails the same way without edits. `compile_atom` ignores `attrs` (they are filtered after matching).

**Acceptance.** Parser unit tests for every new token and shorthand edge case (`function#log.body()` stays field access, `*=` vs `*function`, `[from="log/slog"]`, unknown char errors). `validate` rejects `a --> b` and `(a --> b)`. `cargo test` passes.

## Package: attributes-eval

**Do.**
- `src/semantic/mod.rs` and `src/semantic/attrs.rs`: filter `MatchRow`s by `attrs`, using `start_byte`/`end_byte` from descendant to locate the matched node.
- `from`: the full origin string of the import the identifier's qualifier or binding came through. Go: `FileScope.imports` (`src/resolve/go.rs:219`, `collect_imports` near :304-333). JS/TS, Python, Rust: `src/resolve/imports.rs` (`ImportResolver` trait :133). Add an additive `bindings: Vec<String>` field to `RawImport` (imports.rs:19-34) for local names bound by an import. Ruby, C, C++: exit 2 `attribute from is not supported for <lang>`.
- `in`: the nearest enclosing named declaration (struct, class, impl, interface, module), or for a Go method the receiver type.
- `qual` (dot shorthand): true when `in` equals the value, or when the import's local name (alias, else last path segment) equals the value. So `function#slog.Info` matches `slog.Info` from `"log/slog"`.
- `regex` crate for `/re/` values.
- Export from `src/resolve/go.rs`, additive and pub: `go_imports(root, src) -> HashMap<String, String>` (local name to import path) and `binding_type(...)` over the existing `Binding`/`var_type` (:196-233, :659). flow-engine relies on these.
- `src/main.rs`: apply the attribute filter after `run_rows` for `-q`, and map the `SemanticQuery` error to exit 2 with `use: ctsq path '<query>'`.

**Acceptance.** Test against fixtures through the binary in `tests/cli.rs`-style tests you add in a new `tests/attrs.rs`:
- `&function#log.Printf` matches `log.Printf` and not `fmt.Printf`.
- `[from^=lo]`, `[from*=og]`, `[from$=og]`, `[from=/^log$/]` behave as named. `&function#slog.Info` and `[from="log/slog"]` both match `slog.Info`.
- `field#User.Email` matches `Email` in `User`, not in `Account`.
- JS: `function#info[from=pino]` matches `logger.info` when `logger` is directly bound by `import pino` in that file.
- `ctsq -q 'function#a --> function#b'` exits 2 with the hint.

## Package: call-walk

**Do.**
- Per-site call data as a side table: `CallSite{caller, callee target, line, start_byte, arg byte ranges}` collected by the Go resolver only, through a new default `Resolver` trait method that returns nothing for other resolvers. Do not change `ResolvedCall` or `Def` struct shapes (their literals are in `rust.rs`, `fallback.rs`, `go.rs`).
- Parameter names and positions for Go defs, exposed through the same side table or a pub helper in `go.rs`.
- Parse once: a `TreeStore` (path to parsed tree and source) built once per `path` run. The Go resolver's `parse()` (`go.rs:101`) takes trees from it when given.
- `src/semantic/mod.rs`: split a query at top-level semantic combinators into AST segments. The last selector of each segment supplies the start or end node, matched to a `DefId` by file and byte range.
- `src/semantic/calls.rs`: walk `CallGraph.fwd`/`rev`: `->`/`<-` one edge, `-->`/`<--` bounded BFS shortest first, `max` and `all` options, at most 5 routes per start by default, waypoints, `:not(sel)` exclusion, `Confidence::Guess` marked. Return route data, not text (path-cli prints).

**Acceptance.** Unit tests on inline Go: routes, reversed reading order for `<--`, waypoint, `:not`, depth limit. Add a test that pins `callers`, `callees`, `callgraph` output on `fixtures/` as before.

## Package: field-kind

**Problem.** The abstract kind `field` does not exist, so `field#User.Email` fails `validate`. Found by attributes-eval.

**Do.** Add `field` to `types::ABSTRACT_TYPES` and map it in every language in `src/compiler/lang/*.rs`: struct or class field declarations (`*field`) and field or member accesses (`&field`), with the field name as the name. Go: `field_declaration` names and `selector_expression` field. JS/TS: class fields and `member_expression` property. Python: attribute access and class-level assignments. Rust: struct fields and `field_expression`. Ruby, C, C++: struct members and member access where the grammar has them.

**Acceptance.** `ctsq -q '*field#Email' fixtures/` lists declarations, `&field#Email` lists accesses, and `field#User.Email` (with attributes-eval's filter) matches `Email` in `User` only. `cargo test` passes.

## Package: flow-engine

**Do.** `src/flow/` engine for one Go function, driven by the mechanics struct (never hard-coded node names). Add any keys you need to `src/config/mechanics.rs` and `config/languages/go.toml` additively.
- start points: a variable or parameter node, or every read of a `field` inside the function
- the tracked set and rules in decision 10, tags in decision 09 (`source`, `exact`, `derived`, `conditional@line`)
- an end predicate given as a closure over call nodes, giving `sink`
- outside calls are left as a hook returning `Unknown` for flow-models to fill
- expose `flow::analyse_function(...) -> FlowResult` and `flow::report(...) -> String` in the `## Output format` row shape
- use `resolve::go::go_imports` and `binding_type` from attributes-eval, do not edit `src/resolve/`

**Acceptance.** `#[cfg(test)]` unit tests with inline Go: exact copy, derived concat, overwrite removes, if/else both-branch vs one-branch (`conditional@line`), for-loop fixed point, switch, condition-only use not tracked.

## Package: path-cli

**Do.**
- `src/semantic/print.rs`: the tree printer for call routes and flow trees, `## Output format`, returning `String`. Every row starts `path:line`, two-space indent, tags comma-separated, trailer counts.
- `src/main.rs`: `ctsq path '<query>' [paths...]` with `--max`, `--all`, `--only`, `--body`, wiring call routes from call-walk. `--only` is declared here and read by flow-calls.
- `tests/path_cli.rs` with `fixtures/path/`: call routes through the binary.

**Acceptance.** `ctsq path 'function#main --> function#X' fixtures/path/` prints the route with call lines. `--max 1` limits. `:not` removes a route. `--body` prints bodies under rows.

## Package: flow-models

**Do.** Fill the outside-call hook in `src/flow/` from the model files:
- resolve a call to its model key with `go_imports` (`strings.ToUpper`, `(*html/template.Template).Execute` via `binding_type` for receivers)
- `pass`, `clear`, `whole`, `source`, otherwise `unmodelled` (decision 18)
- `via <function>` recorded for in-repo calls on the route

**Acceptance.** Unit tests: `strings.ToUpper` passes as `derived`, `len` clears, an unknown outside call continues tagged `unmodelled`, a repo `.ctsq/models` override takes effect, `fmt.Printf("%v", s)` with `whole` matches an end predicate on any field of `s`.

## Package: flow-calls

**Do.**
- Arguments into parameters using call-walk's `CallSite` table, returns back to the caller's receiving variable.
- Wire `=>`, `==>` in `src/semantic/` (new `src/semantic/flows.rs`), respecting `--only`.
- Outside rule (decision 16) with `config::gomod::module_path`.
- Field queries group output by source (`## Output format`).

**Acceptance.** `fixtures/flow/flow-calls/` with a value passed through three functions, returned back, normalised by a helper (`via`), handed to `log.Printf` and `json.Marshal`. `tests/path_flow-calls.rs` asserts the routes through the binary.

## Package: flow-objects

**Do.**
- Follow the object (decision 12), including embedded struct promotion (`AdminBooking{Booking}` exposes `.CustomerPhone`).
- Shared stop for maps, package-level variables, channels.
- Generic functions (`ExtractForm[T]`, `JSON[T]`): track by value regardless of `T`. Read struct tags so `json:"phone"` shows in the row.

**Acceptance.** `fixtures/flow/flow-objects/`: a JSON form decoded by a generic helper, a struct stored by a reflection-based helper, a `log.Printf("%v")` of a struct, a JSON response helper, an embedded struct read, a map store that stops. `tests/path_flow-objects.rs` asserts `field#Booking.CustomerPhone ==> function#/Printf|Encode/`.

## Package: flow-funcs

**Do.**
- Calls through function-valued variables (`var send = SendEmail`): resolve when the variable has exactly one assignment in the module, otherwise `stopped: function value`.
- Closures returned by functions (`Template(name, data)` returning a handler): captured variables flow into the closure body.
- Interface method calls are left to widening, mark them `stopped: interface` for now.

**Acceptance.** `fixtures/flow/flow-funcs/` covering both cases. `tests/path_flow-funcs.rs`.

## Package: flow-up

**Do.**
- `<=` and `<==`: the engine run in reverse, from a value back to where it was created. Stops at a `source` model, a literal, a parameter of an exported entry point, or an outside call's return.
- Value sets for strings (decision 25): literals, constants, union over callers for parameters, concatenation as glob, open otherwise.
- `from` for method receivers: `logger.info` gets its origin by walking `logger` upward to its creation. Plug into `src/semantic/attrs.rs`.

**Acceptance.** Unit tests for each value-set rule. `fixtures/flow/flow-up/`: a template name parameter with three literal callers resolves to those three names. `tests/path_flow-up.rs`.

## Package: widening

**Do.** Decision 14. Constructed-type index (composite literals, `&T{}`, `new(T)`, constructor return types) once per run. Extend `interface_impls` (`src/resolve/go.rs:561`) to filter by it. Narrow on type assertions and type switches. Replace flow-funcs' `stopped: interface`. Group outside implementations in the printer, `--expand` lists them.

**Acceptance.** `fixtures/flow/widening/`: an interface with three implementations, two constructed, the walk widens to two, a type assertion narrows to one, an `io.Writer` sink groups outside implementations. `tests/path_widening.rs`.

## Package: templates-grammar

**Do.**
- Add a Go template tree-sitter grammar. Check crates.io for a maintained crate compatible with tree-sitter 0.26 first. Otherwise vendor `ngalaiko/tree-sitter-go-template` under `vendor/` and build it with `cc` in `build.rs`. Record the choice in `DESIGN.md`.
- Register the language in `src/compiler/lang/mod.rs` for `.tmpl`, `.gotmpl` (`.html.tmpl` ends in `.tmpl`) with a `Lang` impl in `lang/gotmpl.rs` mapping the abstract kinds that make sense.
- Add abstract kind `template_action` to `types::ABSTRACT_TYPES`.
- Add template mechanics keys to the config only through a new `config/languages/gotmpl.toml` default (orchestrator note: `config/` belongs to no other active package in this stage).

**Acceptance.** `ctsq -q 'template_action' fixtures/` lists actions in a `.tmpl` fixture. `ctsq tree` on a `.tmpl` file does not crash. `cargo test` passes.

## Package: templates-flow

**Do.** Decision 24 and 25.
- Template flow: `.`, `range`/`with` rebinding, `template`/`block`/`define` as calls, `$x :=`, printing actions tagged `rendered`.
- Linking: from a modelled `Execute`, find the files parsed into that `*Template` through `template_load` models, the value set (flow-up), and `//go:embed` patterns on the `embed.FS`. The first `ParseFS` name argument picks the root template.
- Inline templates: a Go string constant or package-level string variable passed to `Parse` is parsed as a template.

**Acceptance.** `fixtures/flow/templates/` with a layout calling `{{block "body" .}}`, a page `{{define "body"}}` with `{{range .Items}}` printing `{{.Phone}}`, loaded through `ParseFS(files, name, "layout.html.tmpl")` with `name` from literals. `field#Item.Phone ==> template_action` reaches the page line, tagged `rendered`. An inline template is also reached. `tests/path_templates.rs`.

## Package: acceptance

**Do.**
- Build `fixtures/flow/booking/` combining the cases of every earlier fixture, and `tests/path.rs` running every example query in `## Goal` plus:
  1. `field#Booking.CustomerPhone ==> function#log.Printf` finds the `%v` log line.
  2. `field#Booking.CustomerPhone ==> function#/Encode/` finds the JSON response helper.
  3. `field#Booking.CustomerPhone ==> template_action` reaches template lines.
  4. `field#Booking.CustomerPhone ==>` with no end lists every outside leaf and the unmodelled trailer.
- `DESIGN.md` consistency pass over every section the build added.
- Manual run (not a test): the same four queries against `~/workspace/projects/badhabit/rehearsal-booking` with the built binary (`target/release/ctsq`, never `make install`), compared with `## Appendix: hand trace`. Report differences in the handoff.

## Appendix: hand trace`. Report differences in the delivery note.

## Appendix: hand trace

Hand trace of the customer phone number in the booking app, 2026-10-15. Use it to judge the manual run. It is the expected shape, not a byte-exact golden file. Paths are relative to that repo.

Sources:

- public JSON `POST /bookings`, decoded by `ExtractForm` (`pkg/handlers/extract-form.go:17`) and normalised at `pkg/handlers/bookings-create.go:92`
- admin JSON `POST /admin/bookings` (`pkg/handlers/admin-bookings-create.go:34`)
- admin `PUT /admin/bookings/{id}/update` (`pkg/handlers/admin-bookings-update.go:39`), not normalised
- CLI `cmd/book/main.go:209`

Leaves:

- DB insert and update through `StorageDriver` (`pkg/data_access/bookings.go:136`, `:73-75`), reflection-based
- JSON responses through `JSON[T]` (`pkg/handlers/handler.go:63`)
- log line `pkg/handlers/admin-bookings-update-view.go:26` (`%v` of the whole booking)
- SES email body via inline templates in `pkg/handlers/customer-confirmation-email.go` (`:193`, `:201`, `:214`, `:239`)
- templates `admin-bookings-update.html.tmpl:28` and `:126`, `admin-view-daily-bookings.html.tmpl:208`, `:229` (DB-read routes, separate queries under decision 17)

Template name set: `templates.Render` has one caller (`pkg/handlers/handler.go:50`), fed by 15 `Template(...)` calls passing 11 distinct literals. `//go:embed *.html.tmpl` at `web/templates/main.go:12` holds those 11 plus `layout.html.tmpl`.

Hard hops found: generic `ExtractForm[T]` with json tags, handler closures, `any`-typed render data, string-named template chains, inline templates in Go strings, embedded struct promotion, `range` rebinding, generic repository receivers, the `StorageDriver` interface, reflection DB helpers, SQL built from reflection, a function stored in a variable, generic `JSON[T]`, `%v` formatting, the AWS SES SDK, client-side JS.
