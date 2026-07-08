//! Project-wide import graph: every source file indexed as a node, every
//! import statement resolved to the file (or, for Go, the package) it targets,
//! with forward/reverse adjacency for in-memory traversal.
//!
//! Mirrors the call graph (`resolve::graph`) but nodes are *files* (and Go
//! *packages*) rather than definitions. Import→file resolution is per-language
//! and lives behind the `ImportResolver` trait; Go's package model is handled
//! directly in `build`.

use super::{Confidence, DefIndex};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node, Parser};

// ── Types ─────────────────────────────────────────────────────────────────────

/// One import as written, before path resolution.
#[derive(Debug, Clone)]
pub struct RawImport {
    /// The specifier text with quotes/brackets stripped: "./foo", "react",
    /// "crate::a::b::Sym", "foo.h", "os.path".
    pub specifier: String,
    pub kind: ImportKind,
    /// Number of leading dots for Python relative imports (0 otherwise).
    pub dots: usize,
    /// 1-based line of the import statement.
    pub line: usize,
    /// Node label to use when this import stays external (None = use specifier).
    /// Lets Rust collapse `std::collections::HashMap` to just `std`.
    pub ext_label: Option<String>,
    /// When true, an unresolved import produces NO edge instead of an external
    /// node — used for intra-crate Rust `crate::`/`self`/`super` that we can't map.
    pub drop_if_unresolved: bool,
}

impl RawImport {
    fn new(specifier: impl Into<String>, kind: ImportKind, line: usize) -> Self {
        RawImport {
            specifier: specifier.into(), kind, dots: 0, line,
            ext_label: None, drop_if_unresolved: false,
        }
    }
    fn with_dots(mut self, d: usize) -> Self { self.dots = d; self }
    fn dropping(mut self) -> Self { self.drop_if_unresolved = true; self }
    fn ext(mut self, label: impl Into<String>) -> Self { self.ext_label = Some(label.into()); self }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// Path relative to the importing file (C `"..."`, JS `./`/`../`, Python leading-dot).
    Relative,
    /// Module/package path resolved against the project (Rust `crate::`, Python dotted, Go).
    Absolute,
    /// Never a local file (C `<...>`, bare JS specifier, external crate).
    System,
}

/// A scanned source file — a graph node.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Path exactly as it appeared in the job list (for output).
    pub display: String,
    /// `fs::canonicalize` result — the node's identity.
    pub canonical: String,
    pub lang: &'static str,
}

/// A Go package: a directory of `.go` files — a virtual graph node.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// Short display path, e.g. "internal/util".
    pub display: String,
    pub canonical_dir: String,
    /// Indices into `ImportGraph.files` of this package's member files.
    pub members: Vec<usize>,
}

/// Graph node identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    File(usize),      // index into ImportGraph.files
    Package(usize),   // index into ImportGraph.packages
    External(String), // unresolved / system specifier
}

/// Where an import resolves.
#[derive(Debug, Clone)]
pub enum ImportTarget {
    /// A single scanned file (C/JS/TS/Python/Rust).
    File(usize, Confidence),
    /// A Go package plus the specific member file(s) whose symbols the importer
    /// actually uses (empty = whole package, no resolvable sink).
    Package { package: usize, sinks: Vec<(usize, Confidence)> },
    /// External library or unresolved specifier — displayed, not a real node.
    External(String),
}

impl ImportTarget {
    fn node_key(&self) -> NodeKey {
        match self {
            ImportTarget::File(i, _) => NodeKey::File(*i),
            ImportTarget::Package { package, .. } => NodeKey::Package(*package),
            ImportTarget::External(s) => NodeKey::External(s.clone()),
        }
    }
}

/// One resolved import edge (mirrors `ResolvedCall`).
#[derive(Debug, Clone)]
pub struct ResolvedImport {
    /// Index into `files` of the importing (source) file.
    pub importer: usize,
    /// Specifier text, for display.
    pub specifier: String,
    pub line: usize,
    pub target: ImportTarget,
}

pub struct ImportGraph {
    pub files: Vec<FileInfo>,
    pub packages: Vec<PackageInfo>,
    /// canonical file path -> index into `files`
    pub by_canonical: HashMap<String, usize>,
    pub imports: Vec<ResolvedImport>,
    /// source NodeKey -> indices into `imports`
    pub fwd: HashMap<NodeKey, Vec<usize>>,
    /// target NodeKey -> indices into `imports`
    pub rev: HashMap<NodeKey, Vec<usize>>,
}

