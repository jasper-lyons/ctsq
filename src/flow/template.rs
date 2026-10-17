//! Data flow through Go templates (plan decisions 24 and 25).
//!
//! `link` finds the templates a modelled `Execute` or `ExecuteTemplate` runs:
//! it walks the receiver back to the calls that created and loaded it
//! (`New`, `ParseFS`, `ParseFiles`, `ParseGlob`, `Parse`, through `Must`),
//! resolves each name argument to a value set, and matches the names against
//! the `//go:embed` patterns on the `embed.FS` argument. The first name of the
//! first load picks the root template, one template set per value.
//!
//! `render` walks a template set from its root with the tracked data as `.`.
//! Field reads follow the field paths of the data, `range` and `with` rebind
//! `.`, `{{template}}` and `{{block}}` run the matching `{{define}}`, and
//! `$x := ...` assigns. A printing action that reads tracked data is a hop
//! tagged `rendered`. Node kinds and field names come from the gotmpl
//! `Mechanics`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use super::engine::{kids, named, text};
use super::upward::{reaching, Def};
use super::values::value_set;
use super::{Hop, Paths};
use crate::config::models::TemplateLoad;
use crate::config::{gomod, LangConfig, Mechanics};
use crate::query::ast::Query;
use crate::resolve::scoped::FlowGraph;
use crate::resolve::TreeStore;

/// Import paths of the template packages.
const PACKAGES: &[&str] = &["html/template", "text/template"];

/// Deepest chain of template calls followed.
const MAX_CALLS: usize = 32;

/// One template text: a file, or a Go string passed to `Parse`.
#[derive(Debug, Clone)]
pub struct Source {
    /// Key in the set's store, unique per source.
    pub key: String,
    /// Path printed on rows.
    pub file: String,
    /// Printed line of the text's first line.
    pub base_line: usize,
    /// Template name: the file's base name, or the name given to `New`.
    pub name: String,
    pub text: String,
}

/// Where a run starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// The whole text of a source.
    Source(usize),
    /// A template by name, as `ExecuteTemplate` gives it.
    Name(String),
}

/// The templates parsed into one `*Template`, with the entry a run starts at.
pub struct TemplateSet {
    pub sources: Vec<Source>,
    pub entries: Vec<Entry>,
    /// The parsed sources, keyed by `Source::key`.
    pub store: TreeStore,
}

/// A load call found on the receiver's chain, in run order.
struct Load<'t> {
    file: String,
    call: Node<'t>,
    model: TemplateLoad,
}

/// What the receiver walk found.
#[derive(Default)]
struct Chain<'t> {
    loads: Vec<Load<'t>>,
    /// Name given to `New`, for inline sources.
    name: Option<String>,
}

