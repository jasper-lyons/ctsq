# find: match a pasted URL or message against keys, configured per query file

## Goal

`ctsq find route https://app.example.com/api/org/mavexa/team/hr?x=1` prints the route `/api/org/:org_id/team/:team_id` and its handler, with `org_id=mavexa team_id=hr` on the row. How a key becomes a pattern is declared in each query file, so ctsq itself holds no router or printf knowledge. Then Mavexa gets whatever repo queries its real URLs still need.

## Decisions

### 01. `key_pattern`: ordered regex rules that turn a captured key into a matcher

A query file may declare `key_pattern = [[RULE_REGEX, FRAGMENT], ...]`. ctsq scans the captured key left to right; at each position it tries the rules in order, the first that matches at that position emits its FRAGMENT (with `$1`, `$name`, `$0` expanded from that rule's match) and the scan resumes after it. A character no rule matches is emitted regex-escaped. Every occurrence is replaced, not only the first. The result is anchored `^...$` unless Decision 05 says otherwise. Why: config, not code, carries framework knowledge (Jasper, 2026-10-18). It beat extending the selector language (selectors describe code shape, this compares strings after the search) and embedding a scripting language (dependency, second language). It differs from `rewrite`, which replaces the first match only and does not escape.

### 02. `key_input`: rules that clean the text the user pasted

`key_input = [[REGEX, REPLACEMENT], ...]`, applied in order to the user's key, every match replaced (plain `Regex::replace_all`, no escaping). Route packs strip `^[a-z]+://[^/]+`, `[?#].*$` and a trailing `/` (but not a lone `/`). Why: what users paste comes from a browser address bar or a log.

### 03. A row matches by text or by pattern

For a query with `key_pattern`, a row matches when the user's key matches it as today (case-insensitive substring, or `/regex/`), or when the cleaned key fully matches the row's pattern. A `/regex/` key keeps today's behaviour only, it is not tested against patterns. A query with no `key_pattern` behaves exactly as today. Why: `find route workspaces` must keep listing every workspace route.

### 04. Order and display

Pattern matches print before text-only matches. Among pattern matches, more literal characters in the key first (characters emitted escaped in Decision 01, not counting rule matches), then the existing order. Rows carry no marker. A pattern match whose regex has named groups appends `name=value` pairs, space-separated, after the existing columns, in group order. Why: routers pick the most specific route, so `/events/new` before `/events/:id`; the values tell the user which parameter received what.

### 05. `find error` moves onto the same mechanism, output unchanged

The printf and `${...}` placeholder matching now in Rust becomes `key_pattern` rules in `error-go.toml`, `error-js.toml` and `error-ruby.toml` (Ruby `#{...}`), and the Rust special case is deleted. Every `find error` output on Mavexa, emc-gigs and the fixtures is byte-identical to the build tip before this work. If today's error matching is not whole-key (for example a log line that contains the message), add a query-file field for the anchoring (such as `key_match = "within"`) rather than keeping Rust special-casing. Why: one mechanism, and the error packs become editable like the route packs.

### 06. Route packs declare their syntax

Each built-in route pack gets `key_pattern` and `key_input`:
- solid-routes, react-router: `/:name?` optional segment, `:name` one segment, `*name` and `*` the rest
- go-http: a leading `METHOD ` optional in the pasted key (a pasted method must equal the key's), `{$}` end, `{name...}` the rest, `{name}` one segment
- go-chi: `{name:regex}` keeps its regex, `{name}` one segment, `*` the rest
- next-app: `[[...name]]` optional rest, `[...name]` rest, `[name]` one segment (route groups are already stripped)
- rails-routes: `(.:format)` and other `(...)` optional, `:name` one segment, `*name` the rest

Parameters use named groups (`(?P<name>...)`) so Decision 04 can show values. Repeated names in one key get a suffix to stay valid regex.

### 07. Skipping files by key

Today a file whose bytes lack the key is skipped before parsing. For a query with `key_pattern` that skip is off when the key is not a `/regex/`, since a pasted URL never appears in the code. Why: Decision 02 of plans/perf/PLAN.md, a skip must be provably safe.

### 08. Nested routes are checked, not built

Check whether the Solid and React Router packs join a parent route's path onto a child's. If they do not, record it under `## Open questions` with a repro; do not build it here. Why: it needs the selector language, a separate step.

## Open questions

None yet.

## Conventions

- Read `CLAUDE.md`. ctsq for code, mdq for markdown. Never `make install`.
- Gates: `~/.cargo/bin/cargo build --release` with no warnings, `~/.cargo/bin/cargo test`, `bash plans/agent-search/docs/check-examples.sh`, `bash plans/parse-cache/same-output.sh` with 0 differences.
- Reference binary for Decision 05: build tip b19753a, built at `/Users/jasperlyons/.claude/jobs/30494ad5/tmp/fu-ref/target/release/ctsq`.
- Mavexa and emc-gigs are read-only. `find` takes its root from the current directory, so run it from a script that `cd`s there.
- Unit tests beside the code, CLI tests in `tests/cli.rs`, fixtures under `fixtures/`.

## Work packages

| Order | Package | Tier |
|---|---|---|
| 1 | key-pattern | Opus |
| 2 | review | Opus, read-only |
| 3 | mavexa (orchestrator) | after install |

## Package: key-pattern

Files: `src/config/queries.rs` (schema, docs block at the top), `src/find.rs`, `config/queries/*.toml`, `fixtures/` (new route fixtures as needed), `tests/cli.rs`, `src/main.rs` help text for `find`, `README.md` find section, `CLAUDE.md` named-queries paragraph, `plans/agent-search/docs/SKILL.md` and `plans/agent-search/docs/claude-md-ctsq.md` find lines (one short mention of pasting a URL), this plan's `## Open questions`.

Acceptance:
- Decisions 01 to 08 hold. Unit tests for the scanner: escaping (`.`, `(`, `{` with no rule), every-occurrence replacement, named groups, specificity count. CLI tests for one fixture per route pack with a pasted full URL, the ordering case (`/events/new` before `/events/:id`), the params suffix, and a text-only match still found.
- Decision 05 identity: a script under `plans/find-url/` runs a list of `find error` calls (at least 20, Mavexa backend and frontend, emc-gigs, fixtures; real messages taken from the code, some with placeholders filled with values as a log would show them) against the reference and the new binary and reports differences. 0 differences. Also every `find route`, `find text`, `find label`, `find component` call in the same script with keys that exist today: 0 differences.
- Timing: `find route /api/org/x/team/y` on Mavexa root, wall time before and after.

## Package: review

Read-only Opus reviewer tries to break Decisions 01 to 07: regex injection through keys (a key with `[`, `\`, `$`), rules that match empty strings (must not loop), repeated param names, a pasted URL with encoded characters, a lone `/`, a key matching both by text and by pattern (printed once), error identity cases the script missed.

## Package: mavexa

After merge and `make install`: run `ctsq find route` from Mavexa's root with real URLs for each frontend route in `frontend/src/routesConfig.ts` and a sample of backend routes in `backend/internal/api/handler.go`. Anything the built-in packs miss gets a repo query in Mavexa's `.ctsq/queries/`, on a branch in a Mavexa worktree, never on Mavexa's working copy.