// ── Resolver trait ────────────────────────────────────────────────────────────

pub trait ImportResolver {
    /// Extract raw imports from one file.
    fn imports(&self, source: &[u8]) -> Vec<RawImport>;
    /// Resolve one raw import to zero+ canonical target file paths, tested
    /// against the scanned-file set. `importer` is the canonical path of the
    /// importing file. 0 = external/unresolved, 1 = Exact, >1 = ambiguous.
    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String>;
}

pub fn import_resolver_for(lang: &'static str) -> Box<dyn ImportResolver> {
    match lang {
        "c" | "cpp" => Box::new(CImports { lang }),
        "javascript" | "typescript" | "tsx" => Box::new(JsTsImports { lang }),
        "python" => Box::new(PyImports),
        "rust" => Box::new(RustImports),
        _ => Box::new(NoImports),
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

pub fn build(jobs: &[(String, &'static str)]) -> ImportGraph {
    // Pass 1: register every scanned file as a node.
    let mut files: Vec<FileInfo> = Vec::new();
    let mut by_canonical: HashMap<String, usize> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    for (path, lang) in jobs {
        if path == "-" { continue; } // stdin has no path — cannot be a node
        let canonical = canonical_path(path);
        if by_canonical.contains_key(&canonical) { continue; }
        let idx = files.len();
        by_canonical.insert(canonical.clone(), idx);
        known.insert(canonical.clone());
        files.push(FileInfo { display: path.clone(), canonical, lang });
    }

    // Pass 2: resolve imports per file.
    let mut imports: Vec<ResolvedImport> = Vec::new();
    let mut seen: HashSet<(usize, NodeKey)> = HashSet::new();

    // Go is handled separately (package nodes + sink resolution).
    let mut packages: Vec<PackageInfo> = Vec::new();
    let go_jobs: Vec<usize> = files.iter().enumerate()
        .filter(|(_, f)| f.lang == "go").map(|(i, _)| i).collect();

    // tsconfig/jsconfig path aliases (`~/…`, `@app/…`) for JS/TS files.
    let tsconf = super::tsconfig::TsConfig::build(&files);

    for (fi, info) in files.iter().enumerate() {
        if info.lang == "go" { continue; }
        let Ok(source) = std::fs::read(&info.display) else { continue };
        let importer = Path::new(&info.canonical);
        let resolver = import_resolver_for(info.lang);
        for raw in resolver.imports(&source) {
            // For non-relative JS/TS specifiers, prefer a governing tsconfig's
            // alias/baseUrl mapping; fall back to the resolver's suffix heuristic
            // only when no config applies.
            let targets = if is_jsts(info.lang) && raw.kind != ImportKind::Relative {
                tsconf.resolve(importer, &raw.specifier, &known)
                    .unwrap_or_else(|| resolver.resolve(&raw, importer, &known))
            } else {
                resolver.resolve(&raw, importer, &known)
            };
            push_edges(&mut imports, &mut seen, fi, &raw, &targets, &by_canonical);
        }
    }

    if !go_jobs.is_empty() {
        build_go(&go_jobs, &files, &by_canonical, &mut packages, &mut imports, &mut seen);
    }

    // Build adjacency.
    let mut fwd: HashMap<NodeKey, Vec<usize>> = HashMap::new();
    let mut rev: HashMap<NodeKey, Vec<usize>> = HashMap::new();
    for (i, imp) in imports.iter().enumerate() {
        fwd.entry(NodeKey::File(imp.importer)).or_default().push(i);
        rev.entry(imp.target.node_key()).or_default().push(i);
        // Go: also index reverse edges under each sink file, so `importers <file>`
        // finds package edges whose usage resolves into that specific file.
        if let ImportTarget::Package { sinks, .. } = &imp.target {
            for (s, _) in sinks {
                rev.entry(NodeKey::File(*s)).or_default().push(i);
            }
        }
    }
    for list in fwd.values_mut().chain(rev.values_mut()) {
        list.sort_by_key(|&i| (imports[i].line, imports[i].specifier.clone()));
        list.dedup();
    }

    ImportGraph { files, packages, by_canonical, imports, fwd, rev }
}

/// Turn resolved target paths into edges: 0 → External, 1 → Exact File,
/// >1 → one Guess File edge per candidate. Deduped per (importer, target-node).
fn push_edges(
    imports: &mut Vec<ResolvedImport>,
    seen: &mut HashSet<(usize, NodeKey)>,
    importer: usize,
    raw: &RawImport,
    targets: &[String],
    by_canonical: &HashMap<String, usize>,
) {
    let make = |target: ImportTarget| ResolvedImport {
        importer, specifier: raw.specifier.clone(), line: raw.line, target,
    };
    match targets.len() {
        0 => {
            if raw.drop_if_unresolved { return; } // intra-crate, unmappable — not a dependency
            let label = raw.ext_label.clone().unwrap_or_else(|| raw.specifier.clone());
            let t = ImportTarget::External(label);
            if seen.insert((importer, t.node_key())) { imports.push(make(t)); }
        }
        1 => {
            let idx = by_canonical[&targets[0]];
            if idx != importer {
                let t = ImportTarget::File(idx, Confidence::Exact);
                if seen.insert((importer, t.node_key())) { imports.push(make(t)); }
            }
        }
        _ => {
            for path in targets {
                let idx = by_canonical[path];
                if idx == importer { continue; } // a file importing itself is not an edge
                let t = ImportTarget::File(idx, Confidence::Guess);
                if seen.insert((importer, t.node_key())) { imports.push(make(t)); }
            }
        }
    }
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Canonical (symlink/`..`-resolved, absolute) path string. Falls back to a
/// lexical absolute path when the file can't be canonicalized.
pub(crate) fn canonical_path(p: &str) -> String {
    match std::fs::canonicalize(p) {
        Ok(pb) => pb.to_string_lossy().into_owned(),
        Err(_) => normalize_lexical(&absolutize(Path::new(p))).to_string_lossy().into_owned(),
    }
}

fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Fold `.` and `..` components without touching the filesystem (candidates may
/// not exist). Used to resolve `importer_dir + "../foo"` to a comparable string.
pub(crate) fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ── Tree-sitter helpers ───────────────────────────────────────────────────────

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// Depth-first visit every node, calling `f` on each.
fn walk<'a>(node: Node<'a>, f: &mut impl FnMut(Node<'a>)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, f);
    }
}

/// Strip a single pair of surrounding quotes (`"`, `'`, or backtick).
fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 {
        let (f, l) = (b[0], b[b.len() - 1]);
        if (f == b'"' && l == b'"') || (f == b'\'' && l == b'\'') || (f == b'`' && l == b'`') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

// ── C / C++ ───────────────────────────────────────────────────────────────────

struct CImports { lang: &'static str }

impl CImports {
    fn parse(&self, source: &[u8]) -> Option<tree_sitter::Tree> {
        let language = if self.lang == "cpp" {
            tree_sitter_cpp::LANGUAGE.into()
        } else {
            tree_sitter_c::LANGUAGE.into()
        };
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        parser.parse(source, None)
    }
}

impl ImportResolver for CImports {
    fn imports(&self, source: &[u8]) -> Vec<RawImport> {
        let Some(tree) = self.parse(source) else { return vec![] };
        let mut out = Vec::new();
        walk(tree.root_node(), &mut |n| {
            if n.kind() != "preproc_include" { return; }
            let Some(path) = n.child_by_field_name("path") else { return };
            let line = n.start_position().row + 1;
            match path.kind() {
                "string_literal" => out.push(
                    RawImport::new(unquote(text(path, source)), ImportKind::Relative, line)),
                "system_lib_string" => {
                    let raw = text(path, source).trim();
                    let spec = raw.trim_start_matches('<').trim_end_matches('>').to_string();
                    out.push(RawImport::new(spec, ImportKind::System, line));
                }
                _ => {}
            }
        });
        out
    }

    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String> {
        if imp.kind != ImportKind::Relative { return vec![]; }
        let dir = importer.parent().unwrap_or_else(|| Path::new("/"));
        let cand = normalize_lexical(&dir.join(&imp.specifier));
        let s = cand.to_string_lossy().into_owned();
        if known.contains(&s) { vec![s] } else { vec![] }
    }
}

// ── JavaScript / TypeScript ───────────────────────────────────────────────────

pub(crate) const JS_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

pub(crate) fn is_jsts(lang: &str) -> bool {
    matches!(lang, "javascript" | "typescript" | "tsx")
}

struct JsTsImports { lang: &'static str }

impl JsTsImports {
    fn parse(&self, source: &[u8]) -> Option<tree_sitter::Tree> {
        let language = match self.lang {
            "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_javascript::LANGUAGE.into(),
        };
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        parser.parse(source, None)
    }
}

fn classify_js(spec: &str) -> ImportKind {
    if spec.starts_with("./") || spec.starts_with("../") {
        ImportKind::Relative
    } else {
        ImportKind::System
    }
}

impl ImportResolver for JsTsImports {
    fn imports(&self, source: &[u8]) -> Vec<RawImport> {
        let Some(tree) = self.parse(source) else { return vec![] };
        let mut out = Vec::new();
        let mut push = |spec: String, line: usize| {
            let kind = classify_js(&spec);
            out.push(RawImport::new(spec, kind, line));
        };
        walk(tree.root_node(), &mut |n| {
            let line = n.start_position().row + 1;
            match n.kind() {
                // static import / re-export (`export ... from`)
                "import_statement" | "export_statement" => {
                    if let Some(src) = n.child_by_field_name("source") {
                        push(unquote(text(src, source)), line);
                    }
                }
                // require(...) and dynamic import(...)
                "call_expression" => {
                    let callee = n.child_by_field_name("function");
                    let is_import = callee.map(|c| {
                        c.kind() == "import"
                            || (c.kind() == "identifier" && text(c, source) == "require")
                    }).unwrap_or(false);
                    if !is_import { return; }
                    let Some(args) = n.child_by_field_name("arguments") else { return };
                    let mut cur = args.walk();
                    for arg in args.children(&mut cur) {
                        if arg.kind() == "string" {
                            push(unquote(text(arg, source)), line);
                            break;
                        }
                    }
                }
                _ => {}
            }
        });
        out
    }

    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String> {
        if imp.kind == ImportKind::Relative {
            let dir = importer.parent().unwrap_or_else(|| Path::new("/"));
            let base = normalize_lexical(&dir.join(&imp.specifier));
            let mut hits = Vec::new();
            let mut add = |cand: PathBuf| {
                let s = cand.to_string_lossy().into_owned();
                if known.contains(&s) && !hits.contains(&s) { hits.push(s); }
            };
            add(base.clone());                                    // literal (spec carried an extension)
            for ext in JS_EXTS { add(base.with_extension(ext)); } // `foo` -> `foo.{ext}`
            for ext in JS_EXTS { add(base.join("index").with_extension(ext)); } // `foo/index.{ext}`
            return hits;
        }
        // Non-relative: a tsconfig path alias (`~/…`, `@/…`, or a baseUrl bare
        // path) or a real package. Resolve by suffix-matching the path tail
        // against the scanned set — node_modules is gitignored, so package
        // specifiers don't accidentally match. Single-segment bare specifiers
        // (`react`) stay external.
        js_alias_resolve(&imp.specifier, known)
    }
}

/// Strip a leading alias sigil (`~`, `@`) and resolve the remaining path tail by
/// suffix-matching known files. Returns every match (1 = Exact, >1 = Guess).
fn js_alias_resolve(spec: &str, known: &HashSet<String>) -> Vec<String> {
    let had_alias = spec.starts_with('~') || spec.starts_with('@');
    let tail = spec.trim_start_matches(|c| c == '~' || c == '@').trim_start_matches('/');
    if tail.is_empty() { return vec![]; }
    // A bare single-segment specifier with no alias sigil is a package (`react`).
    if !had_alias && !tail.contains('/') { return vec![]; }
    let mut hits: Vec<String> = known.iter()
        .filter(|k| js_tail_matches(k, tail))
        .cloned()
        .collect();
    hits.sort();
    hits
}

/// Does `known_path` end with `<tail>.{ext}` or `<tail>/index.{ext}` on a path boundary?
fn js_tail_matches(known_path: &str, tail: &str) -> bool {
    JS_EXTS.iter().any(|ext| {
        known_path.ends_with(&format!("/{}.{}", tail, ext))
            || known_path.ends_with(&format!("/{}/index.{}", tail, ext))
    })
}

// ── Python ────────────────────────────────────────────────────────────────────

struct PyImports;

/// Join the `identifier` children of a `dotted_name` with ".".
fn dotted_text(node: Node, source: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut cur = node.walk();
    for ch in node.children(&mut cur) {
        if ch.kind() == "identifier" {
            parts.push(text(ch, source).to_string());
        }
    }
    parts.join(".")
}

/// The imported names of a `from … import a, b` statement (module submodules
/// for the `from . import x` case).
fn imported_names(node: Node, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = node.walk();
    for name in node.children_by_field_name("name", &mut cur) {
        let target = if name.kind() == "aliased_import" {
            name.child_by_field_name("name").unwrap_or(name)
        } else {
            name
        };
        let t = dotted_text(target, source);
        if !t.is_empty() { out.push(t); }
    }
    out
}

impl PyImports {
    fn parse(&self, source: &[u8]) -> Option<tree_sitter::Tree> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_python::LANGUAGE.into()).ok()?;
        parser.parse(source, None)
    }
}

impl ImportResolver for PyImports {
    fn imports(&self, source: &[u8]) -> Vec<RawImport> {
        let Some(tree) = self.parse(source) else { return vec![] };
        let mut out = Vec::new();
        walk(tree.root_node(), &mut |n| {
            let line = n.start_position().row + 1;
            match n.kind() {
                "import_statement" => {
                    let mut cur = n.walk();
                    for name in n.children_by_field_name("name", &mut cur) {
                        let target = if name.kind() == "aliased_import" {
                            name.child_by_field_name("name").unwrap_or(name)
                        } else {
                            name
                        };
                        let path = dotted_text(target, source);
                        if !path.is_empty() {
                            out.push(RawImport::new(path, ImportKind::Absolute, line));
                        }
                    }
                }
                "import_from_statement" => {
                    let Some(module) = n.child_by_field_name("module_name") else { return };
                    match module.kind() {
                        "dotted_name" => out.push(
                            RawImport::new(dotted_text(module, source), ImportKind::Absolute, line)),
                        "relative_import" => {
                            let mut dots = 0;
                            let mut mod_path = String::new();
                            let mut cur = module.walk();
                            for ch in module.children(&mut cur) {
                                match ch.kind() {
                                    "import_prefix" => {
                                        dots = text(ch, source).chars().filter(|c| *c == '.').count();
                                    }
                                    "dotted_name" => mod_path = dotted_text(ch, source),
                                    _ => {}
                                }
                            }
                            if mod_path.is_empty() {
                                // `from . import submod` — each imported name is a submodule.
                                for name in imported_names(n, source) {
                                    out.push(RawImport::new(name, ImportKind::Relative, line).with_dots(dots));
                                }
                            } else {
                                out.push(RawImport::new(mod_path, ImportKind::Relative, line).with_dots(dots));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        });
        out
    }

    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String> {
        let segs: Vec<&str> = imp.specifier.split('.').filter(|s| !s.is_empty()).collect();
        match imp.kind {
            ImportKind::Relative => {
                // one dot = importer's package (its dir); each extra dot pops a parent.
                let importer_dir = importer.parent().unwrap_or_else(|| Path::new("/"));
                let mut base = importer_dir.to_path_buf();
                for _ in 0..imp.dots.saturating_sub(1) {
                    base = base.parent().map(|p| p.to_path_buf()).unwrap_or(base);
                }
                for s in &segs { base.push(s); }
                let base = normalize_lexical(&base);
                py_probe(&base, known)
            }
            ImportKind::Absolute => {
                if segs.is_empty() { return vec![]; }
                let tail = segs.join("/");
                let suffix_mod = format!("/{}.py", tail);
                let suffix_pkg = format!("/{}/__init__.py", tail);
                let mut hits: Vec<String> = known.iter()
                    .filter(|k| k.ends_with(&suffix_mod) || k.ends_with(&suffix_pkg))
                    .cloned()
                    .collect();
                hits.sort();
                hits
            }
            ImportKind::System => vec![],
        }
    }
}

/// Probe `<base>.py` and `<base>/__init__.py` in the scanned set.
fn py_probe(base: &Path, known: &HashSet<String>) -> Vec<String> {
    let mut hits = Vec::new();
    let module = base.with_extension("py").to_string_lossy().into_owned();
    if known.contains(&module) { hits.push(module); }
    let pkg = base.join("__init__.py").to_string_lossy().into_owned();
    if known.contains(&pkg) { hits.push(pkg); }
    hits
}

// ── Rust ──────────────────────────────────────────────────────────────────────

struct RustImports;

/// Flatten a `use`-path node into segments, keeping `crate`/`self`/`super`
/// (unlike the call resolver, which drops them — we need the root to classify).
fn rust_flatten(node: Node, source: &[u8], out: &mut Vec<String>) {
    match node.kind() {
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(p) = node.child_by_field_name("path") { rust_flatten(p, source, out); }
            if let Some(n) = node.child_by_field_name("name") { out.push(text(n, source).to_string()); }
        }
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            out.push(text(node, source).to_string());
        }
        _ => {}
    }
}

/// Recursively collect `use`-tree paths (each a full segment vector, root kept).
fn rust_collect(node: Node, source: &[u8], prefix: &[String], out: &mut Vec<Vec<String>>) {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" => {
            let mut path = prefix.to_vec();
            rust_flatten(node, source, &mut path);
            if !path.is_empty() { out.push(path); }
        }
        "scoped_use_list" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.child_by_field_name("path") { rust_flatten(p, source, &mut path); }
            if let Some(list) = node.child_by_field_name("list") {
                let mut c = list.walk();
                for item in list.named_children(&mut c) { rust_collect(item, source, &path, out); }
            }
        }
        "use_list" => {
            let mut c = node.walk();
            for item in node.named_children(&mut c) { rust_collect(item, source, prefix, out); }
        }
        "use_as_clause" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.child_by_field_name("path") { rust_flatten(p, source, &mut path); }
            if !path.is_empty() { out.push(path); } // alias is irrelevant to file resolution
        }
        "use_wildcard" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.named_child(0) { rust_flatten(p, source, &mut path); }
            if !path.is_empty() { out.push(path); } // glob resolves to the module file
        }
        _ => {}
    }
}

/// Module path implied by a file's location (mirror of the call resolver's
/// `file_module_path`): components after the nearest `src/`, with
/// `lib`/`main`/`mod` contributing their directory only.
fn rust_module_of(file: &Path) -> Vec<String> {
    let comps: Vec<&str> = file.components().filter_map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    }).collect();
    let Some(src_i) = comps.iter().rposition(|c| *c == "src") else { return vec![] };
    let after = &comps[src_i + 1..];
    if after.is_empty() { return vec![]; }
    let mut mods: Vec<String> = after[..after.len() - 1].iter().map(|s| s.to_string()).collect();
    let stem = Path::new(after[after.len() - 1]).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    match stem {
        "lib" | "main" | "mod" | "" => {}
        other => mods.push(other.to_string()),
    }
    mods
}

/// The nearest ancestor directory named `src`.
fn rust_src_root(file: &Path) -> Option<PathBuf> {
    file.ancestors().find(|a| a.file_name().map(|n| n == "src").unwrap_or(false))
        .map(|p| p.to_path_buf())
}

/// Probe `<src>/<segs>.rs` and `<src>/<segs>/mod.rs` in the scanned set.
fn rust_probe(src_root: &Path, segs: &[String], known: &HashSet<String>) -> Vec<String> {
    if segs.is_empty() { return vec![]; }
    let mut base = src_root.to_path_buf();
    for s in segs { base.push(s); }
    let mut hits = Vec::new();
    let rs = base.with_extension("rs").to_string_lossy().into_owned();
    if known.contains(&rs) { hits.push(rs); }
    let modrs = base.join("mod.rs").to_string_lossy().into_owned();
    if known.contains(&modrs) { hits.push(modrs); }
    hits
}

impl RustImports {
    fn parse(&self, source: &[u8]) -> Option<tree_sitter::Tree> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).ok()?;
        parser.parse(source, None)
    }
}

