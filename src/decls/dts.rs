//! TypeScript declarations read from `.d.ts` files under `node_modules`.
//!
//! `resolve_package` finds a package's declaration entry (STAGE-D D03),
//! `lookup` finds an exported name from an entry, following re-exports, and
//! `member` finds a member of a type, walking `extends` chains. Each `.d.ts`
//! file is parsed at most once per process and held in memory (D01).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tree_sitter::Node;

use super::TypeRef;

/// How many re-export hops `lookup` follows before giving up.
const MAX_HOPS: usize = 8;
/// How many `extends` steps `member` walks before giving up.
const MAX_BASES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DtsKind {
    Function,
    Class,
    Interface,
    TypeAlias,
    Var,
    Namespace,
    Module,
    Method,
    Property,
}

/// One declaration, with every overload or merged declaration of its name in
/// the file it was found in. `ret[i]` is the return type of `sigs[i]`, or the
/// declared type for a property or var, or the aliased type for a type alias.
/// Interfaces, classes and namespaces carry their header as the signature and
/// `void` as the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DtsDecl {
    pub name: String,
    pub kind: DtsKind,
    pub sigs: Vec<String>,
    pub ret: Vec<TypeRef>,
    pub file: PathBuf,
    pub line: usize,
}

impl DtsDecl {
    /// `node_modules/solid-js/types/reactive/signal.d.ts:8` (D05).
    pub fn location(&self) -> String {
        format!("{}:{}", short_path(&self.file), self.line)
    }
}

/// The path from its last `node_modules` component on, or the whole path when
/// it has none.
pub fn short_path(path: &Path) -> String {
    let comps: Vec<_> = path.components().collect();
    match comps.iter().rposition(|c| c.as_os_str() == "node_modules") {
        Some(i) => comps[i..].iter().collect::<PathBuf>().display().to_string(),
        None => path.display().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Package resolution (D03)

/// The `.d.ts` entry for `specifier` imported from a file in `from_dir`.
/// Relative specifiers map `./x.js` (or `./x`) to `./x.d.ts`, then
/// `./x/index.d.ts`. Bare specifiers are looked up in `node_modules` from
/// `from_dir` upward, through `exports` (with subpaths and single-`*`
/// patterns), `types`/`typings`, `main`, then `@types/<name>`.
pub fn resolve_package(from_dir: &Path, specifier: &str) -> Option<PathBuf> {
    if is_relative(specifier) {
        return rel_dts(&normalize(&from_dir.join(specifier)));
    }
    let (name, sub) = split_specifier(specifier)?;
    for dir in from_dir.ancestors() {
        let nm = dir.join("node_modules");
        let pkg = nm.join(&name);
        if pkg.is_dir() {
            if let Some(p) = package_entry(&pkg, &sub) {
                return Some(normalize(&p));
            }
        }
        let types = nm.join("@types").join(name.trim_start_matches('@').replace('/', "__"));
        if types.is_dir() {
            if let Some(p) = package_entry(&types, &sub) {
                return Some(normalize(&p));
            }
        }
    }
    None
}

/// `node_modules/typescript/lib/lib.dom.d.ts`, when it exists. Only checks the
/// path: nothing is parsed until a lookup asks for a name in it.
pub fn lib_dom(node_modules: &Path) -> Option<PathBuf> {
    let p = node_modules.join("typescript").join("lib").join("lib.dom.d.ts");
    p.is_file().then_some(p)
}

/// A global DOM name (`Element`, `fetch`) declared in `lib.dom.d.ts`.
pub fn lookup_global(node_modules: &Path, name: &str) -> Option<DtsDecl> {
    lookup(&lib_dom(node_modules)?, name)
}

fn is_relative(spec: &str) -> bool {
    spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/')
}

/// `@scope/name/sub/x` -> (`@scope/name`, `./sub/x`), `solid-js` -> (`solid-js`, `.`).
fn split_specifier(spec: &str) -> Option<(String, String)> {
    let mut parts = spec.splitn(if spec.starts_with('@') { 3 } else { 2 }, '/');
    let name = if spec.starts_with('@') {
        format!("{}/{}", parts.next()?, parts.next()?)
    } else {
        parts.next()?.to_string()
    };
    if name.is_empty() {
        return None;
    }
    let sub = match parts.next() {
        Some(rest) if !rest.is_empty() => format!("./{rest}"),
        _ => ".".to_string(),
    };
    Some((name, sub))
}

fn package_entry(pkg: &Path, sub: &str) -> Option<PathBuf> {
    let json = std::fs::read_to_string(pkg.join("package.json"))
        .ok()
        .and_then(|s| Json::parse(&s))
        .unwrap_or(Json::Null);
    if let Some(exports) = json.get("exports") {
        if let Some(p) = exports_entry(pkg, exports, sub) {
            return Some(p);
        }
    }
    if sub != "." {
        let p = pkg.join(&sub[2..]);
        if let Some(d) = rel_dts(&p) {
            return Some(d);
        }
        if p.join("package.json").is_file() {
            return package_entry(&p, ".");
        }
        return None;
    }
    for field in ["types", "typings"] {
        if let Some(t) = json.get(field).and_then(Json::as_str) {
            let p = pkg.join(t);
            if p.is_file() {
                return Some(p);
            }
            if let Some(d) = rel_dts(&p) {
                return Some(d);
            }
        }
    }
    let main = json.get("main").and_then(Json::as_str).unwrap_or("index.js");
    let p = pkg.join(format!("{}.d.ts", strip_js_ext(main)));
    if p.is_file() {
        return Some(p);
    }
    let p = pkg.join("index.d.ts");
    p.is_file().then_some(p)
}

/// The target of `sub` in an `exports` field: an exact key, else the pattern
/// with a single `*` and the longest prefix that matches.
fn exports_entry(pkg: &Path, exports: &Json, sub: &str) -> Option<PathBuf> {
    let accept = |target: &str, star: &str| dts_for(&pkg.join(target.replace('*', star)));
    match exports {
        Json::Obj(entries) if entries.iter().any(|(k, _)| k.starts_with('.')) => {
            if let Some((_, v)) = entries.iter().find(|(k, _)| k == sub) {
                return walk_conditions(v, &|t| accept(t, ""));
            }
            let mut best: Option<(usize, &Json, String)> = None;
            for (k, v) in entries {
                if k.matches('*').count() != 1 {
                    continue;
                }
                let (pre, post) = k.split_once('*')?;
                if sub.len() >= pre.len() + post.len() && sub.starts_with(pre) && sub.ends_with(post) {
                    let star = sub[pre.len()..sub.len() - post.len()].to_string();
                    if best.as_ref().is_none_or(|(n, _, _)| pre.len() > *n) {
                        best = Some((pre.len(), v, star));
                    }
                }
            }
            let (_, v, star) = best?;
            walk_conditions(v, &|t| accept(t, &star))
        }
        _ if sub == "." => walk_conditions(exports, &|t| accept(t, "")),
        _ => None,
    }
}

/// Walks a conditions value: `types`, `import`, `default`, then the other
/// conditions in object order. The first target `accept` takes wins.
fn walk_conditions(v: &Json, accept: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    match v {
        Json::Str(s) => accept(s),
        Json::Arr(items) => items.iter().find_map(|i| walk_conditions(i, accept)),
        Json::Obj(entries) => {
            for first in ["types", "import", "default"] {
                if let Some((_, v)) = entries.iter().find(|(k, _)| k == first) {
                    if let Some(p) = walk_conditions(v, accept) {
                        return Some(p);
                    }
                }
            }
            entries
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "types" | "import" | "default"))
                .find_map(|(_, v)| walk_conditions(v, accept))
        }
        _ => None,
    }
}

