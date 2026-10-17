# Parse cache: one parse per file per run

## Goal

Each file is read from disk once and parsed once per grammar in a ctsq run, and every consumer shares that parse. Output stays byte-identical. Today one `ctsq explain X src/` parses a defining file at about five sites and every other file at least twice (map of 2026-10-17). The last benchmark round left `-q id#name` on this repo at 1.17x the pre-build binary and `callers` at 1.09x, both from re-parsing for test hiding, and `explain` and `callers` on Mavexa at 7.6s and 4.8s, mostly from the call-graph build re-parsing.

Success:
- With `CTSQ_PARSE_STATS=1`, every bench call reports at most one parse per (file, grammar).
- Every comparable bench call is within 1.05x of the pre-build binary, or faster.
- Peak memory, measured with `/usr/bin/time -l`, is within 1.5x of the reference binary on every bench call.

## Decisions

### 01. One process-wide cache module

A new `src/parsed.rs` holds `thread_local! { static CACHE: RefCell<HashMap<(String, &'static str), Rc<ParsedFile>>> }` with `ParsedFile { lang: &'static str, source: Rc<[u8]>, tree: Tree }`, plus a text cache `HashMap<String, Rc<[u8]>>`. Why: ctsq is single-threaded (no rayon, no threads), tree-sitter `Node<'tree>` borrows need the tree to outlive the walk, and an `Rc` handle gives that without lifetimes in every signature. Beat: threading a store parameter through every function (churn across ~40 signatures), and `Arc<Mutex>` (no threads to pay for).

### 02. API

- `parsed::source(path) -> Option<Rc<[u8]>>` reads a file once.
- `parsed::get(path, lang) -> Option<Rc<ParsedFile>>` returns the cached parse, parsing on the first call with the grammar for `lang`.
- `parsed::parse_text(lang, bytes) -> Option<Tree>` parses non-file text, uncached.
- `parsed::stats()` returns parse and hit counts.

The key is the path string exactly as the job gives it plus the lang name. The grammar is chosen by the existing lang name, which is already refined per file (ts vs tsx). Why: jobs carry the same string for the same file within one run. Normalising paths is not needed and risks two keys disagreeing with what is printed.

### 03. Non-file text stays uncached

Stdin `-`, `example` snippets, Go template bodies in `flow/template.rs`, and inline sources in tests parse through `parse_text` or their existing path. Why: they have no path key and are parsed once anyway.

### 04. `TreeStore` becomes a view over the cache

`resolve::TreeStore` (resolve/mod.rs:160-195) keeps its API for the graph builder, backed by `parsed::get`. `ParsedFile` moves to `parsed.rs` and is re-exported from `resolve`. Why: the graph builder and data flow (main.rs:1487, flow/template.rs:321, semantic/flows.rs:388) already share one parse through it, so it is the seed, not a rival.

### 05. No eviction unless memory says so

Trees live until the process exits. Each package measures peak RSS. If any bench call exceeds 1.5x the reference binary's peak, the cache gets a byte cap with least-recently-used eviction, defaulting to 512MB of source. Why: a one-shot CLI that already holds every tree for the call graph rarely needs eviction, and eviction adds re-parse paths to test.

### 06. Results do not change

Every package diffs the stdout, stderr and exit code of a fixed call set against a reference binary built from the build tip before this work. Why: this is a refactor. Any difference is a bug.

### 07. `CTSQ_PARSE_STATS=1`

When set, ctsq prints `parsed N files, M cache hits, K text reads` to stderr at exit. Why: it is the acceptance measure, and it costs nothing when unset. It is undocumented in the user docs and named in CLAUDE.md for developers.

## Open questions

None. Everything here is internal.

## Conventions

- Read `CLAUDE.md` first. Use ctsq for code, mdq for markdown.
- Never `make install`. `cargo build --release` and `cargo test` pass after every package, and `bash plans/agent-search/docs/check-examples.sh` stays green.
- Reference binary: `git archive 1665e83` extracted into `/Users/jasperlyons/.claude/jobs/30494ad5/tmp/pc-ref/` and built with cargo there. The core package builds it. Later packages reuse it.
- The output check is `plans/parse-cache/same-output.sh`, written by the core package. It runs the call set from `plans/agent-search/measure/bench.py` plus the check-examples calls against this repo, Mavexa and emc-gigs, with both binaries, and diffs stdout, stderr and exit codes. It must print no differences.
- Benchmarks: `plans/agent-search/measure/bench.sh`, extended by the core package with a peak RSS column.
- Port every affected test's intent and never weaken an assertion.
- Mavexa (/Users/jasperlyons/workspace/projects/mavexa/mavexa) and emc-gigs (/Users/jasperlyons/workspace/projects/eastbournemusic/emc-gigs) are read-only.