impl ImportResolver for RustImports {
    fn imports(&self, source: &[u8]) -> Vec<RawImport> {
        let Some(tree) = self.parse(source) else { return vec![] };
        let mut out = Vec::new();
        walk(tree.root_node(), &mut |n| {
            if n.kind() != "use_declaration" { return; }
            let Some(arg) = n.child_by_field_name("argument") else { return };
            let line = n.start_position().row + 1;
            let mut paths = Vec::new();
            rust_collect(arg, source, &[], &mut paths);
            for p in paths {
                let raw = match p.first().map(|s| s.as_str()) {
                    // intra-crate: keep full path; drop (not "external") if unmappable
                    Some("crate") => RawImport::new(p.join("::"), ImportKind::Absolute, line).dropping(),
                    Some("self") | Some("super") =>
                        RawImport::new(p.join("::"), ImportKind::Relative, line).dropping(),
                    // bare root: full path enables internal probing; if it stays
                    // external, collapse the label to the crate name.
                    Some(root) => RawImport::new(p.join("::"), ImportKind::System, line).ext(root.to_string()),
                    None => continue,
                };
                out.push(raw);
            }
        });
        out
    }

    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String> {
        let segs: Vec<String> = imp.specifier.split("::")
            .filter(|s| !s.is_empty()).map(String::from).collect();
        if segs.is_empty() { return vec![]; }
        let Some(src_root) = rust_src_root(importer) else { return vec![] };
        let full: Vec<String> = match segs[0].as_str() {
            "crate" => segs[1..].to_vec(),
            "self" => {
                let mut m = rust_module_of(importer);
                m.extend(segs[1..].iter().cloned());
                m
            }
            "super" => {
                let supers = segs.iter().take_while(|s| s.as_str() == "super").count();
                let mut m = rust_module_of(importer);
                for _ in 0..supers { m.pop(); }
                m.extend(segs[supers..].iter().cloned());
                m
            }
            // bare root (crate-root `use module::…` or an external crate): probe
            // the full path as an internal module; misses fall through to external.
            _ => segs.clone(),
        };
        // A `use` names a symbol inside a module; try progressively shorter
        // prefixes (longest first) so both `use a::b` (b a module) and
        // `use a::b::Sym` (Sym a symbol in module b) map to the right file.
        for k in (1..=full.len()).rev() {
            let hits = rust_probe(&src_root, &full[..k], known);
            if !hits.is_empty() { return hits; }
        }
        vec![]
    }
}

