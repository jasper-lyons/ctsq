use crate::files;
use crate::resolve::graph::{self, CallGraph, NodeKey};
use crate::resolve::Confidence;
use std::collections::HashSet;

// ── Public entry points ───────────────────────────────────────────────────────

pub fn callees(function: &str, path: &str, lang_name: Option<&str>, max_depth: usize, no_ignore: bool) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = graph::build(&jobs);
    let roots = g.index.by_name.get(function).cloned().unwrap_or_default();
    if roots.is_empty() {
        println!("{}()", function);
        return;
    }
    let multiple = roots.len() > 1;
    for (i, &r) in roots.iter().enumerate() {
        if i > 0 { println!(); }
        let id = g.index.defs[r].id.clone();
        if multiple {
            println!("{}()  [{}:{}]", function, short_path(&id.file), id.line);
        } else {
            println!("{}()", function);
        }
        let key = NodeKey::Def(id);
        let mut visited = HashSet::new();
        visited.insert(key.clone());
        walk_down(&g, &key, max_depth, 0, "", &mut visited);
    }
}

pub fn callers(function: &str, path: &str, lang_name: Option<&str>, max_depth: usize, no_ignore: bool) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = graph::build(&jobs);
    let roots = g.index.by_name.get(function).cloned().unwrap_or_default();
    if roots.is_empty() {
        // external/undefined name — its callers are still in the graph
        println!("{}()", function);
        let key = NodeKey::External(function.to_string());
        let mut visited = HashSet::new();
        visited.insert(key.clone());
        walk_up(&g, &key, max_depth, 0, "", &mut visited);
        return;
    }
    let multiple = roots.len() > 1;
    for (i, &r) in roots.iter().enumerate() {
        if i > 0 { println!(); }
        let id = g.index.defs[r].id.clone();
        if multiple {
            println!("{}()  [{}:{}]", function, short_path(&id.file), id.line);
        } else {
            println!("{}()", function);
        }
        let key = NodeKey::Def(id);
        let mut visited = HashSet::new();
        visited.insert(key.clone());
        walk_up(&g, &key, max_depth, 0, "", &mut visited);
    }
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

fn walk_down(
    g: &CallGraph,
    key: &NodeKey,
    max_depth: usize,
    depth: usize,
    prefix: &str,
    visited: &mut HashSet<NodeKey>,
) {
    if depth >= max_depth { return; }
    let Some(edges) = g.fwd.get(key) else { return };
    let n = edges.len();
    for (i, &e) in edges.iter().enumerate() {
        let rc = &g.calls[e];
        let is_last = i == n - 1;
        let connector = if is_last { "└── " } else { "├── " };
        match rc.target {
            Some((t, conf)) => {
                let d = &g.index.defs[t];
                let mark = if conf == Confidence::Guess { " ?" } else { "" };
                let loc = format!("[{}:{}]", short_path(&d.id.file), d.id.line);
                let tk = NodeKey::Def(d.id.clone());
                if visited.contains(&tk) {
                    println!("{}{}{}()  {}{} (cycle)", prefix, connector, rc.callee_display, loc, mark);
                } else {
                    println!("{}{}{}()  {}{}", prefix, connector, rc.callee_display, loc, mark);
                    visited.insert(tk.clone());
                    let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
                    walk_down(g, &tk, max_depth, depth + 1, &child_prefix, visited);
                    visited.remove(&tk);
                }
            }
            None => println!("{}{}{}()", prefix, connector, rc.callee_display),
        }
    }
}

fn walk_up(
    g: &CallGraph,
    key: &NodeKey,
    max_depth: usize,
    depth: usize,
    prefix: &str,
    visited: &mut HashSet<NodeKey>,
) {
    if depth >= max_depth { return; }
    let Some(edges) = g.rev.get(key) else { return };
    let n = edges.len();
    for (i, &e) in edges.iter().enumerate() {
        let rc = &g.calls[e];
        let d = &g.index.defs[rc.caller];
        let is_last = i == n - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let mark = match rc.target {
            Some((_, Confidence::Guess)) => " ?",
            _ => "",
        };
        let loc = format!("[{}:{}]", short_path(&d.id.file), d.id.line);
        let tk = NodeKey::Def(d.id.clone());
        if visited.contains(&tk) {
            println!("{}{}{}()  {}{} (cycle)", prefix, connector, d.name, loc, mark);
        } else {
            println!("{}{}{}()  {}{}", prefix, connector, d.name, loc, mark);
            visited.insert(tk.clone());
            let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            walk_up(g, &tk, max_depth, depth + 1, &child_prefix, visited);
            visited.remove(&tk);
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
