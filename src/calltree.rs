use crate::files;
use crate::footer::{self, tail, Hidden};
use crate::resolve::graph::{self, CallGraph, NodeKey};
use crate::resolve::Confidence;
use crate::scope;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Direction {
    /// `callers`: walk up the call chain.
    Up,
    /// `callees`: walk down it.
    Down,
}

// ── Rows ──────────────────────────────────────────────────────────────────────

/// A file and a 1-based line.
#[derive(Debug, Clone, PartialEq)]
pub struct Loc {
    pub file: String,
    pub line: usize,
}

/// One row of a `callers` or `callees` tree: one call.
#[derive(Debug, Clone, PartialEq)]
pub struct CallNode {
    /// The function the row names: the caller (`callers`) or the callee as
    /// written at the call (`callees`).
    pub name: String,
    /// Where that function is defined. None for a callee call with no target,
    /// which prints as `unresolved NAME` and has no children.
    pub def: Option<Loc>,
    /// How the call resolved: None when it has no target, `Guess` prints `?`.
    pub confidence: Option<Confidence>,
    /// The function is already on the path from the root, so the walk stopped.
    pub cycle: bool,
    /// Where the call is written. `--scope` rows name the scope holding it.
    pub site: Loc,
    pub children: Vec<CallNode>,
    /// Children past `--max-matches`: counted by the cap, not kept. They are
    /// always the last ones, so the last kept child does not close the branch.
    pub children_cut: usize,
}

/// One definition a tree starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRoot {
    /// None when the name has no definition: the tree then holds the callers
    /// of the bare name (`callers`) or nothing (`callees`).
    pub def: Option<Loc>,
    pub children: Vec<CallNode>,
    pub children_cut: usize,
}

/// One name's trees, one per definition left after hiding.
#[derive(Debug, Clone, PartialEq)]
pub struct NameTree {
    pub name: String,
    pub roots: Vec<CallRoot>,
    /// Nothing resolved and nothing hidden: the name goes on the `not found:` line.
    pub missing: bool,
}

/// A whole `callers` or `callees` answer. Rows past `--max-matches` are left
/// out and counted by the cap.
#[derive(Debug, Clone)]
pub struct CallTrees {
    pub dir: Direction,
    pub names: Vec<NameTree>,
    /// Rows kept.
    pub shown: usize,
    pub hidden: Hidden,
    /// The names marked `missing`, in order.
    pub missing: Vec<String>,
    /// Some name resolved to a definition or had a row to show.
    pub resolved: bool,
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// The trees for `names`, walked `max_depth` calls up or down.
///
/// The call graph is built over every file, so a test never changes how a call
/// resolves. A definition of a name that sits in a test is left out as a root,
/// and a row whose call is written in a test is left out with everything
/// above it. Each one counts once in `hidden`. The cap is not settled into
/// `hidden` here: `footer::settle` does that.
pub fn build(
    dir: Direction,
    names: &[String],
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
) -> CallTrees {
    let g = graph_for(dir, names, jobs, max_depth);
    let mut w = Walk { g: &g, max_depth, shown: 0, hider, hidden: Hidden::default() };
    let mut trees = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut resolved = false;
    for name in names {
        let shown_before = w.shown;
        let hidden_before = w.hidden.tests + w.hidden.excluded;
        let roots = walk_name(&mut w, dir, name);
        let kept_root = roots.iter().any(|r| r.def.is_some());
        let hid = w.hidden.tests + w.hidden.excluded > hidden_before;
        let mut is_missing = false;
        if kept_root || w.shown > shown_before {
            resolved = true;
        } else if !hid {
            missing.push(name.clone());
            is_missing = true;
        }
        trees.push(NameTree { name: name.clone(), roots, missing: is_missing });
    }
    CallTrees { dir, names: trees, shown: w.shown, hidden: w.hidden, missing, resolved }
}

// ── Printers ──────────────────────────────────────────────────────────────────

fn noun(dir: Direction) -> &'static str {
    if dir == Direction::Up { "caller" } else { "callee" }
}

