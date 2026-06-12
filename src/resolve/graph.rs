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
}