## Schema

```rust
// src/parsed.rs
pub struct ParsedFile { pub lang: &'static str, pub source: Rc<[u8]>, pub tree: Tree }
pub fn source(path: &str) -> Option<Rc<[u8]>>;
pub fn get(path: &str, lang: &'static str) -> Option<Rc<ParsedFile>>;
pub fn parse_text(lang: &str, bytes: &[u8]) -> Option<Tree>;
pub fn stats() -> Stats; // { parses, hits, text_reads }
```

## Endpoints

None. No command, flag or output changes.

## Work packages

| Order | Package | Tier | Depends on |
|---|---|---|---|
| 1 | core | Opus | none |
| 2 | graph | Sonnet | core |
| 3 | consumers | Sonnet | graph |
| 4 | adversarial review and bench | Opus | consumers |

## Package: core

**Files owned:** new src/parsed.rs, src/runner.rs, src/resolve/mod.rs (TreeStore and ParsedFile only), src/main.rs, src/files.rs, src/tree.rs, plans/parse-cache/same-output.sh, plans/agent-search/measure/bench.{sh,py} (RSS column), CLAUDE.md (one line on parsed.rs and the env var), tests beside those files.

**Do.**
- The module and API above, with the stats env var.
- `TreeStore` backed by the cache (Decision 04).
- Runner: a path-aware entry, or callers fetch `parsed::get` and call `run_rows_on`. `run_rows` (runner.rs:69/84) keeps working for stdin and text.
- The query loop in main.rs and `hide_rows` (main.rs:1107) pass the cached tree, so `hider.scope_lines` gets the tree (files.rs:335/341).
- `Hider` parse paths: files.rs:164 `rows()` and :504 `parse_rust`.
- tree.rs: `report` (:42), `collect_function_locs` (:610), `collect_def_locs` (:625).
- Build the reference binary and write same-output.sh.

**Acceptance.**
- same-output.sh prints no differences.
- `CTSQ_PARSE_STATS=1 ctsq -q 'id#run' src/` reports parses equal to the number of files.
- `-q id#run src/` is within 1.05x of the pre-build binary at 5f867dc (the bench base).

## Package: graph

**Files owned:** src/resolve/{go,rust,fallback,imports,graph,mod}.rs (the rest of mod.rs: nested_def_sites :612, line text :387, def_rows, similar_hint), src/explain.rs, src/calltree.rs, tests beside those files.

**Do.**
- go.rs `parse` (:186) and rust.rs `parse` (:75): non-`_tree` entry points fetch from the cache.
- fallback.rs :46 and the six imports.rs parse helpers (:388, :442, :663, :792, :991, :1077) use the cache when they have a path.
- explain.rs: `definition_text` (:362), `references` via `runner::run` (:525), and line-text reads (:584) use `parsed::source`/`get`.
- resolve/mod.rs: `nested_def_sites` and line-text reads.
- `Hider::site` for callers reaches `scope_lines(file, None)` (files.rs:408). Make it fetch the cached tree, a minimal change in files.rs allowed here.

**Acceptance.**
- same-output.sh is clean.
- With `CTSQ_PARSE_STATS=1`, `explain` and `callers` on this repo and Mavexa report at most one parse per file.
- Timings are recorded.

## Package: consumers

**Files owned:** src/find.rs (Scan text cache :54/:907, run_query :487, default_export :627), src/miss.rs (rows_over :265), src/scope.rs (Scopes :368/:377, `parse` :68), src/scopeof.rs, src/cooccur.rs (:28, :84), src/semantic/{mod,attrs,flows,calls}.rs, src/flow/template.rs (TreeStore reuse only), tests beside those files.

**Do.** Route every remaining file parse and file-text read through `parsed`. `Scan` and `Scopes` become thin views or are removed.

**Acceptance.**
- same-output.sh is clean.
- `CTSQ_PARSE_STATS=1` on `find route`, `find error`, `def --scope`, `scopeof`, `files` and a miss reports at most one parse per (file, grammar).
- There is no remaining `Parser::new` outside parsed.rs, example.rs, flow/template.rs text parsing and tests: `ctsq -q 'call#new' src/` filtered to Parser.

## Package: adversarial review and bench

An Opus reviewer that edits nothing. It tries to refute Decisions 01 to 07: stale or wrong-grammar hits (the same path under two grammars, ts and tsx), relative and absolute path strings for the same file, stdin, memory on Mavexa, and output identity. It then re-runs bench.sh with RSS and records the results in plans/agent-search/DELIVERY.md under `## Performance, parse cache`.