/// Links template calls to their templates.
pub struct Linker<'a> {
    pub store: &'a TreeStore,
    pub graph: &'a FlowGraph,
    pub cfg: &'a LangConfig,
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && (s.starts_with('"') && s.ends_with('"') || s.starts_with('`') && s.ends_with('`')) {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

fn same_dir(a: &str, b: &str) -> bool {
    Path::new(a).parent() == Path::new(b).parent()
}

fn args_of(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments").map(named).unwrap_or_default()
}

impl<'a> Linker<'a> {
    fn tree(&self, file: &str) -> Option<(Node<'a>, &'a [u8])> {
        let pf = self.store.get(file)?;
        Some((pf.tree.as_ref()?.root_node(), pf.source.as_slice()))
    }

    /// The template package an import name stands for in `file`.
    fn package(&self, file: &str, local: &str) -> Option<String> {
        let (root, src) = self.tree(file)?;
        let imports = crate::resolve::go::go_imports(root, src);
        imports.get(local).filter(|p| PACKAGES.contains(&p.as_str())).cloned()
    }

    fn load_model(&self, key: &str) -> Option<TemplateLoad> {
        self.cfg.model(key).and_then(|m| m.template_load.clone())
    }

    /// Walk expression `e` back to the calls that made the template. Gives
    /// the template package it came from.
    fn chain(&self, file: &str, e: Node<'a>, out: &mut Chain<'a>, depth: usize) -> Option<String> {
        if depth > 16 {
            return None;
        }
        let (_, src) = self.tree(file)?;
        match e.kind() {
            "parenthesized_expression" => self.chain(file, e.named_child(0)?, out, depth + 1),
            "identifier" => self.variable(file, e, out, depth),
            "call_expression" => {
                let f = e.child_by_field_name("function")?;
                if f.kind() != "selector_expression" {
                    return None;
                }
                let operand = f.child_by_field_name("operand")?;
                let method = text(f.child_by_field_name("field")?, src);
                let pkg_fn = if operand.kind() == "identifier" { self.package(file, text(operand, src)) } else { None };
                let (pkg, key) = match pkg_fn {
                    Some(p) => {
                        if method == "Must" {
                            let a = *args_of(e).first()?;
                            return self.chain(file, a, out, depth + 1).or(Some(p));
                        }
                        let key = format!("{}.{}", p, method);
                        (p, key)
                    }
                    None => {
                        let p = self.chain(file, operand, out, depth + 1)?;
                        let key = format!("(*{}.Template).{}", p, method);
                        (p, key)
                    }
                };
                if method == "New" && out.name.is_none() {
                    if let Some(a) = args_of(e).first() {
                        out.name = value_set(self.store, self.graph, file, *a).values.first().map(|v| v.0.clone());
                    }
                }
                if let Some(model) = self.load_model(&key) {
                    if !out.loads.iter().any(|l| l.file == file && l.call.id() == e.id()) {
                        out.loads.push(Load { file: file.to_string(), call: e, model });
                    }
                }
                Some(pkg)
            }
            _ => None,
        }
    }

    /// A variable holding a template: its reaching assignments, then the
    /// load calls made on it (`t.ParseFS(...)`) in the same scope.
    fn variable(&self, file: &str, id: Node<'a>, out: &mut Chain<'a>, depth: usize) -> Option<String> {
        let (root, src) = self.tree(file)?;
        let name = text(id, src);
        let mut defs: Vec<(String, Def<'a>)> =
            reaching(root, src, name, id.start_byte()).into_iter().map(|d| (file.to_string(), d)).collect();
        if defs.is_empty() {
            // A declaration of `name` is in a file holding it.
            for (f, pf) in self.store.holding(&[name]) {
                if f == file || pf.lang != "go" || !same_dir(f, file) {
                    continue;
                }
                let Some((r, s)) = self.tree(f) else { continue };
                defs.extend(reaching(r, s, name, 0).into_iter().map(|d| (f.to_string(), d)));
            }
        }
        let mut pkg = None;
        for (f, d) in &defs {
            if let Def::Assign { value: Some(v), .. } = d {
                if let Some(p) = self.chain(f, *v, out, depth + 1) {
                    pkg.get_or_insert(p);
                }
            }
        }
        let pkg = pkg?;
        // Load calls on the variable itself, before the use.
        let scope = super::upward::function_at(root, id.start_byte()).unwrap_or(root);
        let mut stack = vec![scope];
        let mut found = vec![];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression" && n.end_byte() <= id.start_byte() {
                let f = n.child_by_field_name("function");
                let on = f.filter(|f| f.kind() == "selector_expression").and_then(|f| f.child_by_field_name("operand"));
                if on.is_some_and(|o| o.kind() == "identifier" && text(o, src) == name) {
                    let m = text(f.and_then(|f| f.child_by_field_name("field")).unwrap(), src);
                    if let Some(model) = self.load_model(&format!("(*{}.Template).{}", pkg, m)) {
                        found.push((n, model));
                    }
                }
            }
            stack.extend(named(n));
        }
        found.sort_by_key(|(n, _)| n.start_byte());
        for (n, model) in found {
            if !out.loads.iter().any(|l| l.file == file && l.call.id() == n.id()) {
                out.loads.push(Load { file: file.to_string(), call: n, model });
            }
        }
        Some(pkg)
    }

    /// The template sets the call `call` in `file` runs. `by_name` is true
    /// for `ExecuteTemplate`, whose argument `name_arg` names the entry.
    pub fn link(&self, file: &str, call: Node<'a>, name_arg: Option<usize>) -> Vec<TemplateSet> {
        let Some(f) = call.child_by_field_name("function") else { return vec![] };
        let Some(recv) = f.child_by_field_name("operand") else { return vec![] };
        let mut chain = Chain::default();
        if self.chain(file, recv, &mut chain, 0).is_none() || chain.loads.is_empty() {
            return vec![];
        }
        // Inline texts and file groups: the first name of the first load
        // with names gives one group per value, every other name is shared.
        let mut inline: Vec<Source> = vec![];
        let mut groups: Vec<Vec<PathBuf>> = vec![];
        let mut shared: Vec<PathBuf> = vec![];
        let mut first = true;
        for l in &chain.loads {
            let args = args_of(l.call);
            if let Some(t) = l.model.text_arg.and_then(|i| args.get(i)) {
                let vs = value_set(self.store, self.graph, &l.file, *t);
                for (v, f, line) in vs.values {
                    inline.push(Source {
                        key: format!("{}:{}", f, line),
                        file: f,
                        base_line: line,
                        name: chain.name.clone().unwrap_or_else(|| "template".into()),
                        text: v,
                    });
                }
            }
            let Some(from) = l.model.names_from else { continue };
            let embedded = l.model.fs_arg.and_then(|i| args.get(i)).map(|a| self.embedded(&l.file, *a));
            for (k, a) in args.iter().enumerate().skip(from) {
                let vs = value_set(self.store, self.graph, &l.file, *a);
                let resolve = |p: &str| match &embedded {
                    Some(Some((dir, files))) => files.iter().filter(|f| glob(p, f)).map(|f| dir.join(f)).collect(),
                    Some(None) => vec![],
                    None => on_disk(&l.file, p),
                };
                if first && k == from {
                    for (v, _, _) in vs.values.iter().chain(vs.globs.iter()) {
                        groups.push(resolve(v));
                    }
                } else {
                    for (v, _, _) in vs.values.iter().chain(vs.globs.iter()) {
                        shared.extend(resolve(v));
                    }
                }
            }
            first = false;
        }
        if groups.is_empty() {
            groups.push(vec![]);
        }
        let mut sets = vec![];
        for g in groups {
            let mut sources: Vec<Source> = vec![];
            let mut entries: Vec<Entry> = vec![];
            let mut add = |p: &PathBuf, root: bool, sources: &mut Vec<Source>| {
                let file = p.display().to_string();
                let i = match sources.iter().position(|s| s.key == file) {
                    Some(i) => i,
                    None => {
                        let Ok(text) = std::fs::read_to_string(p) else { return };
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        sources.push(Source { key: file.clone(), file, base_line: 1, name, text });
                        sources.len() - 1
                    }
                };
                if root && !entries.contains(&Entry::Source(i)) {
                    entries.push(Entry::Source(i));
                }
            };
            for p in &g {
                add(p, true, &mut sources);
            }
            for p in &shared {
                add(p, false, &mut sources);
            }
            for s in &inline {
                sources.push(s.clone());
                entries.push(Entry::Source(sources.len() - 1));
            }
            if sources.is_empty() {
                continue;
            }
            if let Some(i) = name_arg {
                // ExecuteTemplate: the entry is the named template.
                entries = match args_of(call).get(i) {
                    Some(a) => value_set(self.store, self.graph, file, *a)
                        .values
                        .into_iter()
                        .map(|(v, _, _)| Entry::Name(v))
                        .collect(),
                    None => vec![],
                };
            }
            let mut store = TreeStore::default();
            for s in &sources {
                store.insert(&s.key, "gotmpl", s.text.as_bytes().to_vec());
            }
            sets.push(TemplateSet { sources, entries, store });
        }
        sets
    }

    /// The files embedded in the `embed.FS` variable `arg` names: the
    /// directory of its Go file and the paths below it. None when the
    /// variable or its `//go:embed` patterns cannot be found.
    fn embedded(&self, file: &str, arg: Node) -> Option<(PathBuf, Vec<String>)> {
        let (_, src) = self.tree(file)?;
        let name = text(arg, src);
        if arg.kind() != "identifier" {
            return None;
        }
        for (f, pf) in self.store.holding(&[name]) {
            if pf.lang != "go" || !same_dir(f, file) {
                continue;
            }
            let Some((root, s)) = self.tree(f) else { continue };
            let mut stack = vec![root];
            while let Some(n) = stack.pop() {
                if n.kind() == "var_spec" && n.child_by_field_name("name").is_some_and(|x| text(x, s) == name) {
                    let lines: Vec<&str> = std::str::from_utf8(s).unwrap_or("").lines().collect();
                    let mut row = n.start_position().row;
                    let mut patterns = vec![];
                    while row > 0 {
                        row -= 1;
                        let l = lines.get(row).map(|l| l.trim()).unwrap_or("");
                        if !l.starts_with("//") {
                            break;
                        }
                        if let Some(p) = l.strip_prefix("//go:embed") {
                            patterns.extend(p.split_whitespace().map(unquote));
                        }
                    }
                    let dir = Path::new(f).parent().unwrap_or(Path::new(".")).to_path_buf();
                    let all = list_files(&dir);
                    let files = all
                        .into_iter()
                        .filter(|rel| {
                            patterns.iter().any(|p| {
                                glob(p, rel) || {
                                    // A pattern naming a directory embeds what is below it.
                                    let parts: Vec<&str> = rel.split('/').collect();
                                    (1..parts.len()).any(|k| glob(p, &parts[..k].join("/")))
                                        && !parts.iter().any(|x| x.starts_with('.') || x.starts_with('_'))
                                }
                            })
                        })
                        .collect();
                    return Some((dir, files));
                }
                stack.extend(named(n));
            }
        }
        None
    }
}