fn is_dts(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.ends_with(".d.ts") || s.ends_with(".d.mts") || s.ends_with(".d.cts")
}

/// A declaration file for an `exports` or `types` target: the file itself when
/// it is one, else the `.d.ts` beside it.
fn dts_for(p: &Path) -> Option<PathBuf> {
    if is_dts(p) {
        return p.is_file().then(|| p.to_path_buf());
    }
    rel_dts(p)
}

fn strip_js_ext(s: &str) -> &str {
    for ext in [".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx"] {
        if let Some(stem) = s.strip_suffix(ext) {
            return stem;
        }
    }
    s
}

/// `x.js` or `x` -> `x.d.ts`, then `x/index.d.ts`. A `.d.ts` path is kept.
fn rel_dts(p: &Path) -> Option<PathBuf> {
    if is_dts(p) && p.is_file() {
        return Some(p.to_path_buf());
    }
    let s = p.to_string_lossy();
    let stem = strip_js_ext(&s);
    let mut cands = Vec::new();
    if s.ends_with(".mjs") || s.ends_with(".mts") {
        cands.push(PathBuf::from(format!("{stem}.d.mts")));
    }
    if s.ends_with(".cjs") || s.ends_with(".cts") {
        cands.push(PathBuf::from(format!("{stem}.d.cts")));
    }
    cands.push(PathBuf::from(format!("{stem}.d.ts")));
    cands.push(Path::new(stem).join("index.d.ts"));
    cands.into_iter().find(|c| c.is_file())
}

/// Folds `.` and `..` components without touching the file system.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// The bare specifier of the package a file sits in: the component after the
/// last `node_modules` (two for a scope), with `@types/scope__name` read back
/// as `@scope/name`. None outside `node_modules` and for `typescript`'s libs.
fn package_of(path: &Path) -> Option<String> {
    let comps: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let i = comps.iter().rposition(|c| c == "node_modules")?;
    let first = comps.get(i + 1)?;
    if first == "typescript" {
        return None;
    }
    if first == "@types" {
        let n = comps.get(i + 2)?;
        return Some(match n.split_once("__") {
            Some((scope, name)) => format!("@{scope}/{name}"),
            None => n.clone(),
        });
    }
    if first.starts_with('@') {
        return Some(format!("{first}/{}", comps.get(i + 2)?));
    }
    Some(first.clone())
}

/// The `node_modules` directory holding a file, for global names it uses.
fn node_modules_of(path: &Path) -> Option<PathBuf> {
    let comps: Vec<_> = path.components().collect();
    let i = comps.iter().rposition(|c| c.as_os_str() == "node_modules")?;
    Some(comps[..=i].iter().collect())
}

// ---------------------------------------------------------------------------
// The per-file index

struct Sig {
    text: String,
    /// Unqualified: `pkg` is filled in by `qualify` when a decl is returned.
    ret: TypeRef,
    /// Type parameters in scope: the owner's and the signature's own.
    tparams: Vec<String>,
    line: usize,
}

struct Item {
    name: String,
    kind: DtsKind,
    exported: bool,
    sigs: Vec<Sig>,
    /// Members of an interface, class, object-literal alias part or var type.
    members: Vec<Item>,
    /// Type names `member` walks on a miss: `extends`, `implements`, the named
    /// parts of an intersection alias, a var's declared type.
    bases: Vec<String>,
    /// False for a type alias whose shape is neither an object type nor an
    /// intersection: `member` stops there.
    memberable: bool,
    /// The body of a namespace.
    body: Option<Scope>,
}

#[derive(Default)]
struct Scope {
    items: Vec<Item>,
    /// `export { local as exported } [from "src"]`
    reexports: Vec<(String, String, Option<String>)>,
    /// `export * from "src"`
    stars: Vec<String>,
    /// `export * as alias from "src"`
    star_as: Vec<(String, String)>,
    /// `export = id`
    export_eq: Option<String>,
    /// `declare module "name" { … }`
    modules: Vec<(String, Scope)>,
}

struct DtsFile {
    path: PathBuf,
    pkg: Option<String>,
    top: Scope,
    /// local name -> (specifier, imported name, `*` for a namespace import)
    imports: HashMap<String, (String, String)>,
    /// Every name declared anywhere in the file.
    declared: HashSet<String>,
}

