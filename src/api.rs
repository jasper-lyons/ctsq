//! The library the explore browser calls: one function per question, each
//! answering with a `Tree` built from the same rows the matching ctsq command
//! prints from. Nothing here prints or exits.
//!
//! Every endpoint takes the search root (a directory). Paths in the answer are
//! relative to it. Tests are hidden unless the `Api` was made `with_tests`.
//! A miss comes back as `Err(ApiError(text))` where the text is what the
//! command prints on stderr.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::calltree::{self, CallNode, Direction as CallDir};
use crate::compiler;
use crate::compiler::lang;
use crate::config;
use crate::files::{self, HideOpts, Hider};
use crate::find;
use crate::flow::Hop;
use crate::footer;
use crate::query;
use crate::query_run;
use crate::refs;
use crate::resolve;
use crate::scope;
use crate::scopeof;
use crate::semantic::flows::{DownSetup, FlowOptions, StartTree, UpSetup};

// ── Schema ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    /// The history label, e.g. `callers listWorkspaceAssets`.
    pub query: String,
    pub roots: Vec<Node>,
    /// The `hidden:` footer lines ctsq prints today, verbatim (no newline).
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    pub label: String,
    /// Relative to the search root; empty when the node has no location.
    pub path: String,
    /// 1-based; 0 when the node has no location.
    pub line: usize,
    /// 1-based byte column of the named thing on the line; 0 when unknown.
    pub col: usize,
    /// The source line, trimmed.
    pub text: String,
    pub mark: Mark,
    /// Data-flow tags as `ctsq path` prints them. A call-tree node that
    /// closes a cycle carries the tag `cycle`.
    pub tags: Vec<String>,
    /// What ctsq prints as `?`.
    pub guessed: bool,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mark {
    #[default]
    None,
    Fork,
    Loop,
    Unresolved,
    External(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub signature: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// The text ctsq would print on stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.trim_end())
    }
}

impl std::error::Error for ApiError {}

fn err<T>(s: impl Into<String>) -> Result<T, ApiError> {
    Err(ApiError(s.into()))
}

// ── Free functions: tests hidden, the way the CLI does ───────────────────────

pub fn callers(root: &Path, name: &str, depth: usize) -> Result<Tree, ApiError> {
    Api::new(root).callers(name, depth)
}
pub fn callees(root: &Path, name: &str, depth: usize) -> Result<Tree, ApiError> {
    Api::new(root).callees(name, depth)
}
pub fn usages(root: &Path, name: &str) -> Result<Tree, ApiError> {
    Api::new(root).usages(name)
}
pub fn definition(root: &Path, name: &str) -> Result<Tree, ApiError> {
    Api::new(root).definition(name)
}
pub fn scope_of(root: &Path, path: &str, line: usize) -> Result<Scope, ApiError> {
    Api::new(root).scope_of(path, line)
}
pub fn flow(root: &Path, path: &str, line: usize, col: usize, dir: Direction) -> Result<Tree, ApiError> {
    Api::new(root).flow(path, line, col, dir)
}
pub fn find(root: &Path, query: &str, key: Option<&str>) -> Result<Tree, ApiError> {
    Api::new(root).find(query, key)
}
pub fn text_search(root: &Path, needle: &str) -> Result<Tree, ApiError> {
    Api::new(root).text_search(needle)
}
pub fn start(root: &Path, input: &str) -> Result<Tree, ApiError> {
    Api::new(root).start(input)
}

// ── The endpoints ────────────────────────────────────────────────────────────

/// A search root and how it hides things. `Api::new(root)` hides tests;
/// `.with_tests()` is the CLI's `--tests`; `.with_no_ignore()` is `--no-ignore`.
#[derive(Debug, Clone)]
pub struct Api {
    pub root: PathBuf,
    pub hide: HideOpts,
    pub no_ignore: bool,
}

/// What one call walks: the root as given, its files, and the hider over them.
struct Env {
    root: String,
    jobs: Vec<(String, &'static str)>,
    hider: Hider,
}

impl Api {
    pub fn new(root: &Path) -> Api {
        Api { root: root.to_path_buf(), hide: HideOpts::default(), no_ignore: false }
    }
    pub fn with_tests(mut self) -> Api {
        self.hide.tests = true;
        self
    }
    pub fn with_no_ignore(mut self) -> Api {
        self.no_ignore = true;
        self
    }

    fn env(&self) -> Result<Env, ApiError> {
        let root = self.root.display().to_string();
        if !self.root.exists() {
            return err(format!("{}\n", crate::miss::no_such_file(&root)));
        }
        let jobs = files::resolve_jobs(&root, None, self.no_ignore);
        let mut hider = Hider::new(&self.hide);
        hider.add_jobs(&root, &jobs);
        Ok(Env { root, jobs, hider })
    }

    /// The path as the walk holds it, from a path relative to the root.
    fn walked(&self, env: &Env, rel: &str) -> String {
        let p = Path::new(rel);
        if p.is_absolute() || rel.starts_with(&format!("{}/", env.root.trim_end_matches('/'))) {
            rel.to_string()
        } else {
            format!("{}/{}", env.root.trim_end_matches('/'), rel.trim_start_matches("./"))
        }
    }