/// Files a `ParseFiles` or `ParseGlob` pattern names, relative to the Go
/// file's directory, then its module root, then the working directory.
fn on_disk(go_file: &str, pattern: &str) -> Vec<PathBuf> {
    let mut bases = vec![];
    let dir = Path::new(go_file).parent().unwrap_or(Path::new(".")).to_path_buf();
    if let Some(m) = gomod::find_go_mod(&dir) {
        bases.push(dir.clone());
        if let Some(p) = m.parent() {
            bases.push(p.to_path_buf());
        }
    } else {
        bases.push(dir);
    }
    bases.push(PathBuf::from("."));
    for b in bases {
        let hits: Vec<PathBuf> = if pattern.contains(['*', '?', '[']) {
            list_files(&b).into_iter().filter(|f| glob(pattern, f)).map(|f| b.join(f)).collect()
        } else if b.join(pattern).is_file() {
            vec![b.join(pattern)]
        } else {
            vec![]
        };
        if !hits.is_empty() {
            return hits;
        }
    }
    vec![]
}

/// Files below `dir` as slash separated relative paths, hidden directories
/// skipped, at most eight levels and 5000 files.
fn list_files(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![(dir.to_path_buf(), String::new(), 0)];
    while let Some((d, rel, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 8 && !name.starts_with('.') {
                    stack.push((e.path(), r, depth + 1));
                }
            } else if out.len() < 5000 {
                out.push(r);
            }
        }
    }
    out.sort();
    out
}

