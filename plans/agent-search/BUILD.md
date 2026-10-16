# agent-search build notes

Amendments to PLAN.md from the plan review, made by the orchestrator before any code package launched. Where an amendment and PLAN.md disagree, the amendment wins. Each package brief names the amendments it needs.

## Amendments

### A01. `-l ts` picks the grammar per file

`-l ts`/`-l typescript` select `.ts` and `.tsx`, `-l js` selects `.js` and `.jsx`, and every file is parsed with the grammar its own extension implies. Today `--lang` forces one grammar on every file (main.rs:642, files.rs:43), so TS would be parsed with the TS grammar and fail on JSX.

### A02. Aliases do not change `example`

`method`, `string` and `comment` are appended to the end of `types::ABSTRACT_TYPES`, or skipped in `example` classification (example.rs:339 classifies by list order). Existing `example` tests stay green unchanged.

### A03. Already working, regression test only

`*function#Topbar` on a TSX const arrow already works. The kinds package adds a regression test and no code for it.

### A04. `call` is real in every language

`call` maps to `Some(vec![])` and matches nothing in c.rs:75, cpp.rs:118, go.rs:96, javascript.rs:69, rust.rs:105, typescript.rs:153. The kinds package maps it to the language's call node, with the name filter applied to the last segment of a member or scoped call. Python and Ruby already map it.

### A05. One file-discovery path

Query mode duplicates job discovery at main.rs:602-674 instead of calling `files::resolve_jobs` (files.rs:32). The kinds package moves query mode onto `resolve_jobs` before adding shebang detection, so shebang detection, test filtering and the no-such-file message each live in one place. `collect_files` (files.rs:17) drops files with no extension, so shebang detection belongs there too.

### A06. A walk with no files is a miss

A directory with zero matching files exits 0 silently (`!jobs.is_empty()` at main.rs:782). The misses package makes it print a miss report and exit 1.

### A07. The miss fallback matches text, not whole nodes