    /// A walked path relative to the root.
    fn rel(&self, env: &Env, file: &str) -> String {
        let prefix = format!("{}/", env.root.trim_end_matches('/'));
        file.strip_prefix(&prefix).unwrap_or(file).trim_start_matches("./").to_string()
    }

    /// A node at a walked file and line, with the source line filled in and the
    /// column of `name` on it.
    fn located(&self, env: &Env, label: &str, file: &str, line: usize, name: &str) -> Node {
        let (text, col) = line_info(file, line, name);
        Node { label: label.to_string(), path: self.rel(env, file), line, col, text, ..Node::default() }
    }

    /// A node at a definition. Its label is the name with the Go receiver type
    /// in front (`Handler.listWorkspaceAssets`) where the definition is a method.
    fn def_node(&self, env: &Env, file: &str, line: usize, name: &str) -> Node {
        let label = qualified(file, line, name);
        self.located(env, &label, file, line, name)
    }

    pub fn callers(&self, name: &str, depth: usize) -> Result<Tree, ApiError> {
        self.calls(CallDir::Up, name, depth)
    }

    pub fn callees(&self, name: &str, depth: usize) -> Result<Tree, ApiError> {
        self.calls(CallDir::Down, name, depth)
    }

    fn calls(&self, dir: CallDir, name: &str, depth: usize) -> Result<Tree, ApiError> {
        let mut env = self.env()?;
        // A qualified name (`Handler.listWorkspaceAssets`) is a definition
        // lookup: the walk runs on the bare name and keeps the roots that are
        // that definition.
        let mut only: Option<Vec<(String, usize)>> = None;
        if name.contains('.') || name.contains("::") {
            let mut scan = resolve::DefScan::new(&env.jobs);
            scan.cover_exprs(&[name.to_string()]);
            let rows = resolve::def_rows(name, &env.jobs, &mut scan, &mut Vec::new());
            if !rows.is_empty() {
                only = Some(rows.into_iter().map(|(f, l, _)| (f, l)).collect());
            }
        }
        let walk_name = if only.is_some() { bare(name).to_string() } else { name.to_string() };
        let names = [walk_name];
        let mut t = calltree::build(dir, &names, &env.jobs, depth, &mut env.hider);
        footer::settle(&mut t.hidden);
        if let Some(keep) = &only {
            for n in &mut t.names {
                n.roots.retain(|r| r.def.as_ref().is_some_and(|d| keep.iter().any(|(f, l)| *f == d.file && *l == d.line)));
            }
            if t.names.iter().all(|n| n.roots.is_empty()) {
                t.resolved = false;
                t.missing = vec![name.to_string()];
            }
        }
        if !t.resolved {
            let mut out = calltree::render_many(&t, &env.root, &env.jobs, None);
            if only.is_some() && out.err.is_empty() {
                out.err = resolve::not_found_line(&[name.to_string()], &env.jobs);
            }
            return err(out.err);
        }
        let word = if dir == CallDir::Up { "callers" } else { "callees" };
        let mut roots = Vec::new();
        for n in &t.names {
            for r in &n.roots {
                let mut node = match &r.def {
                    Some(d) => self.def_node(&env, &d.file, d.line, bare(&n.name)),
                    None => Node { label: n.name.clone(), ..Node::default() },
                };
                node.children = r.children.iter().map(|c| self.call_node(&env, c)).collect();
                roots.push(node);
            }
        }
        Ok(Tree { query: format!("{} {}", word, name), roots, hidden: hidden_lines(&t.hidden) })
    }

    fn call_node(&self, env: &Env, c: &CallNode) -> Node {
        let mut node = match &c.def {
            Some(d) => self.def_node(env, &d.file, d.line, bare(&c.name)),
            // No target: the branch stops at the call, shown where it is written.
            None => {
                let mut n = self.located(env, &c.name, &c.site.file, c.site.line, bare(&c.name));
                n.mark = Mark::Unresolved;
                n
            }
        };
        node.guessed = c.confidence == Some(resolve::Confidence::Guess);
        if c.cycle {
            node.tags.push("cycle".to_string());
        }
        node.children = c.children.iter().map(|k| self.call_node(env, k)).collect();
        node
    }

    pub fn usages(&self, name: &str) -> Result<Tree, ApiError> {
        let mut env = self.env()?;
        let names = [name.to_string()];
        let mut r = match refs::build(&names, &env.jobs, &mut env.hider) {
            Ok(r) => r,
            Err(out) => return err(out.err),
        };
        footer::settle(&mut r.hidden);
        let rows: Vec<&refs::RefRow> = r.names.iter().flat_map(|n| n.rows.iter()).collect();
        if rows.is_empty() {
            if r.hidden.any() {
                return err(footer::nothing_shown(&format!("no reference to {}", name), &env.root, env.jobs.len(), &r.hidden));
            }
            return err(resolve::not_found_line(&names, &env.jobs));
        }
        let roots = rows
            .iter()
            .map(|x| {
                let line = x.row.line;
                let raw = String::from_utf8_lossy(&x.source).lines().nth(line.saturating_sub(1)).unwrap_or("").to_string();
                Node {
                    label: name.to_string(),
                    path: self.rel(&env, &x.file),
                    line,
                    col: col_of(&x.source, x.row.start_byte),
                    text: raw.trim().to_string(),
                    ..Node::default()
                }
            })
            .collect();
        Ok(Tree { query: format!("usages {}", name), roots, hidden: hidden_lines(&r.hidden) })
    }