/// `path.Match` style matching: `*` is any run without `/`, `?` one
/// character other than `/`, `[...]` a class.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).take_while(|&i| i == 0 || n[i - 1] != '/').any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && n[0] != '/' && go(&p[1..], &n[1..]),
            Some('[') => {
                let Some(end) = p.iter().position(|&c| c == ']') else { return !n.is_empty() && n[0] == '[' && go(&p[1..], &n[1..]) };
                let Some(&c) = n.first() else { return false };
                let class = &p[1..end];
                let (neg, class) = match class.first() {
                    Some('^') | Some('!') => (true, &class[1..]),
                    _ => (false, class),
                };
                let mut hit = false;
                let mut i = 0;
                while i < class.len() {
                    if i + 2 < class.len() && class[i + 1] == '-' {
                        hit |= class[i] <= c && c <= class[i + 2];
                        i += 3;
                    } else {
                        hit |= class[i] == c;
                        i += 1;
                    }
                }
                hit != neg && go(&p[end + 1..], &n[1..])
            }
            Some(&c) => n.first() == Some(&c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

/// A tracked value: where it holds the data and whether it is derived.
type Val = Option<(Paths, bool)>;

#[derive(Clone)]
struct Env {
    dot: Val,
    vars: HashMap<String, Val>,
}

fn join(a: Val, b: Val) -> Val {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some((p, d)), Some((q, e))) => Some((super::join_paths(&p, &q), d || e)),
    }
}

/// Reading field `name` of `v`: a path through it is exact (or as derived as
/// `v`), a whole value makes the read derived, any other field is untracked.
fn read(v: &Val, name: &str) -> Val {
    match v {
        None => None,
        Some((None, _)) => Some((None, true)),
        Some((Some(ps), d)) => {
            let rest: Vec<Vec<String>> =
                ps.iter().filter(|p| p.first().map(|s| s.as_str()) == Some(name)).map(|p| p[1..].to_vec()).collect();
            if rest.is_empty() {
                None
            } else if rest.iter().any(|r| r.is_empty()) {
                Some((None, *d))
            } else {
                Some((Some(rest), *d))
            }
        }
    }
}