/// One name's tree as text: a `name()` heading per root (with its location
/// when there are several), the rows under it as a box-drawn tree, and with
/// `scope` the scope row under each row.
pub fn render_tree(t: &NameTree, scope: &mut Option<scope::Printer>) -> String {
    let mut out = String::new();
    let multiple = t.roots.len() > 1;
    for (i, root) in t.roots.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match (&root.def, multiple) {
            (Some(d), true) => out.push_str(&format!("{}()  [{}:{}]\n", t.name, short_path(&d.file), d.line)),
            _ => out.push_str(&format!("{}()\n", t.name)),
        }
        render_rows(&mut out, &root.children, root.children_cut, "", scope);
    }
    out
}

fn render_rows(out: &mut String, rows: &[CallNode], cut: usize, prefix: &str, scope: &mut Option<scope::Printer>) {
    let n = rows.len();
    for (i, c) in rows.iter().enumerate() {
        let is_last = i == n - 1 && cut == 0;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
        match &c.def {
            // no target: the branch stops at the call
            None => out.push_str(&format!("{}{}unresolved {}\n", prefix, connector, c.name)),
            Some(d) => {
                let mark = if c.confidence == Some(Confidence::Guess) { " ?" } else { "" };
                let cycle = if c.cycle { " (cycle)" } else { "" };
                out.push_str(&format!("{}{}{}()  [{}:{}]{}{}\n", prefix, connector, c.name, short_path(&d.file), d.line, mark, cycle));
            }
        }
        if let Some(p) = scope.as_mut() {
            out.push_str(&p.call_rows(&c.site.file, c.site.line, &child_prefix));
        }
        render_rows(out, &c.children, c.children_cut, &child_prefix, scope);
    }
}