    pub fn definition(&self, name: &str) -> Result<Tree, ApiError> {
        let mut env = self.env()?;
        let mut scan = resolve::DefScan::new(&env.jobs);
        scan.cover_exprs(&[name.to_string()]);
        let mut seen = Vec::new();
        let rows = resolve::def_rows(name, &env.jobs, &mut scan, &mut seen);
        if rows.is_empty() {
            return err(format!(
                "no definition for '{}' in {} ({} {})\n",
                name,
                env.root,
                env.jobs.len(),
                if env.jobs.len() == 1 { "file" } else { "files" }
            ));
        }
        let mut hidden = footer::Hidden::default();
        let rows = env.hider.keep(rows, |(f, l, _)| (f.clone(), *l), &mut hidden);
        footer::settle(&mut hidden);
        if rows.is_empty() {
            return err(footer::nothing_shown(&format!("no definition to {}", name), &env.root, env.jobs.len(), &hidden));
        }
        let roots = rows
            .iter()
            .map(|(file, line, conf)| {
                let mut n = self.def_node(&env, file, *line, bare(name));
                n.guessed = *conf == resolve::Confidence::Guess;
                n
            })
            .collect();
        Ok(Tree { query: format!("definition {}", name), roots, hidden: hidden_lines(&hidden) })
    }

