# Scoped graph

`callers`, `callees`, `explain` and `scopeof --callers` build the call graph from the files that can change their answer, not from the whole folder. The contract is that every printed byte equals the whole-folder graph's output. `CTSQ_WHOLE_GRAPH=1` forces the old path.

## Two file sets

- **Resolved files (R).** The files whose call sites are resolved into edges. The walk reads `rev[K]` (callers) or `fwd[K]` (callees) for each key it reaches.
  - `fwd[Def D]` holds only calls written inside D, so it needs D's own file.
  - `rev[Def D]` and `rev[External N]` hold calls that resolved to D, or stayed unresolved with display name N. Every resolution path ends in a name written in the calling file: the callee name, the inner callee of a Go or Rust function-value binding (`f := X`), a Rust `use` path or alias, or the method name in Go interface and Rust trait dispatch. So a call to a definition named N sits in a file whose bytes hold N, and those files are R for that key.
- **Wanted calls.** In a resolved file only the calls a walk can read are resolved: every call inside a function whose callees are walked, and elsewhere the calls whose callee is a walked name, or a local holding a function value (Go, Rust), or an imported name (Rust), since only those can reach a definition of another name. Each (caller, callee, target) edge keeps all its sites or none, so the first-site rule picks the same line.
- **Holding a name.** A file holds N when N occurs bounded by bytes that cannot continue an identifier (ASCII letters, digits, `_`). A lexer takes the longest identifier run, so every definition and call of N is such an occurrence. Names made only of those bytes are looked up in a per-run token index, others are searched in the text.
- **Index files (P).** The files whose definitions are in the index the resolvers read. R is always inside P, so `def_at` (attributing a call to its enclosing function) and the fallback resolver's per-file definitions are whole.

## Depth

R grows level by level. Level 0 is the roots: every definition named after the asked name, plus `External(name)`. After each level the graph over R gives the next level's keys: the callers or callees just reached. A key needs its edges only while its depth is under `--max-depth`. Breadth-first depth is at most any path's depth, so expanding by breadth-first depth covers every row the walk prints, cycles included. `explain` runs both directions at `--depth`, upward from its sites plus `External(bare)`, downward from its sites.

## Which index is enough: names asked

The resolvers read the index only through names. Each read is recorded on the index:
- a `by_name` lookup of a callee, a binding's callee, an imported path's last segment, or an interface's method names when checking a method set (Go `interface_impls`)
- a scan of interface method definitions of interface T (Go): those are declared in files holding T
- a scan of `impl Trait for Type` blocks of trait T (Rust): those blocks hold T
- whether type T is constructed (Go, used by interface dispatch): a composite literal of T, `new(T)`, a function returning T, and a function named `NewT` all hold T's bytes
- the fallback resolver's project-wide-unique test, which is a `by_name` lookup

Every answer to a read of name N comes from files holding N. The build loops: resolve R against the index over P, collect the names asked that are not yet covered, add every file holding one of them to P, mark them covered, and resolve again the files that asked an uncovered name. It stops when a pass asks nothing new. At that point every read had the whole folder's answer. The index is rebuilt from P in job order each pass, so `by_name` lists and definition order are subsequences of the whole index in the same order, and ties in ranking break the same way. Edges keep job order then document order, so adjacency lists sort the same.

## The definition index stays scoped

A whole-folder index needs a parse of every file, which is the 60% the profile shows. The rule above makes the scoped index give the same answer to every read the resolution makes, so it is never built whole. `def A B C` builds one index from the files holding any of the names, once, and builds the whole index only when a name misses, since the miss report suggests from every name. `explain` reads its outline sites and its references only in files holding the bare name, since an outline entry or an `id#` match of a name holds its bytes, and hands those same parses to the graph builder.

Every loaded file is read for its definitions and its calls (a model with no tree) in the one parse, and the tree is dropped, so memory follows the files loaded, not the trees.

## callgraph

Whole-folder by nature. One parse per file, read into definitions and a call model and dropped, so it no longer holds every tree.

## Fallback to the whole folder

The scoped build is used for every language, since all three resolvers (Go, Rust, fallback) read the index only through the recorded reads. A hub name or `--max-depth 6` can grow P to the whole folder, which costs what the whole-folder build costs plus the passes.
