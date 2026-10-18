# ctsq

A Rust CLI that provides a cross-language, CSS-inspired abstract query language on top of TreeSitter, plus call graph and structural analysis subcommands. See README.md for user-facing documentation.

## Build & run

```sh
cargo build --release
make install          # installs to ~/.local/bin/ctsq
./target/release/ctsq -q 'id#foo' src/
```

## Architecture

```
src/
├── main.rs           CLI routing (clap): dispatches to subcommands or query mode
├── query/
│   ├── ast.rs        Query AST types: Atom, Selector, Combinator, FieldAccess, Sigil
│   └── parser.rs     Parses the abstract query string into the AST
├── compiler/
│   ├── mod.rs        Multi-phase compilation: parse → resolve abstract types → emit TS S-expression
│   │                 Returns CompiledQuery::Simple or CompiledQuery::Scoped (two-phase)
│   └── lang/
│       ├── mod.rs    Lang trait, language registry, extension→language mapping
│       ├── c.rs      C-specific abstract-type → TS node mappings
│       ├── cpp.rs    C++ mappings
│       ├── javascript.rs
│       ├── python.rs
│       ├── ruby.rs
│       ├── rust.rs
│       └── go.rs
├── parsed.rs         Per-run cache: each file read once, parsed once per grammar (`parsed::get`); CTSQ_PARSE_STATS=1 prints counts at exit
├── runner.rs         Executes a CompiledQuery against source bytes; returns Match list
├── explain.rs        `explain` subcommand — composes def + body text + callers + callees + refs
├── tree.rs           `tree` subcommand — structural outline, with a size header and per-entry signatures
├── cooccur.rs        `files` subcommand — files where every one of several queries matches
├── files.rs          File discovery: extension filtering, .gitignore/.ignore respecting, shebang detection, `Hider` applies test hiding and --exclude
├── footer.rs         The count line and `hidden:` line printed after output that dropped something
├── miss.rs           Miss reports (found-as blocks, other-kind definitions, glossary line) and caret query errors
├── scope.rs          Nearest named scope around a byte offset, shared by `--scope` and `scopeof`
├── scopeof.rs        `scopeof` subcommand: from path:line to the scope holding it
├── find.rs           `find` subcommand: lists and runs named queries
├── calltree.rs       `callers`, `callees`, `callgraph` — walks DefIndex to trace call chains
├── importgraph.rs    `imports`, `importers`, `importgraph`: the file-level analogue of calltree
├── example.rs        `example` subcommand: derives queries from a code snippet
├── types.rs          `ABSTRACT_TYPES` and the `types` subcommand
├── config/
│   ├── mod.rs        Layered TOML lookup: repo .ctsq/, XDG config, XDG cache, built-in, `config path|dump`
│   ├── mechanics.rs  Per-language node-kind roles for flow analysis and the `[tests]` tables (config/languages/*.toml)
│   ├── queries.rs    Named query files (config/queries/*.toml) and their schema
│   ├── glossary.rs   `.ctsq/glossary.toml`: product words to code names
│   ├── models.rs     Models of calls into outside code (config/models/)
│   └── gomod.rs      go.mod reading
├── flow/             Data flow inside one function and through Go templates, for `path` (engine, objects, values, upward, widen, template, models, report)
├── semantic/         Stages after tree-sitter matching for `path`: attribute filters, call and data-flow combinators, printer
└── resolve/
    ├── mod.rs        Def and Callee types; DefIndex (name → location map); `def` subcommand
    ├── graph.rs      CallGraph structure and builder
    ├── imports.rs    Project-wide import graph: files as nodes, imports resolved to files
    ├── tsconfig.rs   tsconfig/jsconfig reader (`extends`, `baseUrl`, `paths`) for import aliases
    ├── rust.rs       Rust-specific resolution: imports, qualified paths, trait dispatch (CHA)
    ├── go.rs         Go-specific resolution: packages, interface dispatch
    └── fallback.rs   Name-only resolution for other languages
```

## Key concepts

**Abstract types** — a fixed vocabulary (`function`, `class`, `var`, `variable` (alias `const`), `param`, `type`, `import`, `literal`, `block`, `if`, `for`, `while`, `switch`, `id`, `call`, `method`, `string`, `comment`, plus `module`, `op`, `field` and `template_action` for the languages that use them), listed in `types::ABSTRACT_TYPES`. Unknown identifiers pass through to TreeSitter verbatim as concrete node names — no escape syntax needed. An identifier that is neither an abstract type nor a real node kind in the language is caught by `compiler::validate` before any file is read, which prints `unknown kind 'X' for LANG, valid kinds: …` and exits 1 rather than letting TreeSitter report "Invalid node type" from inside the runner.

