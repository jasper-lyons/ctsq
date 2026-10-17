//! Call-site → definition resolution.
//!
//! Each language gets a `Resolver` that knows how to (1) index definitions and
//! (2) resolve call sites against the project-wide index, following imports,
//! aliases, qualified paths, local value bindings, and (heuristically) method
//! receivers. Languages without a dedicated resolver fall back to
//! `fallback::NameResolver`, which preserves the original name-matching
//! behavior.
//!
//! The resolution pipeline is an ordered sequence of candidate-generation
//! steps (see each resolver's `resolve_calls`). External per-project rules are
//! expected to plug in here later by reordering/augmenting those steps and the
//! ranking in `rank_candidates` — keep new logic in discrete steps.

pub mod fallback;
pub mod go;
pub mod graph;
pub mod imports;
pub mod rust;
pub mod tsconfig;

pub mod scoped;

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};

/// Identity of a definition: where it lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId {
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefKind {
    Function,
    /// Method on a type; `receiver` is the impl/class type name (e.g. "Foo").
    /// `via_trait` is the trait being implemented when the method lives in an
    /// `impl Trait for Type` block — the implements-relation CHA needs.
    Method { receiver: String, via_trait: Option<String> },
    /// Signature inside an interface/trait declaration — a dispatch point,
    /// not a body. Calls through it expand to implementing methods.
    InterfaceMethod { interface: String },
}

#[derive(Debug, Clone)]
pub struct Def {
    /// Bare name, e.g. "new".
    pub name: String,
    /// Module path + impl type + name, e.g. ["util", "Foo", "new"].
    pub qualified: Vec<String>,
    pub kind: DefKind,
    /// Bare name of the (first) return type, when it's a nameable type —
    /// feeds `x := f(); x.m()` inference.
    pub ret: Option<String>,
    pub id: DefId,
}

#[derive(Debug, Default)]
pub struct DefIndex {
    pub defs: Vec<Def>,
    /// bare name -> indices into `defs`
    pub by_name: HashMap<String, Vec<usize>>,
    /// (type, trait) implements-relations from `impl Trait for Type` blocks.
    /// Separate from method defs so empty impls (all defaults) still count.
    pub impls: Vec<(String, String)>,
    /// Every name a resolver read the index by, while recording (see
    /// `scoped`). None when not recording.
    asked: RefCell<Option<HashSet<String>>>,
    /// The names asked through `named`, and through `ask`, while recording.
    asked_named: RefCell<Option<HashSet<String>>>,
    asked_other: RefCell<Option<HashSet<String>>>,
    /// file -> indices into `defs`, in index order, built on first use.
    by_file: OnceCell<HashMap<String, Vec<usize>>>,
    /// The Go resolver's constructed types, built on first use.
    pub(crate) go_constructed: OnceCell<HashSet<String>>,
}

impl DefIndex {
    pub fn push(&mut self, def: Def) {
        self.by_name.entry(def.name.clone()).or_default().push(self.defs.len());
        self.defs.push(def);
    }

    /// The definitions named `name`, in index order. Resolvers read names
    /// through here (or note them with `ask`), so a scoped build learns
    /// which names its answers depend on.
    pub fn named(&self, name: &str) -> &[usize] {
        self.note(name);
        if let Some(s) = self.asked_named.borrow_mut().as_mut() {
            if !s.contains(name) {
                s.insert(name.to_string());
            }
        }
        self.by_name.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Note that an answer depends on `name`: every definition, impl block
    /// or construction it reads holds that name in its file.
    pub fn ask(&self, name: &str) {
        self.note(name);
        if let Some(s) = self.asked_other.borrow_mut().as_mut() {
            if !s.contains(name) {
                s.insert(name.to_string());
            }
        }
    }

    fn note(&self, name: &str) {
        if let Some(s) = self.asked.borrow_mut().as_mut() {
            if !s.contains(name) {
                s.insert(name.to_string());
            }
        }
    }

    /// Start recording the names asked, dropping any recorded before.
    pub fn record(&self) {
        *self.asked.borrow_mut() = Some(HashSet::new());
        *self.asked_named.borrow_mut() = Some(HashSet::new());
        *self.asked_other.borrow_mut() = Some(HashSet::new());
    }

    /// `take_asked`, split into the names only `named` read (the
    /// definitions named so) and every other name.
    pub fn take_asked_split(&self) -> (HashSet<String>, HashSet<String>) {
        self.take_asked();
        let other = self.asked_other.borrow_mut().take().unwrap_or_default();
        let named = self.asked_named.borrow_mut().take().unwrap_or_default();
        (named.into_iter().filter(|n| !other.contains(n)).collect(), other)
    }

    /// Stop recording and hand back what was asked.
    pub fn take_asked(&self) -> HashSet<String> {
        self.asked.borrow_mut().take().unwrap_or_default()
    }

    /// Indices of the definitions in `file`, in index order.
    pub fn in_file(&self, file: &str) -> &[usize] {
        let map = self.by_file.get_or_init(|| {
            let mut m: HashMap<String, Vec<usize>> = HashMap::new();
            for (i, d) in self.defs.iter().enumerate() {
                m.entry(d.id.file.clone()).or_default().push(i);
            }
            m
        });
        map.get(file).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// What a scoped build needs resolved in one file: every call inside the
/// functions declared at `lines`, and every call that can resolve to a
/// definition named in `names`, or stay unresolved under such a name.
pub struct Want<'a> {
    pub names: &'a HashSet<String>,
    pub lines: Option<&'a HashSet<usize>>,
}

impl Want<'_> {
    /// Whether every call in the function declared at `line` is wanted.
    pub fn whole(&self, line: usize) -> bool {
        self.lines.is_some_and(|l| l.contains(&line))
    }
}

/// One file's calls as a resolver extracted them from its tree, so the tree
/// can be dropped before the calls are resolved.
pub enum FileModel {
    Go(go::FileScope),
    Rust(rust::FileScope),
    /// Fallback: for each definition of the file, in order, its (callee, line)
    /// call sites.
    Names(Vec<Vec<(String, usize)>>),
    None,
}

/// Receiver of a method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receiver {
    SelfRecv,
    Var(String),
    /// Anything we can't classify; keeps raw text for display.
    Opaque(String),
}

/// A callee expression, structurally classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// `foo()`
    Bare(String),
    /// `a::b::foo()`, `Foo::new()`
    Path(Vec<String>),
    /// `x.foo()`, `self.foo()`
    Method { receiver: Receiver, name: String },
}

