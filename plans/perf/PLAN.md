# Performance round 2: parse less, scope the graph, fix the outliers

## Goal

Every call in `plans/perf/calls.txt` runs under 2s on Mavexa (decision 15 of the agent-search plan). Output stays byte-identical, and memory comes back toward the reference binary. Baseline: `plans/perf/RESULTS-ce8b812.md` (52 calls). Over 2s there: `path ==>` 78s, a `~` sibling query 5.9s, `path -->` 5.4s, `explain` 4.5s, `scopeof --siblings --callers` 3.9s, `callers`/`callees`/`callgraph` about 2.9s, `def A B C` 2.1s.

## Decisions

### 01. Output never changes

Every package keeps `bash plans/parse-cache/same-output.sh` at 0 differences, and `cargo test` and `check-examples.sh` green. Footer counts of hidden tests are output too. Why: this is all optimisation.

### 02. Skip a file only when it cannot match

A file is skipped before parsing only when its bytes provably cannot produce a match or a counted hidden hit. For example, an exact name filter whose bytes are absent from the file. Test files are not skipped wholesale, because the footer counts their hits. Why: the footer's hidden counts (agent-search decision 03) need hits in tests, so the safe skip is by content, not by path.

### 03. The call graph is built from the files that can matter

`callers`, `callees`, `explain`, `def`, `callgraph`, `scopeof --callers` and `path` build only what the question needs. The design is written by the package, but its contract is that the answer is identical to the whole-folder graph. Why: the profile showed 60 to 80% of these calls is parsing and resolving files unrelated to the name.

### 04. Memory follows from scoping

After package 3 lands, peak RSS on every Mavexa call is within 1.5x of the reference binary (`/Users/jasperlyons/.claude/jobs/30494ad5/tmp/pc-ref/target/release/ctsq`). The graph package's 240MB for `explain`/`callers` is accepted only until then. Why: holding trees for only the relevant files shrinks memory along with time.

## Open questions

None. Everything here is internal.

## Conventions

- Read `CLAUDE.md`. Use ctsq for code, mdq for markdown. Never `make install`.
- Gates after every package:
  - `~/.cargo/bin/cargo build --release` with no warnings
  - `~/.cargo/bin/cargo test`
  - `bash plans/agent-search/docs/check-examples.sh`
  - `bash plans/parse-cache/same-output.sh` with 0 differences
- Measure with `bash plans/perf/profile.sh plans/perf/calls.txt <filter>` (wall, buckets, parse stats) and `/usr/bin/time -l` (peak RSS). Each package reports before and after for the calls it targets.
- Base binary (before agent-search): `/Users/jasperlyons/.claude/jobs/30494ad5/tmp/bench/base/target/release/ctsq`.
- Cache API: src/parsed.rs, `get(path, lang)`, `source(path)`, `try_source`, `parse_text`, `stats()`. Weak handles plus a 4MB recent window, so keep the `Rc` alive across the stages that share a parse. `CTSQ_PARSE_STATS=1` prints parse counts.
- Port every affected test's intent and never weaken an assertion. Mavexa and emc-gigs are read-only.

## Work packages

| Order | Package | Tier |
|---|---|---|
| 1 | def-revert | Sonnet |
| 2 | name-prefilter | Sonnet |
| 3 | scoped-graph | Opus |
| 4 | path-flow | Opus |
| 5 | query-match | Sonnet |
| 6 | review and profile | Opus |

## Package: def-revert

`def` does not hold a TreeStore across its stages: the graph package made it parse all 740 Mavexa files and peak at 208MB, against 18MB for the reference, for no speed gain (0.74s against 0.69s). Restore parse-and-drop for `def`, in resolve/mod.rs (`def_report`, `def_rows`, `mapped_rows`). Keep `explain` and `callers` as they are. Fold this into package 2 if it is small.

## Package: name-prefilter

Before parsing, skip files whose bytes lack every exact name a query needs (Decision 02). `find.rs` already has `needles()` and a regex-literal version of it, so generalise that into one shared helper used by:
- `-q` and `refs` (the main.rs query loop)
- `files` (cooccur.rs, where a file needs all the queries)
- the miss fallbacks (miss.rs, already partly done)
- `def`'s nested search
- `find` (keep its current behaviour)
- the Hider's scope passes

A query with no exact name, such as a bare kind or a regex that is not plain literals, parses every file as now. Targets: the `-q` family on Mavexa, about 1s now, and `files`.

## Package: scoped-graph

Write `plans/perf/scoped-graph.md` first, half a page. It covers how each command gets the same answer from fewer files:
- for `callers` of X: the files whose text holds X, plus what is needed to resolve each call (imports, receiver types, Go interface methods named X)
- for `callees` of X: X's file plus the definitions of the names it calls
- for `explain`: the union of the two

Route the Go and Rust resolver parses (resolve/go.rs `parse` :186, resolve/rust.rs `parse` :75) through parsed.rs. Build the definition index once per run, so `def A B C` costs about one `def`. Make `scopeof --siblings --callers` use the scoped path. Targets: `explain`, `callers`, `callees`, `callgraph` (whole graph by nature, so it only needs the shared parse), `def A B C`, `scopeof --callers`, and Rust `explain`, which parses 64 times for 58 files. Memory per Decision 04.

## Package: path-flow

`path field#signupRequest.Email ==>` takes 78s on Mavexa backend. 70% is tree walking in `resolve::go::walk_children` under the data-flow engine, plus 16% allocation. Profile it, find the repeated walk (likely the same subtree walked once per field or per hop), and fix it with memoisation or an index. `path function#login --> function#writeError` takes 5.4s, 21% of it compiling queries: compile each query once per run. Target: both under 2s with identical output.

## Package: query-match

- A `~` or `+` sibling query on Mavexa backend takes 5.9s, 86% of it query matching, because the single tree-sitter pattern for two siblings in one parent is slow. Run it in two phases: match each side separately, then pair the matches by shared parent and order. The output must be identical.
- In `find text`, `error`, `component`, `route` and `label`, skip files lacking the key text when a key is given, using package 2's helper.
- Ruby `explain` and `callers` spend 73 to 78% compiling call queries in `runner::cached_query`: compile once per language per run and lazily.
- A single-file `-q` spends 78% in `compiler::check_compiles`: check once per language.

## Package: review and profile

An Opus reviewer that edits nothing tries to break Decisions 01 to 04. It checks:
- output identity on edge cases: names in comments or strings, aliases, Go interface dispatch, Rust traits, tests hidden and shown
- skipped files that could have matched

Then it re-runs `plans/perf/profile.sh` on all 52 calls and writes `plans/perf/RESULTS-<sha>.md`, with peak RSS.
