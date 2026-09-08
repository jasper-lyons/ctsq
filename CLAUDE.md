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
│       ├── rust.rs
│       └── go.rs
├── runner.rs         Executes a CompiledQuery against source bytes; returns Match list
├── explain.rs        `explain` subcommand — composes def + body text + callers + callees + refs
├── tree.rs           `tree` subcommand — structural outline, with a size header and per-entry signatures
├── cooccur.rs        `files` subcommand — files where every one of several queries matches
├── files.rs          File discovery: extension filtering, .gitignore/.ignore respecting
├── calltree.rs       `callers`, `callees`, `callgraph` — walks DefIndex to trace call chains
└── resolve/
    ├── mod.rs        Def and Callee types; DefIndex (name → location map); `def` subcommand
    ├── graph.rs      CallGraph structure and builder
    ├── rust.rs       Rust-specific resolution: imports, qualified paths, trait dispatch (CHA)
    ├── go.rs         Go-specific resolution: packages, interface dispatch
    └── fallback.rs   Name-only resolution for other languages
```

## Key concepts

**Abstract types** — a fixed vocabulary (`function`, `class`, `var`, `param`, `type`, `import`, `literal`, `block`, `if`, `for`, `while`, `switch`, `id`). Unknown identifiers pass through to TreeSitter verbatim as concrete node names — no escape syntax needed.

**Sigils** — `*type` = definitions only, `&type` = references/calls only, bare `type` = both.

The sigil also picks which node the abstract `type` reaches: `*type#X` resolves to the declaration the name is introduced by (Go `type_spec`/`type_alias`, TS `type_alias_declaration`/`interface_declaration`, Rust `struct_item`/`enum_item`/`trait_item`/`union_item`/`type_item`, C and C++ tagged specifiers plus `typedef`), while `&type#X` stays on the bare `type_identifier`. Without this, `*type#X --body` printed only the name.

**Query output** — `-q` prints `path:LINE: <the whole source line, trimmed>`, the same rendering `explain` uses for its REFERENCES. `runner::run_rows` returns both that and the older node-text form; `--node` selects the latter, and a match with two or more captures or a multi-line `--body` match falls back to it because one line cannot carry them. No match prints a miss report on stdout and exits 1.

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

Two subcommands build their whole output as a string so tests can assert on it without capturing stdout: `explain::report` and `tree::report`. `resolve::def_rows` is the same split for `def`. Prefer adding to those over testing print paths.
