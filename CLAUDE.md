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
├── tree.rs           `tree` subcommand — structural outline (classes, functions, vars)
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

Fixture source files live in `fixtures/` and are used by manual smoke tests. There are no automated integration tests yet — run the binary against fixtures to verify behaviour after changes.
