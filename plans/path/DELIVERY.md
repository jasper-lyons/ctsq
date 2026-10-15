# ctsq path: delivery note

Branch `path-build`, 2026-10-15. Not merged to main. Merging is Jasper's call.

## What's been built

- **`ctsq path`**, a new command that answers two questions from the command line, printing an indented tree where every row starts `path:line`:
  - "How does A reach B?" over the call graph, with `->` (calls directly), `-->` (calls eventually), `<-` and `<--` (called by).
  - "Where does this data go?" and "where did it come from?" for Go, with `=>`/`==>` (flows to) and `<=`/`<==` (comes from). A query ending in `==>` with nothing after it lists every place the data leaves the module.
- **Data flow for Go** follows values through assignments, function calls and returns, struct fields (including embedded structs), pointer writes, closures, function variables, generics, interfaces (only implementations the program constructs, narrowed by type assertions) and into Go templates (`ParseFS` with `//go:embed`, layout `block` into page `define`, `range`, inline templates in Go strings). Each hop carries tags saying how sure it is: `exact`, `derived`, `moved`, `field F`, `conditional@L`, `widened`, `unmodelled`, `via F`, `rendered`, `sink`, `outside`, `stopped: reason`.
- **Attributes in every query language**: `[from=log]`, `[in=User]` with `^=`, `*=`, `$=` and `/regex/`, plus the dot shorthand `function#log.Printf` and `field#User.Email`. These work in `ctsq -q` too.
- **A `field` kind** for every language.
- **Config and models**: `~/.config/ctsq` (or `$XDG_CONFIG_HOME`), a repo's `.ctsq/models/`, and about 130 built-in Go standard library models describing what library calls do with data. `ctsq config path` and `ctsq config dump <lang>` show what is loaded.
- **Go template language** (`.tmpl`, `.gotmpl`) for `ctsq -q`, with a `template_action` kind for lines that print a value.

## How it affects a user

- New: everything above. `ctsq path --help` explains the arrows, tags and exit codes.
- Changed: a top-level descendant query such as `-q 'function variable'` now finds matches at any depth and prints one row per inner match at its own line. It used to look one level down and print the outer line.
- Changed: `-q` with an arrow or `:not()` exits 2 with a hint to use `ctsq path`. Characters such as `-` and `<` in a bare query are now errors (quote names that contain them).
- Unchanged: `explain`, `callers`, `callees`, `callgraph`, `tree`, `files`, `def`, `example`, `importgraph` outputs are identical to main on the fixtures (checked by diffing 46 commands against a main build).
- Not installed: `make install` was never run. The installed `ctsq` in `~/.local/bin` is still the main version.

## How to test it

From `~/workspace/projects/jasperlyons/ctsq-worktrees/path-build`:

1. `cargo test` gives 18 suites, 502 passed, 0 failed. `cargo build --release` gives 0 warnings.
2. `target/release/ctsq path --help` lists the eight arrows, the tags and two examples.
3. `target/release/ctsq path 'function#main --> function#c' fixtures/path/` prints four call routes with `calls X at :L` on each row.
4. `target/release/ctsq path 'function#handle variable#body ==> function#log.Printf' fixtures/flow/flow-calls/` follows `body` through `first`, `second`, `third` and `normalise` (another file) back to `log.Printf`, tagged `sink`.
5. `target/release/ctsq path 'field#Item.Phone ==> template_action' fixtures/flow/templates` reaches `page.html.tmpl:4 <p>{{.Phone}}</p>` through the layout's `block`, tagged `rendered`, plus the inline email template line.
6. `target/release/ctsq path 'function#render param#name <== ' fixtures/flow/flow-up` lists the three literal callers and the `"admin-" + page` caller.
7. `target/release/ctsq -q '&function#log.Printf' fixtures/attrs/` returns only the `log.Printf` line, not `fmt.Printf`.
8. On the booking app (`cd ~/workspace/projects/badhabit/rehearsal-booking`), run `<path-build>/target/release/ctsq path 'field#Booking.CustomerPhone ==>' .` (under a second). Expect 18 routes to outside, including `data_access.go:63 d.db.Exec` (the DB write), `handler.go:65 Encode` (JSON responses), the reflection helpers, and both confirmation email template lines (`customer-confirmation-email.go:48` and `:97`).

## Known limits

- Values read back from the database are a separate query (decision 17). So the `%v` log line in `admin-bookings-update-view.go:26` and the admin page templates are not reached from a field start. `field#Booking.CustomerPhone ==> function#log.Printf` correctly says 0 routes.
- On the booking app the SES route stops at building `ses.SendEmailInput` (line 218) rather than the `sesClient.SendEmail` call, because `aws.String` has no model.
- Data flow is Go only. Call routes work for any language with a resolver (Go, Rust, others by name).
- Everything else is listed in `plans/path/FOLLOWUPS.md` (21 items).
