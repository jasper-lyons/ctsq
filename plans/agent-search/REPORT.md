# agent-search build report

Branch `worktree-agent-search-build`, built on top of `5f867dc`. Nothing has been merged to main or installed.

## What's been built

- **Worked examples in the docs.** The skill draft (`plans/agent-search/docs/SKILL.md`), the CLAUDE.md block draft (`plans/agent-search/docs/claude-md-ctsq.md`) and `ctsq --help` now teach by example: the question, the call, real output, the next call. `plans/agent-search/docs/check-examples.sh` runs every example and fails if an output line no longer appears.
- **Kinds match how agents name things.** `call` works in every language and matches the last segment of `pkg.Func()`. `method`, `string` and `comment` work everywhere. `id` includes property names, field names and JSX attribute names. `-l ts` includes `.tsx` and `-l js` includes `.jsx`. Each file is parsed with its own grammar. Scripts with no extension are read by their shebang (ruby, python, node).
- **Misses explain themselves.** An `id#` query that matches nothing reports where the text appears as a string, as JSX text or in a comment, with a `try:` query that works. A `*function#X` miss says when X is defined as another kind. A broken query prints the query with a caret under the fault and a hint, and exits 2. A missing path prints `no such file: X (did you mean Y)` and the call carries on.
- **Tests hidden by default.** `tree`, `-q`, `def`, `refs`, `callers`, `callees`, `explain`, `files` and `find` skip test files and test scopes (Rust `mod tests`, `#[cfg(test)]`). The output ends with `hidden: N in tests (--tests)`. `--tests` restores them and `--exclude GLOB` skips paths.
- **`--scope`** on `def`, `refs`, `callers`, `callees`, `files` and `-q` prints the function or class holding each hit, with its span and signature (`--body` adds the body). `def` now also finds names defined inside functions.
- **`ctsq scopeof path:line`** returns the function holding a line, with its body. Options: `--up N`, `--siblings`, `--callers`, `--block`, `--budget N`.
- **`ctsq find`** lists the named queries that apply in the current repo. `ctsq find <query> [key]` runs one:
  - `route`: Solid, React Router, Next.js `app/`, Go net/http and chi, Rails
  - `text`, `label`, `component`: TSX and JSX
  - `error`: Go, TS/JS and Ruby. Keys match through `%s`-style placeholders.

  Repos can add or override queries in `.ctsq/queries/`. A repo glossary (`.ctsq/glossary.toml`) maps product words to code names for `find`, `def` and miss reports.

## How it affects a user

- Results from test files disappear by default. Every command that hides something says how many and names `--tests`.
- Query errors exit 2 (previously 1) and print a caret report, not `Parse error`.
- The `-q` miss header changed from `no match for id#x in P (N files scanned)` to `no identifier matches id#x in P (N files)`, followed by found-as blocks.
- `ctsq tree src/` on this repo takes about 0.22s, against 0.16s before the build. The difference is test-scope detection. With `--tests` it is back to 0.16s.
- On Mavexa, `find route` takes 1.8s, `find error` 1.8s and `find text` 0.5s.
- Unchanged: one name per `def`, `refs`, `callers`, `callees` and `explain` call. `ctsq path --max` still means the hop limit.

## How to test it

From the build worktree, after `cargo build --release`, with `C=./target/release/ctsq`:

1. `$C tree src/` ends with `hidden: N in tests (--tests)`, and the outline has no `mod tests` blocks.
2. `$C refs HiddenLimit fixtures/hidden` shows 2 rows and `hidden: 2 in tests`. Adding `--tests` shows 4 rows and no footer.
3. `$C -q 'id#/Cannot detect/' src/` exits 1 and prints a `found as string literal:` block plus a `try:` line. Running the `try:` line finds those hits.
4. `$C -q 'function#(' src/` prints a caret report and exits 2.
5. `$C def saveAssetNow fixtures/scope --scope` shows two definitions, each under its component (`AssetEditor`, `BrandAssetPanel`).
6. `$C scopeof src/footer.rs:30 src/scope.rs:100` prints two scopes with their bodies.
7. `$C scopeof src/main.rs:600 --budget 20` prints a 20-line window and a `showing lines ... --budget N for all` line.
8. `cd fixtures/queries/go-chi && ../../../target/release/ctsq find route` lists the chi routes with handler locations.
9. From the Mavexa root, `<worktree>/target/release/ctsq find route /library` names `LibraryRoute` at `frontend/src/routes/library.tsx:17`.
10. From the Mavexa root, `find text "Live preview"` names the `Asset` component.
11. `bash plans/agent-search/docs/check-examples.sh` checks 25 skill examples and 24 help examples.

