//! The call graph built from the files a question needs (plans/perf/scoped-graph.md).
//!
//! `callers`, `callees`, `explain` and `scopeof --callers` walk the graph from
//! a few names to a fixed depth. Each walk reads the edges of the keys it
//! reaches, and only those edges have to be the whole-folder graph's. Two
//! file sets get there:
//! - the resolved files: those that can hold an edge the walk reads. A call
//!   to a definition named N, or left unresolved under N, is written in a file
//!   whose text holds N, and the calls of a function sit in its own file.
//! - the index files: those whose definitions the resolvers may read. The
//!   resolvers read the index only by name (`DefIndex::named` and `ask`), and
//!   every answer for a name comes from files holding it. A pass that asks a
//!   name not yet covered loads the files holding it and runs again, until a
//!   pass asks nothing new. Then every answer was the whole folder's.
//!
//! `CTSQ_WHOLE_GRAPH=1` builds the whole-folder graph instead.

use super::graph::{self, CallGraph, Loaded, NodeKey};
use super::{DefId, Want};
use std::collections::{BTreeSet, HashMap, HashSet};

/// What a walk starts from and how far it goes.
pub struct Seeds {
    /// Names whose definitions are the roots. Each also roots its bare-name
    /// node, where unresolved calls under that name sit.
    pub names: Vec<String>,
    /// Levels walked up (callers) from the roots.
    pub up: usize,
    /// Levels walked down (callees) from the roots.
    pub down: usize,
}

/// The graph a walk from `seeds` reads, with every edge it can reach equal to
/// the whole-folder graph's.
pub fn build(jobs: &[(String, &'static str)], seeds: &Seeds) -> CallGraph {
    if whole() {
        return graph::build(jobs);
    }
    Scope::new(jobs).run(seeds)
}

/// Whether `CTSQ_WHOLE_GRAPH` asks for the whole-folder graph.
pub fn whole() -> bool {
    std::env::var_os("CTSQ_WHOLE_GRAPH").is_some_and(|v| !v.is_empty() && v != "0")
}

enum State {
    Unread,
    /// Unreadable, or a path an earlier job already gives.
    Skipped,
    Loaded(Loaded),
}

struct Scope<'a> {
    jobs: &'a [(String, &'static str)],
    words: Words,
    files: Vec<State>,
    /// Names whose holding files are all loaded.
    covered: HashSet<String>,
    job_of: HashMap<&'a str, usize>,
}

