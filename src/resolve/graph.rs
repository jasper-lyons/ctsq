//! Project-wide call graph: every definition indexed, every call site
//! resolved, with forward/reverse adjacency for in-memory traversal.

use super::{resolver_for, DefId, DefIndex, ResolvedCall};
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
}

pub fn build(jobs: &[(String, &'static str)]) -> CallGraph {
    // Pass 1: index every definition.
    let mut index = DefIndex::default();
    let mut sources: Vec<Option<Vec<u8>>> = Vec::with_capacity(jobs.len());
    for (file, lang_name) in jobs {
        let Ok(source) = std::fs::read(file) else {
            sources.push(None);
            continue;
        };
        let resolver = resolver_for(lang_name);
        for def in resolver.collect_defs(file, &source) {
            index.push(def);
        }
        index.impls.extend(resolver.collect_impls(file, &source));
        sources.push(Some(source));
    }

    // Pass 2: resolve call sites against the full index.
    let mut calls: Vec<ResolvedCall> = Vec::new();
    let mut seen: HashSet<(usize, String, Option<usize>)> = HashSet::new();
    for ((file, lang_name), source) in jobs.iter().zip(&sources) {
        let Some(source) = source else { continue };
        for rc in resolver_for(lang_name).resolve_calls(file, source, &index) {
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

    CallGraph { index, calls, fwd, rev }
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
39 typescript/example.ts:26 log@34 -> -
40 typescript/example.ts:27 log@28 -> -
41 typescript/example.ts:33 log@34 -> -
42 typescript/example.tsx:8 <button onClick={onClick}>@9 -> -
43 typescript/example.tsx:12 <span>@12 -> -
44 typescript/example.tsx:14 log@15 -> -
45 typescript/example.tsx:14 <div>@17 -> -
46 typescript/example.tsx:14 <Button label=\"Click me\" onClick={handleClick} />@18 -> -
47 typescript/example.tsx:14 <Icon />@19 -> -
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
fwd typescript/example.ts:26 [39]
fwd typescript/example.ts:27 [40]
fwd typescript/example.ts:33 [41]
fwd typescript/example.tsx:12 [43]
fwd typescript/example.tsx:14 [44, 45, 46, 47]
fwd typescript/example.tsx:8 [42]
rev example.go:10 [13]
rev example.go:14 [12]
rev ext <Button label=\"Click me\" onClick={handleClick} /> [46]
rev ext <Icon /> [47]
rev ext <button onClick={onClick}> [42]
rev ext <div> [45]
rev ext <span> [43]
rev ext Geo [32, 28]
rev ext Info [1]
rev ext Printf [0]
rev ext Println [11, 23]
rev ext Sprintf [25]
rev ext add [21]
rev ext allocate_buffer [19]
rev ext do_thing [33, 29]
rev ext dump [36]
rev ext free [7]
rev ext getpid [31]
rev ext helper [38, 26, 15, 17]
rev ext info [3, 5]
rev ext join [2, 4]
rev ext log [14, 16, 27, 44, 40, 39, 41]
rev ext malloc [18, 6, 9]
rev ext new [37, 35]
rev ext ping [34]
rev ext print [30]
rev ext printf [20]
rev ext process [8, 10]
rev importgraph/go/util/format.go:5 [24]
rev importgraph/go/util/geo.go:3 [22]
"#;
}