// ── No-op (unsupported languages) ─────────────────────────────────────────────

struct NoImports;

impl ImportResolver for NoImports {
    fn imports(&self, _source: &[u8]) -> Vec<RawImport> { vec![] }
    fn resolve(&self, _imp: &RawImport, _dir: &Path, _known: &HashSet<String>) -> Vec<String> { vec![] }
}

// ── Go ────────────────────────────────────────────────────────────────────────
//
// A Go import names a package (a directory of files), so Go gets virtual
// `Package` nodes. Each import edge is annotated with the specific member files
// (sinks) whose symbols the importer actually uses, resolved through a Go
// definition index — making `importers <a_go_file>` meaningful.

struct GoImp { segs: Vec<String>, alias: String, line: usize }

fn parse_go(source: &[u8]) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
    parser.parse(source, None)
}

fn path_components(p: &str) -> Vec<String> {
    Path::new(p).components().filter_map(|c| match c {
        Component::Normal(s) => s.to_str().map(String::from),
        _ => None,
    }).collect()
}

fn common_suffix(a: &[String], b: &[String]) -> usize {
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
}

/// `import "a/b/c"` specs: path segments, local alias (last segment, or the
/// explicit alias / `.` / `_`), and line.
fn go_parse_imports(source: &[u8]) -> Vec<GoImp> {
    let Some(tree) = parse_go(source) else { return vec![] };
    let mut out = Vec::new();
    walk(tree.root_node(), &mut |n| {
        if n.kind() != "import_spec" { return; }
        let Some(path_node) = n.child_by_field_name("path") else { return };
        let segs: Vec<String> = text(path_node, source).trim_matches('"')
            .split('/').filter(|s| !s.is_empty()).map(String::from).collect();
        if segs.is_empty() { return; }
        let alias = match n.child_by_field_name("name") {
            Some(name) if name.kind() == "dot" => ".".to_string(),
            Some(name) if name.kind() == "blank_identifier" => "_".to_string(),
            Some(name) => text(name, source).to_string(),
            None => segs.last().unwrap().clone(),
        };
        out.push(GoImp { segs, alias, line: n.start_position().row + 1 });
    });
    out
}