    pub fn scope_of(&self, path: &str, line: usize) -> Result<Scope, ApiError> {
        let env = self.env()?;
        let file = self.walked(&env, path);
        let coord = scopeof::Coord { path: file.clone(), line, col: None };
        let hits = scopeof::build(&[coord], &scopeof::Options::default(), &self.hide);
        let hit = match hits.into_iter().next() {
            Some(Ok(h)) => h,
            Some(Err(e)) => return err(e),
            None => return err("no coordinate"),
        };
        let Some(s) = hit.scope else {
            return err(format!("{}:{} is at the top level of the file, no scope holds it\n", path, line));
        };
        let body = std::fs::read(&file)
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .lines()
                    .skip(s.start_line.saturating_sub(1))
                    .take(s.end_line + 1 - s.start_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(Scope { path: self.rel(&env, &s.path), start: s.start_line, end: s.end_line, signature: s.signature, body })
    }

    /// Data flow from the value at `path:line:col` (1-based, byte column).
    /// Forward is `ctsq path 'X ==>'`, Backward is `'X <=='`.
    pub fn flow(&self, path: &str, line: usize, col: usize, dir: Direction) -> Result<Tree, ApiError> {
        let env = self.env()?;
        let file = self.walked(&env, path);
        let words = Rc::new(resolve::scoped::Words::new(&env.jobs));
        let store = resolve::TreeStore::lazy(&env.jobs, words.clone());
        let graph = if resolve::scoped::whole() {
            crate::semantic::flows::whole_graph(&resolve::graph::build_from_store(&store))
        } else {
            resolve::scoped::FlowGraph::lazy(&env.jobs, words)
        };
        let scan = Path::new(&env.root);
        let abs = std::fs::canonicalize(scan).unwrap_or_else(|_| scan.to_path_buf());
        let repo = config::find_repo_root(&abs);
        let opts = FlowOptions::default();
        let found: Option<StartTree> = match dir {
            Direction::Forward => {
                let setup = DownSetup::new(&store, &graph, &repo, scan, None, true, &opts).map_err(|e| ApiError(format!("{}\n", e)))?;
                setup.walk().tree_at(&file, line, col).map_err(|e| ApiError(format!("{}\n", e)))?
            }
            Direction::Backward => {
                let setup = UpSetup::new(&store, &graph, &repo, scan, None, true, &opts).map_err(|e| ApiError(format!("{}\n", e)))?;
                setup.walk().tree_at(&file, line, col).map_err(|e| ApiError(format!("{}\n", e)))?
            }
        };
        let Some(hop) = found.and_then(|t| t.hop) else {
            return err(format!("no value to follow at {}:{}:{}\n", path, line, col));
        };
        // The tracked name, to find the same name on the other rows.
        let tracked = std::fs::read(&file).ok().and_then(|b| ident_at(&b, line, col));
        let mut syn = Syntax::default();
        let mut root = self.hop_node(&env, &hop, 0, dir, tracked.as_deref(), &mut syn);
        if root.col == 0 {
            root.col = col;
        }
        let arrow = if dir == Direction::Forward { "==>" } else { "<==" };
        Ok(Tree { query: format!("flow {}:{}:{} {}", path, line, col, arrow), roots: vec![root], hidden: vec![] })
    }

    fn hop_node(&self, env: &Env, h: &Hop, depth: usize, dir: Direction, tracked: Option<&str>, syn: &mut Syntax) -> Node {
        let tags: Vec<String> = h
            .tags
            .iter()
            .filter(|t| !(depth == 0 && dir == Direction::Forward && *t == "source") && *t != crate::flow::upward::END)
            .cloned()
            .collect();
        let children: Vec<Node> = h.children.iter().map(|c| self.hop_node(env, c, depth + 1, dir, tracked, syn)).collect();
        let (_, col) = line_info(&h.file, h.line, tracked.unwrap_or(""));
        let mark = if children.len() >= 2 {
            Mark::Fork
        } else if syn.in_loop(&h.file, h.line) {
            Mark::Loop
        } else {
            Mark::None
        };
        Node {
            label: if h.func.is_empty() { h.text.clone() } else { h.func.clone() },
            path: self.rel(env, &h.file),
            line: h.line,
            col,
            text: h.text.clone(),
            mark,
            tags,
            guessed: false,
            children,
        }
    }

    /// `ctsq find QUERY [KEY]`, run over the root.
    pub fn find(&self, query: &str, key: Option<&str>) -> Result<Tree, ApiError> {
        let root = &self.root;
        if !root.exists() {
            return err(format!("{}\n", crate::miss::no_such_file(&root.display().to_string())));
        }
        let abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let queries = config::queries::load(&abs).map_err(|e| ApiError(format!("error: {}\n", e)))?;
        let glossary = config::glossary::load(&abs).map_err(|e| ApiError(format!("error: {}\n", e)))?;
        let mut scan = find::Scan::new(root, self.no_ignore);
        let mut found = match find::rows(&queries, query, key, &mut scan, &self.hide, glossary.as_ref()) {
            Ok(f) => f,
            Err(r) => return err(r.out),
        };
        footer::settle(&mut found.hidden);
        let strip = |f: &str| f.strip_prefix(&found.prefix).unwrap_or(f).trim_start_matches("./").to_string();
        let mut roots = Vec::new();
        for g in &found.groups {
            for r in &g.rows {
                let walked = if Path::new(&r.file).is_absolute() || r.file.starts_with(&found.prefix) {
                    r.file.clone()
                } else {
                    format!("{}{}", found.prefix, r.file)
                };
                let (text, col) = line_info(&walked, r.line, &r.key);
                let mut node = Node { label: r.key.clone(), path: strip(&r.file), line: r.line, col, text, ..Node::default() };
                if let Some(p) = &r.params {
                    node.tags = p.split_whitespace().map(|s| s.to_string()).collect();
                }
                if !r.target.is_empty() {
                    let mut child = Node { label: r.target.clone(), ..Node::default() };
                    if let Some((f, l)) = r.target_loc.rsplit_once(':') {
                        if let Ok(l) = l.parse::<usize>() {
                            let w = if f.starts_with(&found.prefix) || Path::new(f).is_absolute() { f.to_string() } else { format!("{}{}", found.prefix, f) };
                            let (text, col) = line_info(&w, l, bare(&r.target));
                            child.path = strip(f);
                            child.line = l;
                            child.col = col;
                            child.text = text;
                        }
                    }
                    node.children.push(child);
                }
                roots.push(node);
            }
        }
        if roots.is_empty() {
            let phrase = match key {
                Some(k) => format!("no {} matching {}", query, k),
                None => format!("no {}", query),
            };
            if found.hidden.any() {
                return err(footer::nothing_shown(&phrase, &found.root, found.files, &found.hidden));
            }
            return err(format!("{} in {} ({} files)\n", phrase, found.root, found.files));
        }
        let label = match key {
            Some(k) => format!("find {} {}", query, k),
            None => format!("find {}", query),
        };
        Ok(Tree { query: label, roots, hidden: hidden_lines(&found.hidden) })
    }

    /// `-q 'literal#/needle/i'`, plus JSX text and comments. Case-insensitive.
    pub fn text_search(&self, needle: &str) -> Result<Tree, ApiError> {
        if needle.is_empty() {
            return err("nothing to search for\n");
        }
        let mut env = self.env()?;
        let pattern = format!("(?i){}", regex::escape(needle).replace('/', "\\/"));
        let mut hidden = footer::Hidden::default();
        let mut seen = std::collections::HashSet::new();
        let mut hits: Vec<(String, usize, usize, Node)> = Vec::new();
        for kind in ["literal", "jsx_text", "comment"] {
            let q = format!("{}#/{}/", kind, pattern);
            let ast = query::parse(&q).map_err(|e| ApiError(crate::miss::query_error_report(&q, &e)))?;
            // A kind some grammar lacks (jsx_text) searches the grammars that have it.
            let mut ok: HashMap<&'static str, bool> = HashMap::new();
            let jobs: Vec<(String, &'static str)> = env
                .jobs
                .iter()
                .filter(|(_, l)| {
                    *ok.entry(l).or_insert_with(|| {
                        let Some(lg) = lang::for_name(l) else { return false };
                        compiler::validate(&ast, lg.as_ref(), l).is_ok() && compiler::check_compiles(&ast, lg.as_ref(), l, &q).is_ok()
                    })
                })
                .cloned()
                .collect();
            let (files, h) = query_run::rows(&ast, &[], &jobs, &mut env.hider, false);
            resolve::merge_hidden(&mut hidden, &h);
            for f in files {
                if let query_run::Outcome::Rows { source, rows } = &f.outcome {
                    for r in rows {
                        if !seen.insert((f.file.clone(), r.start_byte)) {
                            continue;
                        }
                        let raw = String::from_utf8_lossy(source).lines().nth(r.line.saturating_sub(1)).unwrap_or("").to_string();
                        let end = r.end_byte.min(source.len());
                        let node_text = String::from_utf8_lossy(&source[r.start_byte.min(end)..end]).into_owned();
                        let label = node_text.lines().next().unwrap_or("").trim().to_string();
                        let node = Node {
                            label,
                            path: self.rel(&env, &f.file),
                            line: r.line,
                            col: col_of(source, r.start_byte),
                            text: raw.trim().to_string(),
                            ..Node::default()
                        };
                        hits.push((node.path.clone(), r.line, r.start_byte, node));
                    }
                }
            }
        }
        footer::settle(&mut hidden);
        if hits.is_empty() {
            if hidden.any() {
                return err(footer::nothing_shown(&format!("no text {}", needle), &env.root, env.jobs.len(), &hidden));
            }
            return err(format!("no match for text {} in {} ({} files)\n", needle, env.root, env.jobs.len()));
        }
        hits.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));
        Ok(Tree { query: format!("text /{}", needle), roots: hits.into_iter().map(|h| h.3).collect(), hidden: hidden_lines(&hidden) })
    }

