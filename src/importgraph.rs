//! `imports`, `importers`, `importgraph` subcommands — the file-level analogue
//! of `calltree`. Traversal and output mirror the call graph exactly.

use crate::calltree::short_path;
use crate::files;
use crate::resolve::imports::{self, ImportGraph, ImportTarget, NodeKey};
use crate::resolve::Confidence;
use std::collections::HashSet;

// ── Public entry points ───────────────────────────────────────────────────────

pub fn imports(file: &str, path: &str, lang_name: Option<&str>, max_depth: usize, no_ignore: bool) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = imports::build(&jobs);
    let Some(root) = find_root(&g, file) else {
        println!("{}", file);
        return;
    };
    println!("{}", g.files[root].display);
    let key = NodeKey::File(root);
    let mut visited = HashSet::new();
    visited.insert(key.clone());
    walk_down(&g, &key, max_depth, 0, "", &mut visited);
}

pub fn importers(file: &str, path: &str, lang_name: Option<&str>, max_depth: usize, no_ignore: bool) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = imports::build(&jobs);
    let Some(root) = find_root(&g, file) else {
        println!("{}", file);
        return;
    };
    println!("{}", g.files[root].display);
    let key = NodeKey::File(root);
    let mut visited = HashSet::new();
    visited.insert(key.clone());
    walk_up(&g, &key, max_depth, 0, "", &mut visited);
}

pub fn importgraph(path: &str, lang_name: Option<&str>, no_ignore: bool, format: &str) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let g = imports::build(&jobs);
    let edges = collect_edges(&g);

    match format {
        "edges" => {
            for (sid, tid, _, _, guess) in &edges {
                println!("{} -> {}{}", sid, tid, if *guess { " ?" } else { "" });
            }
        }
        _ => {
            let mut nodes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            for (sid, tid, sl, tl, _) in &edges {
                nodes.entry(sid.clone()).or_insert_with(|| sl.clone());
                nodes.entry(tid.clone()).or_insert_with(|| tl.clone());
            }
            println!("digraph importgraph {{");
            let mut node_list: Vec<_> = nodes.iter().collect();
            node_list.sort_by_key(|(id, _)| id.as_str());
            for (id, label) in node_list {
                println!("    {:?} [label={:?}];", id, label);
            }
            println!();
            for (sid, tid, _, _, guess) in &edges {
                if *guess {
                    println!("    {:?} -> {:?} [style=dashed];", sid, tid);
                } else {
                    println!("    {:?} -> {:?};", sid, tid);
                }
            }
            println!("}}");
        }
    }
}

fn find_root(g: &ImportGraph, file: &str) -> Option<usize> {
    let canonical = imports::canonical_path(file);
    g.by_canonical.get(&canonical).copied()
}