impl Callee {
    /// Bare name used for display and name-keyed lookups.
    pub fn name(&self) -> &str {
        match self {
            Callee::Bare(n) => n,
            Callee::Path(segs) => segs.last().map(|s| s.as_str()).unwrap_or(""),
            Callee::Method { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Exact,
    /// Best-ranked of several candidates — rendered with a '?' marker.
    Guess,
}

#[derive(Debug, Clone)]
pub struct ResolvedCall {
    /// Index into `DefIndex.defs` of the enclosing (calling) function.
    pub caller: usize,
    /// Bare callee name for display.
    pub callee_display: String,
    /// Line of the call site.
    pub line: usize,
    /// Resolved definition, or None for external/unknown callees.
    pub target: Option<(usize, Confidence)>,
}

#[allow(dead_code)] // wired by the path command
/// One call expression, kept per site. The graph keeps one edge per
/// (caller, callee, target), so this table is where every site lives, with
/// the byte ranges the flow engine needs. Interface dispatch gives one site
/// per target, like `ResolvedCall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Index into `DefIndex.defs` of the enclosing (calling) function.
    pub caller: usize,
    /// Bare callee name for display.
    pub callee_display: String,
    /// Resolved definition, or None for external or unknown callees.
    pub target: Option<(usize, Confidence)>,
    /// Line of the call expression (1-based).
    pub line: usize,
    /// Byte range of the whole call expression.
    pub start_byte: usize,
    pub end_byte: usize,
    /// Byte range of each argument, in order.
    pub args: Vec<(usize, usize)>,
}

/// One parsed file, shared with the process-wide cache in `parsed`.
pub use crate::parsed::ParsedFile;

/// Every file of one run, shared by the call graph and the flow engine. A view
/// over `parsed`: each file is read and parsed once per run, whoever asks
/// first. Files keep the order they were added in.
///
/// A store made with `lazy` parses a file only when it is asked for, by path
/// or by a name its text holds (`holding`), so a walk that visits a few files
/// parses only those. `iter` still visits, and parses, every file.
#[derive(Default)]
pub struct TreeStore {
    files: Vec<StoreEntry>,
    by_path: HashMap<String, usize>,
    /// The word index of a lazy store, over its jobs.
    words: Option<std::rc::Rc<scoped::Words>>,
    /// Job index to entry, for a lazy store.
    entry_of_job: HashMap<usize, usize>,
}

struct StoreEntry {
    path: String,
    lang: &'static str,
    parsed: OnceCell<Option<std::rc::Rc<ParsedFile>>>,
}

impl StoreEntry {
    fn parsed(&self) -> Option<&ParsedFile> {
        self.parsed.get_or_init(|| crate::parsed::get(&self.path, self.lang)).as_deref()
    }
}

impl TreeStore {
    /// Every job from the cache, parsing the ones not yet parsed. Unreadable
    /// files are left out.
    #[allow(dead_code)] // tests build whole stores
    pub fn build(jobs: &[(String, &'static str)]) -> TreeStore {
        let mut store = TreeStore::default();
        for (file, lang) in jobs {
            if let Some(p) = crate::parsed::get(file, lang) {
                store.put(file, p);
            }
        }
        store
    }

    /// The same files as `build`, each parsed on first use. `words` is the
    /// word index over the same `jobs`, which also says which are readable.
    pub fn lazy(jobs: &[(String, &'static str)], words: std::rc::Rc<scoped::Words>) -> TreeStore {
        let mut store = TreeStore::default();
        for (j, (file, lang)) in jobs.iter().enumerate() {
            if words.source(j).is_none() {
                continue;
            }
            let i = match store.by_path.get(file.as_str()) {
                Some(&i) => {
                    // A path given twice keeps its first place and its last grammar.
                    store.files[i].lang = lang;
                    i
                }
                None => {
                    store.by_path.insert(file.clone(), store.files.len());
                    store.files.push(StoreEntry { path: file.clone(), lang, parsed: OnceCell::new() });
                    store.files.len() - 1
                }
            };
            store.entry_of_job.insert(j, i);
        }
        store.words = Some(words);
        store
    }

    /// Parse `source` and add it under `file`, replacing an earlier entry.
    /// Tests and template bodies use this for text with no file behind it, so
    /// it bypasses the cache.
    pub fn insert(&mut self, file: &str, lang: &'static str, source: Vec<u8>) {
        let tree = crate::parsed::parse_text(lang, &source);
        self.put(file, std::rc::Rc::new(ParsedFile { lang, source: std::rc::Rc::new(source), tree }));
    }

    fn put(&mut self, file: &str, entry: std::rc::Rc<ParsedFile>) {
        let e = StoreEntry { path: file.to_string(), lang: entry.lang, parsed: OnceCell::from(Some(entry)) };
        match self.by_path.get(file) {
            Some(&i) => self.files[i] = e,
            None => {
                self.by_path.insert(file.to_string(), self.files.len());
                self.files.push(e);
            }
        }
    }

    pub fn get(&self, file: &str) -> Option<&ParsedFile> {
        self.by_path.get(file).and_then(|&i| self.files[i].parsed())
    }

    /// (path, parsed file) in insertion order, every file parsed.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParsedFile)> {
        self.files.iter().filter_map(|e| e.parsed().map(|p| (e.path.as_str(), p)))
    }

    /// (path, grammar) of every file in insertion order, none parsed.
    pub fn paths(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.files.iter().map(|e| (e.path.as_str(), e.lang))
    }

    /// The text of `file` without parsing it.
    pub fn source(&self, file: &str) -> Option<crate::parsed::Source> {
        let &i = self.by_path.get(file)?;
        let e = &self.files[i];
        match e.parsed.get() {
            Some(p) => p.as_ref().map(|p| p.source.clone()),
            None => crate::parsed::source(&e.path),
        }
    }

    /// The files whose text holds one of `names` as a whole word, in
    /// insertion order, parsed. Every other file is left unparsed. A file
    /// that cannot hold a name cannot define, call or bind it.
    pub fn holding(&self, names: &[&str]) -> Vec<(&str, &ParsedFile)> {
        let idx: Vec<usize> = match &self.words {
            Some(w) => {
                let mut v: Vec<usize> = names
                    .iter()
                    .flat_map(|n| w.holders(n))
                    .filter_map(|j| self.entry_of_job.get(&j).copied())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            None => (0..self.files.len())
                .filter(|&i| self.files[i].parsed().is_some_and(|p| scoped::holds_any(&p.source, names)))
                .collect(),
        };
        idx.into_iter().filter_map(|i| self.files[i].parsed().map(|p| (self.files[i].path.as_str(), p))).collect()
    }

    /// Finds the files of a lazy store holding a name, without the store.
    /// None for a store built whole.
    pub fn finder(&self) -> Option<NameFinder> {
        let words = self.words.clone()?;
        Some(NameFinder {
            words,
            entries: std::rc::Rc::new(self.files.iter().map(|e| (e.path.clone(), e.lang)).collect()),
            entry_of_job: std::rc::Rc::new(self.entry_of_job.clone()),
        })
    }

    /// The path of the parsed file whose text is `src`, by address.
    pub fn file_of(&self, src: &[u8]) -> Option<&str> {
        let ptr = src.as_ptr();
        self.files.iter().find_map(|e| match e.parsed.get() {
            Some(Some(p)) if p.source.as_ptr() == ptr => Some(e.path.as_str()),
            _ => None,
        })
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.files.len()
    }
}

/// The files of a lazy `TreeStore` holding a name as a whole word, as
/// (path, grammar) in store order, for readers that outlive a borrow of it.
#[derive(Clone)]
pub struct NameFinder {
    words: std::rc::Rc<scoped::Words>,
    entries: std::rc::Rc<Vec<(String, &'static str)>>,
    entry_of_job: std::rc::Rc<HashMap<usize, usize>>,
}

impl NameFinder {
    pub fn holders(&self, name: &str) -> Vec<(String, &'static str)> {
        self.entries_of(self.words.holders(name))
    }

    /// The holders of `name` that can declare it (`scoped::go_could_bind`).
    pub fn binders(&self, name: &str) -> Vec<(String, &'static str)> {
        self.entries_of(self.words.binders(name))
    }

    fn entries_of(&self, jobs: Vec<usize>) -> Vec<(String, &'static str)> {
        let mut v: Vec<usize> = jobs.into_iter().filter_map(|j| self.entry_of_job.get(&j).copied()).collect();
        v.sort();
        v.dedup();
        v.into_iter().map(|i| self.entries[i].clone()).collect()
    }
}

pub trait Resolver {
    /// Phase 1: definitions in one file (feeds the project index).
    fn collect_defs(&self, file: &str, source: &[u8]) -> Vec<Def>;
    /// Phase 1 with an already parsed tree. Resolvers that do not use it
    /// parse again.
    fn collect_defs_tree(&self, file: &str, source: &[u8], _tree: Option<&tree_sitter::Tree>) -> Vec<Def> {
        self.collect_defs(file, source)
    }
    /// Phase 2 with an already parsed tree, plus the per-site table. Only the
    /// Go resolver fills the sites, the others return none.
    #[cfg(test)]
    fn resolve_calls_tree(
        &self,
        file: &str,
        source: &[u8],
        tree: Option<&tree_sitter::Tree>,
        index: &DefIndex,
    ) -> (Vec<ResolvedCall>, Vec<CallSite>) {
        let own: Vec<Def> = index.in_file(file).iter().map(|&i| index.defs[i].clone()).collect();
        let model = self.model(file, source, tree, &own);
        self.resolve_model(file, &model, index, None)
    }
    /// The calls of one file, read from its tree. `own` is the file's
    /// definitions in index order.
    fn model(&self, file: &str, source: &[u8], tree: Option<&tree_sitter::Tree>, own: &[Def]) -> FileModel;
    /// Phase 2 over a model: the calls `want` asks for, or all of them,
    /// resolved against `index`.
    fn resolve_model(&self, file: &str, model: &FileModel, index: &DefIndex, want: Option<&Want>) -> (Vec<ResolvedCall>, Vec<CallSite>);
    /// Phase 1b: explicit implements-relations (`impl Trait for Type`).
    /// Languages with structural conformance (Go) don't need this.
    fn collect_impls(&self, _file: &str, _source: &[u8]) -> Vec<(String, String)> {
        vec![]
    }
    /// Phase 1b with an already parsed tree. Resolvers that do not use it
    /// parse again.
    fn collect_impls_tree(&self, file: &str, source: &[u8], _tree: Option<&tree_sitter::Tree>) -> Vec<(String, String)> {
        self.collect_impls(file, source)
    }
    /// Phase 2: call sites in one file, resolved against the full project index.
    #[cfg(test)]
    fn resolve_calls(&self, file: &str, source: &[u8], index: &DefIndex) -> Vec<ResolvedCall> {
        self.resolve_calls_tree(file, source, None, index).0
    }
}

pub fn resolver_for(lang_name: &str) -> Box<dyn Resolver> {
    match lang_name {
        "go" => Box::new(go::GoResolver::default()),
        "rust" => Box::new(rust::RustResolver::default()),
        _ => Box::new(fallback::NameResolver::new(lang_name)),
    }
}

// ── `ctsq def` ────────────────────────────────────────────────────────────────

/// What a command that takes several names produced: stdout text, stderr text
/// and the exit code. Each is built as a string so tests can assert on it.
pub struct NamesOut {
    pub out: String,
    pub err: String,
    pub code: i32,
}

/// Add the counts in `more` to `into`. Only the test and exclude counts are
/// summed, since no command that takes several names uses `--max`.
pub fn merge_hidden(into: &mut crate::footer::Hidden, more: &crate::footer::Hidden) {
    into.tests += more.tests;
    into.excluded += more.excluded;
}

/// The nearest name to `name` among the definitions in `jobs`, as
/// `x in path:line`, or None when nothing is close.
#[cfg(test)]
pub fn similar_hint(name: &str, jobs: &[(String, &'static str)]) -> Option<String> {
    similar_hint_from(name, &outline_of(jobs))
}

/// (file, kind, name, line) of every outline entry in `jobs`.
fn outline_of(jobs: &[(String, &'static str)]) -> Vec<(String, &'static str, String, usize)> {
    let mut v = Vec::new();
    for (file, lang) in jobs {
        for (kind, name, line) in crate::tree::collect_def_locs(file, lang) {
            v.push((file.clone(), kind, name, line));
        }
    }
    v
}

/// `similar_hint` over an outline already collected.
fn similar_hint_from(name: &str, outline: &[(String, &'static str, String, usize)]) -> Option<String> {
    // A `mod x;` or an import line names a file, not something defined.
    let pool: Vec<(String, String, usize)> = outline
        .iter()
        .filter(|(_, kind, ..)| !matches!(*kind, "mod" | "import" | "use"))
        .map(|(file, _, n, line)| (n.clone(), file.clone(), *line))
        .collect();
    let names: Vec<String> = pool.iter().map(|p| p.0.clone()).collect();
    let near = similar_names(crate::explain::bare_name(name), &names);
    let first = near.first()?;
    let (n, file, line) = pool.iter().find(|p| &p.0 == first)?;
    Some(format!("{} in {}:{}", n, file, line))
}

/// The one stderr line listing names that did not resolve, with the nearest
/// name where there is one: `not found: A (similar: a in f.c:3), B`. Empty
/// when every name resolved.
pub fn not_found_line(missing: &[String], jobs: &[(String, &'static str)]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    not_found_from(missing, &outline_of(jobs))
}

/// `not_found_line` over an outline already collected.
fn not_found_from(missing: &[String], outline: &[(String, &'static str, String, usize)]) -> String {
    let parts: Vec<String> = missing
        .iter()
        .map(|m| match similar_hint_from(m, outline) {
            Some(h) => format!("{} (similar: {})", m, h),
            None => m.clone(),
        })
        .collect();
    format!("not found: {}\n", parts.join(", "))
}

/// Entry point for `ctsq def <name-or-expr>... [path]...`: print resolved
/// definition locations for names, qualified paths, or method expressions.
///
/// Resolution runs the two steps `explain` runs, in the same order: the
/// call-graph definition index first, then the `tree` outline. The index only
/// holds callable definitions, so without the outline step a struct, class,
/// type alias or top-level variable resolved by `explain` reported nothing here.
pub fn run_def(
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    mut hider: crate::files::Hider,
    scope: Option<crate::scope::Opts>,
) {
    let glossary = match crate::config::glossary::load(&std::env::current_dir().unwrap_or_default()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    };
    let r = def_report(names, where_, jobs, &mut hider, scope, glossary.as_ref());
    print!("{}", r.out);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprint!("{}", r.err);
    if r.code != 0 {
        std::process::exit(r.code);
    }
}

/// `run_def` as text. With one name a miss gets a report on stderr, ahead of
/// the `not found:` line, and stdout stays empty. With several, a miss is only
/// named on the `not found:` line. The footer counts everything hidden across
/// all names, once, at the end. When every definition was hidden, stdout is
/// empty and stderr says so (BUILD.md A20). The code is 0 when any definition
/// was shown, 1 otherwise.
pub fn def_report(
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    hider: &mut crate::files::Hider,
    scope: Option<crate::scope::Opts>,
    glossary: Option<&crate::config::glossary::Glossary>,
) -> NamesOut {
    let mut out = String::new();
    let mut miss_text = String::new();
    let mut hidden = crate::footer::Hidden::default();
    let mut printer = scope.map(|o| crate::scope::Printer::new(o, jobs));
    let mut shown = 0usize;
    let mut missing: Vec<String> = Vec::new();
    // The index is built once for all the names, from files parsed and dropped
    // in turn. It holds definitions, not trees.
    let mut scan = DefScan::new(jobs);
    scan.cover_exprs(names);
    for expr in names {
        let mut all_names: Vec<String> = Vec::new();
        let mut rows = def_rows(expr, jobs, &mut scan, &mut all_names);
        // A name that is a glossary term is also searched as each name it maps to.
        let mut notes: Vec<String> = Vec::new();
        if let Some(g) = glossary {
            for entry in g.lookup(expr, true) {
                for r in mapped_rows(&entry.names, jobs, &mut scan) {
                    if !rows.iter().any(|(f, l, _)| *f == r.0 && *l == r.1) {
                        rows.push(r);
                    }
                }
                notes.push(entry.note(&g.file));
            }
        }
        if rows.is_empty() {
            missing.push(expr.clone());
            if names.len() == 1 {
                miss_text.push_str(&format!(
                    "no definition for '{}' in {} ({} {})\n",
                    expr,
                    where_,
                    jobs.len(),
                    if jobs.len() == 1 { "file" } else { "files" }
                ));
                let similar = similar_names(crate::explain::bare_name(expr), &all_names);
                if !similar.is_empty() {
                    miss_text.push_str(&format!("similar names: {}\n", similar.join(", ")));
                }
            }
            for n in &notes {
                miss_text.push_str(&format!("{}\n", n));
            }
            continue;
        }
        let rows = hider.keep(rows, |(f, l, _)| (f.clone(), *l), &mut hidden);
        let rows: Vec<_> = rows.into_iter().filter(|(f, _, _)| crate::footer::take(f)).collect();
        shown += rows.len();
        for (file, line, conf) in rows {
            let line_text = crate::parsed::source(&file)
                .and_then(|b| {
                    std::str::from_utf8(&b)
                        .ok()
                        .and_then(|s| s.lines().nth(line.saturating_sub(1)).map(|l| l.trim().to_string()))
                })
                .unwrap_or_default();
            let mark = if conf == Confidence::Guess { "  ?" } else { "" };
            out.push_str(&format!("{}:{}: {}{}\n", file, line, line_text, mark));
            if let Some(p) = printer.as_mut() {
                out.push_str(&p.def_rows(&file, line, ""));
            }
        }
        // The glossary line comes last before the footer.
        for n in &notes {
            out.push_str(&format!("{}\n", n));
        }
    }
    crate::footer::settle(&mut hidden);
    let mut err = miss_text;
    if shown == 0 && hidden.any() {
        // Every definition hidden: stderr says where they are, and exit 1.
        out.clear();
        err.push_str(&crate::footer::nothing_shown(
            &format!("no definition to {}", names.join(", ")),
            where_,
            jobs.len(),
            &hidden,
        ));
    } else if shown > 0 {
        out.push_str(&crate::footer::tail(shown, "definition", &hidden, scope.is_some()));
    }
    if !missing.is_empty() {
        // A miss already read the outline for its report, so it is reused.
        err.push_str(&not_found_from(&missing, &scan.whole_outline()));
    }
    NamesOut { out, err, code: if shown == 0 { 1 } else { 0 } }
}

/// Definition sites for `expr`, in the order `explain` resolves them: the
/// call-graph definition index first, then the `tree` outline for everything
/// the index cannot hold. `names_seen` collects every outline name met on the
/// way, which is the pool the miss report suggests from.
///
/// `scan` holds what the files gave up, never their trees: each file is parsed,
/// read and dropped in turn, so no run holds every tree at once.
pub fn def_rows(
    expr: &str,
    jobs: &[(String, &'static str)],
    scan: &mut DefScan,
    names_seen: &mut Vec<String>,
) -> Vec<(String, usize, Confidence)> {
    let bare = crate::explain::bare_name(expr);
    // A definition of a name holds its bytes, so the files holding the name
    // give every definition and outline entry of it.
    scan.cover(def_query_name(expr));
    scan.cover(bare);
    let index = &scan.index;
    let rows: Vec<(String, usize, Confidence)> = resolve_def_query(expr, index)
        .into_iter()
        .map(|(i, conf)| {
            let d = &index.defs[i];
            (d.id.file.clone(), d.id.line, conf)
        })
        .collect();
    if !rows.is_empty() {
        return rows;
    }

    // A struct, class, type alias or top-level variable is not callable, so the
    // index never held it. This is the step `explain` takes next.
    let mut found: Vec<(String, usize)> = Vec::new();
    for (file, _kind, name, line) in scan.files.iter().flatten().flat_map(|f| f.outline.iter()) {
        if name == bare {
            found.push((file.clone(), *line));
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        found = nested_def_sites(bare, jobs);
    }
    if found.is_empty() {
        // The miss report suggests from every name in the folder.
        scan.cover_all();
        for (_, _, name, _) in scan.files.iter().flatten().flat_map(|f| f.outline.iter()) {
            names_seen.push(name.clone());
        }
        names_seen.extend(scan.index.by_name.keys().cloned());
    }
    let conf = if found.len() == 1 { Confidence::Exact } else { Confidence::Guess };
    found.into_iter().map(|(f, l)| (f, l, conf)).collect()
}

/// The name `resolve_def_query` looks `expr` up by.
fn def_query_name(expr: &str) -> &str {
    if expr.contains("::") {
        expr.split("::")
            .filter(|s| !s.is_empty() && !matches!(*s, "crate" | "self" | "super"))
            .last()
            .unwrap_or("")
    } else if let Some((_, name)) = expr.rsplit_once('.') {
        name
    } else {
        expr
    }
}

/// What `def` learns from one file: its call-graph definitions and its outline
/// entries as (file, kind, name, line).
struct ScannedFile {
    defs: Vec<Def>,
    outline: Vec<(String, &'static str, String, usize)>,
}

/// What `def` learns from the files, shared by every name asked: the call-graph
/// definition index and the outline, over the files holding a name asked so
/// far. Each file is parsed, read and dropped in turn.
pub struct DefScan {
    jobs: Vec<(String, &'static str)>,
    /// Per job, once read. None also for an unreadable file.
    files: Vec<Option<ScannedFile>>,
    read: Vec<bool>,
    covered: HashSet<String>,
    index: DefIndex,
}

impl DefScan {
    /// Nothing read yet: `cover` reads the files a name needs.
    pub fn new(jobs: &[(String, &'static str)]) -> DefScan {
        DefScan {
            jobs: jobs.to_vec(),
            files: (0..jobs.len()).map(|_| None).collect(),
            read: vec![false; jobs.len()],
            covered: HashSet::new(),
            index: DefIndex::default(),
        }
    }

    /// Read every file holding `name`, then rebuild the index in job order.
    fn cover(&mut self, name: &str) {
        self.cover_names(&[name]);
    }

    /// Read every file holding one of `names` as a whole word (see
    /// `scoped::holds_any`), each file's text read once.
    pub fn cover_names(&mut self, names: &[&str]) {
        let new: Vec<&str> = names.iter().copied().filter(|n| self.covered.insert(n.to_string())).collect();
        if new.is_empty() {
            return;
        }
        let pick: Vec<usize> = (0..self.jobs.len())
            .filter(|&j| !self.read[j])
            .filter(|&j| crate::parsed::source(&self.jobs[j].0).is_some_and(|s| scoped::holds_any(&s, &new)))
            .collect();
        self.read_jobs(&pick);
    }

    /// Cover what `def_rows` reads for each of `exprs`, in one pass.
    pub fn cover_exprs(&mut self, exprs: &[String]) {
        let names: Vec<&str> = exprs.iter().flat_map(|e| [def_query_name(e), crate::explain::bare_name(e)]).collect();
        self.cover_names(&names);
    }

    /// Read every file, for a miss report.
    fn cover_all(&mut self) {
        let pick: Vec<usize> = (0..self.jobs.len()).filter(|&j| !self.read[j]).collect();
        self.read_jobs(&pick);
    }

    fn read_jobs(&mut self, pick: &[usize]) {
        if pick.is_empty() {
            return;
        }
        for &j in pick {
            self.read[j] = true;
            let (file, lang) = &self.jobs[j];
            let Some(p) = crate::parsed::get(file, lang) else { continue };
            let defs = resolver_for(p.lang).collect_defs_tree(file, &p.source, p.tree.as_ref());
            // Read while the parse is held, so a miss needs no second parse.
            let outline = crate::tree::collect_def_locs(file, lang)
                .into_iter()
                .map(|(kind, name, line)| (file.clone(), kind, name, line))
                .collect();
            self.files[j] = Some(ScannedFile { defs, outline });
        }
        let mut index = DefIndex::default();
        for f in self.files.iter().flatten() {
            for d in &f.defs {
                index.push(d.clone());
            }
        }
        self.index = index;
    }

    /// (file, kind, name, line) of every outline entry in the folder.
    fn whole_outline(&mut self) -> Vec<(String, &'static str, String, usize)> {
        self.cover_all();
        self.files.iter().flatten().flat_map(|f| f.outline.iter().cloned()).collect()
    }
}

/// The last step of `def_rows`: a name defined below the top level, such as a
/// const inside a component or a closure bound to a local. Neither the
/// call-graph index nor the outline holds those, so this asks the query engine
/// for any function, class, type or variable declaration of that name.
/// `def_rows` for many names over the same jobs, quiet, with the index and the
/// outline fallback built once. `find` resolves every route target with it.
///
/// It is targeted: a definition of a name has the name's bytes in its file, so
/// `find` parses only the files that hold them (a byte search over the sources
/// read once) and builds the index from those. The rows and their ranking are
/// the ones a full index gives. Each file is parsed at most once per lookup.
pub struct DefLookup {
    jobs: Vec<(String, &'static str)>,
    /// Text of each job's file, empty when it cannot be read.
    sources: Vec<String>,
    /// Definitions of a job, once the job was needed.
    defs: HashMap<usize, Vec<Def>>,
    /// (name, line) of a job's outline entries, once a miss needed them.
    locs: HashMap<usize, Vec<(String, usize)>>,
    answers: HashMap<String, Vec<(String, usize, Confidence)>>,
}

pub use crate::needle::holds;

impl DefLookup {
    pub fn new(jobs: &[(String, &'static str)]) -> DefLookup {
        let sources = jobs
            .iter()
            .map(|(f, _)| String::from_utf8_lossy(&crate::parsed::source(f).map(|b| b.to_vec()).unwrap_or_default()).into_owned())
            .collect();
        DefLookup {
            jobs: jobs.to_vec(),
            sources,
            defs: HashMap::new(),
            locs: HashMap::new(),
            answers: HashMap::new(),
        }
    }

    /// The same rows `def_rows` gives for `expr`, in the same order.
    pub fn find(&mut self, expr: &str) -> Vec<(String, usize, Confidence)> {
        if let Some(a) = self.answers.get(expr) {
            return a.clone();
        }
        let bare = crate::explain::bare_name(expr);
        let mine: Vec<usize> = (0..self.jobs.len()).filter(|&i| self.sources[i].contains(bare)).collect();
        let mut index = DefIndex::default();
        for &i in &mine {
            let (file, lang) = &self.jobs[i];
            let source = &self.sources[i];
            let defs = self.defs.entry(i).or_insert_with(|| resolver_for(lang).collect_defs(file, source.as_bytes()));
            for d in defs.iter() {
                index.push(d.clone());
            }
        }
        let mut rows: Vec<(String, usize, Confidence)> = resolve_def_query(expr, &index)
            .into_iter()
            .map(|(i, conf)| (index.defs[i].id.file.clone(), index.defs[i].id.line, conf))
            .collect();
        if rows.is_empty() {
            let mut found: Vec<(String, usize)> = Vec::new();
            for &i in &mine {
                let (file, lang) = &self.jobs[i];
                let locs = self.locs.entry(i).or_insert_with(|| {
                    crate::tree::collect_def_locs(file, lang).into_iter().map(|(_, n, l)| (n, l)).collect()
                });
                found.extend(locs.iter().filter(|(n, _)| n == bare).map(|(_, l)| (file.clone(), *l)));
            }
            found.sort();
            found.dedup();
            if found.is_empty() {
                let subset: Vec<(String, &'static str)> = mine.iter().map(|&i| self.jobs[i].clone()).collect();
                found = nested_def_sites(bare, &subset);
            }
            let conf = if found.len() == 1 { Confidence::Exact } else { Confidence::Guess };
            rows = found.into_iter().map(|(f, l)| (f, l, conf)).collect();
        }
        self.answers.insert(expr.to_string(), rows.clone());
        rows
    }
}

/// Definition sites for each name a glossary entry maps to: an exact name goes
/// through `def_rows`, a regex over names is matched against the outline names.
pub fn mapped_rows(
    names: &[crate::config::glossary::Name],
    jobs: &[(String, &'static str)],
    scan: &mut DefScan,
) -> Vec<(String, usize, Confidence)> {
    use crate::config::glossary::Name;
    let mut out: Vec<(String, usize, Confidence)> = Vec::new();
    for n in names {
        match n {
            Name::Exact(name) => out.extend(def_rows(name, jobs, scan, &mut Vec::new())),
            Name::Re(re) => {
                for (file, lang) in jobs {
                    for (_, name, line) in crate::tree::collect_def_locs(file, lang) {
                        if re.is_match(&name) {
                            out.push((file.clone(), line, Confidence::Guess));
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    out.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    out
}

fn nested_def_sites(name: &str, jobs: &[(String, &'static str)]) -> Vec<(String, usize)> {
    let queries: Vec<crate::query::ast::Query> = ["function", "variable", "class", "type"]
        .iter()
        .filter_map(|k| crate::query::parse(&format!("*{}#\"{}\"", k, name)).ok())
        .collect();
    let needles = crate::needle::Needles::exact(name);
    let mut found: Vec<(String, usize)> = Vec::new();
    // Compiled queries per language, built once.
    let mut compiled_by_lang: HashMap<&'static str, Vec<crate::compiler::CompiledQuery>> = HashMap::new();
    for (file, lang_name) in jobs {
        let Some(source) = crate::parsed::source(file) else { continue };
        // A definition of the name has the name's bytes in its file.
        if !needles.holds(&source) {
            continue;
        }
        let Some(lang) = crate::compiler::lang::for_name(lang_name) else { continue };
        let compiled = compiled_by_lang
            .entry(lang_name)
            .or_insert_with(|| queries.iter().map(|ast| crate::compiler::compile(ast, lang.as_ref())).collect());
        let Some(p) = crate::parsed::get(file, lang_name) else { continue };
        let Some(tree) = p.tree.as_ref() else { continue };
        for c in compiled.iter() {
            let rows = crate::runner::run_rows_on(c, lang.as_ref(), &p.source, tree, false).unwrap_or_default();
            found.extend(rows.into_iter().map(|r| (file.clone(), r.line)));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Up to three names from `pool` that look like `name`, case-insensitively:
/// one contains the other, either way round. Fragments under three characters
/// are skipped, because they match nearly everything. Closest length first.
pub fn similar_names(name: &str, pool: &[String]) -> Vec<String> {
    let needle = name.to_lowercase();
    if needle.len() < 3 {
        return vec![];
    }
    let mut hits: Vec<String> = pool
        .iter()
        .filter(|n| {
            let low = n.to_lowercase();
            low.len() >= 3
                && n.as_str() != name
                && (low.contains(&needle) || needle.contains(&low))
        })
        .cloned()
        .collect();
    hits.sort();
    hits.dedup();
    hits.sort_by_key(|n| (n.len().abs_diff(name.len()), n.clone()));
    hits.truncate(3);
    hits
}

/// Context-free resolution of a textual query against the definition index.
/// `a::b::f` / `Foo::method` match by qualified-path suffix; `x.method` /
/// `self.method` match methods by name (filtered to a receiver type when the
/// receiver looks like one) plus, for a lowercase qualifier, functions in a
/// matching package (Go's `util.Helper`); a bare name matches any definition.
pub fn resolve_def_query(expr: &str, index: &DefIndex) -> Vec<(usize, Confidence)> {
    let candidates: Vec<usize> = if expr.contains("::") {
        let segs: Vec<String> = expr.split("::")
            .filter(|s| !s.is_empty() && !matches!(*s, "crate" | "self" | "super"))
            .map(String::from)
            .collect();
        let Some(name) = segs.last() else { return vec![] };
        by_name(index, name)
            .filter(|&i| {
                let q = &index.defs[i].qualified;
                segs.len() <= q.len() && q[q.len() - segs.len()..] == *segs
            })
            .collect()
    } else if let Some((recv, name)) = expr.rsplit_once('.') {
        let recv_is_type = recv != "self"
            && recv.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        by_name(index, name)
            .filter(|&i| match &index.defs[i].kind {
                DefKind::Method { receiver, .. }
                | DefKind::InterfaceMethod { interface: receiver } =>
                    !recv_is_type || receiver == recv,
                DefKind::Function => {
                    let q = &index.defs[i].qualified;
                    !recv_is_type && q.len() >= 2 && q[q.len() - 2] == recv
                }
            })
            .collect()
    } else {
        by_name(index, expr).collect()
    };

    let mut hits: Vec<(usize, Confidence)> = match candidates.len() {
        1 => vec![(candidates[0], Confidence::Exact)],
        _ => candidates.into_iter().map(|i| (i, Confidence::Guess)).collect(),
    };
    hits.sort_by(|a, b| {
        let (da, db) = (&index.defs[a.0].id, &index.defs[b.0].id);
        da.file.cmp(&db.file).then(da.line.cmp(&db.line))
    });
    hits
}

fn by_name<'a>(index: &'a DefIndex, name: &str) -> impl Iterator<Item = usize> + 'a {
    index.by_name.get(name).map(|v| v.as_slice()).unwrap_or(&[]).iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, qualified: &[&str], kind: DefKind, file: &str, line: usize) -> Def {
        Def {
            name: name.to_string(),
            qualified: qualified.iter().map(|s| s.to_string()).collect(),
            kind,
            ret: None,
            id: DefId { file: file.to_string(), line },
        }
    }

    fn sample_index() -> DefIndex {
        let mut idx = DefIndex::default();
        idx.push(def("helper", &["util", "helper"], DefKind::Function, "src/util.rs", 1));
        idx.push(def("helper", &["other", "helper"], DefKind::Function, "src/other.rs", 1));
        idx.push(def("new", &["geo", "Foo", "new"],
            DefKind::Method { receiver: "Foo".into(), via_trait: None }, "src/geo.rs", 3));
        idx.push(def("new", &["geo", "Bar", "new"],
            DefKind::Method { receiver: "Bar".into(), via_trait: None }, "src/geo.rs", 9));
        idx.push(def("unique_fn", &["unique_fn"], DefKind::Function, "src/a.rs", 5));
        idx
    }

    #[test]
    fn bare_unique_name_is_exact() {
        let idx = sample_index();
        let hits = resolve_def_query("unique_fn", &idx);
        assert_eq!(hits, vec![(4, Confidence::Exact)]);
    }

    #[test]
    fn bare_ambiguous_name_lists_all_as_guesses() {
        let idx = sample_index();
        let hits = resolve_def_query("helper", &idx);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|(_, c)| *c == Confidence::Guess));
    }

    #[test]
    fn qualified_path_narrows_to_exact() {
        let idx = sample_index();
        let hits = resolve_def_query("util::helper", &idx);
        assert_eq!(hits, vec![(0, Confidence::Exact)]);
        let hits = resolve_def_query("Foo::new", &idx);
        assert_eq!(hits, vec![(2, Confidence::Exact)]);
    }

    #[test]
    fn method_expr_filters_by_receiver_type() {
        let idx = sample_index();
        let hits = resolve_def_query("x.new", &idx);
        assert_eq!(hits.len(), 2, "unknown receiver: all methods named new");
        let hits = resolve_def_query("Bar.new", &idx);
        assert_eq!(hits, vec![(3, Confidence::Exact)]);
    }

    #[test]
    fn dot_with_package_qualifier_matches_functions() {
        let idx = sample_index();
        let hits = resolve_def_query("util.helper", &idx);
        assert_eq!(hits, vec![(0, Confidence::Exact)], "Go-style pkg.Fn query");
    }

    #[test]
    fn missing_name_returns_empty() {
        let idx = sample_index();
        assert!(resolve_def_query("nope", &idx).is_empty());
        assert!(resolve_def_query("wrong::helper", &idx).is_empty());
    }

    fn jobs_for(sub: &str) -> Vec<(String, &'static str)> {
        let path = format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), sub);
        crate::files::resolve_jobs(&path, None, false)
    }

    fn rows_for(expr: &str, jobs: &[(String, &'static str)], names: &mut Vec<String>) -> Vec<(String, usize, Confidence)> {
        def_rows(expr, jobs, &mut DefScan::new(jobs), names)
    }

    #[test]
    fn def_finds_a_type_the_call_graph_index_cannot_hold() {
        // Server is a struct, so only the outline step resolves it. This is
        // the case that printed "no definition found" and exited 1.
        let mut names = Vec::new();
        let rows = rows_for("Server", &jobs_for("example.go"), &mut names);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert!(rows[0].0.ends_with("example.go"), "{:?}", rows);
        assert_eq!(rows[0].1, 5, "{:?}", rows);
    }

    #[test]
    fn def_agrees_with_explain_on_a_class() {
        let jobs = jobs_for("example.cpp");
        let mut names = Vec::new();
        let rows = rows_for("Buffer", &jobs, &mut names);
        assert!(!rows.is_empty(), "def must resolve what explain resolves");
        let r = crate::explain::report("Buffer", &jobs, &crate::explain::Options::default());
        assert!(
            r.contains(&format!("{}:{}", rows[0].0, rows[0].1)),
            "def site {:?} missing from explain: {}",
            rows[0],
            r
        );
    }

    #[test]
    fn def_still_prefers_the_call_graph_index_for_functions() {
        let mut names = Vec::new();
        let rows = rows_for("NewServer", &jobs_for("example.go"), &mut names);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert_eq!(rows[0].2, Confidence::Exact, "{:?}", rows);
    }

    #[test]
    fn a_miss_collects_the_names_it_could_have_meant() {
        let mut names = Vec::new();
        let rows = rows_for("NewServe", &jobs_for("example.go"), &mut names);
        assert!(rows.is_empty(), "{:?}", rows);
        assert!(names.contains(&"NewServer".to_string()), "{:?}", names);
    }

    #[test]
    fn similar_names_picks_the_closest_by_length() {
        let pool: Vec<String> = ["UpdateItem", "UpdateItemMeta", "Update", "unrelated"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let hits = similar_names("UpdateItemMet", &pool);
        assert_eq!(hits.first().map(String::as_str), Some("UpdateItemMeta"), "{:?}", hits);
        assert!(hits.len() <= 3, "{:?}", hits);
        assert!(!hits.contains(&"unrelated".to_string()), "{:?}", hits);
    }

    #[test]
    fn similar_names_ignores_fragments_too_short_to_mean_anything() {
        let pool: Vec<String> = ["a", "ab", "abcdef"].iter().map(|s| s.to_string()).collect();
        assert!(similar_names("ab", &pool).is_empty(), "a two-letter needle matches everything");
        let hits = similar_names("abc", &pool);
        assert_eq!(hits, vec!["abcdef".to_string()], "{:?}", hits);
    }

    #[test]
    fn nested_def_sites_find_each_kind_in_files_holding_the_name_and_skip_the_rest() {
        let d = std::env::temp_dir().join(format!("ctsq-nested-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.ts");
        let b = d.join("b.ts");
        std::fs::write(&a, "function outer() {\n  const deepName = () => 1;\n  function deepFn() {}\n}\n").unwrap();
        std::fs::write(&b, "function other() {\n  const x = 1;\n}\n").unwrap();
        let jobs = vec![
            (a.to_string_lossy().to_string(), "typescript"),
            (b.to_string_lossy().to_string(), "typescript"),
        ];
        let v = nested_def_sites("deepName", &jobs);
        assert_eq!(v, vec![(a.to_string_lossy().to_string(), 2)], "{:?}", v);
        let f = nested_def_sites("deepFn", &jobs);
        assert_eq!(f, vec![(a.to_string_lossy().to_string(), 3)], "{:?}", f);
        assert!(nested_def_sites("zzzqq", &jobs).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn similar_names_skips_the_exact_name() {
        let pool = vec!["helper".to_string()];
        assert!(similar_names("helper", &pool).is_empty());
    }
}


#[cfg(test)]
mod multi_name_tests {
    use super::*;
    use crate::files::Hider;

    fn jobs() -> Vec<(String, &'static str)> {
        vec![(format!("{}/fixtures/example.go", env!("CARGO_MANIFEST_DIR")), "go")]
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn def_answers_each_name_in_order_and_lists_the_miss_on_stderr() {
        let r = def_report(&names(&["NewServer", "Missing", "main"]), "x", &jobs(), &mut Hider::none(), None, None);
        let rows: Vec<&str> = r.out.lines().collect();
        assert_eq!(rows.len(), 2, "{}", r.out);
        assert!(rows[0].contains("example.go:14:"), "{}", r.out);
        assert!(rows[1].contains("example.go:18:"), "{}", r.out);
        assert_eq!(r.err, "not found: Missing\n");
        assert_eq!(r.code, 0);
    }

    #[test]
    fn def_with_one_missing_name_keeps_its_report_and_exits_1() {
        let r = def_report(&names(&["Missing"]), "somewhere", &jobs(), &mut Hider::none(), None, None);
        assert_eq!(r.out, "");
        assert!(r.err.starts_with("no definition for 'Missing' in somewhere (1 file)\n"), "{}", r.err);
        assert!(r.err.ends_with("not found: Missing\n"), "{}", r.err);
        assert_eq!(r.code, 1);
    }

    #[test]
    fn def_with_several_misses_exits_1_and_prints_only_the_stderr_line() {
        let r = def_report(&names(&["Nope", "Nada"]), "x", &jobs(), &mut Hider::none(), None, None);
        assert_eq!(r.out, "");
        assert_eq!(r.err, "not found: Nope, Nada\n");
        assert_eq!(r.code, 1);
    }

    #[test]
    fn a_near_name_is_offered_with_its_place() {
        let hint = similar_hint("newserver", &jobs()).unwrap();
        assert!(hint.starts_with("NewServer in "), "{}", hint);
        assert!(hint.ends_with("example.go:14"), "{}", hint);
        let line = not_found_line(&names(&["newserver", "Zed"]), &jobs());
        assert!(line.starts_with("not found: newserver (similar: NewServer in "), "{}", line);
        assert!(line.ends_with(", Zed\n"), "{}", line);
        assert_eq!(not_found_line(&[], &jobs()), "");
    }

    #[test]
    fn callers_report_many_skips_a_missing_name_and_still_exits_0() {
        let up = crate::calltree::Direction::Up;
        let r = crate::calltree::report_many(up, &names(&["NewServer", "Missing"]), "x", &jobs(), 3, &mut Hider::none(), None);
        assert!(r.out.starts_with("NewServer()\n"), "{}", r.out);
        assert!(!r.out.contains("Missing"), "{}", r.out);
        assert_eq!(r.err, "not found: Missing\n");
        assert_eq!(r.code, 0);
        let none = crate::calltree::report_many(up, &names(&["Missing"]), "x", &jobs(), 3, &mut Hider::none(), None);
        assert_eq!(none.out, "");
        assert!(none.err.starts_with("Missing()\n"), "{}", none.err);
        assert_eq!(none.code, 1);
    }

    #[test]
    fn callers_report_many_separates_names_with_a_blank_line() {
        let up = crate::calltree::Direction::Up;
        let r = crate::calltree::report_many(up, &names(&["NewServer", "Start"]), "x", &jobs(), 3, &mut Hider::none(), None);
        assert!(r.out.contains("\n\nStart()"), "{}", r.out);
        assert_eq!(r.err, "");
    }

    #[test]
    fn explain_report_many_answers_each_name_and_names_the_miss() {
        let opts = crate::explain::Options::default();
        let r = crate::explain::report_many(&names(&["NewServer", "Missing"]), "x", &jobs(), &opts, &mut Hider::none());
        assert!(r.out.starts_with("SYMBOL NewServer (1 definition)\n"), "{}", r.out);
        assert!(!r.out.contains("SYMBOL Missing"), "{}", r.out);
        assert_eq!(r.err, "not found: Missing\n");
        assert_eq!(r.code, 0);
        let one = crate::explain::report_many(&names(&["Missing"]), "x", &jobs(), &opts, &mut Hider::none());
        assert_eq!(one.out, "");
        assert!(one.err.starts_with("SYMBOL Missing (no definition resolved)\n"), "{}", one.err);
        assert_eq!(one.code, 1);
    }
}