## Verification

- After every merge, the orchestrator re-ran the release build, `cargo test` and `check-examples.sh` on the build tip. The final tip has 0 warnings and every test suite passes.
- A plan reviewer checked the plan against the code before the build started. Its corrections are amendments A01 to A16 in BUILD.md.
- An adversarial reviewer checked the kinds, misses and tests-and-footers work. Its four major findings were fixed.
- A final reviewer re-ran every example in the plan's Goal section and compared output and timings with the old binary. Its three major findings were fixed: `def` speed, the Mavexa Solid route shape, and JSX strings inside expressions.

## Left undone

- Held for Jasper (QUESTIONS.md): several names per call (decision 07), and `--max N`.
- The question script could not post them. `brief ask` needs a `.queue` folder holding `agent-search.md`, and this repo has none.
- Footer shape (QUESTIONS.md item 3). The build follows the plan's sample, one `hidden:` line. The plan's prose says one line per reason.
- Minor findings that were not fixed are listed under "Deferred minors" in BUILD.md.
- Goal measures: not re-measured. Jasper re-runs `measure/` after installing, as the plan says, and fills in DELIVERY.md.

## Choices made where the plan was silent (for review)

- An explicitly named file is never hidden as a test. Test paths are judged relative to the search root.
- When every match was hidden, the output is `0 matches` plus the footer, and the exit is 1.
- With `--scope`, the output always ends with a count line. A top-level hit gets no scope row. `--scope` with `-C` is rejected.
- `scopeof` uses the scope holding the line, so a function's own signature line returns that function. A top-level line prints `at top level` and a window around it.
- `find` searches the current directory. Each row is `path:line  key  target  target-path:line`, and keys are cut to 40 characters for display. An unresolved target prints `?`. Each detected framework that matched nothing still gets a `0 routes (...)` line.
- `find` with no arguments prints one row per query: `<source>:<line>: <query>  <framework>  (<why it applies>)`.
- Glossary use is announced on one line, `.ctsq/glossary.toml:LINE: "term" also searched as Name`.
- Built-in packs use concrete node kinds, not `id#`/`literal`, because abstract kinds inside field positions can make tree-sitter reject the whole pattern.
- In JS and TS, a `def` target that is a quoted module path resolves to that module's default export. That is how the Solid lazy-import shape works.
- `string` matches only string literals, never numbers. `bash`/`sh` shebangs are recognised, but those files are skipped because there is no shell grammar.
- The Rails `resources :x` row has key `/x` and target `XController`, through a new `key_rewrite` field.

## Follow-up round (2026-10-16), after Jasper's answers

This round supersedes the "Left undone" items above about the held questions and the footer shape.

- **Several names per call:** `def`, `refs`, `callers`, `callees` and `explain` take names, then paths. An argument that exists on disk is a path, `--` forces paths, the search root defaults to `.`, and the names that missed are listed on stderr as `not found: X (similar: ...)`. The exit is 0 if any name resolved.
- **Limits:** `--max-matches N` on every command that returns hits. `path --max` is renamed `--max-depth`, and `callers`/`callees` `--depth` is renamed `--max-depth`, with `--depth` still accepted.
- **Footer:** one `hidden:` line per reason.
- **Errors on stderr:** every miss report goes to stderr with exit 1, and so does the everything-hidden report (`no reference to X outside tests in P (N files)`).
- **`find` listing:** the columns line up.
- **Performance:** `plans/agent-search/measure/bench.sh` compares the new binary with the pre-build one (5f867dc). The tables are in DELIVERY.md. On Mavexa, `explain` went from 33s to 7.6s and `callers` from 29s to 4.8s. Two calls are still slower than before: `-q id#` on this repo at 1.17x and `callers` at 1.09x. Both are the cost of hiding tests, and both match the old binary with `--tests`.
- **Docs and install:** the docs drafts were re-copied to `~/.claude/CLAUDE.md`, `~/.claude/skills/ctsq/SKILL.md` and `control/.agents/skills/ctsq/SKILL.md`, and `make install` was run from this branch.
