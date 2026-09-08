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

use std::collections::HashMap;

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
}

impl DefIndex {
    pub fn push(&mut self, def: Def) {
        self.by_name.entry(def.name.clone()).or_default().push(self.defs.len());
        self.defs.push(def);
    }
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

pub trait Resolver {
    /// Phase 1: definitions in one file (feeds the project index).
    fn collect_defs(&self, file: &str, source: &[u8]) -> Vec<Def>;
    /// Phase 1b: explicit implements-relations (`impl Trait for Type`).
    /// Languages with structural conformance (Go) don't need this.
    fn collect_impls(&self, _file: &str, _source: &[u8]) -> Vec<(String, String)> {
        vec![]
    }
    /// Phase 2: call sites in one file, resolved against the full project index.
    fn resolve_calls(&self, file: &str, source: &[u8], index: &DefIndex) -> Vec<ResolvedCall>;
}

pub fn resolver_for(lang_name: &str) -> Box<dyn Resolver> {
    match lang_name {
        "go" => Box::new(go::GoResolver::default()),
        "rust" => Box::new(rust::RustResolver::default()),
        _ => Box::new(fallback::NameResolver::new(lang_name)),
    }
}

// ── `ctsq def` ────────────────────────────────────────────────────────────────

/// Entry point for `ctsq def <name-or-expr> <path>`: print resolved
/// definition locations for a name, qualified path, or method expression.
///
/// Resolution runs the two steps `explain` runs, in the same order: the
/// call-graph definition index first, then the `tree` outline. The index only
/// holds callable definitions, so without the outline step a struct, class,
/// type alias or top-level variable resolved by `explain` reported nothing here.
pub fn run_def(expr: &str, path: &str, lang_name: Option<&str>, no_ignore: bool) {
    let jobs = crate::files::resolve_jobs(path, lang_name, no_ignore);
    let mut all_names: Vec<String> = Vec::new();
    let rows = def_rows(expr, &jobs, &mut all_names);

    if rows.is_empty() {
        println!(
            "no definition for '{}' in {} ({} files scanned)",
            expr,
            path,
            jobs.len()
        );
        let similar = similar_names(crate::explain::bare_name(expr), &all_names);
        if !similar.is_empty() {
            println!("similar names: {}", similar.join(", "));
        }
        std::process::exit(1);
    }

    for (file, line, conf) in rows {
        let line_text = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.lines().nth(line.saturating_sub(1)).map(|l| l.trim().to_string()))
            .unwrap_or_default();
        let mark = if conf == Confidence::Guess { "  ?" } else { "" };
        println!("{}:{}: {}{}", file, line, line_text, mark);
    }
}

/// Definition sites for `expr`, in the order `explain` resolves them: the
/// call-graph definition index first, then the `tree` outline for everything
/// the index cannot hold. `names_seen` collects every outline name met on the
/// way, which is the pool the miss report suggests from.
pub fn def_rows(
    expr: &str,
    jobs: &[(String, &'static str)],
    names_seen: &mut Vec<String>,
) -> Vec<(String, usize, Confidence)> {
    let mut index = DefIndex::default();
    for (file, lang) in jobs {
        let Ok(source) = std::fs::read(file) else { continue };
        for def in resolver_for(lang).collect_defs(file, &source) {
            index.push(def);
        }
    }

    let rows: Vec<(String, usize, Confidence)> = resolve_def_query(expr, &index)
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
    let bare = crate::explain::bare_name(expr);
    let mut found: Vec<(String, usize)> = Vec::new();
    for (file, lang) in jobs {
        for (_kind, name, line) in crate::tree::collect_def_locs(file, lang) {
            if name == bare {
                found.push((file.clone(), line));
            }
            names_seen.push(name);
        }
    }
    names_seen.extend(index.by_name.keys().cloned());
    found.sort();
    found.dedup();
    let conf = if found.len() == 1 { Confidence::Exact } else { Confidence::Guess };
    found.into_iter().map(|(f, l)| (f, l, conf)).collect()
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
                && low != needle
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

    #[test]
    fn def_finds_a_type_the_call_graph_index_cannot_hold() {
        // Server is a struct, so only the outline step resolves it. This is
        // the case that printed "no definition found" and exited 1.
        let mut names = Vec::new();
        let rows = def_rows("Server", &jobs_for("example.go"), &mut names);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert!(rows[0].0.ends_with("example.go"), "{:?}", rows);
        assert_eq!(rows[0].1, 5, "{:?}", rows);
    }

    #[test]
    fn def_agrees_with_explain_on_a_class() {
        let jobs = jobs_for("example.cpp");
        let mut names = Vec::new();
        let rows = def_rows("Buffer", &jobs, &mut names);
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
        let rows = def_rows("NewServer", &jobs_for("example.go"), &mut names);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert_eq!(rows[0].2, Confidence::Exact, "{:?}", rows);
    }

    #[test]
    fn a_miss_collects_the_names_it_could_have_meant() {
        let mut names = Vec::new();
        let rows = def_rows("NewServe", &jobs_for("example.go"), &mut names);
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
    fn similar_names_skips_the_exact_name() {
        let pool = vec!["helper".to_string()];
        assert!(similar_names("helper", &pool).is_empty());
    }
}