**Sigils** — `*type` = definitions only, `&type` = references/calls only, bare `type` = both.

The sigil also picks which node the abstract `type` reaches: `*type#X` resolves to the declaration the name is introduced by (Go `type_spec`/`type_alias`, TS `type_alias_declaration`/`interface_declaration`, Rust `struct_item`/`enum_item`/`trait_item`/`union_item`/`type_item`, C and C++ tagged specifiers plus `typedef`), while `&type#X` stays on the bare `type_identifier`. Without this, `*type#X --body` printed only the name.

`variable` (alias `const`) is the same shape one level down: `*variable#X` resolves to the statement the name is bound by (JS/TS `lexical_declaration`/`variable_declaration`, Go `var_declaration`/`const_declaration`/`short_var_declaration`, Rust `let_declaration`/`const_item`/`static_item`), `&variable#X` stays on the identifier. C, C++ and Python have no mapping, because a C `declaration` node covers variables, prototypes and pointer declarators alike, so it needs more than a table entry. In JS/TS the declarator holding the name is a positional child rather than a field, which is what `TsVariant.name_field: Some("")` means: take the name via `name_path` from the node itself, with no field step. An empty `inner_field` in `name_path` means the same thing one level down (Go `expression_list`).

**Query output** — `-q` prints `path:LINE: <the whole source line, trimmed>`, the same rendering `explain` uses for its REFERENCES. `runner::run_rows` returns both that and the older node-text form; `--node` selects the latter, and a match with two or more captures or a multi-line `--body` match falls back to it because one line cannot carry them. The node-text form carries a `<match>` placeholder where the file should be, since the runner does not know the path. `main::with_path` swaps the path in at print time, or prefixes it for the capture form, which has no placeholder, so no printed match is missing the file it is in. Stdin prints as `<stdin>`. No match prints a miss report on stderr, nothing on stdout, and exits 1 (BUILD.md A20: the same holds for every command's miss and for the all-hidden report, `footer::nothing_shown`). `--max-matches N` is a process-wide cap in `footer.rs` (`take` before each printed row, `settle` copies the cut count into `Hidden::over_max`). With `--scope` each row is followed by its scope row, and a count line ends the output.

**Test hiding and the footer**: test files and test scopes are hidden by default in `tree`, `-q`, `def`, `refs`, `callers`, `callees`, `explain`, `files` and `find`. What counts as a test comes from the `[tests]` table of each `config/languages/*.toml` (`ctsq config dump <lang>` shows it), applied by `files::Hider`. `--tests` restores them and `--exclude GLOB` (repeatable, judged from the search root) hides paths. A file named directly is never hidden. `footer.rs` builds the last lines: a count and one `hidden:` line per reason (tests, excluded paths, over `--max-matches`). When everything was hidden stdout is empty, stderr gets `footer::nothing_shown` and the exit is 1. `callers`, `callees` and `path` take `--max-depth` (`--depth` stays a hidden alias on the first two, `path --max` is an error). Output that hid nothing has no footer.

**Several names**: `def`, `refs`, `callers`, `callees` and `explain` take `<NAME>... [PATH]...`. `main::split_names_paths` does the git-style split (an argument that exists is a path, `--` forces paths, an identifier-like argument that exists is a usage error, a missing argument with a `/` is dropped with `miss::no_such_file`), and `main::targets` turns it into names, jobs and a `Hider`. Each command has a report fn that answers every name in order and returns `resolve::NamesOut` (stdout, stderr, code): `resolve::def_report`, `calltree::report_many`, `explain::report_many`. The `not found:` line is `resolve::not_found_line`. Footers count across all names and print once. Exit 0 when any name resolved.

**Miss reports**: `miss.rs`. A lone `id#name` that matches nothing reruns the name over string literals, JSX text and comments and prints `found as` blocks and `try:` lines. A lone `*kind#name` miss names the other definition kinds holding the name. A query that does not parse prints the query with a caret and a hint and exits 2, as does an unknown field after `.`. A missing path prints `no such file: X (did you mean Y)` on stderr and the call continues.

**Scope and scopeof**: `scope.rs` finds the nearest named scope (function, class, type, impl, trait) around a byte offset. `--scope` on `-q`, `def`, `refs`, `callers`, `callees` and `files` prints `  in path:START-END  signature` under each hit plus a count line, and `--body` adds the scope's body. `scopeof path:line[:col]` is the reverse of `def`, with `--up`, `--siblings`, `--callers`, `--block` and `--budget`.

**Named queries and the glossary**: `find` runs TOML query files from `.ctsq/queries/`, `$XDG_CONFIG_HOME/ctsq/queries`, `$XDG_CACHE_HOME/ctsq/queries`, then the built-in packs in `config/queries/` (routes for Solid, React Router, Next.js app, Go net/http and chi, Rails, plus `text`, `label`, `component` for TSX/JSX and `error` for Go, TS/JS and Ruby). The schema (`config/queries.rs`) is `framework`, `langs`, `detect` (import, file, dependency), `match` (a selector or list), `target` (def, enclosing, self, default-export), `rewrite`, `key_rewrite`, `paths`, `key_from_path`, and the key matchers `key_pattern`, `key_input`, `key_text`, `key_match` (whole or within), `key_min_literal`, `key_nest`. A repo or XDG table that replaces a built-in one of the same framework and name inherits each `key_*` field it leaves out (`queries::load_with`). `key_nest` (Solid and React Router) joins the keys of enclosing rows of the same query onto a nested row's key (`find::nest_keys`). `key_pattern` rules turn each row's key into a regex (`config::queries::KeyScanner`), which `find::KeyMatcher` tests the user's key against beside the substring test: route packs use it so a pasted URL finds its route (most literal characters first, `name=value` params after the row), the error packs so printf placeholders match a log line. No framework or printf knowledge is left in Rust. `.ctsq/glossary.toml` maps product words to code names and is used by `find`, `def` and the `-q` miss reports, each saying so in a line starting with the glossary's `path:line`.

Selector syntax also has `A ~ B` (later sibling in the same parent), `A + B` (next sibling), bare `@cap` as a field argument and `.field(kind @cap)`. `call` matches the last segment of a member or scoped call, `method` aliases `function`, `string` and `comment` exist in every language, `id` includes property and field identifiers and JSX attribute names, and `function` includes const-bound function expressions. `-l ts` includes `.tsx` and `-l js` includes `.jsx`, each file parsed by its own grammar, and a file with no extension is detected by its shebang (ruby, python, node).

`-C N` (`--context N`) prints N source lines either side of each match, modelled on `grep -C`: matched lines keep the colon form, context lines use `path-LINE- text`, groups are separated by `--`, and windows that overlap or touch merge. Lines keep their indentation there, unlike the one-line form. It lives in `main::print_with_context`, is rejected with exit 2 alongside `--node`, and does not apply to matches that fell back to the node-text form.

**Multi-phase (scoped) queries** — field access `.body(inner)` compiles to a two-phase `CompiledQuery::Scoped`: run the outer query to find container nodes, then search their subtrees with the inner query.

**Confidence** — call targets are `Exact` or `Guess` (`?`). Multiple candidates are ranked by locality: same file > same directory > imported module.

**Structural mapping** — abstract types map to structural TreeSitter patterns, not just node-name substitution. Arrow functions in JS, for example, have their name in the parent `variable_declarator`, so the mapping emits a pattern that captures the parent.

## Adding a new language

1. Create `src/compiler/lang/<lang>.rs` — implement the `Lang` trait (define `abstract_type_patterns`, `extension`, etc.)
2. Register it in `src/compiler/lang/mod.rs` — add to `for_name`, `lang_for_extension`, `all_known_extensions`
3. Add call resolution logic in `src/resolve/<lang>.rs` if needed, otherwise the fallback (name-only) applies automatically
4. Add fixture files under `fixtures/<lang>/` for manual testing

## Testing

```sh
cargo test
```

Unit tests live beside the code they cover. `tests/cli.rs` runs the built binary against `fixtures/` for the argument handling, printed forms and exit codes that only the process boundary shows.

Several subcommands build their whole output as a string so tests can assert on it without capturing stdout: `explain::report_hiding`, `calltree::report_scoped`, `find::list_report` and `find::report_with`, `scopeof::report`, and `tree::report` (which returns an `Outline`). `resolve::def_rows` is the same split for `def`. Prefer adding to those over testing print paths.

`bash plans/agent-search/docs/check-examples.sh` runs the commands in the skill's worked examples against the built binary.
