# agent-search delivery

The plan is in `PLAN.md`. This file records the three goal measures. The baseline comes from the plan's Evidence section (Claude Code session logs, 2026-08-17 to 2026-10-16). The "After" column is blank until Jasper installs the merged branch, lets sessions run on it, and re-runs `measure/` (steps in the plan under Re-measuring). Jasper fills it in. The build does not re-measure.

| Measure | Baseline | After |
|---|---|---|
| ctsq calls followed by a Read of a file the call had just named | 1,437 of 15,475 | |
| `-q` queries that error or match nothing | 10% error, 11% empty | |
| Share of code characters agents take in through whole-file reads | 44% | |

Other baseline figures worth comparing: `explain` is 6% of ctsq calls, `callers` and `files` about 1% each, and 20% of `files` calls error.

## Performance

`bash plans/agent-search/measure/bench.sh` times each call with the base binary (commit 5f867dc, built out of tree) and the new one, median of 5 runs after one warm-up, in seconds. `+t` is the new binary with `--tests`, since it hides tests by default and the base binary has no notion of them. Ratio is new over base. Roots: this repo (`src`), Mavexa and emc-gigs (`.`). Names used are written in `measure/bench.py`. Run on 2026-10-16 on Jasper's Mac.

### Before the fixes (build branch at 7502edd)

| call | ctsq base / new / +t | ratio | Mavexa base / new | ratio | emc-gigs base / new | ratio |
|---|---|---|---|---|---|---|
| tree | 0.184 / 0.236 / 0.182 | 1.29 | 1.344 / 1.303 | 0.97 | 0.124 / 0.129 | 1.04 |
| -q id#common | 0.340 / 0.402 / 0.235 | 1.18 | 2.739 / 1.723 | 0.63 | 0.936 / 0.127 | 0.14 |
| -q *function --body | 0.341 / 0.240 / 0.235 | 0.70 | 3.558 / 1.681 | 0.47 | 2.383 / 0.129 | 0.05 |
| def | 0.183 / 0.183 / 0.179 | 1.00 | 1.303 / 1.255 | 0.96 | 0.128 / 0.129 | 1.01 |
| def nested | 0.342 / 0.395 / 0.345 | 1.16 | 2.535 / 2.532 | 1.00 | 0.181 / 0.182 | 1.01 |
| refs | 0.341 / 0.288 / 0.233 | 0.84 | 2.697 / 1.680 | 0.62 | 0.982 / 0.129 | 0.13 |
| explain | 1.086 / 1.041 / 0.984 | 0.96 | 33.061 / 28.512 | 0.86 | 4.690 / 3.031 | 0.65 |
| callers | 0.611 / 0.666 / 0.614 | 1.09 | 29.045 / 25.372 | 0.87 | 3.772 / 2.872 | 0.76 |
| files | 0.392 / 0.288 / 0.292 | 0.74 | 3.710 / 2.530 | 0.68 | 1.148 / 0.182 | 0.16 |

New-only calls, before (ctsq / Mavexa / emc-gigs): find 0.040 / 0.180 / 0.019, find route 0.234 / 2.157 / 0.291, find error 0.128 / 1.694 / 0.129, find text 0.128 / 1.473 / 0.128, scopeof 0.010 / 0.010 / 0.005, def --scope 0.181 / 1.309 / 0.128, miss (-q id#zzqqx) 0.671 / 3.580 / 0.236.

### After the fixes

| call | ctsq base / new / +t | ratio | Mavexa base / new | ratio | emc-gigs base / new | ratio |
|---|---|---|---|---|---|---|
| tree | 0.183 / 0.180 / 0.180 | 0.99 | 1.725 / 1.310 | 0.76 | 0.125 / 0.128 | 1.02 |
| -q id#common | 0.339 / 0.397 / 0.235 | 1.17 | 2.761 / 1.729 | 0.63 | 0.937 / 0.130 | 0.14 |
| -q *function --body | 0.343 / 0.240 / 0.234 | 0.70 | 3.618 / 1.685 | 0.47 | 2.368 / 0.128 | 0.05 |
| def | 0.181 / 0.180 / 0.182 | 0.99 | 1.309 / 1.303 | 1.00 | 0.129 / 0.128 | 1.00 |
| def nested | 0.344 / 0.345 / 0.346 | 1.00 | 2.525 / 2.583 | 1.02 | 0.181 / 0.177 | 0.98 |
| refs | 0.343 / 0.289 / 0.237 | 0.84 | 2.691 / 1.669 | 0.62 | 0.931 / 0.129 | 0.14 |
| explain | 1.094 / 1.029 / 0.987 | 0.94 | 32.543 / 7.622 | 0.23 | 4.567 / 0.617 | 0.14 |
| callers | 0.611 / 0.665 / 0.613 | 1.09 | 28.726 / 4.780 | 0.17 | 3.628 / 0.427 | 0.12 |
| files | 0.397 / 0.291 / 0.290 | 0.73 | 3.657 / 2.491 | 0.68 | 1.103 / 0.183 | 0.17 |

New-only calls, after (ctsq / Mavexa / emc-gigs): find 0.039 / 0.184 / 0.023, find route 0.238 / 1.830 / 0.290, find error 0.128 / 1.678 / 0.139, find text 0.129 / 1.465 / 0.135, scopeof 0.010 / 0.010 / 0.005, def --scope 0.180 / 1.256 / 0.086, miss 0.675 / 3.563 / 0.249. The table run predates the last fix (the miss report), measured alone afterwards: `-q id#zzqqx .` on Mavexa 3.55s before, 1.70s after.

### What changed

- Test scopes: a scope selector with a name filter (Rust `module#tests`) ran over the whole tree of every file. It now runs only around each occurrence of the name, and a file without the name is not searched or parsed. `tree src/` 0.236s to 0.180s.
- Name-only resolver (JS, TS, Python, Ruby, C, C++): it compiled a tree-sitter query and parsed the file again for every function. It now parses once and runs one query per file, and the query is built once per language. `explain` on Mavexa 28.5s to 7.6s, `callers` 25.4s to 4.8s. The base binary has the same cost, which is why these calls now beat it.
- Miss report: for an exact name, files without the name's bytes are skipped.

### Still over target

- `-q id#common src/` on this repo is 1.17x base. With `--tests` it is 0.69x, so the cost is test hiding: each Rust file with a match is parsed a second time to find its `mod tests`, because `-q` does not hand its tree to the hider. Next step: pass the tree from the query run.
- `callers` on this repo is 1.09x, inside the 1.2x line but not the 1.1x goal. With `--tests` it equals base, so the cost is again test hiding on Rust files.
- Mavexa calls over 2s: `explain` 7.6s and `callers` 4.8s (the base takes 30s), `files` 2.5s, `def nested` 2.6s (both the same as or faster than base). What remains is the call-graph build, which parses each Go file several times (definitions, implementations, call sites, outline). Sharing one parse across those steps is the next step and a larger change than this package. `find route` is 1.8s, under decision 15's 2s.