impl DtsFile {
    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    fn scope(&self, steps: &[Step]) -> &Scope {
        let mut s = &self.top;
        for st in steps {
            s = match *st {
                Step::Module(i) => &s.modules[i].1,
                Step::Ns(i) => s.items[i].body.as_ref().expect("namespace body"),
            };
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Step {
    Module(usize),
    Ns(usize),
}

type Cache = Mutex<HashMap<PathBuf, Option<Arc<DtsFile>>>>;

fn cache() -> &'static Cache {
    static FILES: OnceLock<Cache> = OnceLock::new();
    FILES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The parsed index of a `.d.ts` file, parsed once per process.
fn load(path: &Path) -> Option<Arc<DtsFile>> {
    let path = normalize(path);
    let mut files = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = files.get(&path) {
        return f.clone();
    }
    let f = index_file(&path).map(Arc::new);
    files.insert(path, f.clone());
    f
}

fn index_file(path: &Path) -> Option<DtsFile> {
    let src = std::fs::read(path).ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).ok()?;
    let tree = parser.parse(&src, None)?;
    let mut ix = Indexer { src: &src, imports: HashMap::new(), declared: HashSet::new() };
    let top = ix.scope(tree.root_node());
    Some(DtsFile {
        path: path.to_path_buf(),
        pkg: package_of(path),
        top,
        imports: ix.imports,
        declared: ix.declared,
    })
}

struct Indexer<'a> {
    src: &'a [u8],
    imports: HashMap<String, (String, String)>,
    declared: HashSet<String>,
}

fn named_children(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

fn line_of(n: Node) -> usize {
    n.start_position().row + 1
}

/// Collapses runs of whitespace to one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unquote(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
}

impl<'a> Indexer<'a> {
    fn text(&self, n: Node) -> &'a str {
        n.utf8_text(self.src).unwrap_or("")
    }

    /// The node's text up to its body (or whole), squashed, without a trailing `;`.
    fn head(&self, n: Node) -> String {
        let end = n.child_by_field_name("body").map_or(n.end_byte(), |b| b.start_byte());
        let s = std::str::from_utf8(&self.src[n.start_byte()..end]).unwrap_or("");
        squash(s.trim().trim_end_matches(';'))
    }

    fn string_arg(&self, n: Node) -> Option<String> {
        n.child_by_field_name("source").map(|s| unquote(self.text(s)))
    }

    fn scope(&mut self, block: Node) -> Scope {
        let mut s = Scope::default();
        for n in named_children(block) {
            self.statement(n, &mut s);
        }
        s
    }

    fn statement(&mut self, n: Node, s: &mut Scope) {
        match n.kind() {
            "export_statement" => self.export(n, s),
            "import_statement" => self.import(n),
            "ambient_declaration" => {
                for c in named_children(n) {
                    match c.kind() {
                        // `declare global { … }`: globals, reachable by name.
                        "statement_block" => {
                            let g = self.scope(c);
                            s.items.extend(g.items);
                        }
                        _ => self.declaration(c, false, s),
                    }
                }
            }
            "expression_statement" => {
                for c in named_children(n) {
                    if c.kind() == "internal_module" {
                        self.declaration(c, false, s);
                    }
                }
            }
            _ => self.declaration(n, false, s),
        }
    }

    fn export(&mut self, n: Node, s: &mut Scope) {
        let mut c = n.walk();
        let tokens: Vec<&str> = n.children(&mut c).filter(|k| !k.is_named()).map(|k| k.kind()).collect();
        if let Some(d) = n.child_by_field_name("declaration") {
            let before = s.items.len();
            self.declaration(d, true, s);
            if tokens.contains(&"default") {
                if let Some(it) = s.items.get(before) {
                    s.reexports.push((it.name.clone(), "default".into(), None));
                }
            }
            return;
        }
        let source = self.string_arg(n);
        let children = named_children(n);
        if let Some(clause) = children.iter().find(|c| c.kind() == "export_clause") {
            for spec in named_children(*clause) {
                let Some(name) = spec.child_by_field_name("name") else { continue };
                let local = unquote(self.text(name));
                let exported = spec.child_by_field_name("alias").map_or(local.clone(), |a| unquote(self.text(a)));
                s.reexports.push((local, exported, source.clone()));
            }
            return;
        }
        if let Some(ns) = children.iter().find(|c| c.kind() == "namespace_export") {
            if let (Some(id), Some(src)) = (named_children(*ns).first(), source) {
                s.star_as.push((unquote(self.text(*id)), src));
            }
            return;
        }
        if let Some(src) = source {
            s.stars.push(src);
            return;
        }
        let value = children.iter().find(|c| c.kind() != "comment");
        if tokens.contains(&"=") {
            s.export_eq = value.map(|v| self.text(*v).to_string());
        } else if tokens.contains(&"default") {
            if let Some(v) = value.filter(|v| v.kind() == "identifier") {
                s.reexports.push((self.text(*v).to_string(), "default".into(), None));
            }
        }
    }

