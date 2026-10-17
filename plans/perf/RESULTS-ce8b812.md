# ctsq perf baseline at ce8b812

Method: release build with debug symbols; wall = median of 3 plain runs; samples = /usr/bin/sample at 1ms
on a process spawned suspended (summed over up to 10 runs for calls under 1s), only samples under
ctsq::main; each sample goes to the bucket of its deepest matching frame. `parses/hits/reads` is
CTSQ_PARSE_STATS=1 (the Go and Rust resolvers parse outside the counter, so def/callers/callees/callgraph
show 0/0/0 there; their parse cost is the `resolve::go::parse` / `resolve::rust::parse` rows).
Calls: plans/perf/calls.txt. Re-run: `plans/perf/profile.sh > table.md`.

Reading the `files` column in this table: it is the NON-TEST count (the miss report hides tests).
Counting tests too (profile.py now prints both), parses equal files in scope everywhere
(backend 740, frontend/src 303, src 58, app 79) except #49 explain on Rust (64 vs 58, six repeat parses)
and #24 (scopeof --callers parses 383 files to answer for 1). So the `P>F` flags below mean
"test files were parsed and then hidden": 361 of 740 backend files, 103 of 303 frontend files.
`find` files (659) include .claude/skills jsx under Mavexa.

| # | root | call | wall s | samples | parses/hits/reads | files | parse incl % | top buckets (top ctsq fn) |
|--|--|--|--|--|--|--|--|--|
| 1 | mavexa | `-q 'id#writeError' backend` | 1.01 | 879 | 740/0/740 **P>F** | 379 | 68.3 | ts_parse 62.8% (parsed::parse_text); query_match 23.4% (runner::run_core); alloc 7.2% (parsed::parse_text) |
| 2 | mavexa | `-q 'id#/^write/' backend` | 0.93 | 1735 | 740/0/740 **P>F** | 379 | 70.7 | ts_parse 65.4% (parsed::parse_text); query_match 21.2% (runner::run_core); alloc 7.9% (parsed::parse_text) |
| 3 | mavexa | `-q '*function#register' --body backend` | 0.91 | 1775 | 740/0/740 **P>F** | 379 | 69.7 | ts_parse 64.3% (parsed::parse_text); query_match 21.1% (runner::run_core); alloc 8.0% (parsed::parse_text) |
| 4 | mavexa | `-q '*function#register.body(&function#writeError)' backend` | 0.94 | 1941 | 740/0/740 **P>F** | 379 | 68.8 | ts_parse 64.1% (parsed::parse_text); query_match 20.6% (runner::run_core); alloc 7.8% (parsed::parse_text) |
| 5 | mavexa | `-q '(*function#register @a) ~ (*function#login @b)' backend` | 5.87 **>2s** | 5455 | 740/0/740 **P>F** | 379 | 12.0 | query_match 86.3% (runner::run_core); ts_parse 11.0% (parsed::parse_text) |
| 6 | mavexa | `-q 'id#writeError' --scope backend` | 1.03 | 971 | 740/0/740 **P>F** | 379 | 70.1 | ts_parse 63.1% (parsed::parse_text); query_match 21.0% (runner::run_core); alloc 9.5% (parsed::parse_text) |
| 7 | mavexa | `-q 'id#Landing' frontend/src` | 0.37 | 1051 | 303/0/303 **P>F** | 200 | 62.6 | ts_parse 58.5% (parsed::parse_text); query_match 24.7% (runner::run_core); alloc 8.0% (parsed::parse_text) |
| 8 | mavexa | `-q '*function' --body frontend/src/components/Landing.tsx` | 0.01 | 50 | 1/0/1 | 1 | 8.0 | query_compile 78.0% (compiler::check_compiles); regex 10.0% (files::glob_regex); ts_parse 4.0% (parsed::parse_text) |
| 9 | mavexa | `-q 'id#zzqqx' backend` | 1.04 | 885 | 740/0/740 **P>F** | 379 | 69.2 | ts_parse 63.3% (parsed::parse_text); query_match 21.6% (runner::run_core); alloc 7.9% (parsed::parse_text) |
| 10 | mavexa | `tree backend` | 0.72 | 1303 | 740/0/740 **P>F** | 379 | 87.6 | ts_parse 80.0% (parsed::parse_text); alloc 11.2% (parsed::parse_text); ts_other 4.7% (parsed::Cache::keep) |
| 11 | mavexa | `tree backend/internal/api/auth.go` | 0.01 | 29 | 1/0/1 | 1 | 79.3 | ts_parse 69.0% (parsed::parse_text); alloc 10.3% (parsed::parse_text); regex 10.3% (files::glob_regex) |
| 12 | mavexa | `tree frontend/src` | 0.27 | 949 | 303/0/303 **P>F** | 200 | 87.8 | ts_parse 81.8% (parsed::parse_text); alloc 9.3% (parsed::parse_text); ts_other 4.2% (parsed::Cache::keep) |
| 13 | mavexa | `def writeError backend` | 0.71 | 1441 | 0/0/0 | 379 | 86.9 | ts_parse 80.4% (resolve::go::parse); alloc 10.7% (resolve::go::parse); ts_other 6.0% (resolve::def_rows) |
| 14 | mavexa | `def writeError --scope backend` | 0.75 | 1345 | 0/0/0 | 379 | 88.2 | ts_parse 81.4% (resolve::go::parse); alloc 10.5% (resolve::go::parse); ts_other 4.6% (resolve::def_rows) |
| 15 | mavexa | `def writeError register login backend` | 2.11 **>2s** | 1905 | 0/0/0 | 379 | 89.6 | ts_parse 81.5% (resolve::go::parse); alloc 11.2% (resolve::go::parse); ts_other 4.9% (resolve::def_rows) |
| 16 | mavexa | `refs writeError backend` | 0.93 | 1700 | 740/0/740 **P>F** | 379 | 66.4 | ts_parse 61.2% (parsed::parse_text); query_match 24.5% (runner::run_core); alloc 7.2% (parsed::parse_text) |
| 17 | mavexa | `explain writeError backend` | 4.46 **>2s** | 4002 | 740/0/740 **P>F** | 379 | 69.5 | ts_parse 65.1% (resolve::go::parse); alloc 11.8% (resolve::graph::assemble); ts_other 10.9% (resolve::graph::assemble) |
| 18 | mavexa | `callers writeError backend` | 2.83 **>2s** | 2642 | 0/0/0 | 379 | 65.5 | ts_parse 61.2% (resolve::go::parse); ts_other 14.8% (resolve::graph::assemble); alloc 13.6% (resolve::graph::assemble) |
| 19 | mavexa | `callees register backend` | 2.95 **>2s** | 2579 | 0/0/0 | 379 | 67.5 | ts_parse 63.4% (resolve::go::parse); ts_other 14.1% (resolve::graph::assemble); alloc 13.2% (resolve::graph::assemble) |
| 20 | mavexa | `callgraph backend --format edges` | 2.88 **>2s** | 2600 | 0/0/0 | 379 | 65.2 | ts_parse 61.0% (resolve::go::parse); ts_other 14.9% (resolve::graph::assemble); alloc 13.3% (resolve::graph::assemble) |
| 21 | mavexa | `files -q 'id#writeError' -q 'id#decodeBody' backend` | 1.00 | 921 | 0/0/0 | 379 | 66.9 | ts_parse 61.8% (runner::run_rows); query_match 23.5% (runner::run_core); alloc 7.5% (runner::run_rows) |
| 22 | mavexa | `scopeof backend/internal/api/auth.go:200` | 0.01 | 32 | 0/0/0 | 1 | 78.1 | ts_parse 68.8% (scope::parse); alloc 12.5% (scope::parse); ts_other 9.4% (scopeof::report) |
| 23 | mavexa | `scopeof backend/internal/api/auth.go:200 backend/internal/api/handler.go:1215 backend/internal/api/campaigns.go:80` | 0.01 | 83 | 0/0/0 | 1 | 90.4 | ts_parse 83.1% (scope::parse); alloc 10.8% (scope::parse) |
| 24 | mavexa | `scopeof backend/internal/api/auth.go:200 --siblings --callers` | 3.86 **>2s** | 3489 | 383/0/383 **P>F** | 1 | 63.3 | ts_parse 58.6% (resolve::go::parse); alloc 14.2% (resolve::graph::assemble); ts_other 11.8% (resolve::graph::assemble) |
| 25 | mavexa | `find` | 0.13 | 1014 | 0/0/0 | 659 | 4.0 | regex 55.8% (find::applies); alloc 22.7% (find::applies); fs_read 20.5% (find::applies) |
| 26 | mavexa | `find route /api/auth` | 1.71 | 1006 | 0/0/0 | 659 | 45.4 | ts_parse 41.6% (find::run_query); query_match 26.4% (runner::run_core); alloc 9.5% (find::run_query) |
| 27 | mavexa | `find error 'decode: %w (%s)'` | 1.36 | 1214 | 0/0/0 | 659 | 41.1 | query_match 39.4% (runner::run_core); ts_parse 38.2% (find::run_query); alloc 7.2% (find::run_query) |
| 28 | mavexa | `find text 'Welcome'` | 1.16 | 1058 | 0/0/0 | 659 | 19.8 | query_match 65.2% (runner::run_core); ts_parse 19.3% (find::run_query); query_compile 5.9% (runner::cached_query) |
| 29 | mavexa | `find label Email` | 0.67 | 1132 | 0/0/0 | 659 | 48.6 | ts_parse 45.9% (find::run_query); query_match 36.2% (runner::run_core); alloc 5.8% (find::run_query) |
| 30 | mavexa | `find component Landing` | 0.74 | 1308 | 0/0/0 | 659 | 29.5 | query_match 56.3% (runner::run_core); ts_parse 27.7% (find::run_query); alloc 4.6% (find::run_query) |
| 31 | mavexa | `path 'function#login --> function#writeError' backend` | 5.39 **>2s** | 4866 | 740/0/740 **P>F** | 379 | 47.1 | ts_parse 43.6% (runner::run_rows); query_compile 21.4% (runner::cached_query); alloc 11.4% (resolve::graph::assemble) |
| 32 | mavexa | `path 'field#signupRequest.Email ==>' backend` | 78.33 **>2s** | 48692 | 740/0/740 **P>F** | 379 | 5.1 | ts_other 70.5% (resolve::go::walk_children); alloc 16.3% (resolve::go::walk_children); resolve 7.3% (resolve::go::walk_scope) |
| 33 | mavexa | `imports frontend/src/app.tsx frontend/src` | 0.36 | 951 | 0/0/0 | 200 | 64.0 | ts_parse 60.7% (resolve::imports::build); ts_other 22.6% (resolve::imports::walk); alloc 10.5% (resolve::imports::build) |
| 34 | mavexa | `importers frontend/src/components/Landing.tsx frontend/src` | 0.36 | 963 | 0/0/0 | 200 | 64.2 | ts_parse 60.9% (resolve::imports::build); ts_other 21.6% (resolve::imports::walk); alloc 11.1% (resolve::imports::build) |
| 35 | mavexa | `importgraph frontend/src --format edges` | 0.36 | 974 | 0/0/0 | 200 | 65.2 | ts_parse 60.9% (resolve::imports::build); ts_other 20.9% (resolve::imports::walk); alloc 11.8% (resolve::imports::build) |
| 36 | mavexa | `example 'writeError(w, 400, msg)' -l go` | 0.01 | 29 | 0/0/0 | 659 | 3.4 | query_compile 89.7% (example::derive); other 6.9% (main); ts_parse 3.4% (example::derive) |
| 37 | mavexa | `types go` | 0.00 | 2 | 0/0/0 | 659 | 0.0 | output 50.0% (types::run); other 50.0% (main) |
| 38 | mavexa | `config path` | 0.00 | 2 | 0/0/0 | 659 | 0.0 | other 50.0% (config::path_report::_$u7b$$u7b$closure$u7d$$u7d$); alloc 50.0% (main) |
| 39 | mavexa | `config dump go` | 0.00 | 8 | 0/0/0 | 659 | 0.0 | other 100.0% (main) |
| 40 | emc-gigs | `-q 'id#authenticate' app` | 0.03 | 225 | 79/0/79 | 79 | 44.4 | ts_parse 40.9% (parsed::parse_text); query_compile 26.7% (compiler::check_compiles); fs_read 11.1% (files::resolve_jobs) |
| 41 | emc-gigs | `def authenticate app` | 0.02 | 141 | 79/0/79 | 79 | 66.0 | ts_parse 59.6% (parsed::parse_text); fs_read 24.1% (files::resolve_jobs); alloc 9.2% (parsed::parse_text) |
| 42 | emc-gigs | `explain authenticate app` | 0.29 | 1004 | 79/79/79 | 79 | 10.4 | query_compile 73.4% (runner::cached_query); ts_parse 9.6% (parsed::parse_text); alloc 5.8% (runner::cached_query) |
| 43 | emc-gigs | `callers authenticate app` | 0.26 | 958 | 79/0/79 | 79 | 8.0 | query_compile 77.6% (runner::cached_query); ts_parse 7.6% (resolve::Resolver::resolve_calls_tree); alloc 6.5% (runner::cached_query) |
| 44 | emc-gigs | `find route /gigs` | 0.12 | 959 | 4/4/4 | 146 | 22.4 | query_compile 35.8% (compiler::check_compiles); ts_parse 19.7% (find::run_query); regex 14.6% (config::queries::apply_rewrite) |
| 45 | emc-gigs | `tree app` | 0.02 | 137 | 79/0/79 | 79 | 62.8 | ts_parse 58.4% (parsed::parse_text); fs_read 21.2% (parsed::try_source); other 8.0% (tree::report) |
| 46 | agent-af8b64c91b61820cf | `-q 'id#run' src` | 0.20 | 1021 | 58/0/58 | 58 | 60.4 | ts_parse 56.0% (parsed::parse_text); query_match 27.0% (runner::run_core); alloc 7.6% (parsed::parse_text) |
| 47 | agent-af8b64c91b61820cf | `tree src` | 0.15 | 887 | 58/0/58 | 58 | 82.1 | ts_parse 74.4% (parsed::parse_text); alloc 12.9% (parsed::parse_text); ts_other 6.2% (parsed::Cache::keep) |
| 48 | agent-af8b64c91b61820cf | `def run_rows src` | 0.14 | 965 | 1/0/1 | 58 | 85.6 | ts_parse 77.2% (resolve::rust::parse); alloc 11.7% (resolve::rust::parse); ts_other 7.2% (resolve::def_rows) |
| 49 | agent-af8b64c91b61820cf | `explain run_rows src` | 0.83 | 1482 | 64/0/64 **P>F** | 58 | 71.1 | ts_parse 65.4% (resolve::rust::parse); ts_other 13.6% (resolve::rust::walk_impls); alloc 11.3% (resolve::rust::parse) |
| 50 | agent-af8b64c91b61820cf | `callers run_rows src` | 0.52 | 950 | 10/0/10 | 58 | 70.3 | ts_parse 66.2% (resolve::rust::parse); ts_other 18.8% (resolve::rust::walk_impls); alloc 11.5% (resolve::rust::parse) |
| 51 | agent-af8b64c91b61820cf | `callgraph src --format edges` | 0.50 | 1355 | 0/0/0 | 58 | 68.5 | ts_parse 63.7% (resolve::rust::parse); ts_other 19.0% (resolve::rust::walk_impls); alloc 13.0% (resolve::rust::parse) |
| 52 | agent-af8b64c91b61820cf | `importgraph src --format edges` | 0.17 | 904 | 0/0/0 | 58 | 66.2 | ts_parse 61.7% (resolve::imports::build); ts_other 21.8% (resolve::imports::walk); alloc 12.4% (resolve::imports::build) |