/// `pkg.Symbol` references as (operand, field) pairs — used to pin sink files.
fn go_parse_refs(source: &[u8]) -> Vec<(String, String)> {
    let Some(tree) = parse_go(source) else { return vec![] };
    let mut out = Vec::new();
    walk(tree.root_node(), &mut |n| {
        if n.kind() != "selector_expression" { return; }
        let (Some(op), Some(field)) =
            (n.child_by_field_name("operand"), n.child_by_field_name("field")) else { return };
        if op.kind() == "identifier" {
            out.push((text(op, source).to_string(), text(field, source).to_string()));
        }
    });
    out
}

/// Member files of package `pidx` that define a symbol referenced through `alias`.
fn go_sinks(
    alias: &str,
    refs: &[(String, String)],
    members: &HashSet<usize>,
    index: &DefIndex,
    display_to_idx: &HashMap<&str, usize>,
    pkg_ambiguous: bool,
) -> Vec<(usize, Confidence)> {
    let mut sinks: Vec<usize> = Vec::new();
    for (a, sym) in refs {
        if a != alias { continue; }
        let Some(cands) = index.by_name.get(sym) else { continue };
        for &di in cands {
            if let Some(&fidx) = display_to_idx.get(index.defs[di].id.file.as_str()) {
                if members.contains(&fidx) && !sinks.contains(&fidx) {
                    sinks.push(fidx);
                }
            }
        }
    }
    sinks.sort();
    let conf = if pkg_ambiguous { Confidence::Guess } else { Confidence::Exact };
    sinks.into_iter().map(|i| (i, conf)).collect()
}