/// Walks a template set.
struct Render<'a> {
    set: &'a TemplateSet,
    m: &'a Mechanics,
    /// End rows by source key. None when every tracked print is an end.
    ends: Option<HashMap<String, Vec<(usize, usize)>>>,
    /// Define and block nodes by template name.
    defs: HashMap<String, Vec<(usize, Node<'a>)>>,
    active: RefCell<Vec<(String, String)>>,
}

/// Walk `set` from each entry with `data` as `.`. One tree per entry that
/// renders tracked data, its root the entry template tagged `moved`.
/// `tail` is the end selector: a print action holding one of its rows is a
/// sink. With no tail every tracked print is one.
pub fn render(set: &TemplateSet, m: &Mechanics, data: Paths, tail: Option<&Query>) -> Vec<Hop> {
    let ends = tail.map(|q| {
        let mut ends: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        if let Ok(rows) = crate::semantic::segment_rows(q, &set.store) {
            for (f, rs) in rows {
                ends.insert(f, rs.iter().map(|r| (r.start_byte, r.end_byte)).collect());
            }
        }
        ends
    });
    let mut r = Render { set, m, ends, defs: HashMap::new(), active: RefCell::new(vec![]) };
    for (i, s) in set.sources.iter().enumerate() {
        let Some(root) = r.root(i) else { continue };
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if Mechanics::is(&m.nodes.define, n.kind()) {
                if let Some(name) = kids(n, &m.fields.name).first() {
                    let name = unquote(text(*name, s.text.as_bytes()));
                    r.defs.entry(name).or_default().push((i, n));
                }
            }
            stack.extend(named(n));
        }
    }
    let data: Val = Some((data, false));
    let mut out = vec![];
    for e in &set.entries {
        let (hops, src, line, func, text) = match e {
            Entry::Source(i) => {
                let Some(root) = r.root(*i) else { continue };
                let s = &set.sources[*i];
                let env = Env { dot: data.clone(), vars: HashMap::from([("$".to_string(), data.clone())]) };
                let hops = r.body(*i, &named(root), env, &s.name);
                let first = s.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                (hops, *i, s.base_line, s.name.clone(), first)
            }
            Entry::Name(name) => {
                let targets = r.targets(name, None);
                let Some(&(i, n)) = targets.first() else { continue };
                let hops = r.run(name, &targets, data.clone());
                let (line, text) = r.at(i, n);
                (hops, i, line, name.clone(), text)
            }
        };
        if hops.is_empty() {
            continue;
        }
        let mut h = r.hop_at(src, line, &func, text, vec!["moved".into()], hops);
        h.into = true;
        out.push(h);
    }
    out
}

