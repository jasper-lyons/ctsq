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
    /// Local names this import binds in the importing file: `logger` for
    /// `import logger from 'pino'`, `os` for `import os.path`, `Map` for
    /// `use std::collections::HashMap as Map`. Empty when nothing is bound
    /// or the language does not report bindings (C, C++, Ruby, Go).
    pub bindings: Vec<String>,
    /// JS/TS only: each binding with the name it has in the target file and
    /// how it was imported. Empty for other languages and for side-effect imports.
    #[allow(dead_code)] // wired by B3
    pub import_bindings: Vec<ImportBinding>,
}

/// How a JS/TS import names what it takes from the target module.
#[allow(dead_code)] // wired by B3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `import {a}`, `import {a as b}`, `const {a} = require(m)`.
    Named,
    /// `import a from m`, `import {default as a} from m`.
    Default,
    /// `import * as ns from m`, `const ns = require(m)`.
    Namespace,
}

/// One local name bound by a JS/TS import: `imported` is the name in the
/// target module (`"default"` for a default import, `"*"` for a namespace).
#[allow(dead_code)] // wired by B3
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub local: String,
    pub imported: String,
    pub kind: BindingKind,
}

impl RawImport {
    fn new(specifier: impl Into<String>, kind: ImportKind, line: usize) -> Self {
        RawImport {
            specifier: specifier.into(), kind, dots: 0, line,
            ext_label: None, drop_if_unresolved: false, bindings: Vec::new(),
            import_bindings: Vec::new(),
        }
    }
    fn bind_detail(mut self, b: Vec<ImportBinding>) -> Self { self.import_bindings = b; self }
    fn with_dots(mut self, d: usize) -> Self { self.dots = d; self }
    fn dropping(mut self) -> Self { self.drop_if_unresolved = true; self }
    fn ext(mut self, label: impl Into<String>) -> Self { self.ext_label = Some(label.into()); self }
    fn bind(mut self, names: Vec<String>) -> Self { self.bindings = names; self }
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
        "ruby" => Box::new(RubyImports),
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

/// Resolves a JS/TS module specifier (`./pages/Home`, `~/routes/library`) to
/// the scanned files it can name, with the same rules the import graph uses:
/// relative probing with the usual extensions and `index.*`, then tsconfig
/// aliases, then the suffix match.
pub struct JsSpecResolver {
    known: HashSet<String>,
    display_of: HashMap<String, String>,
    tsconf: super::tsconfig::TsConfig,
}

impl JsSpecResolver {
    pub fn new(jobs: &[(String, &'static str)]) -> JsSpecResolver {
        let mut files: Vec<FileInfo> = Vec::new();
        let mut known = HashSet::new();
        let mut display_of = HashMap::new();
        for (path, lang) in jobs {
            if path == "-" { continue; }
            let canonical = canonical_path(path);
            if !known.insert(canonical.clone()) { continue; }
            display_of.insert(canonical.clone(), path.clone());
            files.push(FileInfo { display: path.clone(), canonical, lang });
        }
        let tsconf = super::tsconfig::TsConfig::build(&files);
        JsSpecResolver { known, display_of, tsconf }
    }

    /// The scanned files (as listed in the jobs) `spec` names from `importer`.
    pub fn resolve(&self, importer: &str, spec: &str) -> Vec<String> {
        let imp = canonical_path(importer);
        let importer = Path::new(&imp);
        let raw = RawImport::new(spec, classify_js(spec), 0);
        let resolver = JsTsImports { lang: "typescript" };
        let hits = if raw.kind == ImportKind::Relative {
            resolver.resolve(&raw, importer, &self.known)
        } else {
            self.tsconf
                .resolve(importer, spec, &self.known)
                .unwrap_or_else(|| resolver.resolve(&raw, importer, &self.known))
        };
        hits.into_iter().filter_map(|h| self.display_of.get(&h).cloned()).collect()
    }
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
        let mut push = |spec: String, line: usize, bindings: Vec<String>, detail: Vec<ImportBinding>| {
            let kind = classify_js(&spec);
            out.push(RawImport::new(spec, kind, line).bind(bindings).bind_detail(detail));
        };
        walk(tree.root_node(), &mut |n| {
            let line = n.start_position().row + 1;
            match n.kind() {
                // static import / re-export (`export ... from`)
                "import_statement" | "export_statement" => {
                    if let Some(src) = n.child_by_field_name("source") {
                        push(unquote(text(src, source)), line, js_import_bindings(n, source),
                            js_import_detail(n, source));
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
                            push(unquote(text(arg, source)), line, js_require_bindings(n, source),
                                js_require_detail(n, source));
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

/// Local names bound by an `import` statement: the default import, the
/// namespace import, and each named import (its alias when it has one).
fn js_import_bindings(stmt: Node, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = stmt.walk();
    for clause in stmt.named_children(&mut cur) {
        if clause.kind() != "import_clause" { continue; }
        let mut c = clause.walk();
        for part in clause.named_children(&mut c) {
            match part.kind() {
                "identifier" => out.push(text(part, source).to_string()),
                "namespace_import" => {
                    let mut pc = part.walk();
                    if let Some(id) = part.named_children(&mut pc).find(|x| x.kind() == "identifier") {
                        out.push(text(id, source).to_string());
                    }
                }
                "named_imports" => {
                    let mut pc = part.walk();
                    for spec in part.named_children(&mut pc) {
                        if spec.kind() != "import_specifier" { continue; }
                        let local = spec.child_by_field_name("alias")
                            .or_else(|| spec.child_by_field_name("name"));
                        if let Some(id) = local {
                            out.push(unquote(text(id, source)));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Local names bound by `const x = require('m')` or
/// `const { a, b: c } = require('m')`. A require whose value is not assigned
/// straight to a declarator binds nothing.
fn js_require_bindings(call: Node, source: &[u8]) -> Vec<String> {
    let mut node = call;
    // `await import('m')` wraps the call once.
    if let Some(p) = node.parent().filter(|p| p.kind() == "await_expression") {
        node = p;
    }
    let Some(decl) = node.parent().filter(|p| p.kind() == "variable_declarator") else {
        return vec![];
    };
    let Some(name) = decl.child_by_field_name("name") else { return vec![] };
    let mut out = Vec::new();
    match name.kind() {
        "identifier" => out.push(text(name, source).to_string()),
        "object_pattern" => {
            let mut c = name.walk();
            for prop in name.named_children(&mut c) {
                match prop.kind() {
                    "shorthand_property_identifier_pattern" => out.push(text(prop, source).to_string()),
                    "pair_pattern" => {
                        if let Some(v) = prop.child_by_field_name("value").filter(|v| v.kind() == "identifier") {
                            out.push(text(v, source).to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// Parse JS/TS source with the grammar for `lang` ("javascript", "typescript", "tsx").
#[allow(dead_code)] // wired by B3
pub fn js_parse(lang: &'static str, source: &[u8]) -> Option<tree_sitter::Tree> {
    JsTsImports { lang }.parse(source)
}

/// Like `js_import_bindings`, but keeps the imported name and the kind.
fn js_import_detail(stmt: Node, source: &[u8]) -> Vec<ImportBinding> {
    let mut out = Vec::new();
    let mut cur = stmt.walk();
    for clause in stmt.named_children(&mut cur) {
        if clause.kind() != "import_clause" { continue; }
        let mut c = clause.walk();
        for part in clause.named_children(&mut c) {
            match part.kind() {
                "identifier" => out.push(ImportBinding {
                    local: text(part, source).to_string(),
                    imported: "default".to_string(),
                    kind: BindingKind::Default,
                }),
                "namespace_import" => {
                    let mut pc = part.walk();
                    if let Some(id) = part.named_children(&mut pc).find(|x| x.kind() == "identifier") {
                        out.push(ImportBinding {
                            local: text(id, source).to_string(),
                            imported: "*".to_string(),
                            kind: BindingKind::Namespace,
                        });
                    }
                }
                "named_imports" => {
                    let mut pc = part.walk();
                    for spec in part.named_children(&mut pc) {
                        if spec.kind() != "import_specifier" { continue; }
                        let Some(name) = spec.child_by_field_name("name") else { continue };
                        let imported = unquote(text(name, source));
                        let local = spec.child_by_field_name("alias")
                            .map(|a| unquote(text(a, source)))
                            .unwrap_or_else(|| imported.clone());
                        let kind = if imported == "default" { BindingKind::Default } else { BindingKind::Named };
                        out.push(ImportBinding { local, imported, kind });
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Like `js_require_bindings`, but keeps the imported name and the kind:
/// `const x = require(m)` is a namespace, `const {a, b: c} = require(m)` is named.
fn js_require_detail(call: Node, source: &[u8]) -> Vec<ImportBinding> {
    let mut node = call;
    if let Some(p) = node.parent().filter(|p| p.kind() == "await_expression") {
        node = p;
    }
    let Some(decl) = node.parent().filter(|p| p.kind() == "variable_declarator") else {
        return vec![];
    };
    let Some(name) = decl.child_by_field_name("name") else { return vec![] };
    let mut out = Vec::new();
    match name.kind() {
        "identifier" => out.push(ImportBinding {
            local: text(name, source).to_string(),
            imported: "*".to_string(),
            kind: BindingKind::Namespace,
        }),
        "object_pattern" => {
            let mut c = name.walk();
            for prop in name.named_children(&mut c) {
                match prop.kind() {
                    "shorthand_property_identifier_pattern" => {
                        let n = text(prop, source).to_string();
                        out.push(ImportBinding { local: n.clone(), imported: n, kind: BindingKind::Named });
                    }
                    "pair_pattern" => {
                        let key = prop.child_by_field_name("key");
                        let val = prop.child_by_field_name("value").filter(|v| v.kind() == "identifier");
                        if let (Some(k), Some(v)) = (key, val) {
                            out.push(ImportBinding {
                                local: text(v, source).to_string(),
                                imported: unquote(text(k, source)),
                                kind: BindingKind::Named,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// One `export ... from S` clause. `exported`/`imported` are both None for
/// `export * from S`.
#[allow(dead_code)] // wired by B3
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReExport {
    pub specifier: String,
    /// The name this file exposes (`y` in `export {x as y} from S`).
    pub exported: Option<String>,
    /// The name taken from S (`x`; `"default"` for `export {default as X}`).
    pub imported: Option<String>,
}

/// What one JS/TS file exports.
#[allow(dead_code)] // wired by B3
#[derive(Debug, Clone, Default)]
pub struct ExportTable {
    /// Exported name -> the local name that defines it in this file.
    pub named: HashMap<String, String>,
    /// True when the file has a default export, named or not.
    pub has_default: bool,
    /// The default export's local name; None when anonymous.
    pub default_local: Option<String>,
    pub reexports: Vec<ReExport>,
}

fn js_default_name(node: Node, source: &[u8]) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(text(node, source).to_string());
    }
    node.child_by_field_name("name").map(|n| text(n, source).to_string())
}

fn js_decl_names(decl: Node, source: &[u8]) -> Vec<String> {
    match decl.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut c = decl.walk();
            decl.named_children(&mut c)
                .filter(|d| d.kind() == "variable_declarator")
                .filter_map(|d| d.child_by_field_name("name"))
                .filter(|n| n.kind() == "identifier")
                .map(|n| text(n, source).to_string())
                .collect()
        }
        _ => decl.child_by_field_name("name")
            .map(|n| vec![text(n, source).to_string()])
            .unwrap_or_default(),
    }
}

/// The export table of one parsed JS/TS file. Only top-level
/// `export` statements count.
#[allow(dead_code)] // wired by B3
pub fn js_exports(tree: &tree_sitter::Tree, source: &[u8]) -> ExportTable {
    let mut t = ExportTable::default();
    let root = tree.root_node();
    let mut cur = root.walk();
    for st in root.named_children(&mut cur) {
        if st.kind() != "export_statement" { continue; }
        let from = st.child_by_field_name("source").map(|s| unquote(text(s, source)));
        let is_default = {
            let mut c = st.walk();
            st.children(&mut c).any(|x| x.kind() == "default")
        };
        let (clause, ns_export) = {
            let mut c = st.walk();
            let kids: Vec<Node> = st.named_children(&mut c).collect();
            (
                kids.iter().find(|k| k.kind() == "export_clause").copied(),
                kids.iter().any(|k| k.kind() == "namespace_export"),
            )
        };
        if let Some(clause) = clause {
            let mut c = clause.walk();
            for spec in clause.named_children(&mut c) {
                if spec.kind() != "export_specifier" { continue; }
                let Some(name) = spec.child_by_field_name("name") else { continue };
                let name = unquote(text(name, source));
                let exported = spec.child_by_field_name("alias")
                    .map(|a| unquote(text(a, source)))
                    .unwrap_or_else(|| name.clone());
                match &from {
                    Some(s) => t.reexports.push(ReExport {
                        specifier: s.clone(), exported: Some(exported), imported: Some(name),
                    }),
                    None => {
                        if exported == "default" {
                            t.has_default = true;
                            t.default_local = Some(name.clone());
                        }
                        t.named.insert(exported, name);
                    }
                }
            }
        } else if let Some(s) = &from {
            // `export * from S`; `export * as ns from S` names a namespace, not a symbol.
            if !ns_export {
                t.reexports.push(ReExport { specifier: s.clone(), exported: None, imported: None });
            }
        } else if let Some(decl) = st.child_by_field_name("declaration") {
            if is_default {
                t.has_default = true;
                t.default_local = js_default_name(decl, source);
            } else {
                for n in js_decl_names(decl, source) { t.named.insert(n.clone(), n); }
            }
        } else if is_default {
            t.has_default = true;
            t.default_local = st.child_by_field_name("value").and_then(|v| js_default_name(v, source));
        }
    }
    t
}

/// The file and local name that define export `name` of `file`, following
/// re-exports through `r`. `file` and every path `table_of` is asked about are
/// display paths as `JsSpecResolver::resolve` returns them. `"default"` asks
/// for the default export; an anonymous default comes back as `(file,
/// "default")`. A re-export whose specifier names more than one file ends that
/// branch with no answer. At most 8 files deep, cycles end the branch.
#[allow(dead_code)] // wired by B3
pub fn resolve_export<'t>(
    r: &JsSpecResolver,
    table_of: impl Fn(&str) -> Option<&'t ExportTable>,
    file: &str,
    name: &str,
) -> Option<(String, String)> {
    let mut visited = HashSet::new();
    export_walk(r, &table_of, file, name, 1, &mut visited)
}

fn export_walk<'t>(
    r: &JsSpecResolver,
    table_of: &impl Fn(&str) -> Option<&'t ExportTable>,
    file: &str,
    name: &str,
    depth: usize,
    visited: &mut HashSet<(String, String)>,
) -> Option<(String, String)> {
    if depth > 8 { return None; }
    if !visited.insert((file.to_string(), name.to_string())) { return None; }
    let table = table_of(file)?;
    let follow = |spec: &str, wanted: &str, visited: &mut HashSet<(String, String)>| {
        let mut hits = r.resolve(file, spec);
        if hits.len() != 1 { return None; }
        export_walk(r, table_of, &hits.remove(0), wanted, depth + 1, visited)
    };
    if name == "default" {
        if table.has_default {
            let local = table.default_local.clone().unwrap_or_else(|| "default".to_string());
            return Some((file.to_string(), local));
        }
    } else if let Some(local) = table.named.get(name) {
        return Some((file.to_string(), local.clone()));
    }
    for re in &table.reexports {
        if re.exported.as_deref() == Some(name) {
            if let Some(hit) = follow(&re.specifier, re.imported.as_deref().unwrap_or(name), visited) {
                return Some(hit);
            }
        }
    }
    // `export *` never carries the default export.
    if name != "default" {
        for re in table.reexports.iter().filter(|re| re.exported.is_none()) {
            if let Some(hit) = follow(&re.specifier, name, visited) { return Some(hit); }
        }
    }
    None
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

/// Local names bound by a `from … import a, b as c` statement: `a` and `c`.
fn py_from_bindings(node: Node, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = node.walk();
    for name in node.children_by_field_name("name", &mut cur) {
        let local = if name.kind() == "aliased_import" {
            name.child_by_field_name("alias").map(|a| text(a, source).to_string())
        } else {
            Some(dotted_text(name, source))
        };
        if let Some(l) = local.filter(|l| !l.is_empty()) { out.push(l); }
    }
    out
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
                            // `import a.b` binds `a`, `import a.b as c` binds `c`.
                            let local = if name.kind() == "aliased_import" {
                                name.child_by_field_name("alias")
                                    .map(|a| text(a, source).to_string())
                                    .unwrap_or_default()
                            } else {
                                path.split('.').next().unwrap_or("").to_string()
                            };
                            let raw = RawImport::new(path, ImportKind::Absolute, line);
                            out.push(if local.is_empty() { raw } else { raw.bind(vec![local]) });
                        }
                    }
                }
                "import_from_statement" => {
                    let Some(module) = n.child_by_field_name("module_name") else { return };
                    let bound = py_from_bindings(n, source);
                    match module.kind() {
                        "dotted_name" => out.push(
                            RawImport::new(dotted_text(module, source), ImportKind::Absolute, line)
                                .bind(bound)),
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
                                for (i, name) in imported_names(n, source).into_iter().enumerate() {
                                    let local = bound.get(i).cloned().into_iter().collect();
                                    out.push(RawImport::new(name, ImportKind::Relative, line)
                                        .with_dots(dots).bind(local));
                                }
                            } else {
                                out.push(RawImport::new(mod_path, ImportKind::Relative, line)
                                    .with_dots(dots).bind(bound));
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

// ── Ruby ──────────────────────────────────────────────────────────────────────
//
// Ruby has no import statement. `require`, `require_relative` and `load` are
// ordinary method calls, so the resolver looks for calls by name and reads the
// string literal argument. A call with a computed argument is skipped, since
// there is no path to resolve.

struct RubyImports;

impl RubyImports {
    fn parse(&self, source: &[u8]) -> Option<tree_sitter::Tree> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_ruby::LANGUAGE.into()).ok()?;
        parser.parse(source, None)
    }
}

/// The literal text of the first string argument, or None when the call takes
/// something that is not a plain string.
fn ruby_string_arg(call: Node, source: &[u8]) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let mut cur = args.walk();
    let first = args.named_children(&mut cur).next()?;
    if first.kind() != "string" {
        return None;
    }
    let inner = first.named_children(&mut first.walk())
        .find(|c| c.kind() == "string_content")
        .map(|c| text(c, source).to_string())
        .unwrap_or_else(|| unquote(text(first, source)));
    if inner.is_empty() { None } else { Some(inner) }
}

/// `active_support/core_ext` is a file inside the `active_support` gem, so an
/// unresolved require collapses to the gem name the way Rust collapses
/// `std::collections::HashMap` to `std`.
fn ruby_gem_label(spec: &str) -> String {
    spec.split('/').next().unwrap_or(spec).to_string()
}

impl ImportResolver for RubyImports {
    fn imports(&self, source: &[u8]) -> Vec<RawImport> {
        let Some(tree) = self.parse(source) else { return vec![] };
        let mut out = Vec::new();
        walk(tree.root_node(), &mut |n| {
            if n.kind() != "call" { return; }
            let Some(method) = n.child_by_field_name("method") else { return };
            let line = n.start_position().row + 1;
            let spec = match text(method, source) {
                // `require_relative "x"` names a file next to this one.
                "require_relative" => {
                    let Some(s) = ruby_string_arg(n, source) else { return };
                    out.push(RawImport::new(s, ImportKind::Relative, line));
                    return;
                }
                // `load "x.rb"` also takes a path, and in practice a relative one.
                "load" => {
                    let Some(s) = ruby_string_arg(n, source) else { return };
                    out.push(RawImport::new(s, ImportKind::Relative, line));
                    return;
                }
                "require" => ruby_string_arg(n, source),
                _ => return,
            };
            // `require "x"` searches the load path, so it is either a project
            // file reached through `lib/` or a gem.
            if let Some(s) = spec {
                let label = ruby_gem_label(&s);
                out.push(RawImport::new(s, ImportKind::Absolute, line).ext(label));
            }
        });
        out
    }

    fn resolve(&self, imp: &RawImport, importer: &Path, known: &HashSet<String>) -> Vec<String> {
        match imp.kind {
            ImportKind::Relative => {
                let dir = importer.parent().unwrap_or_else(|| Path::new("/"));
                let joined = normalize_lexical(&dir.join(&imp.specifier));
                let mut path = joined.to_string_lossy().into_owned();
                if !path.ends_with(".rb") {
                    path.push_str(".rb");
                }
                if known.contains(&path) { vec![path] } else { vec![] }
            }
            ImportKind::Absolute => {
                let spec = imp.specifier.trim_end_matches(".rb");
                if spec.is_empty() { return vec![]; }
                let suffix = format!("/{}.rb", spec);
                let mut hits: Vec<String> = known.iter()
                    .filter(|k| k.ends_with(&suffix))
                    .cloned()
                    .collect();
                hits.sort();
                hits
            }
            ImportKind::System => vec![],
        }
    }
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

/// The local name a plain `use` path binds: its last segment, or the segment
/// before a trailing `self` (`use a::b::{self}` binds `b`).
fn rust_binding(path: &[String]) -> Option<String> {
    let last = path.last()?;
    if last == "self" {
        return path.len().checked_sub(2).map(|i| path[i].clone());
    }
    if matches!(last.as_str(), "crate" | "super") { return None; }
    Some(last.clone())
}

/// Recursively collect `use`-tree paths (each a full segment vector, root kept)
/// with the local name each one binds.
fn rust_collect(node: Node, source: &[u8], prefix: &[String], out: &mut Vec<(Vec<String>, Option<String>)>) {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" => {
            let mut path = prefix.to_vec();
            rust_flatten(node, source, &mut path);
            let bound = rust_binding(&path);
            if !path.is_empty() { out.push((path, bound)); }
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
            // the alias is irrelevant to file resolution but is the local binding
            let bound = node.child_by_field_name("alias").map(|a| text(a, source).to_string());
            if !path.is_empty() { out.push((path, bound)); }
        }
        "use_wildcard" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.named_child(0) { rust_flatten(p, source, &mut path); }
            if !path.is_empty() { out.push((path, None)); } // glob resolves to the module file
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
            for (p, bound) in paths {
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
                out.push(raw.bind(bound.into_iter().collect()));
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

    fn bindings_of(lang: &'static str, src: &str) -> Vec<(String, Vec<String>)> {
        import_resolver_for(lang).imports(src.as_bytes()).into_iter()
            .map(|r| (r.specifier, r.bindings))
            .collect()
    }

    fn b(spec: &str, names: &[&str]) -> (String, Vec<String>) {
        (spec.to_string(), names.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn js_imports_report_local_bindings() {
        let got = bindings_of("javascript", "import pino from 'pino'\nimport * as ns from './ns'\nimport { a, b as c } from 'm'\nimport 'side'\nconst logger = require('winston')\nconst { x, y: z } = require('obj')\n");
        assert_eq!(got, vec![
            b("pino", &["pino"]), b("./ns", &["ns"]), b("m", &["a", "c"]), b("side", &[]),
            b("winston", &["logger"]), b("obj", &["x", "z"]),
        ]);
    }

    #[test]
    fn python_imports_report_local_bindings() {
        let got = bindings_of("python", "import os.path\nimport numpy as np\nfrom log import info, warn as w\nfrom . import sib\n");
        assert_eq!(got, vec![
            b("os.path", &["os"]), b("numpy", &["np"]), b("log", &["info", "w"]), b("sib", &["sib"]),
        ]);
    }

    #[test]
    fn rust_imports_report_local_bindings() {
        let got = bindings_of("rust", "use std::collections::HashMap;\nuse std::fmt::{self, Write as W};\nuse crate::a::*;\n");
        assert_eq!(got, vec![
            b("std::collections::HashMap", &["HashMap"]), b("std::fmt::self", &["fmt"]),
            b("std::fmt::Write", &["W"]), b("crate::a", &[]),
        ]);
    }

    // ── import bindings and export tables over fixtures/ts-resolve/imports ──

    fn fx_dir() -> String {
        format!("{}/fixtures/ts-resolve/imports", env!("CARGO_MANIFEST_DIR"))
    }
    fn fx(rel: &str) -> String { format!("{}/{}", fx_dir(), rel) }

    fn fx_files(dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { fx_files(&p, out); }
            else if p.extension().map(|x| x == "ts").unwrap_or(false) {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }

    fn fx_setup() -> (JsSpecResolver, HashMap<String, ExportTable>) {
        let mut files = Vec::new();
        fx_files(Path::new(&fx_dir()), &mut files);
        files.sort();
        let jobs: Vec<(String, &'static str)> = files.iter().map(|f| (f.clone(), "typescript")).collect();
        let mut tables = HashMap::new();
        for f in &files {
            let src = std::fs::read(f).unwrap();
            let tree = js_parse("typescript", &src).unwrap();
            tables.insert(f.clone(), js_exports(&tree, &src));
        }
        (JsSpecResolver::new(&jobs), tables)
    }

    fn export_of(rel_file: &str, name: &str) -> Option<(String, String)> {
        let (r, tables) = fx_setup();
        resolve_export(&r, |f| tables.get(f), &fx(rel_file), name)
            .map(|(f, l)| (f.strip_prefix(&format!("{}/", fx_dir())).unwrap().to_string(), l))
    }

    fn some(f: &str, l: &str) -> Option<(String, String)> { Some((f.to_string(), l.to_string())) }

    fn detail_of(rel: &str) -> Vec<(String, Vec<(String, String, BindingKind)>)> {
        let src = std::fs::read(fx(rel)).unwrap();
        import_resolver_for("typescript").imports(&src).into_iter()
            .map(|r| (r.specifier, r.import_bindings.into_iter()
                .map(|b| (b.local, b.imported, b.kind)).collect()))
            .collect()
    }

    fn ib(l: &str, i: &str, k: BindingKind) -> (String, String, BindingKind) {
        (l.to_string(), i.to_string(), k)
    }

    #[test]
    fn import_detail_named_aliased_default_namespace() {
        let got = detail_of("src/main.ts");
        assert_eq!(got[0], ("./api".to_string(), vec![ib("listAssets", "listAssets", BindingKind::Named)]));
        assert_eq!(got[1], ("~/api".to_string(), vec![ib("o", "orig", BindingKind::Named)]));
        assert_eq!(got[2], ("./widget".to_string(), vec![ib("Widget", "default", BindingKind::Default)]));
        assert_eq!(got[3], ("./barrel1".to_string(), vec![ib("deep", "*", BindingKind::Namespace)]));
        assert_eq!(got[4], ("side".to_string(), vec![]));
    }

    #[test]
    fn import_detail_require_namespace_and_destructure() {
        let got = detail_of("src/main.ts");
        assert_eq!(got[5], ("./api".to_string(), vec![ib("whole", "*", BindingKind::Namespace)]));
        assert_eq!(got[6], ("./api".to_string(), vec![
            ib("la", "listAssets", BindingKind::Named), ib("orig", "orig", BindingKind::Named)]));
    }

    #[test]
    fn export_table_reads_every_form() {
        let (_, tables) = fx_setup();
        let api = &tables[&fx("src/api.ts")];
        assert_eq!(api.named.get("listAssets").map(|s| s.as_str()), Some("listAssets"));
        assert_eq!(api.named.get("k2").map(|s| s.as_str()), Some("k2"));
        assert_eq!(api.named.get("Thing").map(|s| s.as_str()), Some("Thing"));
        assert_eq!(api.named.get("k1alias").map(|s| s.as_str()), Some("k1"));
        assert!(!api.has_default);
        let w = &tables[&fx("src/widget.ts")];
        assert!(w.has_default);
        assert_eq!(w.default_local.as_deref(), Some("Widget"));
        let a = &tables[&fx("src/anon.ts")];
        assert!(a.has_default);
        assert_eq!(a.default_local, None);
        let b = &tables[&fx("src/barrel1.ts")];
        assert_eq!(b.reexports, vec![ReExport {
            specifier: "./barrel2".into(), exported: None, imported: None }]);
    }

    #[test]
    fn resolve_export_named_and_aliased() {
        assert_eq!(export_of("src/api.ts", "listAssets"), some("src/api.ts", "listAssets"));
        assert_eq!(export_of("src/api.ts", "k1alias"), some("src/api.ts", "k1"));
        assert_eq!(export_of("src/api.ts", "missing"), None);
    }

    #[test]
    fn resolve_export_default() {
        assert_eq!(export_of("src/widget.ts", "default"), some("src/widget.ts", "Widget"));
    }

    #[test]
    fn resolve_export_star_through_two_barrels() {
        assert_eq!(export_of("src/barrel1.ts", "deepFn"), some("src/deep.ts", "deepFn"));
        assert_eq!(export_of("src/barrel1.ts", "default"), None);
    }

    #[test]
    fn resolve_export_renamed_reexport() {
        assert_eq!(export_of("src/renamed.ts", "shiny"), some("src/api.ts", "orig"));
        assert_eq!(export_of("src/renamed.ts", "orig"), None);
    }

    #[test]
    fn resolve_export_default_as_named_reexport() {
        assert_eq!(export_of("src/defas.ts", "Gadget"), some("src/widget.ts", "Widget"));
    }

    #[test]
    fn resolve_export_anonymous_default() {
        assert_eq!(export_of("src/anon.ts", "default"), some("src/anon.ts", "default"));
    }

    #[test]
    fn resolve_export_cycle_ends() {
        assert_eq!(export_of("src/cycle_a.ts", "nothing"), None);
    }

    #[test]
    fn resolve_export_through_tsconfig_alias() {
        let (r, _) = fx_setup();
        let hits = r.resolve(&fx("src/main.ts"), "~/api");
        assert_eq!(hits, vec![fx("src/api.ts")]);
        let (r, tables) = fx_setup();
        let got = resolve_export(&r, |f| tables.get(f), &hits[0], "orig");
        assert_eq!(got, Some((fx("src/api.ts"), "orig".to_string())));
    }

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