## Over 2s on Mavexa (decision 15)
#5 sibling `~` query 5.87s; #15 def with 3 names 2.11s; #17 explain 4.46s; #18 callers 2.83s; #19 callees 2.95s;
#20 callgraph 2.88s; #24 scopeof --siblings --callers 3.86s; #31 path call route 5.39s; #32 path data-flow 78.3s.

## Next optimisations, ranked by expected wall-time gain
1. Skip test files before reading/parsing (tests are hidden by default). Backend parses 740 files, 361 are tests (49%);
   frontend 103 of 303 (34%). Helps every backend-wide call (#1-4, 6, 9, 10, 16-21, 31, 32). ts_parse is 61-88% of those, so
   removes about 30-45% of wall (~0.3-0.45s of a 1s -q; ~1.3-2s of explain).
2. Skip files lacking the exact name before parsing (byte search of the already-read text). Helps -q id#X, refs, def, files,
   explain, callers, callees, scopeof --callers, path. Only a minority of the 379 non-test files mention writeError, so
   ts_parse+alloc (70-75% of -q, 75-80% of graph calls) shrinks to the matching files plus those they need; est 55-65% of
   wall on #1-4, 6, 9, 16, 21 and 40-60% on #17-19, 24 (callers need the name's files plus transitive callers, so name-scope the graph).
3. Resolver parses bypass the parse cache and repeat per name/per phase: `def A B C` costs 2.11s vs 0.71s for one name (3x, linear
   in names), explain on Rust parses 64 for 58 files, scopeof --callers parses 383 files for a one-file scope.
   Route resolve::go::parse / resolve::rust::parse through parsed.rs and build the DefIndex once per invocation.
   Helps #14, 15, 17-20, 24, 31, 48-51. Est 66% off #15, ~10% off explain, plus the shared-parse gain for callers/callees.
4. Data-flow `path ==>`: 78.3s, 70% ts_other + 16% alloc inside resolve::go::walk_children (re-walking subtrees, 5% parse only).
   Per-function summary memo or visited-set. Only #32, but it is 25x the next slowest call; est 80-95% of that call.
   Also path `-->` (#31, 5.4s): 21% is query compile in runner::cached_query, so compile once.
5. Query match cost for sibling `~`: #5 spends 86% in query_match (runner::run_core), 5.9s vs 1.0s for equivalent name query;
   run as two-phase (find both name sets, then group by parent) instead of one tree-sitter pattern; est 80% off #5.
   Same bucket dominates `find text` (65%), `find component` (56%), `find error` (39%): prefilter on the key text before
   running the query on a file; est 40-60% off #26-30 (1.2-1.7s).
Minor: Ruby/Mavexa small calls spend 73-78% in query compile (runner::cached_query, #42, 43: 0.26-0.29s for 79 files);
compile lazily/once per language (est 70% off those). `-q` startup `check_compiles` is 78% of a 10ms single-file call.