impl<'a> Scope<'a> {
    fn new(jobs: &'a [(String, &'static str)]) -> Scope<'a> {
        let mut job_of: HashMap<&str, usize> = HashMap::new();
        let mut files = Vec::with_capacity(jobs.len());
        for (i, (f, _)) in jobs.iter().enumerate() {
            if job_of.contains_key(f.as_str()) {
                files.push(State::Skipped);
            } else {
                job_of.insert(f, i);
                files.push(State::Unread);
            }
        }
        Scope { jobs, words: Words::new(jobs), files, covered: HashSet::new(), job_of }
    }

    fn load(&mut self, j: usize) {
        if !matches!(self.files[j], State::Unread) {
            return;
        }
        let (file, lang) = &self.jobs[j];
        self.files[j] = match crate::parsed::get(file, lang) {
            Some(p) => State::Loaded(Loaded::read(file, &p)),
            None => State::Skipped,
        };
    }

    /// Load every file holding one of `names`.
    fn cover(&mut self, names: &[String]) {
        for n in names {
            if self.covered.insert(n.clone()) {
                for j in self.words.holders(n) {
                    self.load(j);
                }
            }
        }
    }

    fn loaded(&self) -> Vec<&Loaded> {
        self.files
            .iter()
            .filter_map(|s| match s {
                State::Loaded(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    fn run(mut self, seeds: &Seeds) -> CallGraph {
        self.cover(&seeds.names);
        let mut up_front: Vec<(NodeKey, String)> = Vec::new();
        let mut down_front: Vec<DefId> = Vec::new();
        {
            let index = graph::index_of(&self.loaded());
            for name in &seeds.names {
                for &i in index.by_name.get(name).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let d = &index.defs[i];
                    if seeds.up > 0 {
                        up_front.push((NodeKey::Def(d.id.clone()), d.name.clone()));
                    }
                    if seeds.down > 0 {
                        down_front.push(d.id.clone());
                    }
                }
                if seeds.up > 0 {
                    up_front.push((NodeKey::External(name.clone()), name.clone()));
                }
            }
        }
        let mut up_seen: HashSet<NodeKey> = up_front.iter().map(|(k, _)| k.clone()).collect();
        let mut down_seen: HashSet<DefId> = down_front.iter().cloned().collect();
        let mut up_names: HashSet<String> = HashSet::new();
        let mut lines: HashMap<usize, HashSet<usize>> = HashMap::new();
        let mut resolved: BTreeSet<usize> = BTreeSet::new();
        let mut depth = 0;
        loop {
            for (_, name) in &up_front {
                if up_names.insert(name.clone()) {
                    resolved.extend(self.words.holders(name));
                }
            }
            for id in &down_front {
                if let Some(&j) = self.job_of.get(id.file.as_str()) {
                    resolved.insert(j);
                    lines.entry(j).or_default().insert(id.line);
                }
            }
            let g = self.settle(&resolved, &up_names, &lines);
            depth += 1;
            let mut next_up = Vec::new();
            let mut next_down = Vec::new();
            if depth < seeds.up {
                for (k, _) in &up_front {
                    for &e in g.rev.get(k).map(|v| v.as_slice()).unwrap_or(&[]) {
                        let d = &g.index.defs[g.calls[e].caller];
                        let key = NodeKey::Def(d.id.clone());
                        if up_seen.insert(key.clone()) {
                            next_up.push((key, d.name.clone()));
                        }
                    }
                }
            }
            if depth < seeds.down {
                for id in &down_front {
                    for &e in g.fwd.get(&NodeKey::Def(id.clone())).map(|v| v.as_slice()).unwrap_or(&[]) {
                        if let Some((t, _)) = g.calls[e].target {
                            let d = &g.index.defs[t];
                            if down_seen.insert(d.id.clone()) {
                                next_down.push(d.id.clone());
                            }
                        }
                    }
                }
            }
            if next_up.is_empty() && next_down.is_empty() {
                return g;
            }
            up_front = next_up;
            down_front = next_down;
        }
    }

    /// The graph over the `resolved` files, each resolving what its `Want`
    /// asks, against an index grown until no pass asks an uncovered name.
    fn settle(
        &mut self,
        resolved: &BTreeSet<usize>,
        names: &HashSet<String>,
        lines: &HashMap<usize, HashSet<usize>>,
    ) -> CallGraph {
        for &j in resolved {
            self.load(j);
        }
        loop {
            let new: Vec<String> = {
                let index = graph::index_of(&self.loaded());
                index.record();
                let mut out = Vec::new();
                for &j in resolved {
                    if let State::Loaded(l) = &self.files[j] {
                        let want = Want { names, lines: lines.get(&j) };
                        out.push(l.resolve(&index, Some(&want)));
                    }
                }
                let mut new: Vec<String> =
                    index.take_asked().into_iter().filter(|n| !self.covered.contains(n)).collect();
                if new.is_empty() {
                    return graph::finish(index, out);
                }
                new.sort();
                new
            };
            self.cover(&new);
        }
    }
}

/// Which files hold a name as a whole word. Names made of ASCII letters,
/// digits and `_` are looked up in an index of each file's maximal runs of
/// those bytes. Any other name is searched in the text, as a whole word on
/// each side that ends in one of those bytes.
///
/// A definition's name, and a call's callee name, is a token of its file,
/// and a lexer takes the longest run of identifier bytes, so a file holding
/// a name holds it bounded by bytes that cannot continue it.
struct Words {
    sources: Vec<Option<crate::parsed::Source>>,
    tokens: HashMap<Box<str>, Vec<u32>>,
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl Words {
    fn new(jobs: &[(String, &'static str)]) -> Words {
        let mut tokens: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        let mut sources = Vec::with_capacity(jobs.len());
        for (j, (file, _)) in jobs.iter().enumerate() {
            let src = crate::parsed::source(file);
            if let Some(s) = &src {
                let bytes = s.as_slice();
                let mut i = 0;
                while i < bytes.len() {
                    if !word_byte(bytes[i]) {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < bytes.len() && word_byte(bytes[i]) {
                        i += 1;
                    }
                    // ASCII only, so the run is valid UTF-8.
                    let w = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                    match tokens.get_mut(w) {
                        Some(v) => {
                            if v.last() != Some(&(j as u32)) {
                                v.push(j as u32);
                            }
                        }
                        None => {
                            tokens.insert(w.into(), vec![j as u32]);
                        }
                    }
                }
            }
            sources.push(src);
        }
        Words { sources, tokens }
    }

    /// Jobs whose file holds `name`, in job order.
    fn holders(&self, name: &str) -> Vec<usize> {
        if !name.is_empty() && name.bytes().all(word_byte) {
            return self.tokens.get(name).map(|v| v.iter().map(|&j| j as usize).collect()).unwrap_or_default();
        }
        (0..self.sources.len())
            .filter(|&j| self.sources[j].as_ref().is_some_and(|s| holds_bounded(s, name)))
            .collect()
    }
}

/// Whether `hay` holds `name` with no identifier byte touching an end of it
/// that is itself an identifier byte.
fn holds_bounded(hay: &[u8], name: &str) -> bool {
    let n = name.as_bytes();
    if n.is_empty() {
        return true;
    }
    let (check_before, check_after) = (word_byte(n[0]), word_byte(n[n.len() - 1]));
    let mut from = 0;
    while from + n.len() <= hay.len() {
        let Some(k) = find(&hay[from..], n) else { return false };
        let i = from + k;
        let end = i + n.len();
        let before = !check_before || i == 0 || !word_byte(hay[i - 1]);
        let after = !check_after || end == hay.len() || !word_byte(hay[end]);
        if before && after {
            return true;
        }
        from = i + 1;
    }
    false
}

fn find(hay: &[u8], n: &[u8]) -> Option<usize> {
    hay.windows(n.len()).position(|w| w == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_held_only_as_a_whole_word() {
        assert!(holds_bounded(b"x := a.Set(1)", "Set"));
        assert!(!holds_bounded(b"x := a.Settings(1)", "Set"));
        assert!(!holds_bounded(b"x := Reset(1)", "Set"));
        assert!(holds_bounded(b"Reset() Set()", "Set"));
        assert!(holds_bounded(b"def nil?(x)", "nil?"));
        assert!(holds_bounded(b"a==b", "=="));
        assert!(holds_bounded(b"pkg.Fn()", "pkg.Fn"));
        assert!(!holds_bounded(b"mypkg.Fn()", "pkg.Fn"));
    }

    #[test]
    fn word_names_come_from_the_token_index_and_others_from_the_text() {
        let d = std::env::temp_dir().join(format!("ctsq-scoped-words-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.go");
        let b = d.join("b.go");
        std::fs::write(&a, "func Set() {}\n").unwrap();
        std::fs::write(&b, "func Settings() { x.nil?() }\n").unwrap();
        let jobs = vec![(a.to_string_lossy().to_string(), "go"), (b.to_string_lossy().to_string(), "go")];
        let w = Words::new(&jobs);
        assert_eq!(w.holders("Set"), vec![0]);
        assert_eq!(w.holders("Settings"), vec![1]);
        assert_eq!(w.holders("nil?"), vec![1]);
        assert!(w.holders("Nope").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Every edge of `g` a walk from `keys` reads, as text, up `up` levels
    /// and down `down` levels.
    fn reachable(g: &CallGraph, names: &[&str], up: usize, down: usize) -> Vec<String> {
        let id = |d: &DefId| format!("{}:{}", d.file, d.line);
        let mut out = Vec::new();
        let mut front: Vec<NodeKey> = Vec::new();
        for n in names {
            for &i in g.index.by_name.get(*n).map(|v| v.as_slice()).unwrap_or(&[]) {
                front.push(NodeKey::Def(g.index.defs[i].id.clone()));
            }
            front.push(NodeKey::External(n.to_string()));
        }
        for (dir, depth) in [(true, up), (false, down)] {
            let mut f = front.clone();
            let mut seen: HashSet<NodeKey> = f.iter().cloned().collect();
            for _ in 0..depth {
                let mut next = Vec::new();
                for k in &f {
                    let edges = if dir { g.rev.get(k) } else { g.fwd.get(k) };
                    for &e in edges.map(|v| v.as_slice()).unwrap_or(&[]) {
                        let rc = &g.calls[e];
                        let t = rc.target.map(|(t, c)| format!("{} {:?}", id(&g.index.defs[t].id), c)).unwrap_or_default();
                        out.push(format!("{} {} {}@{} {}", dir, id(&g.index.defs[rc.caller].id), rc.callee_display, rc.line, t));
                        let nk = if dir {
                            Some(NodeKey::Def(g.index.defs[rc.caller].id.clone()))
                        } else {
                            rc.target.map(|(t, _)| NodeKey::Def(g.index.defs[t].id.clone()))
                        };
                        if let Some(nk) = nk {
                            if seen.insert(nk.clone()) {
                                next.push(nk);
                            }
                        }
                    }
                }
                f = next;
            }
        }
        out
    }

    #[test]
    fn the_scoped_graph_reads_the_same_edges_as_the_whole_graph_on_the_fixtures() {
        let path = format!("{}/fixtures", env!("CARGO_MANIFEST_DIR"));
        let jobs = crate::files::resolve_jobs(&path, None, false);
        let whole = graph::build(&jobs);
        for name in ["NewServer", "Start", "main", "process", "helper", "Distance", "New", "log", "authed", "Routes"] {
            for depth in [1, 2, 4] {
                let seeds = Seeds { names: vec![name.to_string()], up: depth, down: depth };
                let g = Scope::new(&jobs).run(&seeds);
                assert_eq!(reachable(&g, &[name], depth, depth), reachable(&whole, &[name], depth, depth), "{} {}", name, depth);
            }
        }
    }

    #[test]
    fn go_interface_dispatch_and_function_values_keep_their_edges() {
        let d = std::env::temp_dir().join(format!("ctsq-scoped-go-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        let files = [
            ("a/w.go", "package a\ntype Writer interface {\n  Write()\n  Close()\n}\n"),
            ("a/file.go", "package a\ntype File struct{}\nfunc (f *File) Write() {}\nfunc (f *File) Close() {}\n"),
            ("a/half.go", "package a\ntype Half struct{}\nfunc (h *Half) Write() {}\n"),
            ("a/mk.go", "package a\nfunc mk() { _, _ = &File{}, &Half{} }\n"),
            ("a/save.go", "package a\nfunc Save(w Writer) {\n  w.Write()\n  f := helper\n  f()\n}\nfunc helper() { Save(nil) }\n"),
            ("a/other.go", "package a\nfunc Other() { helper() }\n"),
        ];
        let mut jobs = Vec::new();
        for (f, src) in files {
            let p = d.join(f);
            std::fs::write(&p, src).unwrap();
            jobs.push((p.to_string_lossy().to_string(), "go"));
        }
        let whole = graph::build(&jobs);
        for name in ["Write", "Save", "helper", "Close", "mk"] {
            for depth in [1, 3] {
                let seeds = Seeds { names: vec![name.to_string()], up: depth, down: depth };
                let g = Scope::new(&jobs).run(&seeds);
                assert_eq!(reachable(&g, &[name], depth, depth), reachable(&whole, &[name], depth, depth), "{} {}", name, depth);
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
