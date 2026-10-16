# agent-search: two command-line shapes the plan leaves open

I am building everything else while I wait. Only the two pieces below are held back.

## 1. Telling names apart from paths when `def`, `refs`, `callers` and `callees` take several names (decision 07)

Today these four commands require exactly one name and then one path, so `ctsq def A B Missing` would treat `Missing` as the path and fail with "Failed to access". The plan's examples need three things at once:

- `ctsq def LoadUnits LoadUnits3 GameKeyCheck src/`, where `src/` is the path
- `ctsq def A B Missing`, which exits 0 and lists `Missing` as not found, so the path must become optional
- `no such file: X (did you mean Y)` for a path that does not exist

Options:

- **A (my recommendation).** A trailing argument is a path when it exists on disk, or when it contains `/` or a file extension. Every other argument is a name. With no path, search `.`, as `explain` and `files` do now. A trailing `src/old/` that does not exist prints `no such file`. Edge case: a name that happens to match a file or directory in the current directory (`ctsq def src`) is read as a path.
- **B.** Names are comma-separated in one argument (`ctsq def A,B,Missing src/`), and the path stays positional and becomes optional. No guessing, but it does not match the plan's examples.
- **C.** The path moves to a flag (`ctsq def A B Missing --in src/`). No guessing, but it breaks every existing call and doc that passes a path positionally.

## 2. `--max N` as a global flag clashes with `ctsq path --max`

`ctsq path` already has `--max N`, meaning the hop limit (default 12). The plan makes `--max N` a global flag meaning "stop after N matches, the footer counts the rest". The command-line parser cannot hold both under one name.

Options:

- **A (my recommendation).** Add `--max N` (match limit) to every command that returns hits, but not as a global flag. `path --max` keeps its hop-limit meaning. The same flag name means two different things on two commands.
- **B.** Rename the hop limit to `ctsq path --hops N` and make `--max` global as the plan says. Breaks existing `path --max` calls and docs.
- **C.** Name the match limit differently, for example `--limit N`, everywhere. Differs from the plan and the footer wording (`over --max 2`).

## 3. Footer: one line, or one line per reason (decision 03)

The plan's prose under Output format says "one line per reason", but its sample prints every reason on one line: `hidden: 6 in tests (--tests), 31 over --max 2 in 4 files (--max 40)`.

I built the sample's shape: one `hidden:` line with the reasons separated by commas, printed after a count line such as `2 matches`. The wording lives in one shared function, so switching to one line per reason is a one-function change. Say if you want the other shape.

## Note

These questions could not be posted with `brief ask agent-search`. This repo has no `.queue` folder holding an `agent-search.md` brief, and creating one would have meant running `brief init` here. They are in the build report instead.