/// The first name's tree with the count line or footer, and the exit code: 1
/// when every row was hidden. What `scopeof --callers` reads.
pub fn render_one(t: &CallTrees, jobs: &[(String, &'static str)], scope: Option<scope::Opts>) -> (String, i32) {
    let mut printer = scope.map(|o| scope::Printer::new(o, jobs));
    let mut out = t.names.first().map(|n| render_tree(n, &mut printer)).unwrap_or_default();
    let code = if t.shown == 0 && t.hidden.any() { 1 } else { 0 };
    out.push_str(&tail(t.shown, noun(t.dir), &t.hidden, scope.is_some()));
    (out, code)
}

/// The whole `callers` or `callees` answer. Each name's tree prints in order
/// with a blank line between. A name that resolved to nothing goes on the
/// `not found:` stderr line, and when it is the one name asked for its bare
/// `name()` line goes to stderr too, with stdout empty. When every row was
/// hidden stdout is empty and stderr says so (BUILD.md A20). The footer counts
/// across all names, once. The code is 0 when any name resolved to a
/// definition or had a row to show. `t.hidden` should be settled.
pub fn render_many(t: &CallTrees, where_: &str, jobs: &[(String, &'static str)], scope: Option<scope::Opts>) -> crate::resolve::NamesOut {
    let mut printer = scope.map(|o| scope::Printer::new(o, jobs));
    let mut out = String::new();
    for n in &t.names {
        if n.missing && t.names.len() > 1 {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_tree(n, &mut printer));
    }
    let names: Vec<&str> = t.names.iter().map(|n| n.name.as_str()).collect();
    let mut err = String::new();
    if !t.resolved {
        // Nothing to print on stdout: a hidden-everything report or the miss.
        if t.hidden.any() {
            out.clear();
            err.push_str(&footer::nothing_shown(
                &format!("no {} to {}", noun(t.dir), names.join(", ")),
                where_,
                jobs.len(),
                &t.hidden,
            ));
        } else {
            err.push_str(&out);
            out.clear();
        }
    } else {
        out.push_str(&tail(t.shown, noun(t.dir), &t.hidden, scope.is_some()));
    }
    err.push_str(&crate::resolve::not_found_line(&t.missing, jobs));
    crate::resolve::NamesOut { out, err, code: if t.resolved { 0 } else { 1 } }
}

// ── Builder then printer ──────────────────────────────────────────────────────

/// `build` for one name, then `render_one`. The cap is not settled.
pub fn report_scoped(
    dir: Direction,
    function: &str,
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
    scope: Option<scope::Opts>,
) -> (String, i32) {
    let t = build(dir, &[function.to_string()], jobs, max_depth, hider);
    render_one(&t, jobs, scope)
}

#[cfg(test)]
pub fn report(
    dir: Direction,
    function: &str,
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
) -> (String, i32) {
    report_scoped(dir, function, jobs, max_depth, hider, None)
}

/// `build`, the cap settled into the footer, then `render_many`.
pub fn report_many(
    dir: Direction,
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
    scope: Option<scope::Opts>,
) -> crate::resolve::NamesOut {
    let mut t = build(dir, names, jobs, max_depth, hider);
    footer::settle(&mut t.hidden);
    render_many(&t, where_, jobs, scope)
}

/// The graph a walk from `names` reads, built from the files it needs. Its
/// edges within `max_depth` of the names are the whole-folder graph's, so a
/// test never changes how a call resolves.
fn graph_for(dir: Direction, names: &[String], jobs: &[(String, &'static str)], max_depth: usize) -> CallGraph {
    let (up, down) = if dir == Direction::Up { (max_depth, 0) } else { (0, max_depth) };
    crate::resolve::scoped::build(jobs, &crate::resolve::scoped::Seeds { names: names.to_vec(), up, down })
}

/// One name's roots: one per definition left after hiding, or one with no
/// definition when the name has none at all.
fn walk_name(w: &mut Walk, dir: Direction, function: &str) -> Vec<CallRoot> {
    let g = w.g;
    let all_roots = g.index.by_name.get(function).cloned().unwrap_or_default();
    let had_roots = !all_roots.is_empty();
    let roots = w.hider.keep(
        all_roots,
        |&r| (g.index.defs[r].id.file.clone(), g.index.defs[r].id.line),
        &mut w.hidden,
    );
    let mut out = Vec::new();
    if !had_roots {
        let mut root = CallRoot { def: None, children: vec![], children_cut: 0 };
        if dir == Direction::Up {
            // external/undefined name: its callers are still in the graph
            let key = NodeKey::External(function.to_string());
            let mut visited = HashSet::new();
            visited.insert(key.clone());
            (root.children, root.children_cut) = w.up(&key, 0, &mut visited);
        }
        out.push(root);
    }
    for &r in &roots {
        let id = g.index.defs[r].id.clone();
        let def = Some(Loc { file: id.file.clone(), line: id.line });
        let key = NodeKey::Def(id);
        let mut visited = HashSet::new();
        visited.insert(key.clone());
        let (children, children_cut) = match dir {
            Direction::Up => w.up(&key, 0, &mut visited),
            Direction::Down => w.down(&key, 0, &mut visited),
        };
        out.push(CallRoot { def, children, children_cut });
    }
    out
}

pub fn callgraph(path: &str, lang_name: Option<&str>, no_ignore: bool, format: &str) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = graph::build(&jobs);
    let edges = collect_edges(&g);

    match format {
        "edges" => {
            for (crid, ceid, _, _, guess) in &edges {
                println!("{} -> {}{}", crid, ceid, if *guess { " ?" } else { "" });
            }
        }
        _ => {
            let mut nodes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            for (crid, ceid, crl, cel, _) in &edges {
                nodes.entry(crid.clone()).or_insert_with(|| crl.clone());
                nodes.entry(ceid.clone()).or_insert_with(|| cel.clone());
            }
            println!("digraph callgraph {{");
            let mut node_list: Vec<_> = nodes.iter().collect();
            node_list.sort_by_key(|(id, _)| id.as_str());
            for (id, label) in node_list {
                println!("    {:?} [label={:?}];", id, label);
            }
            println!();
            for (crid, ceid, _, _, guess) in &edges {
                if *guess {
                    println!("    {:?} -> {:?} [style=dashed];", crid, ceid);
                } else {
                    println!("    {:?} -> {:?};", crid, ceid);
                }
            }
            println!("}}");
        }
    }
}

// Returns (caller_id, callee_id, caller_label, callee_label, guess).
// Ids use "filename:line" for resolved definitions, bare name for externals.
pub(crate) fn collect_edges(g: &CallGraph) -> Vec<(String, String, String, String, bool)> {
    let mut edges: Vec<_> = g.calls.iter().map(|rc| {
        let cd = &g.index.defs[rc.caller];
        let crid = format!("{}:{}", short_path(&cd.id.file), cd.id.line);
        let crl = format!("{}\\n{}:{}", cd.name, short_path(&cd.id.file), cd.id.line);
        let (ceid, cel, guess) = match rc.target {
            Some((t, conf)) => {
                let d = &g.index.defs[t];
                (
                    format!("{}:{}", short_path(&d.id.file), d.id.line),
                    format!("{}\\n{}:{}", d.name, short_path(&d.id.file), d.id.line),
                    conf == Confidence::Guess,
                )
            }
            None => (rc.callee_display.clone(), rc.callee_display.clone(), false),
        };
        (crid, ceid, crl, cel, guess)
    }).collect();
    edges.sort();
    edges.dedup();
    edges
}

// ── Tree walkers ──────────────────────────────────────────────────────────────

/// One `callers` or `callees` walk: the rows kept so far and what was hidden.
struct Walk<'a> {
    g: &'a CallGraph,
    max_depth: usize,
    shown: usize,
    hider: &'a mut files::Hider,
    hidden: Hidden,
}

impl Walk<'_> {
    /// The edges whose call is not written in a hidden place.
    fn visible(&mut self, edges: &[usize]) -> Vec<usize> {
        let g = self.g;
        self.hider.keep(
            edges.to_vec(),
            |&e| {
                let rc = &g.calls[e];
                (g.index.defs[rc.caller].id.file.clone(), rc.line)
            },
            &mut self.hidden,
        )
    }

    /// The callee rows under `key`, and how many were cut by the cap.
    fn down(&mut self, key: &NodeKey, depth: usize, visited: &mut HashSet<NodeKey>) -> (Vec<CallNode>, usize) {
        let g = self.g;
        let mut rows = Vec::new();
        let mut cut = 0;
        if depth >= self.max_depth { return (rows, cut); }
        let Some(edges) = g.fwd.get(key) else { return (rows, cut) };
        let edges = self.visible(edges);
        for &e in &edges {
            let rc = &g.calls[e];
            let caller_file = &g.index.defs[rc.caller].id.file;
            // Past --max-matches a row is counted, not kept, and so is
            // everything below it.
            let keep = footer::take(caller_file);
            if keep {
                self.shown += 1;
            } else {
                cut += 1;
            }
            let site = Loc { file: caller_file.clone(), line: rc.line };
            let mut node = CallNode {
                name: rc.callee_display.clone(),
                def: None,
                confidence: None,
                cycle: false,
                site,
                children: vec![],
                children_cut: 0,
            };
            // no target: the branch stops at the call
            if let Some((t, conf)) = rc.target {
                let d = &g.index.defs[t];
                node.def = Some(Loc { file: d.id.file.clone(), line: d.id.line });
                node.confidence = Some(conf);
                let tk = NodeKey::Def(d.id.clone());
                if visited.contains(&tk) {
                    node.cycle = true;
                } else {
                    visited.insert(tk.clone());
                    (node.children, node.children_cut) = self.down(&tk, depth + 1, visited);
                    visited.remove(&tk);
                }
            }
            if keep {
                rows.push(node);
            }
        }
        (rows, cut)
    }

    /// The caller rows above `key`, and how many were cut by the cap.
    fn up(&mut self, key: &NodeKey, depth: usize, visited: &mut HashSet<NodeKey>) -> (Vec<CallNode>, usize) {
        let g = self.g;
        let mut rows = Vec::new();
        let mut cut = 0;
        if depth >= self.max_depth { return (rows, cut); }
        let Some(edges) = g.rev.get(key) else { return (rows, cut) };
        let edges = self.visible(edges);
        for &e in &edges {
            let rc = &g.calls[e];
            let d = &g.index.defs[rc.caller];
            let keep = footer::take(&d.id.file);
            if keep {
                self.shown += 1;
            } else {
                cut += 1;
            }
            let tk = NodeKey::Def(d.id.clone());
            let mut node = CallNode {
                name: d.name.clone(),
                def: Some(Loc { file: d.id.file.clone(), line: d.id.line }),
                confidence: rc.target.map(|(_, c)| c),
                cycle: false,
                site: Loc { file: d.id.file.clone(), line: rc.line },
                children: vec![],
                children_cut: 0,
            };
            if visited.contains(&tk) {
                node.cycle = true;
            } else {
                visited.insert(tk.clone());
                (node.children, node.children_cut) = self.up(&tk, depth + 1, visited);
                visited.remove(&tk);
            }
            if keep {
                rows.push(node);
            }
        }
        (rows, cut)
    }
}

pub(crate) fn short_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Write src to a named temp file and return its path as a String.
    fn tmp(name: &str, src: &str) -> String {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, src).expect("write tmp file");
        path.to_str().unwrap().to_string()
    }

    type Jobs = Vec<(String, &'static str)>;

    // Pull all (caller_id, callee_id, guess) tuples out of the edge list.
    fn pairs(jobs: &Jobs) -> Vec<(String, String, bool)> {
        let g = graph::build(jobs);
        collect_edges(&g).into_iter().map(|(a, b, _, _, gss)| (a, b, gss)).collect()
    }

    fn go_pair(name: &str) -> (std::path::PathBuf, Jobs, files::Hider) {
        let d = std::env::temp_dir().join(format!("ctsq_calltree_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("calc.go"), "package calc\n\nfunc Add(a, b int) int { return a + b }\n\nfunc Twice(a int) int { return Add(a, a) }\n").unwrap();
        std::fs::write(d.join("calc_test.go"), "package calc\n\nfunc TestAdd() { Twice(1) }\n").unwrap();
        let root = d.to_str().unwrap();
        let jobs = files::resolve_jobs(root, None, true);
        let mut h = files::Hider::new(&Default::default());
        h.add_jobs(root, &jobs);
        (d, jobs, h)
    }

    #[test]
    fn callers_leave_out_a_test_caller_and_its_subtree() {
        let (d, jobs, mut h) = go_pair("up");
        let (text, code) = report(Direction::Up, "Add", &jobs, 3, &mut h);
        assert_eq!(code, 0);
        assert!(text.starts_with("Add()\n└── Twice()  [calc.go:5]\n"), "{}", text);
        assert!(!text.contains("TestAdd"), "{}", text);
        assert!(text.ends_with("1 caller\nhidden: 1 in tests (--tests)\n"), "{}", text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn scope_rows_sit_under_their_call_row_and_the_count_merges_with_the_footer() {
        let (d, jobs, mut h) = go_pair("scoped");
        let root = d.to_str().unwrap();
        let (text, code) = report_scoped(Direction::Up, "Add", &jobs, 3, &mut h, Some(scope::Opts::default()));
        assert_eq!(code, 0);
        let want = format!(
            "Add()\n└── Twice()  [calc.go:5]\n      in {root}/calc.go:5-5  func Twice(a int) int {{ return Add(a, a) }}\n1 caller\nhidden: 1 in tests (--tests)\n"
        );
        assert_eq!(text, want);
        let mut all = files::Hider::new(&files::HideOpts { tests: true, exclude: vec![] });
        all.add_jobs(root, &jobs);
        let (text, _) = report_scoped(Direction::Up, "Add", &jobs, 3, &mut all, Some(scope::Opts { body: true }));
        assert!(text.contains(&format!("      in {root}/calc.go:5-5  func Twice(a int) int {{ return Add(a, a) }}\n        func Twice(a int) int {{ return Add(a, a) }}\n")), "{}", text);
        assert!(text.contains("TestAdd()") && text.ends_with("2 callers\n"), "{}", text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_function_defined_only_in_tests_is_hidden_with_exit_1() {
        let (d, jobs, mut h) = go_pair("root");
        let (text, code) = report(Direction::Down, "TestAdd", &jobs, 3, &mut h);
        assert_eq!(code, 1);
        assert_eq!(text, "0 callees\nhidden: 1 in tests (--tests)\n");
        let mut all = files::Hider::new(&files::HideOpts { tests: true, exclude: vec![] });
        all.add_jobs(d.to_str().unwrap(), &jobs);
        let (text, code) = report(Direction::Down, "TestAdd", &jobs, 3, &mut all);
        assert_eq!(code, 0);
        assert!(text.contains("Twice()") && !text.contains("hidden:"), "{}", text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_callgraph_resolves_unambiguous_callee() {
        // unique_helper is defined only in file_a — should resolve to a file:line id.
        let src_a = "fn unique_helper() {}\nfn caller_a() { unique_helper(); }\n";
        let path_a = tmp("ctsq_test_unambig_a.rs", src_a);
        let jobs: Jobs = vec![(path_a.clone(), "rust")];

        let p = pairs(&jobs);
        let edge = p.iter().find(|(from, _, _)| from.ends_with(":2"));
        assert!(edge.is_some(), "expected edge from caller_a (line 2), got: {:?}", p);
        let (_, to, guess) = edge.unwrap();
        assert!(
            to.contains("ctsq_test_unambig_a.rs") && to.contains(":1"),
            "unambiguous callee should resolve to file:line, got: {}",
            to
        );
        assert!(!guess, "unambiguous resolution is exact, not a guess");
    }

    #[test]
    fn test_callgraph_resolves_ambiguous_callee_to_same_file() {
        // shared_helper is defined in both files — each caller resolves to the
        // definition in its own file (exact: it's the unique in-scope match).
        let src_a = "fn shared_helper() {}\nfn caller_a() { shared_helper(); }\n";
        let src_b = "fn shared_helper() {}\nfn caller_b() { shared_helper(); }\n";

        let path_a = tmp("ctsq_test_ambig_a.rs", src_a);
        let path_b = tmp("ctsq_test_ambig_b.rs", src_b);

        let jobs: Jobs = vec![(path_a.clone(), "rust"), (path_b.clone(), "rust")];

        let p = pairs(&jobs);
        for (from, to, _) in &p {
            if from.starts_with("ctsq_test_ambig_a.rs") {
                assert!(
                    to.starts_with("ctsq_test_ambig_a.rs:1"),
                    "caller_a's call should resolve to its own file's def, got: {}",
                    to
                );
            }
            if from.starts_with("ctsq_test_ambig_b.rs") {
                assert!(
                    to.starts_with("ctsq_test_ambig_b.rs:1"),
                    "caller_b's call should resolve to its own file's def, got: {}",
                    to
                );
            }
        }
    }

    #[test]
    fn test_callgraph_leaves_an_out_of_scope_bare_call_unresolved() {
        // helper only defined in two *other* files that caller_c does not
        // import: no name-only fallback picks either, the edge stays bare.
        let src_a = "fn helper() {}\n";
        let src_b = "fn helper() {}\n";
        let src_c = "fn caller_c() { helper(); }\n";

        let path_a = tmp("ctsq_test_guess_a.rs", src_a);
        let path_b = tmp("ctsq_test_guess_b.rs", src_b);
        let path_c = tmp("ctsq_test_guess_c.rs", src_c);

        let jobs: Jobs = vec![
            (path_a.clone(), "rust"),
            (path_b.clone(), "rust"),
            (path_c.clone(), "rust"),
        ];

        let p = pairs(&jobs);
        let edge = p.iter().find(|(from, _, _)| from.starts_with("ctsq_test_guess_c.rs"));
        let (_, to, guess) = edge.expect("caller_c should have an edge");
        assert_eq!(to, "helper", "an out-of-scope call has no target");
        assert!(!guess);
    }

    #[test]
    fn test_fallback_lang_keeps_name_matching_behavior() {
        // JavaScript goes through the fallback resolver: unique names resolve
        // as guesses, ambiguous names stay bare.
        let src_a = "function shared() {}\nfunction unique_js() {}\nfunction caller_a() { shared(); unique_js(); }\n";
        let src_b = "function shared() {}\n";

        let path_a = tmp("ctsq_test_fb_a.js", src_a);
        let path_b = tmp("ctsq_test_fb_b.js", src_b);

        let jobs: Jobs = vec![(path_a.clone(), "javascript"), (path_b.clone(), "javascript")];

        let p = pairs(&jobs);
        let from_caller: Vec<_> = p.iter()
            .filter(|(from, _, _)| from.starts_with("ctsq_test_fb_a.js:3"))
            .collect();
        assert!(!from_caller.is_empty(), "expected edges from caller_a, got: {:?}", p);
        for (_, to, guess) in &from_caller {
            if to.contains("unique_js") || to.ends_with(":2") {
                assert!(to.contains(":2"), "unique callee resolves to file:line, got: {}", to);
                assert!(guess, "a unique-name match is still a guess");
            } else {
                assert_eq!(to, "shared", "ambiguous callee must remain a bare name, got: {}", to);
                assert!(!guess);
            }
        }
    }
}
