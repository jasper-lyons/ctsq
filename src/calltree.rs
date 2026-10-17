use crate::files;
use crate::footer::{self, tail, Hidden};
use crate::resolve::graph::{self, CallGraph, NodeKey};
use crate::resolve::Confidence;
use crate::scope;
use std::collections::HashSet;

// ── Public entry points ───────────────────────────────────────────────────────

pub fn callees(names: &[String], where_: &str, jobs: &[(String, &'static str)], hider: files::Hider, max_depth: usize, scope: Option<scope::Opts>) {
    run(Direction::Down, names, where_, jobs, hider, max_depth, scope);
}

pub fn callers(names: &[String], where_: &str, jobs: &[(String, &'static str)], hider: files::Hider, max_depth: usize, scope: Option<scope::Opts>) {
    run(Direction::Up, names, where_, jobs, hider, max_depth, scope);
}

fn run(dir: Direction, names: &[String], where_: &str, jobs: &[(String, &'static str)], mut hider: files::Hider, max_depth: usize, scope: Option<scope::Opts>) {
    let r = report_many(dir, names, where_, jobs, max_depth, &mut hider, scope);
    print!("{}", r.out);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprint!("{}", r.err);
    if r.code != 0 {
        std::process::exit(r.code);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Direction {
    /// `callers`: walk up the call chain.
    Up,
    /// `callees`: walk down it.
    Down,
}

/// The whole `callers` or `callees` tree as text, with the footer when
/// something was hidden, and the exit code: 1 when every row was hidden.
///
/// The call graph is built over every file, so a test never changes how a call
/// resolves. A definition of `function` that sits in a test is left out as a
/// root, and a row whose call is written in a test is left out with everything
/// above it. Each one counts once in the footer.
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

/// `report`, with `--scope` rows under each row when `scope` is set: the named
/// scope holding that row's call site, then the count line.
pub fn report_scoped(
    dir: Direction,
    function: &str,
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
    scope: Option<scope::Opts>,
) -> (String, i32) {
    let g = graph_for(dir, &[function.to_string()], jobs, max_depth);
    let printer = scope.map(|o| scope::Printer::new(o, jobs));
    let mut w = Walk { g: &g, max_depth, out: String::new(), shown: 0, hider, hidden: Hidden::default(), scope: printer };
    walk_name(&mut w, dir, function);
    let noun = if dir == Direction::Up { "caller" } else { "callee" };
    let code = if w.shown == 0 && w.hidden.any() { 1 } else { 0 };
    let mut out = w.out;
    out.push_str(&tail(w.shown, noun, &w.hidden, scope.is_some()));
    (out, code)
}

/// `report_scoped` for several names. Each name's tree prints in order with a
/// blank line between. A name that resolved to nothing goes on the `not found:`
/// stderr line, and when it is the one name asked for its bare `name()` line
/// goes to stderr too, with stdout empty. When every row was hidden stdout is
/// empty and stderr says so (BUILD.md A20). The footer counts across all names,
/// once. The code is 0 when any name resolved to a definition or had a row to
/// show.
pub fn report_many(
    dir: Direction,
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    max_depth: usize,
    hider: &mut files::Hider,
    scope: Option<scope::Opts>,
) -> crate::resolve::NamesOut {
    let g = graph_for(dir, names, jobs, max_depth);
    let printer = scope.map(|o| scope::Printer::new(o, jobs));
    let mut w = Walk { g: &g, max_depth, out: String::new(), shown: 0, hider, hidden: Hidden::default(), scope: printer };
    let mut missing: Vec<String> = Vec::new();
    let mut resolved = false;
    for name in names {
        let sep = !w.out.is_empty();
        if sep {
            w.out.push('\n');
        }
        let start = w.out.len();
        let shown_before = w.shown;
        let hidden_before = w.hidden.tests + w.hidden.excluded;
        let kept_root = walk_name(&mut w, dir, name);
        let hid = w.hidden.tests + w.hidden.excluded > hidden_before;
        if kept_root || w.shown > shown_before {
            resolved = true;
        } else if !hid {
            missing.push(name.clone());
            if names.len() > 1 {
                w.out.truncate(start);
                if sep {
                    w.out.pop();
                }
            }
        }
    }
    let noun = if dir == Direction::Up { "caller" } else { "callee" };
    footer::settle(&mut w.hidden);
    let mut err = String::new();
    let mut out = w.out;
    if !resolved {
        // Nothing to print on stdout: a hidden-everything report or the miss.
        if w.hidden.any() {
            out.clear();
            err.push_str(&footer::nothing_shown(
                &format!("no {} to {}", noun, names.join(", ")),
                where_,
                jobs.len(),
                &w.hidden,
            ));
        } else {
            err.push_str(&out);
            out.clear();
        }
    } else {
        out.push_str(&tail(w.shown, noun, &w.hidden, scope.is_some()));
    }
    err.push_str(&crate::resolve::not_found_line(&missing, jobs));
    crate::resolve::NamesOut { out, err, code: if resolved { 0 } else { 1 } }
}

/// The graph a walk from `names` reads, built from the files it needs. Its
/// edges within `max_depth` of the names are the whole-folder graph's, so a
/// test never changes how a call resolves.
fn graph_for(dir: Direction, names: &[String], jobs: &[(String, &'static str)], max_depth: usize) -> CallGraph {
    let (up, down) = if dir == Direction::Up { (max_depth, 0) } else { (0, max_depth) };
    crate::resolve::scoped::build(jobs, &crate::resolve::scoped::Seeds { names: names.to_vec(), up, down })
}

/// One name's tree, appended to `w.out`. True when a definition of the name
/// is left after hiding.
fn walk_name(w: &mut Walk, dir: Direction, function: &str) -> bool {
    let g = w.g;
    let all_roots = g.index.by_name.get(function).cloned().unwrap_or_default();
    let had_roots = !all_roots.is_empty();
    let roots = w.hider.keep(
        all_roots,
        |&r| (g.index.defs[r].id.file.clone(), g.index.defs[r].id.line),
        &mut w.hidden,
    );
    if !had_roots {
        w.out.push_str(&format!("{}()\n", function));
        if dir == Direction::Up {
            // external/undefined name: its callers are still in the graph
            let key = NodeKey::External(function.to_string());
            let mut visited = HashSet::new();
            visited.insert(key.clone());
            w.up(&key, 0, "", &mut visited);
        }
    }
    let multiple = roots.len() > 1;
    for (i, &r) in roots.iter().enumerate() {
        if i > 0 { w.out.push('\n'); }
        let id = g.index.defs[r].id.clone();
        if multiple {
            w.out.push_str(&format!("{}()  [{}:{}]\n", function, short_path(&id.file), id.line));
        } else {
            w.out.push_str(&format!("{}()\n", function));
        }
        let key = NodeKey::Def(id);
        let mut visited = HashSet::new();
        visited.insert(key.clone());
        match dir {
            Direction::Up => w.up(&key, 0, "", &mut visited),
            Direction::Down => w.down(&key, 0, "", &mut visited),
        }
    }
    !roots.is_empty()
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

/// One `callers` or `callees` walk: the rows printed so far and what was hidden.
struct Walk<'a> {
    g: &'a CallGraph,
    max_depth: usize,
    out: String,
    shown: usize,
    hider: &'a mut files::Hider,
    hidden: Hidden,
    scope: Option<scope::Printer>,
}

impl Walk<'_> {
    /// The `--scope` rows for the call of edge `e`, under the row just printed.
    fn scope_rows(&mut self, e: usize, indent: &str) {
        let g = self.g;
        let rc = &g.calls[e];
        let file = &g.index.defs[rc.caller].id.file;
        if let Some(p) = self.scope.as_mut() {
            self.out.push_str(&p.call_rows(file, rc.line, indent));
        }
    }

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

    fn down(&mut self, key: &NodeKey, depth: usize, prefix: &str, visited: &mut HashSet<NodeKey>) {
        let g = self.g;
        if depth >= self.max_depth { return; }
        let Some(edges) = g.fwd.get(key) else { return };
        let edges = self.visible(edges);
        let n = edges.len();
        for (i, &e) in edges.iter().enumerate() {
            let rc = &g.calls[e];
            // Past --max-matches a row is counted, not printed, and so is
            // everything below it.
            let print = footer::take(&g.index.defs[rc.caller].id.file);
            if print {
                self.shown += 1;
            }
            let is_last = i == n - 1;
            let connector = if is_last { "└── " } else { "├── " };
            let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            match rc.target {
                Some((t, conf)) => {
                    let d = &g.index.defs[t];
                    let mark = if conf == Confidence::Guess { " ?" } else { "" };
                    let loc = format!("[{}:{}]", short_path(&d.id.file), d.id.line);
                    let tk = NodeKey::Def(d.id.clone());
                    if visited.contains(&tk) {
                        if print {
                            self.out.push_str(&format!("{}{}{}()  {}{} (cycle)\n", prefix, connector, rc.callee_display, loc, mark));
                            self.scope_rows(e, &child_prefix);
                        }
                    } else {
                        if print {
                            self.out.push_str(&format!("{}{}{}()  {}{}\n", prefix, connector, rc.callee_display, loc, mark));
                            self.scope_rows(e, &child_prefix);
                        }
                        visited.insert(tk.clone());
                        self.down(&tk, depth + 1, &child_prefix, visited);
                        visited.remove(&tk);
                    }
                }
                None => {
                    if print {
                        self.out.push_str(&format!("{}{}{}()\n", prefix, connector, rc.callee_display));
                        self.scope_rows(e, &child_prefix);
                    }
                }
            }
        }
    }

    fn up(&mut self, key: &NodeKey, depth: usize, prefix: &str, visited: &mut HashSet<NodeKey>) {
        let g = self.g;
        if depth >= self.max_depth { return; }
        let Some(edges) = g.rev.get(key) else { return };
        let edges = self.visible(edges);
        let n = edges.len();
        for (i, &e) in edges.iter().enumerate() {
            let rc = &g.calls[e];
            let d = &g.index.defs[rc.caller];
            let print = footer::take(&d.id.file);
            if print {
                self.shown += 1;
            }
            let is_last = i == n - 1;
            let connector = if is_last { "└── " } else { "├── " };
            let mark = match rc.target {
                Some((_, Confidence::Guess)) => " ?",
                _ => "",
            };
            let loc = format!("[{}:{}]", short_path(&d.id.file), d.id.line);
            let tk = NodeKey::Def(d.id.clone());
            let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            if visited.contains(&tk) {
                if print {
                    self.out.push_str(&format!("{}{}{}()  {}{} (cycle)\n", prefix, connector, d.name, loc, mark));
                    self.scope_rows(e, &child_prefix);
                }
            } else {
                if print {
                    self.out.push_str(&format!("{}{}{}()  {}{}\n", prefix, connector, d.name, loc, mark));
                    self.scope_rows(e, &child_prefix);
                }
                visited.insert(tk.clone());
                self.up(&tk, depth + 1, &child_prefix, visited);
                visited.remove(&tk);
            }
        }
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
    fn test_callgraph_marks_cross_file_ambiguity_as_guess() {
        // helper only defined in two *other* files — best-ranked candidate
        // wins but the edge is marked as a guess.
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
        assert!(to.contains(":1"), "should resolve to one of the candidates, got: {}", to);
        assert!(guess, "cross-file ambiguous resolution must be marked as a guess");
    }

    #[test]
    fn test_fallback_lang_keeps_name_matching_behavior() {
        // JavaScript goes through the fallback resolver: unique names resolve
        // exactly, ambiguous names stay bare, nothing is marked as a guess.
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
            } else {
                assert_eq!(to, "shared", "ambiguous callee must remain a bare name, got: {}", to);
            }
            assert!(!guess, "fallback resolver never guesses");
        }
    }
}