    fn import(&mut self, n: Node) {
        let Some(src) = self.string_arg(n) else { return };
        for clause in named_children(n).into_iter().filter(|c| c.kind() == "import_clause") {
            for c in named_children(clause) {
                match c.kind() {
                    "identifier" => {
                        self.imports.insert(self.text(c).to_string(), (src.clone(), "default".into()));
                    }
                    "namespace_import" => {
                        if let Some(id) = named_children(c).first() {
                            self.imports.insert(self.text(*id).to_string(), (src.clone(), "*".into()));
                        }
                    }
                    "named_imports" => {
                        for spec in named_children(c) {
                            let Some(name) = spec.child_by_field_name("name") else { continue };
                            let imported = unquote(self.text(name));
                            let local =
                                spec.child_by_field_name("alias").map_or(imported.clone(), |a| self.text(a).to_string());
                            self.imports.insert(local, (src.clone(), imported));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn tparams(&self, n: Node) -> Vec<String> {
        n.child_by_field_name("type_parameters")
            .map(|tp| {
                named_children(tp)
                    .into_iter()
                    .filter_map(|p| p.child_by_field_name("name").map(|x| self.text(x).to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn item(&mut self, name: String, kind: DtsKind, exported: bool, sig: Sig) -> Item {
        self.declared.insert(name.clone());
        Item { name, kind, exported, sigs: vec![sig], members: vec![], bases: vec![], memberable: true, body: None }
    }

    fn sig(&self, n: Node, ret: TypeRef, tparams: Vec<String>) -> Sig {
        Sig { text: self.head(n), ret, tparams, line: line_of(n) }
    }

    fn declaration(&mut self, d: Node, exported: bool, s: &mut Scope) {
        let name_of = |ix: &Self, n: Node| n.child_by_field_name("name").map(|x| unquote(ix.text(x)));
        match d.kind() {
            "ambient_declaration" => {
                for c in named_children(d) {
                    self.declaration(c, exported, s);
                }
            }
            "function_signature" | "function_declaration" | "generator_function_declaration" => {
                let Some(name) = name_of(self, d) else { return };
                let tp = self.tparams(d);
                let ret = self.return_type(d);
                let sig = self.sig(d, ret, tp);
                let it = self.item(name, DtsKind::Function, exported, sig);
                s.items.push(it);
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                let Some(name) = name_of(self, d) else { return };
                let tp = self.tparams(d);
                let sig = self.sig(d, void(), tp.clone());
                let mut it = self.item(name, DtsKind::Class, exported, sig);
                for c in named_children(d).into_iter().filter(|c| c.kind() == "class_heritage") {
                    for clause in named_children(c) {
                        match clause.kind() {
                            "extends_clause" => {
                                if let Some(v) = clause.child_by_field_name("value") {
                                    it.bases.push(self.text(v).to_string());
                                }
                            }
                            "implements_clause" => {
                                for t in named_children(clause) {
                                    it.bases.push(self.base_name(t));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(body) = d.child_by_field_name("body") {
                    it.members = self.members(body, &tp);
                }
                s.items.push(it);
            }
            "interface_declaration" => {
                let Some(name) = name_of(self, d) else { return };
                let tp = self.tparams(d);
                let sig = self.sig(d, void(), tp.clone());
                let mut it = self.item(name, DtsKind::Interface, exported, sig);
                for c in named_children(d).into_iter().filter(|c| c.kind() == "extends_type_clause") {
                    for t in named_children(c) {
                        it.bases.push(self.base_name(t));
                    }
                }
                if let Some(body) = d.child_by_field_name("body") {
                    it.members = self.members(body, &tp);
                }
                s.items.push(it);
            }
            "type_alias_declaration" => {
                let Some(name) = name_of(self, d) else { return };
                let tp = self.tparams(d);
                let value = d.child_by_field_name("value");
                let ret = value.map_or_else(void, |v| self.type_ref(v));
                let sig = self.sig(d, ret, tp.clone());
                let mut it = self.item(name, DtsKind::TypeAlias, exported, sig);
                let mut v = value;
                while let Some(p) = v.filter(|p| p.kind() == "parenthesized_type") {
                    v = named_children(p).first().copied();
                }
                it.memberable = match v {
                    Some(v) if matches!(v.kind(), "object_type" | "intersection_type") => {
                        self.alias_parts(v, &tp, &mut it.members, &mut it.bases)
                    }
                    _ => false,
                };
                s.items.push(it);
            }
            "lexical_declaration" | "variable_declaration" => {
                let kw = d.child(0).map_or("var", |k| self.text(k)).to_string();
                for decl in named_children(d).into_iter().filter(|c| c.kind() == "variable_declarator") {
                    let Some(name) = name_of(self, decl) else { continue };
                    let ty = decl.child_by_field_name("type").and_then(|t| named_children(t).first().copied());
                    let ret = ty.map_or_else(|| named("any"), |t| self.type_ref(t));
                    let sig = Sig { text: squash(&format!("{kw} {}", self.text(decl))), ret, tparams: vec![], line: line_of(decl) };
                    let mut it = self.item(name, DtsKind::Var, exported, sig);
                    if let Some(t) = ty {
                        match t.kind() {
                            "object_type" => it.members = self.members(t, &[]),
                            "type_identifier" | "generic_type" | "nested_type_identifier" => {
                                it.bases.push(self.base_name(t))
                            }
                            _ => {}
                        }
                    }
                    s.items.push(it);
                }
            }
            "internal_module" | "module" => {
                let Some(n) = d.child_by_field_name("name") else { return };
                let body = d.child_by_field_name("body").map(|b| self.scope(b)).unwrap_or_default();
                if n.kind() == "string" {
                    s.modules.push((unquote(self.text(n)), body));
                    return;
                }
                let name = self.text(n).to_string();
                let sig = self.sig(d, void(), vec![]);
                let mut it = self.item(name, DtsKind::Namespace, exported, sig);
                it.body = Some(body);
                s.items.push(it);
            }
            _ => {}
        }
    }

    /// Flattens an object type or intersection into members and base names.
    /// False when a part has another shape.
    fn alias_parts(&mut self, n: Node, tp: &[String], members: &mut Vec<Item>, bases: &mut Vec<String>) -> bool {
        match n.kind() {
            "object_type" => {
                members.extend(self.members(n, tp));
                true
            }
            "intersection_type" => {
                named_children(n).into_iter().all(|c| self.alias_parts(c, tp, members, bases))
            }
            "parenthesized_type" => {
                named_children(n).first().is_some_and(|c| self.alias_parts(*c, tp, members, bases))
            }
            "type_identifier" | "generic_type" | "nested_type_identifier" => {
                bases.push(self.base_name(n));
                true
            }
            _ => false,
        }
    }

    /// Methods and properties of an interface body, class body or object type.
    /// Call, construct and index signatures are skipped.
    fn members(&mut self, body: Node, owner_tp: &[String]) -> Vec<Item> {
        let mut out = Vec::new();
        for m in named_children(body) {
            let kind = match m.kind() {
                "method_signature" | "method_definition" | "abstract_method_signature" => DtsKind::Method,
                "property_signature" | "public_field_definition" => DtsKind::Property,
                _ => continue,
            };
            let Some(n) = m.child_by_field_name("name") else { continue };
            if n.kind() == "computed_property_name" {
                continue;
            }
            let name = unquote(self.text(n));
            let mut tp = owner_tp.to_vec();
            tp.extend(self.tparams(m));
            let ret = if kind == DtsKind::Method {
                self.return_type(m)
            } else {
                m.child_by_field_name("type")
                    .and_then(|t| named_children(t).first().copied())
                    .map_or_else(|| named("any"), |t| self.type_ref(t))
            };
            let sig = self.sig(m, ret, tp);
            out.push(Item {
                name,
                kind,
                exported: true,
                sigs: vec![sig],
                members: vec![],
                bases: vec![],
                memberable: true,
                body: None,
            });
        }
        out
    }

    fn return_type(&self, n: Node) -> TypeRef {
        n.child_by_field_name("return_type")
            .map(|r| if r.kind() == "type_annotation" { named_children(r).first().copied() } else { Some(r) })
            .and_then(|t| t)
            .map_or_else(|| named("any"), |t| self.type_ref(t))
    }

    /// `J<T>` -> `J`, `a.B` -> `a.B`.
    fn base_name(&self, n: Node) -> String {
        let n = if n.kind() == "generic_type" { n.child_by_field_name("name").unwrap_or(n) } else { n };
        squash(self.text(n))
    }

    /// An unqualified `TypeRef` for a type node (see `TypeRef`'s contract).
    fn type_ref(&self, n: Node) -> TypeRef {
        match n.kind() {
            "type_annotation" | "parenthesized_type" => match named_children(n).first() {
                Some(c) => self.type_ref(*c),
                None => text_ref(self.text(n)),
            },
            "type_identifier" | "predefined_type" | "nested_type_identifier" | "identifier" => named(&squash(self.text(n))),
            "generic_type" => {
                let name = n.child_by_field_name("name").map_or(String::new(), |x| squash(self.text(x)));
                let elems = n
                    .child_by_field_name("type_arguments")
                    .map(|a| named_children(a).into_iter().map(|t| self.type_ref(t)).collect())
                    .unwrap_or_default();
                TypeRef { pkg: None, name, pointer: false, elems }
            }
            "array_type" => {
                let elems = named_children(n).first().map(|t| vec![self.type_ref(*t)]).unwrap_or_default();
                TypeRef { pkg: None, name: "[]".into(), pointer: false, elems }
            }
            _ => text_ref(self.text(n)),
        }
    }
}

fn named(name: &str) -> TypeRef {
    TypeRef { pkg: None, name: name.to_string(), pointer: false, elems: vec![] }
}

fn void() -> TypeRef {
    named("void")
}

fn text_ref(s: &str) -> TypeRef {
    named(&squash(s))
}

fn is_type_path(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|seg| {
            let mut cs = seg.chars();
            cs.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
                && cs.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
}

/// Fills in `pkg` on every named type: the package of the file when the head
/// of the name is declared in it or imported relatively, the imported
/// package's name when imported from one, None for type parameters, builtins
/// and globals.
fn qualify(t: &TypeRef, f: &DtsFile, tparams: &[String]) -> TypeRef {
    let elems = t.elems.iter().map(|e| qualify(e, f, tparams)).collect();
    let pkg = if t.name == "[]" || !is_type_path(&t.name) {
        None
    } else {
        let head = t.name.split('.').next().unwrap_or("");
        if tparams.iter().any(|p| p == head) {
            None
        } else if let Some((spec, _)) = f.imports.get(head) {
            if is_relative(spec) {
                f.pkg.clone()
            } else {
                split_specifier(spec).map(|(n, _)| n)
            }
        } else if f.declared.contains(head) {
            f.pkg.clone()
        } else {
            None
        }
    };
    TypeRef { pkg, name: t.name.clone(), pointer: t.pointer, elems }
}

// ---------------------------------------------------------------------------
// Lookup

/// Where a name was found: the items named `name` in a scope of a file, or a
/// whole module (`export * as name`, `import * as name`).
enum Found {
    Items { file: Arc<DtsFile>, steps: Vec<Step>, name: String },
    Module { file: Arc<DtsFile>, name: String },
}

/// The declaration exported as `name` from the `.d.ts` file `entry`. Follows
/// `export {..} from`, `export *` (at most 8 hops, with a visited set),
/// `export =`, a `declare module` named for the entry's package and
/// namespaces (`JSX.Element`). A name declared in the file but not exported
/// is found last: `.d.ts` scripts such as `lib.dom.d.ts` declare globals.
pub fn lookup(entry: &Path, name: &str) -> Option<DtsDecl> {
    let file = load(entry)?;
    let found = find(&file, &[], name, 0, &mut HashSet::new())?;
    Some(to_decl(&found))
}

/// The member `name` of the type `ty` (a name `lookup` finds from `entry`):
/// every `interface ty` declaration is merged and preferred over a `declare
/// var ty`, an alias is followed to an object type or intersection only, and
/// `extends` (and a class's `implements`) are walked on a miss.
pub fn member(entry: &Path, ty: &str, name: &str) -> Option<DtsDecl> {
    let file = load(entry)?;
    match find(&file, &[], ty, 0, &mut HashSet::new())? {
        Found::Items { file, steps, name: tname } => {
            member_of(&file, &steps, &tname, name, 0, &mut HashSet::new())
        }
        Found::Module { file, .. } => lookup(&file.path, name),
    }
}

type Seen = HashSet<(PathBuf, Vec<Step>, String)>;

fn target(f: &DtsFile, spec: &str) -> Option<Arc<DtsFile>> {
    load(&resolve_package(f.dir(), spec)?)
}

fn find(f: &Arc<DtsFile>, steps: &[Step], name: &str, depth: usize, seen: &mut Seen) -> Option<Found> {
    if depth > MAX_HOPS || !seen.insert((f.path.clone(), steps.to_vec(), name.to_string())) {
        return None;
    }
    if let Some((head, rest)) = name.split_once('.') {
        return match find(f, steps, head, depth, seen)? {
            Found::Items { file, steps: st, name: h } => {
                let i = file.scope(&st).items.iter().position(|it| it.name == h && it.body.is_some())?;
                let mut inner = st.clone();
                inner.push(Step::Ns(i));
                find(&file, &inner, rest, depth + 1, seen)
            }
            Found::Module { file, .. } => find(&file, &[], rest, depth + 1, seen),
        };
    }
    let s = f.scope(steps);
    let items = |exported_only: bool| {
        s.items.iter().any(|it| it.name == name && (it.exported || !exported_only)).then(|| Found::Items {
            file: f.clone(),
            steps: steps.to_vec(),
            name: name.to_string(),
        })
    };
    if let Some(hit) = items(true) {
        return Some(hit);
    }
    for (local, exported, from) in &s.reexports {
        if exported != name {
            continue;
        }
        let hit = match from {
            Some(spec) => match target(f, spec) {
                Some(t) => find(&t, &[], local, depth + 1, seen),
                None => None,
            },
            None => find_local(f, steps, local, depth, seen),
        };
        if hit.is_some() {
            return hit;
        }
    }
    for (alias, spec) in &s.star_as {
        if alias == name {
            if let Some(t) = target(f, spec) {
                return Some(Found::Module { file: t, name: name.to_string() });
            }
        }
    }
    if name != "default" {
        for spec in &s.stars {
            if let Some(hit) = target(f, spec).and_then(|t| find(&t, &[], name, depth + 1, seen)) {
                return Some(hit);
            }
        }
    }
    if let Some(id) = &s.export_eq {
        if id == name {
            if let Some(hit) = find_local(f, steps, id, depth, seen) {
                return Some(hit);
            }
        } else if let Some(i) = s.items.iter().position(|it| &it.name == id && it.body.is_some()) {
            let mut inner = steps.to_vec();
            inner.push(Step::Ns(i));
            if let Some(hit) = find(f, &inner, name, depth + 1, seen) {
                return Some(hit);
            }
        }
    }
    if steps.is_empty() {
        for (i, (m, _)) in s.modules.iter().enumerate() {
            if f.pkg.as_deref() == Some(m.as_str()) {
                if let Some(hit) = find(f, &[Step::Module(i)], name, depth + 1, seen) {
                    return Some(hit);
                }
            }
        }
    }
    items(false)
}

/// A name as seen from inside a scope: declared in it (or an enclosing
/// scope), or imported into the file.
fn find_local(f: &Arc<DtsFile>, steps: &[Step], name: &str, depth: usize, seen: &mut Seen) -> Option<Found> {
    let head = name.split('.').next().unwrap_or(name);
    for k in (0..=steps.len()).rev() {
        if f.scope(&steps[..k]).items.iter().any(|it| it.name == head) {
            return if head == name {
                Some(Found::Items { file: f.clone(), steps: steps[..k].to_vec(), name: name.to_string() })
            } else {
                find(f, &steps[..k], name, depth + 1, seen)
            };
        }
    }
    if let Some((spec, imported)) = f.imports.get(head) {
        let t = target(f, spec)?;
        let rest = &name[head.len()..];
        if imported == "*" {
            return match rest.strip_prefix('.') {
                Some(r) => find(&t, &[], r, depth + 1, seen),
                None => Some(Found::Module { file: t, name: name.to_string() }),
            };
        }
        return find(&t, &[], &format!("{imported}{rest}"), depth + 1, seen);
    }
    None
}

/// Lower ranks win when several kinds share a name.
fn rank(k: DtsKind) -> usize {
    use DtsKind::*;
    [Function, Class, Interface, TypeAlias, Var, Namespace, Module, Method, Property]
        .iter()
        .position(|x| *x == k)
        .unwrap_or(usize::MAX)
}

fn decl_of(f: &DtsFile, name: &str, items: &[&Item]) -> Option<DtsDecl> {
    let kind = items.iter().map(|it| it.kind).min_by_key(|k| rank(*k))?;
    let sigs: Vec<&Sig> = items.iter().filter(|it| it.kind == kind).flat_map(|it| it.sigs.iter()).collect();
    Some(DtsDecl {
        name: name.to_string(),
        kind,
        sigs: sigs.iter().map(|s| s.text.clone()).collect(),
        ret: sigs.iter().map(|s| qualify(&s.ret, f, &s.tparams)).collect(),
        file: f.path.clone(),
        line: sigs.first().map_or(1, |s| s.line),
    })
}

fn to_decl(found: &Found) -> DtsDecl {
    match found {
        Found::Items { file, steps, name } => {
            let items: Vec<&Item> = file.scope(steps).items.iter().filter(|it| &it.name == name).collect();
            decl_of(file, name, &items).expect("found items exist")
        }
        Found::Module { file, name } => DtsDecl {
            name: name.clone(),
            kind: DtsKind::Module,
            sigs: vec![],
            ret: vec![],
            file: file.path.clone(),
            line: 1,
        },
    }
}

fn member_of(
    f: &Arc<DtsFile>,
    steps: &[Step],
    tname: &str,
    name: &str,
    depth: usize,
    seen: &mut Seen,
) -> Option<DtsDecl> {
    if depth > MAX_BASES || !seen.insert((f.path.clone(), steps.to_vec(), tname.to_string())) {
        return None;
    }
    let all: Vec<&Item> = f.scope(steps).items.iter().filter(|it| it.name == tname).collect();
    let pick = [DtsKind::Interface, DtsKind::Class, DtsKind::TypeAlias, DtsKind::Var]
        .into_iter()
        .map(|k| all.iter().copied().filter(|it| it.kind == k).collect::<Vec<_>>())
        .find(|v| !v.is_empty())?;
    let hits: Vec<&Item> = pick.iter().flat_map(|it| it.members.iter()).filter(|m| m.name == name).collect();
    if let Some(d) = decl_of(f, name, &hits) {
        return Some(d);
    }
    for it in &pick {
        if !it.memberable {
            return None;
        }
        for base in &it.bases {
            if let Some(d) = base_member(f, steps, base, name, depth, seen) {
                return Some(d);
            }
        }
    }
    None
}

/// A member of a base type, the base resolved from the scope that names it,
/// then as a global in the package tree's `lib.dom.d.ts`.
fn base_member(f: &Arc<DtsFile>, steps: &[Step], base: &str, name: &str, depth: usize, seen: &mut Seen) -> Option<DtsDecl> {
    let found = find_local(f, steps, base, 0, &mut HashSet::new()).or_else(|| {
        let dom = lib_dom(&node_modules_of(&f.path)?)?;
        if dom == f.path {
            return None;
        }
        let file = load(&dom)?;
        find(&file, &[], base, 0, &mut HashSet::new())
    })?;
    match found {
        Found::Items { file, steps, name: tname } => member_of(&file, &steps, &tname, name, depth + 1, seen),
        Found::Module { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// package.json: a small JSON reader that keeps object key order, which the
// `exports` condition walk depends on.

enum Json {
    Null,
    Bool,
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn parse(src: &str) -> Option<Json> {
        let mut p = JsonParser { s: src.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.s.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut entries = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(entries));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    entries.push((k, self.value()?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Json::Obj(entries));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Json::Arr(items));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' | b'f' | b'n' => {
                let rest = &self.s[self.i..];
                for (kw, v) in [("true", Json::Bool), ("false", Json::Bool), ("null", Json::Null)] {
                    if rest.starts_with(kw.as_bytes()) {
                        self.i += kw.len();
                        return Some(v);
                    }
                }
                None
            }
            _ => {
                let start = self.i;
                while self.s.get(self.i).is_some_and(|c| b"+-.eE0123456789".contains(c)) {
                    self.i += 1;
                }
                (self.i > start).then_some(Json::Num)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.to_string().as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/decls/dts")
    }

    fn nm() -> PathBuf {
        root().join("node_modules")
    }

    fn app() -> PathBuf {
        root().join("app/src")
    }

    fn entry(spec: &str) -> PathBuf {
        resolve_package(&app(), spec).unwrap_or_else(|| panic!("{spec} did not resolve"))
    }

    fn tref(pkg: Option<&str>, name: &str, elems: Vec<TypeRef>) -> TypeRef {
        TypeRef { pkg: pkg.map(str::to_string), name: name.into(), pointer: false, elems }
    }

    #[test]
    fn exports_condition_wins_over_top_level_types() {
        assert_eq!(entry("solid-js"), nm().join("solid-js/types/index.d.ts"));
        assert!(lookup(&entry("solid-js"), "legacyOnly").is_none());
    }

    #[test]
    fn create_signal_overloads_through_the_barrel() {
        let d = lookup(&entry("solid-js"), "createSignal").unwrap();
        assert_eq!(d.kind, DtsKind::Function);
        assert_eq!(d.sigs.len(), 2, "{:?}", d.sigs);
        assert_eq!(d.sigs[1], "function createSignal<T>(value: T, options?: SignalOptions<T>): Signal<T>");
        assert_eq!(d.file, nm().join("solid-js/types/reactive/signal.d.ts"));
        assert_eq!(d.line, 8);
        assert_eq!(d.location(), "node_modules/solid-js/types/reactive/signal.d.ts:8");
        assert_eq!(d.ret[0], tref(Some("solid-js"), "Signal", vec![tref(None, "T | undefined", vec![])]));
        assert_eq!(d.ret[1], tref(Some("solid-js"), "Signal", vec![tref(None, "T", vec![])]));
    }

    #[test]
    fn renamed_reexport_and_star_chain() {
        let d = lookup(&entry("solid-js"), "memo").unwrap();
        assert_eq!(d.name, "createMemo");
        assert_eq!(d.ret, vec![tref(Some("solid-js"), "Accessor", vec![tref(None, "T", vec![])])]);
        let c = lookup(&entry("solid-js"), "createComponent").unwrap();
        assert_eq!(c.file, nm().join("solid-js/types/render/component.d.ts"));
        assert_eq!(c.ret, vec![tref(Some("solid-js"), "JSX.Element", vec![])]);
        let j = lookup(&entry("solid-js"), "JSX.Element").unwrap();
        assert_eq!(j.kind, DtsKind::TypeAlias);
        assert_eq!(j.file, nm().join("solid-js/types/jsx.d.ts"));
    }

    #[test]
    fn export_star_cycle_terminates() {
        let e = entry("solid-js");
        assert!(lookup(&e, "notDeclaredAnywhere").is_none());
        assert_eq!(lookup(&e, "fromCycleA").unwrap().kind, DtsKind::Var);
    }

    #[test]
    fn router_resolves_through_top_level_types() {
        let e = entry("@solidjs/router");
        assert_eq!(e, nm().join("@solidjs/router/dist/index.d.ts"));
        let d = lookup(&e, "useNavigate").unwrap();
        assert_eq!(d.kind, DtsKind::Var);
        assert_eq!(d.sigs, vec!["const useNavigate: () => Navigator"]);
        assert_eq!(d.ret, vec![tref(None, "() => Navigator", vec![])]);
        assert_eq!(d.file, nm().join("@solidjs/router/dist/routing.d.ts"));
        let p = member(&e, "Location", "pathname").unwrap();
        assert_eq!(p.kind, DtsKind::Property);
        assert_eq!(p.ret, vec![tref(None, "string", vec![])]);
        let q = member(&e, "Location", "query").unwrap();
        assert_eq!(q.ret[0].name, "Record");
    }

    #[test]
    fn subpath_exports_resolve() {
        assert_eq!(entry("solid-js/web"), nm().join("solid-js/web/types/index.d.ts"));
        assert_eq!(entry("solid-js/types/jsx.js"), nm().join("solid-js/types/jsx.d.ts"));
        assert!(resolve_package(&app(), "solid-js/missing").is_none());
        let r = lookup(&entry("solid-js/web"), "render").unwrap();
        assert_eq!(r.ret, vec![tref(None, "() => void", vec![])]);
    }

    #[test]
    fn relative_specifiers_map_to_declarations() {
        let types = nm().join("solid-js/types");
        assert_eq!(resolve_package(&types, "./reactive/signal.js"), Some(types.join("reactive/signal.d.ts")));
        assert_eq!(resolve_package(&types, "./render"), Some(types.join("render/index.d.ts")));
        assert_eq!(resolve_package(&types.join("render"), "../jsx"), Some(types.join("jsx.d.ts")));
    }

    #[test]
    fn scoped_package_falls_back_to_at_types() {
        let e = entry("@scope/name");
        assert_eq!(e, nm().join("@types/scope__name/index.d.ts"));
        let d = lookup(&e, "scoped").unwrap();
        assert_eq!(d.ret, vec![tref(Some("@scope/name"), "Scoped", vec![])]);
        assert_eq!(member(&e, "Thing", "own").unwrap().kind, DtsKind::Method);
        assert_eq!(member(&e, "Thing", "base").unwrap().sigs, vec!["base(): number"]);
        assert_eq!(member(&e, "Thing", "label").unwrap().kind, DtsKind::Property);
    }

    #[test]
    fn dom_members_through_extends() {
        let dom = lib_dom(&nm()).unwrap();
        let d = member(&dom, "Element", "appendChild").unwrap();
        assert_eq!(d.kind, DtsKind::Method);
        assert_eq!(d.sigs, vec!["appendChild<T extends Node>(node: T): T"]);
        assert_eq!(d.ret, vec![tref(None, "T", vec![])]);
        assert_eq!(d.line, 11);
        // merged interface declarations, a second base, and a two-step chain
        assert!(member(&dom, "Element", "closest").is_some());
        assert!(member(&dom, "Element", "childElementCount").is_some());
        assert!(member(&dom, "Element", "addEventListener").is_some());
        // the interface wins over `declare var Node`
        assert!(member(&dom, "Node", "prototype").is_none());
        let el = lookup_global(&nm(), "Element").unwrap();
        assert_eq!(el.kind, DtsKind::Interface);
        assert_eq!(el.sigs.len(), 2);
        assert_eq!(lookup_global(&nm(), "alert").unwrap().kind, DtsKind::Function);
    }

    #[test]
    fn aliases_follow_object_and_intersection_only() {
        let dom = lib_dom(&nm()).unwrap();
        assert!(member(&dom, "HTMLish", "dataset").is_some());
        assert_eq!(member(&dom, "HTMLish", "nodeName").unwrap().file, dom);
        assert!(member(&entry("solid-js"), "Accessor", "length").is_none());
        let opts = member(&entry("solid-js/types/reactive/signal.js"), "SignalOptions", "name").unwrap();
        assert_eq!(opts.kind, DtsKind::Property);
    }

    #[test]
    fn base_type_outside_the_package_is_a_dom_global() {
        let d = member(&entry("solid-js/web"), "Mountable", "appendChild").unwrap();
        assert_eq!(d.file, lib_dom(&nm()).unwrap());
        assert_eq!(member(&entry("solid-js/web"), "Mountable", "mount").unwrap().kind, DtsKind::Method);
    }

    #[test]
    fn declare_module_resolves() {
        let e = entry("ambient-mod");
        let d = lookup(&e, "greet").unwrap();
        assert_eq!(d.sigs, vec!["function greet(name: string): Greeting"]);
        assert_eq!(d.ret, vec![tref(Some("ambient-mod"), "Greeting", vec![])]);
        assert_eq!(member(&e, "Greeting", "text").unwrap().kind, DtsKind::Property);
    }

    #[test]
    fn export_equals_resolves() {
        let e = entry("cjs-lib");
        assert_eq!(e, nm().join("cjs-lib/index.d.ts"));
        let h = lookup(&e, "helper").unwrap();
        assert_eq!(h.kind, DtsKind::Function);
        assert_eq!(h.sigs, vec!["function helper(flag?: boolean): string"]);
        let c = lookup(&e, "cjs").unwrap();
        assert_eq!(c.kind, DtsKind::Function);
        assert_eq!(c.ret, vec![tref(Some("cjs-lib"), "cjs.Result", vec![])]);
        assert_eq!(member(&e, "Result", "ok").unwrap().kind, DtsKind::Property);
        assert_eq!(member(&e, "cjs.Result", "ok").unwrap().kind, DtsKind::Property);
    }

    #[test]
    fn json_keeps_key_order_and_escapes() {
        let j = Json::parse(r#"{"b": 1, "a": ["x\"y", null, true], "c": {"A": "v"}}"#).unwrap();
        let Json::Obj(e) = &j else { panic!() };
        assert_eq!(e.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(j.get("c").and_then(|c| c.get("A")).and_then(Json::as_str), Some("v"));
        assert!(Json::parse("{\"a\": }").is_none());
    }

    #[test]
    fn package_names_from_paths() {
        assert_eq!(package_of(Path::new("/x/node_modules/solid-js/types/a.d.ts")).as_deref(), Some("solid-js"));
        assert_eq!(package_of(Path::new("/x/node_modules/@a/b/c.d.ts")).as_deref(), Some("@a/b"));
        assert_eq!(package_of(Path::new("/x/node_modules/@types/a__b/c.d.ts")).as_deref(), Some("@a/b"));
        assert_eq!(package_of(Path::new("/x/node_modules/@types/node/c.d.ts")).as_deref(), Some("node"));
        assert_eq!(package_of(Path::new("/x/node_modules/typescript/lib/lib.dom.d.ts")), None);
    }

    const MAVEXA: &str = "workspace/projects/mavexa/mavexa/frontend";

    fn mavexa() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap()).join(MAVEXA)
    }

    #[test]
    #[ignore]
    fn mavexa_solid_and_router() {
        let t = std::time::Instant::now();
        let from = mavexa().join("src");
        let solid = resolve_package(&from, "solid-js").unwrap();
        assert!(solid.ends_with("node_modules/solid-js/types/index.d.ts"), "{}", solid.display());
        let d = lookup(&solid, "createSignal").unwrap();
        assert!(d.sigs.len() >= 2, "{:?}", d.sigs);
        assert!(d.file.ends_with("solid-js/types/reactive/signal.d.ts"));
        assert_eq!(d.line, 138);
        assert!(d.ret.iter().all(|r| r.pkg.as_deref() == Some("solid-js")), "{:?}", d.ret);
        let router = resolve_package(&from, "@solidjs/router").unwrap();
        assert!(router.ends_with("@solidjs/router/dist/index.d.ts"), "{}", router.display());
        let n = lookup(&router, "useNavigate").unwrap();
        assert_eq!(n.kind, DtsKind::Var);
        assert!(n.file.ends_with("@solidjs/router/dist/routing.d.ts"));
        eprintln!("solid + router: {:?}", t.elapsed());
    }

    #[test]
    #[ignore]
    fn mavexa_lib_dom_element_append_child() {
        let t = std::time::Instant::now();
        let dom = lib_dom(&mavexa().join("node_modules")).unwrap();
        let d = member(&dom, "Element", "appendChild").unwrap();
        assert_eq!(d.kind, DtsKind::Method);
        assert!(d.sigs[0].starts_with("appendChild<T extends Node>"), "{:?}", d.sigs);
        assert!(member(&dom, "HTMLElement", "addEventListener").is_some());
        eprintln!("lib.dom: {:?}", t.elapsed());
    }
}
