//! Project-wide call graph: every definition indexed, every call site
//! resolved, with forward/reverse adjacency for in-memory traversal.

use super::{resolver_for, CallSite, DefId, DefIndex, ResolvedCall, TreeStore};
use std::collections::{HashMap, HashSet};

/// Graph node identity: a known definition (file:line) or an external name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Def(DefId),
    External(String),
}

pub struct CallGraph {
    pub index: DefIndex,
    pub calls: Vec<ResolvedCall>,
    /// caller NodeKey -> indices into `calls`
    pub fwd: HashMap<NodeKey, Vec<usize>>,
    /// callee NodeKey -> indices into `calls`
    pub rev: HashMap<NodeKey, Vec<usize>>,
    /// Every call site, not deduplicated, in file then document order. Only
    /// resolvers that fill `Resolver::resolve_calls_tree` sites add to it (Go).
    #[allow(dead_code)] // read by the flow engine
    pub sites: Vec<CallSite>,
}

/// Build from files on disk. Each resolver parses its own files.
pub fn build(jobs: &[(String, &'static str)]) -> CallGraph {
    let sources: Vec<Option<Vec<u8>>> = jobs.iter().map(|(f, _)| std::fs::read(f).ok()).collect();
    let files: Vec<(&str, &'static str, &[u8], Option<&tree_sitter::Tree>)> = jobs
        .iter()
        .zip(&sources)
        .filter_map(|((f, l), s)| s.as_ref().map(|s| (f.as_str(), *l, s.as_slice(), None)))
        .collect();
    assemble(&files)
}

#[allow(dead_code)] // wired by the path command
/// Build from a `TreeStore`, so resolvers that take a tree (Go) do not parse
/// again. Same graph as `build` on the same files.
pub fn build_from_store(store: &TreeStore) -> CallGraph {
    let files: Vec<(&str, &'static str, &[u8], Option<&tree_sitter::Tree>)> = store
        .iter()
        .map(|(f, p)| (f, p.lang, p.source.as_slice(), p.tree.as_ref()))
        .collect();
    assemble(&files)
}

fn assemble(files: &[(&str, &'static str, &[u8], Option<&tree_sitter::Tree>)]) -> CallGraph {
    // Pass 1: index every definition.
    let mut index = DefIndex::default();
    for &(file, lang_name, source, tree) in files {
        let resolver = resolver_for(lang_name);
        for def in resolver.collect_defs_tree(file, source, tree) {
            index.push(def);
        }
        index.impls.extend(resolver.collect_impls(file, source));
    }

    // Pass 2: resolve call sites against the full index.
    let mut calls: Vec<ResolvedCall> = Vec::new();
    let mut sites: Vec<CallSite> = Vec::new();
    let mut seen: HashSet<(usize, String, Option<usize>)> = HashSet::new();
    for &(file, lang_name, source, tree) in files {
        let (resolved, file_sites) = resolver_for(lang_name).resolve_calls_tree(file, source, tree, &index);
        sites.extend(file_sites);
        for rc in resolved {
            // one edge per (caller, callee) pair; first call site wins
            if seen.insert((rc.caller, rc.callee_display.clone(), rc.target.map(|(t, _)| t))) {
                calls.push(rc);
            }
        }
    }

    let mut fwd: HashMap<NodeKey, Vec<usize>> = HashMap::new();
    let mut rev: HashMap<NodeKey, Vec<usize>> = HashMap::new();
    for (i, rc) in calls.iter().enumerate() {
        let caller_key = NodeKey::Def(index.defs[rc.caller].id.clone());
        let callee_key = match rc.target {
            Some((t, _)) => NodeKey::Def(index.defs[t].id.clone()),
            None => NodeKey::External(rc.callee_display.clone()),
        };
        fwd.entry(caller_key).or_default().push(i);
        rev.entry(callee_key).or_default().push(i);
    }
    // deterministic traversal: edges in call-site order
    for list in fwd.values_mut().chain(rev.values_mut()) {
        list.sort_by_key(|&i| (calls[i].line, calls[i].callee_display.clone()));
    }

    CallGraph { index, calls, fwd, rev, sites }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Confidence;

    fn tmp(name: &str, src: &str) -> String {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, src).expect("write tmp file");
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn same_named_fns_in_different_files_are_distinct_nodes() {
        // a::helper calls b_entry; b::helper calls a_entry. With name-keyed
        // resolution these used to collapse / falsely cycle.
        let path_a = tmp("ctsq_graph_a.rs", "fn helper() { b_entry(); }\nfn a_entry() {}\n");
        let path_b = tmp("ctsq_graph_b.rs", "fn helper() { a_entry(); }\nfn b_entry() {}\n");
        let jobs = vec![(path_a.clone(), "rust"), (path_b.clone(), "rust")];

        let g = build(&jobs);
        let helpers: Vec<_> = g.index.by_name.get("helper").unwrap().clone();
        assert_eq!(helpers.len(), 2);
        for &h in &helpers {
            let key = NodeKey::Def(g.index.defs[h].id.clone());
            let edges = g.fwd.get(&key).expect("each helper has its own edges");
            assert_eq!(edges.len(), 1, "each helper calls exactly one entry fn");
        }
    }

    /// Every edge, then each adjacency list, in a stable text form. The
    /// `callers`, `callees` and `callgraph` printers read nothing else.
    fn dump(g: &CallGraph) -> String {
        let root = format!("{}/fixtures/", env!("CARGO_MANIFEST_DIR"));
        let id = |d: &DefId| format!("{}:{}", d.file.trim_start_matches(&root), d.line);
        let key = |k: &NodeKey| match k {
            NodeKey::Def(d) => id(d),
            NodeKey::External(n) => format!("ext {}", n),
        };
        let mut out = String::new();
        for (i, rc) in g.calls.iter().enumerate() {
            let target = match rc.target {
                Some((t, c)) => format!("{} {:?}", id(&g.index.defs[t].id), c),
                None => "-".into(),
            };
            out.push_str(&format!(
                "{} {} {}@{} -> {}\n",
                i, id(&g.index.defs[rc.caller].id), rc.callee_display, rc.line, target
            ));
        }
        for (name, map) in [("fwd", &g.fwd), ("rev", &g.rev)] {
            let mut keys: Vec<_> = map.iter().map(|(k, v)| (key(k), v.clone())).collect();
            keys.sort();
            for (k, v) in keys {
                out.push_str(&format!("{} {} {:?}\n", name, k, v));
            }
        }
        out
    }

    #[test]
    fn graph_on_fixtures_is_pinned() {
        let path = format!("{}/fixtures", env!("CARGO_MANIFEST_DIR"));
        let jobs = crate::files::resolve_jobs(&path, None, false);
        let got = dump(&build(&jobs));
        let want = GRAPH_PIN;
        assert_eq!(got, want, "call graph on fixtures/ changed:\n{}", got);
    }

    #[test]
    fn rust_method_calls_appear_as_edges() {
        let path = tmp(
            "ctsq_graph_methods.rs",
            "struct Foo;\nimpl Foo {\n  fn m(&self) {}\n}\nfn go() {\n  let x: Foo = make();\n  x.m();\n  Foo::m(&x);\n}\n",
        );
        let jobs = vec![(path.clone(), "rust")];
        let g = build(&jobs);

        let go = g.index.by_name.get("go").unwrap()[0];
        let key = NodeKey::Def(g.index.defs[go].id.clone());
        let edges = g.fwd.get(&key).expect("go has edges");
        let resolved: Vec<_> = edges.iter()
            .filter_map(|&e| g.calls[e].target)
            .collect();
        assert!(
            resolved.iter().any(|(t, c)| g.index.defs[*t].name == "m" && *c == Confidence::Exact),
            "method call x.m() should resolve to Foo::m"
        );
    }

    #[test]
    fn graph_from_a_store_matches_graph_from_disk() {
        let path = format!("{}/fixtures", env!("CARGO_MANIFEST_DIR"));
        let jobs = crate::files::resolve_jobs(&path, None, false);
        let store = TreeStore::build(&jobs);
        assert_eq!(dump(&build_from_store(&store)), GRAPH_PIN);
    }

    #[test]
    fn go_call_sites_keep_every_site_with_argument_ranges() {
        let src = "package a\nfunc h(x int, y int) {}\nfunc Go() {\n  h(1, 2)\n  h(3, 4)\n  println()\n}\n";
        let mut store = TreeStore::default();
        store.insert("a/a.go", "go", src.as_bytes().to_vec());
        let g = build_from_store(&store);
        assert_eq!(g.calls.len(), 2, "one edge per pair: {:?}", g.calls);
        let to_h: Vec<_> = g.sites.iter().filter(|s| s.callee_display == "h").collect();
        assert_eq!(to_h.len(), 2, "every site is kept: {:?}", g.sites);
        assert_eq!(to_h[1].line, 5);
        assert_eq!(&src[to_h[1].start_byte..to_h[1].end_byte], "h(3, 4)");
        let args: Vec<&str> = to_h[1].args.iter().map(|&(a, b)| &src[a..b]).collect();
        assert_eq!(args, vec!["3", "4"]);
        let h = to_h[1].target.unwrap().0;
        assert_eq!(g.index.defs[h].name, "h");
        assert!(g.sites.iter().any(|s| s.callee_display == "println" && s.target.is_none()));
    }

    #[test]
    fn non_go_resolvers_add_no_sites() {
        let path = tmp("ctsq_graph_sites.rs", "fn a() { b(); }\nfn b() {}\n");
        let g = build(&[(path, "rust")]);
        assert_eq!(g.calls.len(), 1);
        assert!(g.sites.is_empty());
    }

    /// Recorded from `build` before the call-walk side tables were added.
    const GRAPH_PIN: &str = r#"0 attrs/logs.go:17 Printf@18 -> -
1 attrs/logs.go:17 Info@20 -> -
2 attrs/paths.py:6 join@7 -> -
3 attrs/paths.py:6 info@8 -> -
4 attrs/paths.py:11 join@7 -> -
5 attrs/paths.py:11 info@8 -> -
6 example.c:4 malloc@6 -> -
7 example.c:4 free@7 -> -
8 example.c:10 process@11 -> -
9 example.cpp:6 malloc@7 -> -
10 example.cpp:24 process@26 -> -
11 example.go:10 Println@11 -> -
12 example.go:18 NewServer@19 -> example.go:14 Exact
13 example.go:18 Start@20 -> example.go:10 Exact
14 example.js:1 log@2 -> -
15 example.js:1 helper@11 -> -
16 example.js:10 log@2 -> -
17 example.js:10 helper@11 -> -
18 example.py:1 malloc@2 -> -
19 example.rb:6 allocate_buffer@7 -> -
20 importgraph/c/main.c:4 printf@5 -> -
21 importgraph/c/main.c:4 add@5 -> -
22 importgraph/go/main.go:9 Distance@10 -> importgraph/go/util/geo.go:3 Exact
23 importgraph/go/main.go:9 Println@11 -> -
24 importgraph/go/main.go:9 Pretty@11 -> importgraph/go/util/format.go:5 Exact
25 importgraph/go/util/format.go:5 Sprintf@6 -> -
26 importgraph/js/app.ts:6 helper@7 -> -
27 importgraph/js/app.ts:6 log@9 -> -
28 importgraph/py/app.py:7 Geo@8 -> -
29 importgraph/py/app.py:7 do_thing@9 -> -
30 importgraph/py/app.py:7 print@10 -> -
31 importgraph/py/app.py:7 getpid@10 -> -
32 importgraph/py/helpers.py:4 Geo@5 -> -
33 importgraph/py/util/geo.py:5 do_thing@6 -> -
34 importgraph/ruby/app.rb:6 ping@7 -> -
35 importgraph/ruby/app.rb:6 new@7 -> -
36 importgraph/ruby/app.rb:6 dump@9 -> -
37 importgraph/ruby/helpers.rb:3 new@4 -> -
38 importgraph/ts-alias/src/app.ts:5 helper@5 -> -
39 queries/go-chi/main.go:17 NewRouter@18 -> -
40 queries/go-chi/main.go:17 Get@20 -> -
41 queries/go-chi/main.go:17 Post@21 -> -
42 queries/go-chi/main.go:17 Route@23 -> -
43 queries/go-chi/main.go:17 Delete@24 -> -
44 queries/go-chi/main.go:17 ListenAndServe@26 -> -
45 queries/go-http/main.go:19 HandleFunc@20 -> -
46 queries/go-http/main.go:19 authed@22 -> queries/go-http/main.go:15 Exact
47 queries/go-http/main.go:19 Handle@23 -> -
48 queries/go-http/main.go:19 NotFoundHandler@23 -> -
49 queries/go-http/main.go:26 NewServeMux@28 -> -
50 queries/go-http/main.go:26 Routes@29 -> queries/go-http/main.go:19 Exact
51 queries/go-http/main.go:26 HandleFunc@30 -> -
52 queries/go-http/main.go:26 ListenAndServe@31 -> -
53 queries/next-app/app/about/page.tsx:1 <p>@1 -> -
54 queries/next-app/app/blog/[slug]/page.tsx:1 <p>@1 -> -
55 queries/next-app/app/page.tsx:1 <h1>@1 -> -
56 queries/solid-mavexa/frontend/src/routes/library.tsx:3 createSignal@4 -> -
57 queries/solid-mavexa/frontend/src/routes/library.tsx:3 <div>@5 -> -
58 queries/solid-mavexa/frontend/src/routes/library.tsx:3 String@5 -> -
59 queries/solid-mavexa/frontend/src/routes/library.tsx:3 open@5 -> -
60 queries/solid-mavexa/frontend/src/routes/library/[id].tsx:1 <div>@2 -> -
61 typescript/example.ts:26 log@34 -> -
62 typescript/example.ts:27 log@28 -> -
63 typescript/example.ts:33 log@34 -> -
64 typescript/example.tsx:8 <button onClick={onClick}>@9 -> -
65 typescript/example.tsx:12 <span>@12 -> -
66 typescript/example.tsx:14 log@15 -> -
67 typescript/example.tsx:14 <div>@17 -> -
68 typescript/example.tsx:14 <Button label=\"Click me\" onClick={handleClick} />@18 -> -
69 typescript/example.tsx:14 <Icon />@19 -> -
fwd attrs/logs.go:17 [0, 1]
fwd attrs/paths.py:11 [4, 5]
fwd attrs/paths.py:6 [2, 3]
fwd example.c:10 [8]
fwd example.c:4 [6, 7]
fwd example.cpp:24 [10]
fwd example.cpp:6 [9]
fwd example.go:10 [11]
fwd example.go:18 [12, 13]
fwd example.js:1 [14, 15]
fwd example.js:10 [16, 17]
fwd example.py:1 [18]
fwd example.rb:6 [19]
fwd importgraph/c/main.c:4 [21, 20]
fwd importgraph/go/main.go:9 [22, 24, 23]
fwd importgraph/go/util/format.go:5 [25]
fwd importgraph/js/app.ts:6 [26, 27]
fwd importgraph/py/app.py:7 [28, 29, 31, 30]
fwd importgraph/py/helpers.py:4 [32]
fwd importgraph/py/util/geo.py:5 [33]
fwd importgraph/ruby/app.rb:6 [35, 34, 36]
fwd importgraph/ruby/helpers.rb:3 [37]
fwd importgraph/ts-alias/src/app.ts:5 [38]
fwd queries/go-chi/main.go:17 [39, 40, 41, 42, 43, 44]
fwd queries/go-http/main.go:19 [45, 46, 47, 48]
fwd queries/go-http/main.go:26 [49, 50, 51, 52]
fwd queries/next-app/app/about/page.tsx:1 [53]
fwd queries/next-app/app/blog/[slug]/page.tsx:1 [54]
fwd queries/next-app/app/page.tsx:1 [55]
fwd queries/solid-mavexa/frontend/src/routes/library.tsx:3 [56, 57, 58, 59]
fwd queries/solid-mavexa/frontend/src/routes/library/[id].tsx:1 [60]
fwd typescript/example.ts:26 [61]
fwd typescript/example.ts:27 [62]
fwd typescript/example.ts:33 [63]
fwd typescript/example.tsx:12 [65]
fwd typescript/example.tsx:14 [66, 67, 68, 69]
fwd typescript/example.tsx:8 [64]
rev example.go:10 [13]
rev example.go:14 [12]
rev ext <Button label=\"Click me\" onClick={handleClick} /> [68]
rev ext <Icon /> [69]
rev ext <button onClick={onClick}> [64]
rev ext <div> [60, 57, 67]
rev ext <h1> [55]
rev ext <p> [53, 54]
rev ext <span> [65]
rev ext Delete [43]
rev ext Geo [32, 28]
rev ext Get [40]
rev ext Handle [47]
rev ext HandleFunc [45, 51]
rev ext Info [1]
rev ext ListenAndServe [44, 52]
rev ext NewRouter [39]
rev ext NewServeMux [49]
rev ext NotFoundHandler [48]
rev ext Post [41]
rev ext Printf [0]
rev ext Println [11, 23]
rev ext Route [42]
rev ext Sprintf [25]
rev ext String [58]
rev ext add [21]
rev ext allocate_buffer [19]
rev ext createSignal [56]
rev ext do_thing [33, 29]
rev ext dump [36]
rev ext free [7]
rev ext getpid [31]
rev ext helper [38, 26, 15, 17]
rev ext info [3, 5]
rev ext join [2, 4]
rev ext log [14, 16, 27, 66, 62, 61, 63]
rev ext malloc [18, 6, 9]
rev ext new [37, 35]
rev ext open [59]
rev ext ping [34]
rev ext print [30]
rev ext printf [20]
rev ext process [8, 10]
rev importgraph/go/util/format.go:5 [24]
rev importgraph/go/util/geo.go:3 [22]
rev queries/go-http/main.go:15 [46]
rev queries/go-http/main.go:19 [50]
"#;
}