// Returns (source_id, target_id, source_label, target_label, guess).
// Ids are display paths (external → specifier). Go package edges expand to one
// file→file edge per resolved sink.
pub(crate) fn collect_edges(g: &ImportGraph) -> Vec<(String, String, String, String, bool)> {
    let mut edges: Vec<(String, String, String, String, bool)> = Vec::new();
    for imp in &g.imports {
        let src = &g.files[imp.importer];
        let sid = src.display.clone();
        let sl = short_path(&src.display);
        match &imp.target {
            ImportTarget::File(t, conf) => {
                let d = &g.files[*t];
                edges.push((sid, d.display.clone(), sl, short_path(&d.display), *conf == Confidence::Guess));
            }
            ImportTarget::External(s) => {
                edges.push((sid, s.clone(), sl, s.clone(), false));
            }
            ImportTarget::Package { package, sinks } => {
                if sinks.is_empty() {
                    let p = &g.packages[*package];
                    edges.push((sid, p.display.clone(), sl, p.display.clone(), false));
                } else {
                    for (s, conf) in sinks {
                        let d = &g.files[*s];
                        edges.push((sid.clone(), d.display.clone(), sl.clone(),
                            short_path(&d.display), *conf == Confidence::Guess));
                    }
                }
            }
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

// ── Tree walkers ──────────────────────────────────────────────────────────────

/// A single rendered child line and, when it's a file/package, the node to recurse into.
struct Child {
    key: NodeKey,
    label: String,
    guess: bool,
    recurse: bool,
}

fn walk_down(
    g: &ImportGraph,
    key: &NodeKey,
    max_depth: usize,
    depth: usize,
    prefix: &str,
    visited: &mut HashSet<NodeKey>,
) {
    if depth >= max_depth { return; }
    let Some(edges) = g.fwd.get(key) else { return };
    let children: Vec<Child> = edges.iter().flat_map(|&e| down_children(g, &g.imports[e])).collect();
    render(g, &children, max_depth, depth, prefix, visited, walk_down);
}

fn walk_up(
    g: &ImportGraph,
    key: &NodeKey,
    max_depth: usize,
    depth: usize,
    prefix: &str,
    visited: &mut HashSet<NodeKey>,
) {
    if depth >= max_depth { return; }
    let Some(edges) = g.rev.get(key) else { return };
    let children: Vec<Child> = edges.iter().map(|&e| {
        let imp = &g.imports[e];
        let src = &g.files[imp.importer];
        Child {
            key: NodeKey::File(imp.importer),
            label: file_label(&src.display),
            guess: target_guess(&imp.target),
            recurse: true,
        }
    }).collect();
    render(g, &children, max_depth, depth, prefix, visited, walk_up);
}

/// Expand one forward edge into its rendered children (a Go package edge with
/// resolved sinks yields one file child per sink).
fn down_children(g: &ImportGraph, imp: &crate::resolve::imports::ResolvedImport) -> Vec<Child> {
    match &imp.target {
        ImportTarget::File(t, conf) => vec![Child {
            key: NodeKey::File(*t),
            label: file_label(&g.files[*t].display),
            guess: *conf == Confidence::Guess,
            recurse: true,
        }],
        ImportTarget::External(s) => vec![Child {
            key: NodeKey::External(s.clone()),
            label: format!("{}  (external)", s),
            guess: false,
            recurse: false,
        }],
        ImportTarget::Package { package, sinks } => {
            if sinks.is_empty() {
                let p = &g.packages[*package];
                vec![Child {
                    key: NodeKey::Package(*package),
                    label: format!("{}  (package)", p.display),
                    guess: false,
                    recurse: false,
                }]
            } else {
                let pkg = &g.packages[*package];
                sinks.iter().map(|(s, conf)| Child {
                    key: NodeKey::File(*s),
                    label: format!("{}  (pkg {})", short_path(&g.files[*s].display), pkg.display),
                    guess: *conf == Confidence::Guess,
                    recurse: true,
                }).collect()
            }
        }
    }
}

/// Shared child-list renderer with connectors, cycle detection, and recursion.
fn render(
    g: &ImportGraph,
    children: &[Child],
    max_depth: usize,
    depth: usize,
    prefix: &str,
    visited: &mut HashSet<NodeKey>,
    recurse_fn: fn(&ImportGraph, &NodeKey, usize, usize, &str, &mut HashSet<NodeKey>),
) {
    let n = children.len();
    for (i, child) in children.iter().enumerate() {
        let is_last = i == n - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let mark = if child.guess { " ?" } else { "" };
        if child.recurse && visited.contains(&child.key) {
            println!("{}{}{}{} (cycle)", prefix, connector, child.label, mark);
        } else if child.recurse {
            println!("{}{}{}{}", prefix, connector, child.label, mark);
            visited.insert(child.key.clone());
            let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            recurse_fn(g, &child.key, max_depth, depth + 1, &child_prefix, visited);
            visited.remove(&child.key);
        } else {
            println!("{}{}{}{}", prefix, connector, child.label, mark);
        }
    }
}

fn file_label(display: &str) -> String {
    format!("{}  [{}]", short_path(display), display)
}

fn target_guess(t: &ImportTarget) -> bool {
    match t {
        ImportTarget::File(_, c) => *c == Confidence::Guess,
        ImportTarget::Package { sinks, .. } => sinks.iter().any(|(_, c)| *c == Confidence::Guess),
        ImportTarget::External(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// Write `(relative_path, contents)` into a unique temp dir; return (dir, jobs).
    fn setup(lang: &'static str, files: &[(&str, &str)]) -> (PathBuf, Vec<(String, &'static str)>) {
        let id = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ctsq_ig_{}_{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        let mut jobs = Vec::new();
        for (rel, content) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
            jobs.push((p.to_string_lossy().into_owned(), lang));
        }
        (dir, jobs)
    }

    /// (source basename, target basename-or-external, guess) tuples.
    fn edges(jobs: &[(String, &'static str)]) -> Vec<(String, String, bool)> {
        let g = imports::build(jobs);
        collect_edges(&g).into_iter()
            .map(|(s, t, _, _, guess)| (short_path(&s), short_path(&t), guess))
            .collect()
    }

    fn has(es: &[(String, String, bool)], from: &str, to: &str) -> bool {
        es.iter().any(|(f, t, _)| f == from && t == to)
    }

    #[test]
    fn c_relative_include_resolves_system_include_external() {
        let (_d, jobs) = setup("c", &[
            ("main.c", "#include <stdio.h>\n#include \"util.h\"\n"),
            ("util.h", "#include \"types.h\"\nint add(int,int);\n"),
            ("types.h", "typedef int number;\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "main.c", "util.h"), "relative include resolves: {:?}", es);
        assert!(has(&es, "main.c", "stdio.h"), "<stdio.h> stays external: {:?}", es);
        assert!(has(&es, "util.h", "types.h"), "transitive include: {:?}", es);
        assert!(es.iter().find(|(f, t, _)| f == "main.c" && t == "util.h").unwrap().2 == false,
            "exact resolution is not a guess");
    }

    #[test]
    fn js_extension_index_external_require_reexport() {
        let (_d, jobs) = setup("typescript", &[
            ("app.ts", "import {h} from './helper';\nimport x from './util';\nimport React from 'react';\nconst c = require('./config');\n"),
            ("helper.ts", "export {Geo} from './util';\n"),
            ("util/index.ts", "export const Geo = 1;\n"),
            ("config.ts", "export const c = 1;\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "app.ts", "helper.ts"), "extension probe ./helper: {:?}", es);
        assert!(has(&es, "app.ts", "index.ts"), "./util -> util/index.ts: {:?}", es);
        assert!(has(&es, "app.ts", "react"), "bare specifier external: {:?}", es);
        assert!(has(&es, "app.ts", "config.ts"), "require('./config'): {:?}", es);
        assert!(has(&es, "helper.ts", "index.ts"), "export ... from re-export: {:?}", es);
    }

    #[test]
    fn ts_tsconfig_paths_resolve_aliases() {
        let (dir, jobs) = setup("typescript", &[
            ("src/app.ts", "import {t} from '@app/thing';\nimport {h} from '~/lib/helper';\nimport R from 'react';\n"),
            ("src/shared/thing.ts", "export const t = 1;\n"),
            ("src/lib/helper.ts", "export const h = 1;\n"),
        ]);
        // A real tsconfig whose @app/* target does NOT share the import's path tail —
        // only correct alias reading (not suffix matching) resolves it.
        std::fs::write(dir.join("tsconfig.json"),
            "{\n  // aliases\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"~/*\": [\"src/*\"], \"@app/*\": [\"src/shared/*\"], },\n  },\n}\n").unwrap();
        let es = edges(&jobs);
        assert!(has(&es, "app.ts", "thing.ts"), "@app/* -> src/shared/* via tsconfig: {:?}", es);
        assert!(has(&es, "app.ts", "helper.ts"), "~/* -> src/* via tsconfig: {:?}", es);
        assert!(has(&es, "app.ts", "react"), "bare package stays external under tsconfig: {:?}", es);
    }

    #[test]
    fn ts_path_alias_resolves_by_suffix() {
        let (_d, jobs) = setup("typescript", &[
            ("src/app.ts", "import {h} from '~/lib/helper';\nimport {B} from '@/components/Button';\nimport React from 'react';\n"),
            ("src/lib/helper.ts", "export const h = 1;\n"),
            ("src/components/Button.ts", "export const B = 1;\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "app.ts", "helper.ts"), "~/ alias resolves by suffix: {:?}", es);
        assert!(has(&es, "app.ts", "Button.ts"), "@/ alias resolves by suffix: {:?}", es);
        assert!(has(&es, "app.ts", "react"), "bare package stays external: {:?}", es);
    }

    #[test]
    fn js_ambiguous_extension_is_guess() {
        let (_d, jobs) = setup("typescript", &[
            ("app.ts", "import x from './foo';\n"),
            ("foo.ts", "export const a = 1;\n"),
            ("foo/index.ts", "export const b = 2;\n"),
        ]);
        let es = edges(&jobs);
        let from_app: Vec<_> = es.iter().filter(|(f, _, _)| f == "app.ts").collect();
        assert_eq!(from_app.len(), 2, "both candidates emitted: {:?}", es);
        assert!(from_app.iter().all(|(_, _, g)| *g), "ambiguous resolution marked guess: {:?}", es);
    }

    #[test]
    fn python_from_import_relative_and_pkg() {
        let (_d, jobs) = setup("python", &[
            ("app.py", "import util.geo\nfrom helpers import do\nfrom . import config\nimport os\n"),
            ("helpers.py", "x = 1\n"),
            ("config.py", "y = 2\n"),
            ("util/__init__.py", ""),
            ("util/geo.py", "z = 3\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "app.py", "geo.py"), "dotted import util.geo: {:?}", es);
        assert!(has(&es, "app.py", "helpers.py"), "from helpers import: {:?}", es);
        assert!(has(&es, "app.py", "config.py"), "from . import config: {:?}", es);
        assert!(has(&es, "app.py", "os"), "stdlib os external: {:?}", es);
    }

    #[test]
    fn rust_crate_module_and_external_crate() {
        let (_d, jobs) = setup("rust", &[
            ("src/main.rs", "use crate::util::geo::Geo;\nuse serde::Serialize;\nfn main() {}\n"),
            ("src/util/geo.rs", "pub struct Geo;\n"),
            ("src/util/mod.rs", "pub mod geo;\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "main.rs", "geo.rs"), "crate::util::geo::Geo -> geo.rs: {:?}", es);
        assert!(has(&es, "main.rs", "serde"), "external crate collapses to name: {:?}", es);
    }

    #[test]
    fn go_package_import_resolves_sink_file() {
        let (_d, jobs) = setup("go", &[
            ("main.go", "package main\nimport \"demo/util\"\nfunc main() { util.Distance() }\n"),
            ("util/geo.go", "package util\nfunc Distance() int { return 0 }\n"),
            ("util/other.go", "package util\nfunc Unused() {}\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "main.go", "geo.go"),
            "util.Distance() usage pins sink to geo.go: {:?}", es);
        assert!(!has(&es, "main.go", "other.go"),
            "unused package file is not a sink: {:?}", es);
    }

    #[test]
    fn cycle_builds_and_renders_without_hanging() {
        let (dir, jobs) = setup("typescript", &[
            ("a.ts", "import {b} from './b';\nexport const a = 1;\n"),
            ("b.ts", "import {a} from './a';\nexport const b = 2;\n"),
        ]);
        let es = edges(&jobs);
        assert!(has(&es, "a.ts", "b.ts") && has(&es, "b.ts", "a.ts"), "both edges present: {:?}", es);
        // render both directions — the visited-set guard must terminate.
        let a = jobs[0].0.clone();
        imports(&a, dir.to_str().unwrap(), Some("typescript"), 5, true);
        importers(&a, dir.to_str().unwrap(), Some("typescript"), 5, true);
    }

    #[test]
    fn file_importing_itself_is_not_an_edge() {
        let (_d, jobs) = setup("c", &[
            ("a.h", "#include \"a.h\"\nint a;\n"),
        ]);
        let es = edges(&jobs);
        assert!(!has(&es, "a.h", "a.h"), "self-import produces no edge: {:?}", es);
    }
}