fn build_go(
    go_files: &[usize],
    files: &[FileInfo],
    by_canonical: &HashMap<String, usize>,
    packages: &mut Vec<PackageInfo>,
    imports: &mut Vec<ResolvedImport>,
    seen: &mut HashSet<(usize, NodeKey)>,
) {
    // 1. Group Go files into packages by canonical directory.
    let mut by_dir: HashMap<String, usize> = HashMap::new();
    let mut display_to_idx: HashMap<&str, usize> = HashMap::new();
    for &fi in go_files {
        display_to_idx.insert(files[fi].display.as_str(), fi);
        let dir = Path::new(&files[fi].canonical).parent()
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let pidx = *by_dir.entry(dir.clone()).or_insert_with(|| {
            let display = Path::new(&files[fi].display).parent()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| dir.clone());
            packages.push(PackageInfo { display, canonical_dir: dir.clone(), members: vec![] });
            packages.len() - 1
        });
        packages[pidx].members.push(fi);
    }
    let pkg_dirs: Vec<Vec<String>> = packages.iter().map(|p| path_components(&p.canonical_dir)).collect();
    let pkg_members: Vec<HashSet<usize>> =
        packages.iter().map(|p| p.members.iter().copied().collect()).collect();

    // 2. Build a Go definition index for sink resolution.
    let go_resolver = super::resolver_for("go");
    let mut index = DefIndex::default();
    for &fi in go_files {
        if let Ok(src) = std::fs::read(&files[fi].display) {
            for def in go_resolver.collect_defs(&files[fi].display, &src) {
                index.push(def);
            }
        }
    }

    // 3. Resolve each file's imports to package edges annotated with sinks.
    for &fi in go_files {
        let Ok(src) = std::fs::read(&files[fi].display) else { continue };
        let raws = go_parse_imports(&src);
        let refs = go_parse_refs(&src);
        for imp in raws {
            // Best-matching package by import-path/on-disk-dir shared suffix.
            let mut best = 0;
            let mut matches: Vec<usize> = Vec::new();
            for (pidx, dirs) in pkg_dirs.iter().enumerate() {
                let l = common_suffix(dirs, &imp.segs);
                if l > best { best = l; matches = vec![pidx]; }
                else if l == best && l > 0 { matches.push(pidx); }
            }
            let specifier = imp.segs.join("/");
            if best == 0 {
                // external package (stdlib or third-party, not in the scan set)
                let raw = RawImport::new(specifier, ImportKind::System, imp.line);
                push_edges(imports, seen, fi, &raw, &[], by_canonical);
                continue;
            }
            let pidx = matches[0];
            let sinks = go_sinks(&imp.alias, &refs, &pkg_members[pidx], &index,
                &display_to_idx, matches.len() > 1);
            let target = ImportTarget::Package { package: pidx, sinks };
            if seen.insert((fi, target.node_key())) {
                imports.push(ResolvedImport { importer: fi, specifier, line: imp.line, target });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lexical_folds_dotdot_and_dot() {
        let p = normalize_lexical(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
    }

    #[test]
    fn common_suffix_matches_shared_tail() {
        let dirs = vec!["tmp".into(), "demo".into(), "util".into()];
        let import = vec!["example.com".into(), "demo".into(), "util".into()];
        assert_eq!(common_suffix(&dirs, &import), 2);
        let none = vec!["x".into(), "y".into()];
        assert_eq!(common_suffix(&dirs, &none), 0);
    }
}