`literal#Foo` compares the whole node text including quotes, so the fallback runs the name as a case-sensitive substring (or the user's regex as written) over string literals, JSX text and comments. Each found-as block names what it found: `found as string literal:`, `found as JSX text:`, `found as comment:`. The `try:` line suggests a query that really matches those hits, checked by a test. The plan's own example is JSX text, which `literal` does not cover.

### A08. Name filters escape what tree-sitter needs

`name_predicate` (compiler/mod.rs:655) escapes `"` and `\` when it emits a predicate, and the lexer accepts `\/` inside `/.../`. Today `literal#/call"/` gives a raw tree-sitter error per file and `id#/a\/b/` a parse error.

### A09. Rust test scopes are found in code

`mod#tests` is not a valid selector (`mod` is an anonymous keyword, it slips past `validate` at compiler/mod.rs:211). Rust's default `scopes` is `["module#tests"]`, and items under `#[cfg(test)]` are detected in code because `attribute_item + mod_item` has no sibling semantics yet. The `scopes` field stays in the schema.

### A10. Built-in `[tests]` tables

Built-in language TOMLs are `include_str!`ed in `BUILTIN_LANGUAGES` (config/mod.rs:120), which holds only go and gotmpl. Add a `Tests {files, dirs, scopes}` struct to `Mechanics` (mechanics.rs:5, already `#[serde(default)]`). Add built-in `config/languages/<canonical>.toml` files for typescript, tsx, javascript, python, ruby, rust, c, cpp holding only `[tests]`, append `[tests]` to go.toml, register them. `config dump` accepts the same aliases as `-l` (today it passes the raw argument, so `dump ts` fails). Add a light `load_tests(root, lang)` that merges only the `[tests]` layers, skips go.mod and models, and is cached once per language per run. Data flow keeps its hardcoded Go load (semantic/flows.rs:113).

### A11. `tree` footer totals

`tree::report` runs once per file (main.rs:449) and exits on errors. The hidden count is summed per file in main, then printed once through the shared footer function.

### A12. What the selector extension really is

Generic field access already works: the parser takes any field name (parser.rs:321) and the compiler emits it through `inject_field` (compiler/mod.rs:125). The named-queries extensions are:
- `A ~ B` and `A + B` with real sibling semantics inside one shared parent. Today they compile as unrelated alternatives (compiler/mod.rs:402).
- a bare capture as a field argument, `.value(@target)`, and `literal @key` without parentheses inside field access. Both are parse errors today.
- a multi-capture row shape that `find` prints as key and target.

### A13. `--scope` stays on callers and callees

The review noted that callers and callees already print the enclosing function per site. Decision 05 names them, so `--scope` applies there too and prints the scope's `path:START-END` span and signature in the Output format shape.

### A14. Held for Jasper

Two command-line shapes are asked in QUESTIONS.md and are not built until he answers:
- how `def`, `refs`, `callers` and `callees` tell several names from a path (decision 07). Their path is a required positional today (main.rs:268).
- `--max N`, because `ctsq path --max` already means the hop limit (main.rs:386).

Everything else in the misses and tests-and-footers packages is built. The footer function takes an over-max reason so `--max` is a small change once decided.

### A15. Package cuts

The plan's packages run in plan order, split where one agent would not finish in about 80 turns:

| Order | Package | Tier |
|---|---|---|
| 1 | docs-examples | Sonnet (done) |
| 2 | kinds | Sonnet |
| 3 | misses (all but multi-name) | Sonnet |
| 4 | tests-and-footers (all but `--max`) | Opus |
| 5 | adversarial review of 2 to 4 | Opus |
| 6 | scope-flag: the resolver and `--scope` | Sonnet |
| 7 | scopeof | Sonnet |
| 8 | queries-core: A12 extensions, `queries/` loader, `detect`, `target`, `find`, Solid and React Router packs | Opus |
| 9 | queries-packs: `rewrite`, `paths`/`key_from_path`, glossary, decision 14, Next.js, Go net/http and chi, Rails, TSX, `error` packs | Sonnet |
| 10 | final review | Opus |

### A16. Docs stay runnable

Every package that adds a flag or command adds a worked example to `plans/agent-search/docs/SKILL.md` (a fenced block whose first line starts with `ctsq `, under `## Worked examples`), a line to the help EXAMPLES in `src/main.rs`, and a line to `plans/agent-search/docs/claude-md-ctsq.md` if it changes what an agent should reach for first. `bash plans/agent-search/docs/check-examples.sh` must pass after every package. Examples quote line numbers from this repo's source, so a package that shifts those lines updates the examples.

## Deferred minors

Minor findings from the mid-build adversarial review, not fixed in this build:

- `call#X` misses nested C++ qualified calls (`ns::Cls::target()`), template-scoped and template member calls (`obj.foo<int>()`), and Rust turbofish method calls (`x.target::<u8>()`). Single-level member and scoped calls work in all 8 languages.
- `callers` exits 0 on a walk with no files. `refs zzz dir` prints nothing and exits 0 (as before this build). An unknown kind exits 1, not 2 (as before). `tree emptydir` exits 1 with no message.
- Suggested fixes for quotes are wrong (`id#a"b` suggests `id#a"b"`, which also fails). `id#x > ` puts the caret at 0, not the dangling `>`. `id#/(/` puts the caret on the opening `/`.
- Rust test detection misses `#[cfg(all(test, ...))]`, a `#[test] fn` outside a cfg(test) mod, `mod test` (singular), and `src/tests.rs` loaded by `#[cfg(test)] mod tests;`. No false positives were found.
- An `--exclude` glob with a slash is matched relative to the search root, so the path as ctsq prints it (`th/ts/**`) excludes nothing, silently.
- `string` and `literal` do not cover JS/TS template strings, so the id# miss fallback misses them too.
- Test-scope detection still costs time: `tree src/` takes 0.27s against 0.10s before this build (0.41s before the fix). Next step: cache compiled scope queries across files.