impl<'a> Render<'a> {
    fn root(&self, i: usize) -> Option<Node<'a>> {
        Some(self.set.store.get(&self.set.sources[i].key)?.tree.as_ref()?.root_node())
    }

    fn src(&self, i: usize) -> &'a [u8] {
        self.set.sources[i].text.as_bytes()
    }

    /// Printed line and trimmed text of node `n` in source `i`.
    fn at(&self, i: usize, n: Node) -> (usize, String) {
        let s = &self.set.sources[i];
        let row = n.start_position().row;
        (s.base_line + row, s.text.lines().nth(row).unwrap_or("").trim().to_string())
    }

    fn hop_at(&self, i: usize, line: usize, func: &str, text: String, tags: Vec<String>, children: Vec<Hop>) -> Hop {
        Hop {
            file: self.set.sources[i].file.clone(),
            line,
            func: func.to_string(),
            text,
            tags,
            unmodelled: None,
            returns: false,
            into: false,
            children,
        }
    }

    fn hop(&self, i: usize, n: Node, func: &str, tags: Vec<String>, children: Vec<Hop>) -> Hop {
        let (line, text) = self.at(i, n);
        self.hop_at(i, line, func, text, tags, children)
    }

    fn is(&self, list: &[String], n: Node) -> bool {
        Mechanics::is(list, n.kind())
    }

    fn primary(v: &Val) -> Vec<String> {
        match v {
            Some((_, true)) => vec!["derived".into()],
            Some(_) => vec!["exact".into()],
            None => vec![],
        }
    }

    /// Walk a list of nodes in order.
    fn body(&self, i: usize, nodes: &[Node<'a>], mut env: Env, func: &str) -> Vec<Hop> {
        let mut out = vec![];
        for n in nodes {
            out.extend(self.action(i, *n, &mut env, func));
        }
        out
    }

    fn action(&self, i: usize, n: Node<'a>, env: &mut Env, func: &str) -> Vec<Hop> {
        let m = self.m;
        let src = self.src(i);
        if self.is(&m.nodes.print, n) {
            let Some(c) = n.named_child(0) else { return vec![] };
            if self.is(&m.nodes.assign, c) {
                self.assign(i, c, env);
                return vec![];
            }
            let v = self.eval(i, c, env);
            if v.is_none() {
                return vec![];
            }
            let mut tags = Self::primary(&v);
            tags.push("rendered".into());
            if self.is_end(i, n) {
                tags.push("sink".into());
            }
            return vec![self.hop(i, n, func, tags, vec![])];
        }
        if self.is(&m.nodes.template_call, n) {
            let Some(name) = kids(n, &m.fields.name).first().map(|x| unquote(text(*x, src))) else { return vec![] };
            let arg = kids(n, &m.fields.argument).first().map(|a| self.eval(i, *a, env)).unwrap_or(None);
            let own = if self.is(&m.nodes.define, n) { Some((i, n)) } else { None };
            let targets = self.targets(&name, own);
            let hops = self.run(&name, &targets, arg.clone());
            if hops.is_empty() {
                return vec![];
            }
            let mut tags = Self::primary(&arg);
            tags.push(format!("via {}", name));
            return vec![self.hop(i, n, func, tags, hops)];
        }
        if self.is(&m.nodes.define, n) {
            return vec![];
        }
        if self.is(&m.nodes.rebind, n) {
            return self.rebind(i, n, env, func);
        }
        if self.is(&m.nodes.assign, n) {
            self.assign(i, n, env);
            return vec![];
        }
        // if, else and anything else: every part but the conditions.
        let conds = kids(n, &m.fields.condition);
        for c in &conds {
            if self.is(&m.nodes.assign, *c) {
                self.assign(i, *c, env);
            }
        }
        let parts: Vec<Node<'a>> = named(n).into_iter().filter(|c| !conds.iter().any(|x| x.id() == c.id())).collect();
        let mut out = vec![];
        for p in parts {
            out.extend(self.action(i, p, &mut env.clone(), func));
        }
        out
    }

    /// `range` and `with`: the body runs with `.` rebound to the element or
    /// the value, the else part with `.` as it was.
    fn rebind(&self, i: usize, n: Node<'a>, env: &mut Env, func: &str) -> Vec<Hop> {
        let m = self.m;
        let mut inner = env.clone();
        let mut skip: Vec<usize> = vec![];
        let is_loop = self.is(&m.nodes.loop_, n);
        let bodies = if is_loop { kids(n, &m.fields.body) } else { kids(n, &m.fields.consequence) };
        let value: Val;
        if is_loop {
            let defs: Vec<Node> = named(n).into_iter().filter(|c| self.is(&m.nodes.range, *c)).collect();
            if let Some(d) = defs.first() {
                skip.push(d.id());
                let coll = kids(*d, &m.fields.range).first().map(|r| self.eval(i, *r, env)).unwrap_or(None);
                let elem = kids(*d, &m.fields.value);
                let index = kids(*d, &m.fields.key);
                let src = self.src(i);
                match (elem.first(), index.first()) {
                    (Some(e), Some(k)) => {
                        inner.vars.insert(text(*k, src).to_string(), None);
                        inner.vars.insert(text(*e, src).to_string(), coll.clone());
                    }
                    (Some(e), None) | (None, Some(e)) => {
                        inner.vars.insert(text(*e, src).to_string(), coll.clone());
                    }
                    _ => {}
                }
                value = coll;
            } else {
                let r = kids(n, &m.fields.range);
                skip.extend(r.iter().map(|x| x.id()));
                value = r.first().map(|r| self.eval(i, *r, env)).unwrap_or(None);
            }
        } else {
            let c = kids(n, &m.fields.condition);
            skip.extend(c.iter().map(|x| x.id()));
            value = match c.first() {
                Some(c) if self.is(&m.nodes.assign, *c) => {
                    let v = kids(*c, &m.fields.right).first().map(|x| self.eval(i, *x, env)).unwrap_or(None);
                    if let Some(l) = kids(*c, &m.fields.left).first() {
                        inner.vars.insert(text(*l, self.src(i)).to_string(), v.clone());
                    }
                    v
                }
                Some(c) => self.eval(i, *c, env),
                None => None,
            };
        }
        inner.dot = value.clone();
        skip.extend(bodies.iter().map(|x| x.id()));
        let hops = self.body(i, &bodies, inner, func);
        let mut out = vec![];
        if !hops.is_empty() {
            if value.is_some() {
                out.push(self.hop(i, n, func, Self::primary(&value), hops));
            } else {
                out.extend(hops);
            }
        }
        let rest: Vec<Node<'a>> = named(n).into_iter().filter(|c| !skip.contains(&c.id())).collect();
        out.extend(self.body(i, &rest, env.clone(), func));
        out
    }

    fn assign(&self, i: usize, n: Node<'a>, env: &mut Env) {
        let v = kids(n, &self.m.fields.right).first().map(|x| self.eval(i, *x, env)).unwrap_or(None);
        if let Some(l) = kids(n, &self.m.fields.left).first() {
            env.vars.insert(text(*l, self.src(i)).to_string(), v);
        }
    }

    /// The templates a call of `name` runs: its `define`s, else a file of
    /// that name, else the calling block's own body.
    fn targets(&self, name: &str, own: Option<(usize, Node<'a>)>) -> Vec<(usize, Node<'a>)> {
        let defs = self.defs.get(name).cloned().unwrap_or_default();
        let defines: Vec<(usize, Node<'a>)> =
            defs.iter().copied().filter(|(_, n)| !self.is(&self.m.nodes.template_call, *n)).collect();
        if !defines.is_empty() {
            return defines;
        }
        let files: Vec<(usize, Node<'a>)> = self
            .set
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == name && s.base_line == 1 && !s.key.contains(".go:"))
            .filter_map(|(k, _)| self.root(k).map(|r| (k, r)))
            .collect();
        if !files.is_empty() {
            return files;
        }
        match own {
            Some(o) => vec![o],
            None => defs,
        }
    }

    /// Run each target with `arg` as `.`: a define or block is a `moved` row
    /// holding its body, a file's top-level actions hang directly.
    fn run(&self, name: &str, targets: &[(usize, Node<'a>)], arg: Val) -> Vec<Hop> {
        let key = (name.to_string(), format!("{:?}", arg));
        if self.active.borrow().contains(&key) || self.active.borrow().len() >= MAX_CALLS {
            return vec![];
        }
        self.active.borrow_mut().push(key);
        let mut out = vec![];
        for &(k, n) in targets {
            let env = Env { dot: arg.clone(), vars: HashMap::from([("$".to_string(), arg.clone())]) };
            if self.is(&self.m.nodes.define, n) {
                let hops = self.body(k, &kids(n, &self.m.fields.body), env, name);
                if !hops.is_empty() {
                    out.push(self.hop(k, n, name, vec!["moved".into()], hops));
                }
            } else {
                out.extend(self.body(k, &named(n), env, name));
            }
        }
        self.active.borrow_mut().pop();
        out
    }

    fn is_end(&self, i: usize, n: Node) -> bool {
        match &self.ends {
            None => true,
            Some(e) => e
                .get(&self.set.sources[i].key)
                .is_some_and(|rs| rs.iter().any(|&(s, x)| s >= n.start_byte() && x <= n.end_byte())),
        }
    }

    /// What expression `n` holds of the tracked data.
    fn eval(&self, i: usize, n: Node<'a>, env: &Env) -> Val {
        let m = self.m;
        let src = self.src(i);
        if self.is(&m.nodes.dot, n) {
            return env.dot.clone();
        }
        if self.is(&m.nodes.name, n) {
            let name = text(n, src);
            return env.vars.get(name).cloned().unwrap_or(None);
        }
        if self.is(&m.nodes.field, n) {
            let base = match kids(n, &m.fields.operand).first() {
                Some(o) => self.eval(i, *o, env),
                None => env.dot.clone(),
            };
            let f = kids(n, &m.fields.field).first().map(|f| text(*f, src))?;
            return read(&base, f);
        }
        if self.is(&m.nodes.composite, n) {
            return n.named_child(0).map(|c| self.eval(i, c, env)).unwrap_or(None);
        }
        // Calls, pipelines and anything else: derived from any tracked part.
        let mut v: Val = None;
        let mut stack = named(n);
        while let Some(c) = stack.pop() {
            let x = if self.is(&m.nodes.dot, c)
                || self.is(&m.nodes.name, c)
                || self.is(&m.nodes.field, c)
                || self.is(&m.nodes.composite, c)
            {
                self.eval(i, c, env)
            } else {
                stack.extend(named(c));
                None
            };
            v = join(v, x);
        }
        v.map(|_| (None, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mech() -> Mechanics {
        let d = std::env::temp_dir();
        crate::config::load(&d, "gotmpl").unwrap().mechanics
    }

    fn set(files: &[(&str, &str)], entry: usize) -> TemplateSet {
        let sources: Vec<Source> = files
            .iter()
            .map(|(n, t)| Source { key: n.to_string(), file: n.to_string(), base_line: 1, name: n.to_string(), text: t.to_string() })
            .collect();
        let mut store = TreeStore::default();
        for s in &sources {
            store.insert(&s.key, "gotmpl", s.text.as_bytes().to_vec());
        }
        TemplateSet { sources, entries: vec![Entry::Source(entry)], store }
    }

    fn rows(h: &[Hop]) -> Vec<String> {
        h.iter().flat_map(|h| h.walk()).map(|h| format!("{}:{} {} [{}]", h.file, h.line, h.text, h.tags.join(", "))).collect()
    }

    fn path(p: &[&str]) -> Paths {
        Some(vec![p.iter().map(|s| s.to_string()).collect()])
    }

    #[test]
    fn glob_matches_like_path_match() {
        assert!(glob("*.html.tmpl", "page.html.tmpl"));
        assert!(!glob("*.html.tmpl", "sub/page.html.tmpl"));
        assert!(glob("admin-*.tmpl", "admin-a.tmpl"));
        assert!(glob("p?ge.[a-z]*", "page.tmpl"));
        assert!(!glob("[^p]*", "page"));
    }

    #[test]
    fn field_paths_make_reads_exact_or_untracked() {
        let s = set(&[("p", "{{.Other}}\n{{.Items}}\n{{range .Items}}\n<p>{{.Phone}}</p>\n{{.Name}}\n{{end}}\n")], 0);
        let h = render(&s, &mech(), path(&["Items", "Phone"]), None);
        let r = rows(&h);
        assert!(r.contains(&"p:4 <p>{{.Phone}}</p> [exact, rendered, sink]".to_string()), "{:?}", r);
        assert!(r.contains(&"p:2 {{.Items}} [exact, rendered, sink]".to_string()), "{:?}", r);
        assert!(!r.iter().skip(1).any(|x| x.contains(".Other") || x.contains(".Name")), "{:?}", r);
    }

    #[test]
    fn a_whole_value_makes_every_read_derived_and_variables_carry_it() {
        let s = set(&[("p", "{{$x := .A}}\n{{with .B}}{{.C}}{{end}}\n{{$x | printf \"%s\"}}\n{{\"lit\"}}\n")], 0);
        let r = rows(&render(&s, &mech(), None, None));
        assert!(r.contains(&"p:2 {{with .B}}{{.C}}{{end}} [derived, rendered, sink]".to_string()), "{:?}", r);
        assert!(r.contains(&"p:3 {{$x | printf \"%s\"}} [derived, rendered, sink]".to_string()), "{:?}", r);
        assert!(!r.iter().any(|x| x.contains("lit")), "{:?}", r);
    }

    #[test]
    fn template_and_block_calls_run_the_matching_define() {
        let layout = "<html>\n{{block \"body\" .}}none{{end}}\n</html>\n";
        let page = "{{template \"layout\" .}}\n{{define \"body\"}}\n{{.Phone}}\n{{end}}\n";
        let s = set(&[("page", page), ("layout", layout)], 0);
        let h = render(&s, &mech(), path(&["Phone"]), None);
        let r = rows(&h);
        assert_eq!(
            r,
            vec![
                "page:1 {{template \"layout\" .}} [moved]",
                "page:1 {{template \"layout\" .}} [exact, via layout]",
                "layout:2 {{block \"body\" .}}none{{end}} [exact, via body]",
                "page:2 {{define \"body\"}} [moved]",
                "page:3 {{.Phone}} [exact, rendered, sink]",
            ],
            "{:?}",
            r
        );
    }

    #[test]
    fn a_tail_selector_picks_the_sinks() {
        let s = set(&[("p", "{{.Phone}}\n{{.Phone | printf \"%s\"}}\n")], 0);
        let q = crate::query::parser::parse("template_action").unwrap();
        let r = rows(&render(&s, &mech(), None, Some(&q)));
        assert!(r.iter().skip(1).all(|x| x.ends_with("rendered, sink]")), "{:?}", r);
        let q = crate::query::parser::parse("function#log.Println").unwrap();
        let r = rows(&render(&s, &mech(), None, Some(&q)));
        assert!(r.iter().all(|x| !x.contains("sink")), "{:?}", r);
    }
}
