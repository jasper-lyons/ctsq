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

/// Build from files on disk, each read and parsed once through the cache. The
/// trees live only as long as this call; a caller that wants later stages to
/// share them builds a `TreeStore` and keeps it (see `build_from_store`).
pub fn build(jobs: &[(String, &'static str)]) -> CallGraph {
    build_from_store(&TreeStore::build(jobs))
}

/// Build from a `TreeStore`, so no resolver parses again. Same graph as `build`
/// on the same files.
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
        index.impls.extend(resolver.collect_impls_tree(file, source, tree));
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
20 glossary/src/pipeline.go:5 New@6 -> -
21 glossary/src/pipeline.go:9 New@10 -> -
22 glossary/src/pipeline.go:13 New@14 -> -
23 glossary/src/pipeline.go:17 New@18 -> -
24 importgraph/c/main.c:4 printf@5 -> -
25 importgraph/c/main.c:4 add@5 -> -
26 importgraph/go/main.go:9 Distance@10 -> importgraph/go/util/geo.go:3 Exact
27 importgraph/go/main.go:9 Println@11 -> -
28 importgraph/go/main.go:9 Pretty@11 -> importgraph/go/util/format.go:5 Exact
29 importgraph/go/util/format.go:5 Sprintf@6 -> -
30 importgraph/js/app.ts:6 helper@7 -> -
31 importgraph/js/app.ts:6 log@9 -> -
32 importgraph/py/app.py:7 Geo@8 -> -
33 importgraph/py/app.py:7 do_thing@9 -> -
34 importgraph/py/app.py:7 print@10 -> -
35 importgraph/py/app.py:7 getpid@10 -> -
36 importgraph/py/helpers.py:4 Geo@5 -> -
37 importgraph/py/util/geo.py:5 do_thing@6 -> -
38 importgraph/ruby/app.rb:6 ping@7 -> -
39 importgraph/ruby/app.rb:6 new@7 -> -
40 importgraph/ruby/app.rb:6 dump@9 -> -
41 importgraph/ruby/helpers.rb:3 new@4 -> -
42 importgraph/ts-alias/src/app.ts:5 helper@5 -> -
43 queries/errors/go/load.go:10 Errorf@14 -> -
44 queries/errors/go/load.go:10 New@14 -> -
45 queries/errors/go/load.go:17 Errorf@18 -> -
46 queries/errors/go/load.go:21 New@22 -> -
47 queries/errors/go/load.go:25 Errorf@26 -> -
48 queries/errors/rb/gig.rb:2 raise \"Slot #{slot} is taken\@3 -> -
49 queries/errors/rb/gig.rb:2 nil?@3 -> -
50 queries/errors/rb/gig.rb:2 raise ArgumentError@4 -> -
51 queries/go-chi/main.go:17 NewRouter@18 -> -
52 queries/go-chi/main.go:17 Get@20 -> -
53 queries/go-chi/main.go:17 Post@21 -> -
54 queries/go-chi/main.go:17 Route@23 -> -
55 queries/go-chi/main.go:17 Delete@24 -> -
56 queries/go-chi/main.go:17 ListenAndServe@26 -> -
57 queries/go-http/main.go:19 HandleFunc@20 -> -
58 queries/go-http/main.go:19 authed@22 -> queries/go-http/main.go:15 Exact
59 queries/go-http/main.go:19 Handle@23 -> -
60 queries/go-http/main.go:19 NotFoundHandler@23 -> -
61 queries/go-http/main.go:26 NewServeMux@28 -> -
62 queries/go-http/main.go:26 Routes@29 -> queries/go-http/main.go:19 Exact
63 queries/go-http/main.go:26 HandleFunc@30 -> -
64 queries/go-http/main.go:26 ListenAndServe@31 -> -
65 queries/jsx/src/Card.tsx:3 <section title=\"Profile card\">@5 -> -
66 queries/jsx/src/Card.tsx:3 <h2>@6 -> -
67 queries/jsx/src/Card.tsx:3 <input placeholder=\"Search people\" aria-label={\"Find a person\"} />@7 -> -
68 queries/jsx/src/Card.tsx:3 name} />@8 -> -
69 queries/jsx/src/Card.tsx:3 <Button label=\"Save changes\" tooltip=\"Stores the card\" />@9 -> -
70 queries/jsx/src/Card.tsx:14 <footer>@14 -> -
71 queries/jsx/src/Card.tsx:16 <div>@18 -> -
72 queries/jsx/src/Card.tsx:21 <Divider />@21 -> -
73 queries/jsx/src/Live.tsx:1 <div className=\"Not text\">@3 -> -
74 queries/jsx/src/Live.tsx:1 <b>@4 -> -
75 queries/jsx/src/Live.tsx:1 <i>@5 -> -
76 queries/jsx/src/Live.tsx:1 <u>@7 -> -
77 queries/next-app/app/about/page.tsx:1 <p>@1 -> -
78 queries/next-app/app/blog/[slug]/page.tsx:1 <p>@1 -> -
79 queries/next-app/app/page.tsx:1 <h1>@1 -> -
80 queries/solid-mavexa/frontend/src/routes/library.tsx:3 createSignal@4 -> -
81 queries/solid-mavexa/frontend/src/routes/library.tsx:3 <div>@5 -> -
82 queries/solid-mavexa/frontend/src/routes/library.tsx:3 String@5 -> -
83 queries/solid-mavexa/frontend/src/routes/library.tsx:3 open@5 -> -
84 queries/solid-mavexa/frontend/src/routes/library/[id].tsx:1 <div>@2 -> -
85 typescript/example.ts:26 log@34 -> -
86 typescript/example.ts:27 log@28 -> -
87 typescript/example.ts:33 log@34 -> -
88 typescript/example.tsx:8 <button onClick={onClick}>@9 -> -
89 typescript/example.tsx:12 <span>@12 -> -
90 typescript/example.tsx:14 log@15 -> -
91 typescript/example.tsx:14 <div>@17 -> -
92 typescript/example.tsx:14 <Button label=\"Click me\" onClick={handleClick} />@18 -> -
93 typescript/example.tsx:14 <Icon />@19 -> -
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
fwd glossary/src/pipeline.go:13 [22]
fwd glossary/src/pipeline.go:17 [23]
fwd glossary/src/pipeline.go:5 [20]
fwd glossary/src/pipeline.go:9 [21]
fwd importgraph/c/main.c:4 [25, 24]
fwd importgraph/go/main.go:9 [26, 28, 27]
fwd importgraph/go/util/format.go:5 [29]
fwd importgraph/js/app.ts:6 [30, 31]
fwd importgraph/py/app.py:7 [32, 33, 35, 34]
fwd importgraph/py/helpers.py:4 [36]
fwd importgraph/py/util/geo.py:5 [37]
fwd importgraph/ruby/app.rb:6 [39, 38, 40]
fwd importgraph/ruby/helpers.rb:3 [41]
fwd importgraph/ts-alias/src/app.ts:5 [42]
fwd queries/errors/go/load.go:10 [43, 44]
fwd queries/errors/go/load.go:17 [45]
fwd queries/errors/go/load.go:21 [46]
fwd queries/errors/go/load.go:25 [47]
fwd queries/errors/rb/gig.rb:2 [49, 48, 50]
fwd queries/go-chi/main.go:17 [51, 52, 53, 54, 55, 56]
fwd queries/go-http/main.go:19 [57, 58, 59, 60]
fwd queries/go-http/main.go:26 [61, 62, 63, 64]
fwd queries/jsx/src/Card.tsx:14 [70]
fwd queries/jsx/src/Card.tsx:16 [71]
fwd queries/jsx/src/Card.tsx:21 [72]
fwd queries/jsx/src/Card.tsx:3 [65, 66, 67, 68, 69]
fwd queries/jsx/src/Live.tsx:1 [73, 74, 75, 76]
fwd queries/next-app/app/about/page.tsx:1 [77]
fwd queries/next-app/app/blog/[slug]/page.tsx:1 [78]
fwd queries/next-app/app/page.tsx:1 [79]
fwd queries/solid-mavexa/frontend/src/routes/library.tsx:3 [80, 81, 82, 83]
fwd queries/solid-mavexa/frontend/src/routes/library/[id].tsx:1 [84]
fwd typescript/example.ts:26 [85]
fwd typescript/example.ts:27 [86]
fwd typescript/example.ts:33 [87]
fwd typescript/example.tsx:12 [89]
fwd typescript/example.tsx:14 [90, 91, 92, 93]
fwd typescript/example.tsx:8 [88]
rev example.go:10 [13]
rev example.go:14 [12]
rev ext <Button label=\"Click me\" onClick={handleClick} /> [92]
rev ext <Button label=\"Save changes\" tooltip=\"Stores the card\" /> [69]
rev ext <Divider /> [72]
rev ext <Icon /> [93]
rev ext <b> [74]
rev ext <button onClick={onClick}> [88]
rev ext <div className=\"Not text\"> [73]
rev ext <div> [84, 81, 91, 71]
rev ext <footer> [70]
rev ext <h1> [79]
rev ext <h2> [66]
rev ext <i> [75]
rev ext <input placeholder=\"Search people\" aria-label={\"Find a person\"} /> [67]
rev ext <p> [77, 78]
rev ext <section title=\"Profile card\"> [65]
rev ext <span> [89]
rev ext <u> [76]
rev ext Delete [55]
rev ext Errorf [43, 45, 47]
rev ext Geo [36, 32]
rev ext Get [52]
rev ext Handle [59]
rev ext HandleFunc [57, 63]
rev ext Info [1]
rev ext ListenAndServe [56, 64]
rev ext New [20, 21, 22, 44, 23, 46]
rev ext NewRouter [51]
rev ext NewServeMux [61]
rev ext NotFoundHandler [60]
rev ext Post [53]
rev ext Printf [0]
rev ext Println [11, 27]
rev ext Route [54]
rev ext Sprintf [29]
rev ext String [82]
rev ext add [25]
rev ext allocate_buffer [19]
rev ext createSignal [80]
rev ext do_thing [37, 33]
rev ext dump [40]
rev ext free [7]
rev ext getpid [35]
rev ext helper [42, 30, 15, 17]
rev ext info [3, 5]
rev ext join [2, 4]
rev ext log [14, 16, 31, 90, 86, 85, 87]
rev ext malloc [18, 6, 9]
rev ext name} /> [68]
rev ext new [41, 39]
rev ext nil? [49]
rev ext open [83]
rev ext ping [38]
rev ext print [34]
rev ext printf [24]
rev ext process [8, 10]
rev ext raise ArgumentError [50]
rev ext raise \"Slot #{slot} is taken\ [48]
rev importgraph/go/util/format.go:5 [28]
rev importgraph/go/util/geo.go:3 [26]
rev queries/go-http/main.go:15 [58]
rev queries/go-http/main.go:19 [62]
"#;
}
