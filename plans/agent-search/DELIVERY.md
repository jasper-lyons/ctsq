# agent-search delivery

The plan is in `PLAN.md`. This file records the three goal measures. The baseline comes from the plan's Evidence section (Claude Code session logs, 2026-08-17 to 2026-10-16). The "After" column is blank until Jasper installs the merged branch, lets sessions run on it, and re-runs `measure/` (steps in the plan under Re-measuring). Jasper fills it in. The build does not re-measure.

| Measure | Baseline | After |
|---|---|---|
| ctsq calls followed by a Read of a file the call had just named | 1,437 of 15,475 | |
| `-q` queries that error or match nothing | 10% error, 11% empty | |
| Share of code characters agents take in through whole-file reads | 44% | |

Other baseline figures worth comparing: `explain` is 6% of ctsq calls, `callers` and `files` about 1% each, and 20% of `files` calls error.