    /// The `:` prompt. `/x` searches text, a URL finds its route,
    /// `path:line` names the scope holding the line, anything else is a definition.
    pub fn start(&self, input: &str) -> Result<Tree, ApiError> {
        let input = input.trim();
        if let Some(needle) = input.strip_prefix('/') {
            return self.text_search(needle);
        }
        if input.starts_with("http://") || input.starts_with("https://") {
            return self.find("route", Some(input));
        }
        if let Ok(c) = scopeof::parse_coord(input) {
            let env = self.env()?;
            if Path::new(&self.walked(&env, &c.path)).is_file() {
                let s = self.scope_of(&c.path, c.line)?;
                let node = Node {
                    label: s.signature.clone(),
                    path: s.path.clone(),
                    line: s.start,
                    text: s.signature.clone(),
                    ..Node::default()
                };
                return Ok(Tree { query: input.to_string(), roots: vec![node], hidden: vec![] });
            }
        }
        self.definition(input)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn hidden_lines(h: &footer::Hidden) -> Vec<String> {
    footer::hidden_lines(h).lines().map(|l| l.to_string()).collect()
}

/// `Handler.listWorkspaceAssets` -> `listWorkspaceAssets`.
fn bare(name: &str) -> &str {
    crate::explain::bare_name(name)
}

/// The trimmed source line, and the 1-based byte column of `name` on it (0 when
/// absent or empty).
fn line_info(file: &str, line: usize, name: &str) -> (String, usize) {
    let Some(src) = crate::parsed::source(file) else { return (String::new(), 0) };
    let text = String::from_utf8_lossy(&src);
    let raw = text.lines().nth(line.saturating_sub(1)).unwrap_or("");
    let col = if name.is_empty() { 0 } else { raw.find(name).map(|i| i + 1).unwrap_or(0) };
    (raw.trim().to_string(), col)
}

/// `Type.name` for a Go method defined on the line, else `name`.
fn qualified(file: &str, line: usize, name: &str) -> String {
    if !file.ends_with(".go") {
        return name.to_string();
    }
    let Some(src) = crate::parsed::source(file) else { return name.to_string() };
    let text = String::from_utf8_lossy(&src);
    let raw = text.lines().nth(line.saturating_sub(1)).unwrap_or("");
    let Some(rest) = raw.trim_start().strip_prefix("func") else { return name.to_string() };
    let Some(inner) = rest.trim_start().strip_prefix('(') else { return name.to_string() };
    let Some(end) = inner.find(')') else { return name.to_string() };
    // `(h *Handler)`, `(Handler)`, `(s *Store[K, V])`: the type follows the optional receiver name.
    let recv = inner[..end].trim();
    let ty = match recv.split_once(char::is_whitespace) {
        Some((_, t)) => t.trim(),
        None => recv,
    };
    let ty = ty.trim_start_matches('*').split('[').next().unwrap_or("").trim();
    if ty.is_empty() || !inner[end + 1..].trim_start().starts_with(name) {
        return name.to_string();
    }
    format!("{}.{}", ty, name)
}

/// 1-based byte column of `byte` on its line.
fn col_of(src: &[u8], byte: usize) -> usize {
    let byte = byte.min(src.len());
    let start = src[..byte].iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
    byte - start + 1
}

/// The identifier at a 1-based line and byte column.
fn ident_at(src: &[u8], line: usize, col: usize) -> Option<String> {
    let at = crate::semantic::flows::byte_at(src, line, col)?;
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut s = at;
    while s > 0 && word(src[s - 1]) {
        s -= 1;
    }
    let mut e = at;
    while e < src.len() && word(src[e]) {
        e += 1;
    }
    (e > s).then(|| String::from_utf8_lossy(&src[s..e]).into_owned())
}

/// Parsed files, kept so the loop test parses each once.
#[derive(Default)]
struct Syntax {
    files: HashMap<String, Option<(tree_sitter::Tree, Rc<Vec<u8>>)>>,
}

const LOOPS: &[&str] = &[
    "for_statement",
    "for_in_statement",
    "for_of_statement",
    "while_statement",
    "do_statement",
    "range_action",
    "for_expression",
    "while_expression",
    "loop_expression",
    "for_range_loop",
];

const FUNCTIONS: &[&str] = &[
    "function_declaration",
    "method_declaration",
    "func_literal",
    "function_item",
    "closure_expression",
    "function_definition",
    "function_expression",
    "arrow_function",
    "method_definition",
];

impl Syntax {
    /// Whether `line` sits inside a for, while or range block of the function
    /// that holds it. The first token on the line decides.
    fn in_loop(&mut self, file: &str, line: usize) -> bool {
        let slot = self.files.entry(file.to_string()).or_insert_with(|| {
            let lang_name = lang::lang_for_path(file)?;
            let src = crate::parsed::source(file)?;
            let tree = scope::parse(lang_name, &src)?;
            Some((tree, src))
        });
        let Some((tree, src)) = slot else { return false };
        let (first, _) = scope::line_bytes(src, line);
        let mut at = first;
        while at < src.len() && (src[at] == b' ' || src[at] == b'\t') {
            at += 1;
        }
        let Some(mut n) = tree.root_node().descendant_for_byte_range(at, at) else { return false };
        loop {
            if LOOPS.contains(&n.kind()) {
                return true;
            }
            if FUNCTIONS.contains(&n.kind()) {
                return false;
            }
            match n.parent() {
                Some(p) => n = p,
                None => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(rel)
    }

    /// The `file:LINE` tokens of CLI text, by file name: `[main.go:16]`,
    /// `path/f.go:5:` and the `main.go:20` that `find` prints.
    fn cli_locs(text: &str) -> BTreeSet<(String, usize)> {
        let re = regex::Regex::new(r"([\w./-]+\.\w+):(\d+)").unwrap();
        re.captures_iter(text).map(|c| (base(&c[1]), c[2].parse().unwrap())).collect()
    }

    fn base(p: &str) -> String {
        p.rsplit('/').next().unwrap_or(p).to_string()
    }

    fn api_locs(t: &Tree) -> BTreeSet<(String, usize)> {
        fn walk(n: &Node, out: &mut BTreeSet<(String, usize)>) {
            if n.line > 0 {
                out.insert((base(&n.path), n.line));
            }
            n.children.iter().for_each(|c| walk(c, out));
        }
        let mut out = BTreeSet::new();
        t.roots.iter().for_each(|r| walk(r, &mut out));
        out
    }

    fn labels(t: &Tree) -> Vec<String> {
        fn walk(n: &Node, out: &mut Vec<String>) {
            out.push(n.label.clone());
            n.children.iter().for_each(|c| walk(c, out));
        }
        let mut out = vec![];
        t.roots.iter().for_each(|r| walk(r, &mut out));
        out
    }

    /// What the CLI's builder and printer make of the same call.
    fn cli_env(root: &Path, tests: bool) -> (String, Vec<(String, &'static str)>, Hider) {
        let root = root.display().to_string();
        let jobs = files::resolve_jobs(&root, None, false);
        let mut hider = Hider::new(&HideOpts { tests, exclude: vec![] });
        hider.add_jobs(&root, &jobs);
        (root, jobs, hider)
    }

    fn api(root: &Path, tests: bool) -> Api {
        let a = Api::new(root);
        if tests { a.with_tests() } else { a }
    }

    #[test]
    fn callers_and_callees_match_the_cli() {
        let root = fixture("path");
        for tests in [false, true] {
            for (dir, name, depth) in [(CallDir::Up, "d", 5), (CallDir::Down, "main", 5)] {
                let (w, jobs, mut hider) = cli_env(&root, tests);
                let cli = calltree::report_many(dir, &[name.to_string()], &w, &jobs, depth, &mut hider, None);
                let a = api(&root, tests);
                let t = if dir == CallDir::Up { a.callers(name, depth) } else { a.callees(name, depth) }.unwrap();
                // The CLI heads a single root with a bare `name()`, no location.
                let below = Tree { roots: t.roots.iter().flat_map(|r| r.children.clone()).collect(), ..Tree::default() };
                assert_eq!(api_locs(&below), cli_locs(&cli.out), "{name} tests={tests}");
                for l in labels(&t) {
                    let bare = l.rsplit('.').next().unwrap();
                    assert!(cli.out.contains(&format!("{}()", bare)), "{l} in\n{}", cli.out);
                }
                assert_eq!(t.hidden, Vec::<String>::new());
            }
        }
        // A Go method is labelled with its receiver type.
        let t = callees(&root, "main", 1).unwrap();
        assert!(labels(&t).contains(&"Store.Save".to_string()), "{:?}", labels(&t));
        // A qualified name finds the same definition.
        let t = callees(&root, "Store.Save", 1).unwrap();
        assert_eq!(t.roots.len(), 1);
        assert_eq!(t.roots[0].label, "Store.Save");
        assert_eq!(t.roots[0].children[0].label, "c");
    }

    #[test]
    fn a_missing_name_is_an_error_with_the_cli_text() {
        let root = fixture("path");
        let e = callers(&root, "nosuchfn", 3).unwrap_err();
        assert!(e.0.contains("not found: nosuchfn"), "{}", e.0);
        assert!(definition(&root, "nosuchfn").is_err());
        assert!(usages(&root, "nosuchfn").is_err());
        assert!(callers(Path::new("/no/such/dir"), "a", 1).is_err());
    }

    #[test]
    fn usages_match_the_cli_with_tests_hidden_and_shown() {
        let root = fixture("hidden");
        for (tests, rows, hidden) in [(false, 2, vec!["hidden: 2 in tests (--tests)"]), (true, 4, vec![])] {
            let (w, jobs, mut hider) = cli_env(&root, tests);
            let cli = refs::report(&["HiddenLimit".to_string()], &w, &jobs, &mut hider, None);
            let t = api(&root, tests).usages("HiddenLimit").unwrap();
            assert_eq!(t.roots.len(), rows);
            assert_eq!(api_locs(&t), cli_locs(&cli.out));
            assert_eq!(t.hidden, hidden);
            assert!(cli.out.contains(hidden.first().copied().unwrap_or("")));
            assert!(t.roots.iter().all(|n| n.col > 0 && n.text.contains("HiddenLimit")));
        }
    }

    #[test]
    fn definition_matches_the_cli_with_tests_hidden_and_shown() {
        let root = fixture("hidden");
        for tests in [false, true] {
            let (w, jobs, mut hider) = cli_env(&root, tests);
            let cli = resolve::def_report(&["HiddenLimit".to_string()], &w, &jobs, &mut hider, None, None);
            let t = api(&root, tests).definition("HiddenLimit").unwrap();
            assert_eq!(api_locs(&t), cli_locs(&cli.out));
            assert_eq!(t.roots[0].label, "HiddenLimit");
            assert_eq!(t.roots[0].text, "const HiddenLimit = 3");
            assert_eq!(t.roots[0].col, 7);
        }
        let t = definition(&fixture("path"), "Store.Save").unwrap();
        assert_eq!((t.roots[0].path.as_str(), t.roots[0].line, t.roots[0].label.as_str()), ("main.go", 32, "Store.Save"));
    }

    #[test]
    fn scope_of_matches_the_cli() {
        let root = fixture("path");
        let s = scope_of(&root, "main.go", 17).unwrap();
        let cli = scopeof::report(
            &[scopeof::Coord { path: format!("{}/main.go", root.display()), line: 17, col: None }],
            &scopeof::Options::default(),
            &HideOpts::default(),
        );
        assert_eq!((s.path.as_str(), s.start, s.end), ("main.go", 16, 19));
        assert_eq!(s.signature, "func a() {");
        assert_eq!(s.body, "func a() {\n\tb()\n\td()\n}");
        assert!(cli.out.contains(&format!("main.go:{}-{}", s.start, s.end)), "{}", cli.out);
        assert!(scope_of(&root, "main.go", 1).is_err());
        assert!(scope_of(&root, "nope.go", 1).is_err());
        // With tests shown the answer is the same.
        assert_eq!(Api::new(&root).with_tests().scope_of("main.go", 17).unwrap(), s);
    }

    #[test]
    fn find_matches_the_cli_with_tests_hidden_and_shown() {
        let root = fixture("queries/go-chi");
        for tests in [false, true] {
            let abs = std::fs::canonicalize(&root).unwrap();
            let queries = config::queries::load(&abs).unwrap();
            let mut scan = find::Scan::new(&root, false);
            let hide = HideOpts { tests, exclude: vec![] };
            let cli = find::report_with(&queries, "route", Some("/users/7"), &mut scan, &hide, None);
            let t = api(&root, tests).find("route", Some("/users/7")).unwrap();
            assert_eq!(api_locs(&t), cli_locs(&cli.out), "{}", cli.out);
            assert_eq!(t.roots.len(), 2);
            for r in &t.roots {
                assert!(cli.out.contains(&r.label));
                assert_eq!(r.tags, vec!["id=7"]);
                assert!(!r.children[0].label.is_empty() && r.children[0].line > 0);
            }
            let all = api(&root, tests).find("route", None).unwrap();
            assert_eq!(all.roots.len(), 4);
        }
        assert!(find(&root, "nosuchquery", None).is_err());
    }

    #[test]
    fn text_search_finds_text_jsx_and_comments() {
        let t = text_search(&fixture("queries/jsx"), "ALL rights").unwrap();
        assert_eq!(t.roots.len(), 1);
        assert_eq!((t.roots[0].path.as_str(), t.roots[0].line), ("src/Card.tsx", 14));
        assert_eq!(t.roots[0].label, "All rights reserved");
        for tests in [false, true] {
            let t = api(&fixture("hidden"), tests).text_search("hiddenlimit is read").unwrap();
            assert_eq!(t.roots.len(), 1);
            assert_eq!((t.roots[0].path.as_str(), t.roots[0].line), ("limit.go", 3));
        }
        assert!(text_search(&fixture("hidden"), "zzzz-nothing").is_err());
    }

    #[test]
    fn start_routes_the_input() {
        let jsx = fixture("queries/jsx");
        assert_eq!(start(&jsx, "/all rights").unwrap().roots[0].line, 14);
        let hidden = fixture("hidden");
        let mut d = definition(&hidden, "HiddenLimit").unwrap();
        d.query = "definition HiddenLimit".to_string();
        assert_eq!(start(&hidden, "HiddenLimit").unwrap(), d);
        let path = fixture("path");
        let t = start(&path, "main.go:17").unwrap();
        assert_eq!((t.roots[0].path.as_str(), t.roots[0].line, t.roots[0].text.as_str()), ("main.go", 16, "func a() {"));
        // A URL goes to the route query.
        let chi = fixture("queries/go-chi");
        let t = start(&chi, "http://localhost:3000/users/7").unwrap();
        assert_eq!(t.roots.len(), 2);
        assert_eq!(t.roots[0].label, "/users/{id}");
    }

    fn explore() -> Api {
        Api::new(&fixture("explore-api/go")).with_no_ignore()
    }

    #[test]
    fn flow_marks_the_node_before_an_if_else_split_as_fork() {
        let t = explore().flow("fork.go", 5, 11, Direction::Forward).unwrap();
        let start = &t.roots[0];
        assert_eq!((start.line, start.mark.clone()), (5, Mark::None));
        let split = &start.children[0];
        assert_eq!((split.line, split.text.as_str(), split.mark.clone()), (6, "y := x", Mark::Fork));
        assert_eq!(split.tags, vec!["exact"]);
        let lines: Vec<usize> = split.children.iter().map(|c| c.line).collect();
        assert_eq!(lines, vec![8, 10]);
        assert!(split.children.iter().all(|c| c.mark == Mark::None && c.tags == vec!["outside"]));
        assert!(split.children.iter().all(|c| c.path == "fork.go"));
    }

    #[test]
    fn flow_marks_the_node_inside_a_loop_as_loop() {
        let t = explore().flow("loop.go", 5, 11, Direction::Forward).unwrap();
        let y = &t.roots[0].children[0];
        assert_eq!((y.line, y.mark.clone()), (6, Mark::None));
        assert_eq!(y.children.len(), 1);
        let inside = &y.children[0];
        assert_eq!((inside.line, inside.mark.clone()), (8, Mark::Loop));
    }

    #[test]
    fn flow_errors_and_backward() {
        let a = explore();
        assert!(a.flow("nope.go", 1, 1, Direction::Forward).is_err());
        assert!(a.flow("fork.go", 1, 1, Direction::Forward).is_err());
        // Backward from y in the first print reaches y := x and then the parameter.
        let t = a.flow("fork.go", 8, 22, Direction::Backward).unwrap();
        assert_eq!(t.roots[0].line, 8);
        assert!(t.query.ends_with("<=="));
        let mut lines = vec![];
        let mut n = &t.roots[0];
        loop {
            lines.push(n.line);
            match n.children.first() {
                Some(c) => n = c,
                None => break,
            }
        }
        assert_eq!(lines, vec![8, 6, 5]);
    }
}

/// The three checks on Mavexa that `plans/explore/check-mavexa.sh` runs. The
/// root is $MAVEXA, default ~/workspace/projects/mavexa/mavexa; a missing root
/// skips the check.
#[cfg(test)]
mod mavexa {
    use super::*;

    fn root() -> Option<PathBuf> {
        let r = match std::env::var_os("MAVEXA") {
            Some(r) => PathBuf::from(r),
            None => PathBuf::from(std::env::var_os("HOME")?).join("workspace/projects/mavexa/mavexa"),
        };
        if r.is_dir() {
            Some(r)
        } else {
            eprintln!("skipped: no Mavexa at {}", r.display());
            None
        }
    }

    fn all(n: &Node, out: &mut Vec<String>) {
        out.push(n.label.clone());
        n.children.iter().for_each(|c| all(c, out));
    }

    #[test]
    #[ignore]
    fn start_finds_the_assets_route() {
        let Some(root) = root() else { return };
        let t = start(&root, "http://localhost:3000/w/ws_101da8083e228c9f/assets").unwrap();
        let labels: Vec<&str> = t.roots.iter().map(|n| n.label.as_str()).collect();
        println!("start: {:?}", labels);
        assert!(labels.contains(&"/w/:workspaceID/assets"), "{labels:?}");
    }

    #[test]
    #[ignore]
    fn callers_has_no_footer_set() {
        let Some(root) = root() else { return };
        let t = callers(&root, "listWorkspaceAssets", 3).unwrap();
        let mut labels = vec![];
        t.roots.iter().for_each(|r| all(r, &mut labels));
        println!("callers: {:?}", labels);
        assert!(!labels.iter().any(|l| l == "Footer.Set"), "{labels:?}");
    }

    /// The root is backend/, passed to the api: nothing here changes directory.
    #[test]
    #[ignore]
    fn callees_from_backend_reach_the_store() {
        let Some(root) = root() else { return };
        let t = callees(&root.join("backend"), "Handler.listWorkspaceAssets", 2).unwrap();
        let mut labels = vec![];
        t.roots.iter().for_each(|r| all(r, &mut labels));
        println!("callees: {:?}", labels);
        assert!(labels.iter().any(|l| l == "WorkspaceStore.ListWorkspaceAssets"), "{labels:?}");
    }
}
