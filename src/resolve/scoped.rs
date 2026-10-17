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
use std::cell::RefCell;
use std::rc::Rc;

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

/// A scoped build that can take parses its caller already holds, so a file
/// the caller reads for its own reasons is not parsed a second time.
pub struct Prepared<'a>(Scope<'a>);

impl<'a> Prepared<'a> {
    pub fn new(jobs: &'a [(String, &'static str)]) -> Prepared<'a> {
        Prepared(Scope::new(jobs))
    }

    /// A scoped build over a word index the caller already built.
    pub fn with_words(jobs: &'a [(String, &'static str)], words: Rc<Words>) -> Prepared<'a> {
        Prepared(Scope::with_words(jobs, words))
    }

    /// Put every file holding one of `names` in the index, so definitions a
    /// caller reads by those names are all there, without walking from them.
    pub fn cover(&mut self, names: &[String]) {
        self.0.cover(names);
    }

    /// Read job `j` from a parse of it, if it is not read yet. A file read
    /// that no walk needs only adds definitions the index may hold anyway.
    pub fn offer(&mut self, j: usize, p: &crate::parsed::ParsedFile) {
        if matches!(self.0.files[j], State::Unread) {
            self.0.files[j] = State::Loaded(Loaded::read(&self.0.jobs[j].0, p));
        }
    }

    pub fn build(mut self, seeds: &Seeds) -> CallGraph {
        self.0.run(seeds)
    }

    /// `build`, keeping every file read so far for a later, wider build.
    pub fn build_keep(&mut self, seeds: &Seeds) -> CallGraph {
        self.0.run(seeds)
    }
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
    words: Rc<Words>,
    files: Vec<State>,
    /// Names whose holding files are all loaded.
    covered: HashSet<String>,
    job_of: HashMap<&'a str, usize>,
}

impl<'a> Scope<'a> {
    fn new(jobs: &'a [(String, &'static str)]) -> Scope<'a> {
        Scope::with_words(jobs, Rc::new(Words::new(jobs)))
    }

    fn with_words(jobs: &'a [(String, &'static str)], words: Rc<Words>) -> Scope<'a> {
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
        Scope { jobs, words, files, covered: HashSet::new(), job_of }
    }

    /// Read job `j`: its definitions and its calls, so it is parsed once
    /// whether it later turns out to be resolved or only indexed.
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

    fn run(&mut self, seeds: &Seeds) -> CallGraph {
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
                        // No walk reads the per-site table.
                        out.push((l.resolve(&index, Some(&want)).0, Vec::new()));
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
pub(crate) struct Words {
    sources: Vec<Option<crate::parsed::Source>>,
    langs: Vec<&'static str>,
    tokens: HashMap<Box<str>, Vec<u32>>,
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl Words {
    pub(crate) fn new(jobs: &[(String, &'static str)]) -> Words {
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
        Words { sources, tokens, langs: jobs.iter().map(|(_, l)| *l).collect() }
    }

    /// The text of job `j`, None when it could not be read.
    pub(crate) fn source(&self, j: usize) -> Option<&crate::parsed::Source> {
        self.sources.get(j).and_then(|s| s.as_ref())
    }

    /// The jobs of `holders(name)` that can give an index read of `name`
    /// an answer: a Go file only when `go_could_declare` says so, a file of
    /// any other language always.
    pub(crate) fn declarers(&self, name: &str) -> Vec<usize> {
        self.holders(name)
            .into_iter()
            .filter(|&j| self.langs[j] != "go" || self.sources[j].as_ref().is_some_and(|s| go_could_declare(s, name)))
            .collect()
    }

    /// The jobs of `holders(name)` that `go_could_bind` keeps, any file of
    /// another language kept.
    pub(crate) fn binders(&self, name: &str) -> Vec<usize> {
        self.holders(name)
            .into_iter()
            .filter(|&j| self.langs[j] != "go" || self.sources[j].as_ref().is_some_and(|s| go_could_bind(s, name)))
            .collect()
    }

    /// The jobs of `holders(name)` that can define `name` (`go_could_name`),
    /// any file of another language kept.
    pub(crate) fn namers(&self, name: &str) -> Vec<usize> {
        self.holders(name)
            .into_iter()
            .filter(|&j| self.langs[j] != "go" || self.sources[j].as_ref().is_some_and(|s| go_could_name(s, name)))
            .collect()
    }

    /// Jobs whose file holds `name`, in job order.
    pub(crate) fn holders(&self, name: &str) -> Vec<usize> {
        if !name.is_empty() && name.bytes().all(word_byte) {
            return self.tokens.get(name).map(|v| v.iter().map(|&j| j as usize).collect()).unwrap_or_default();
        }
        (0..self.sources.len())
            .filter(|&j| self.sources[j].as_ref().is_some_and(|s| holds_bounded(s, name)))
            .collect()
    }
}

/// Where on a line opening with `func` a name at byte `at` sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FuncPart {
    Receiver,
    Name,
    TypeParams,
    Params,
    /// The results, or anything after the parameters.
    Rest,
}

/// The index past the bracket closing the one at `open`, or None when the
/// line ends first.
fn close_of(line: &[u8], open: usize) -> Option<usize> {
    let (o, c) = (line[open], if line[open] == b'(' { b')' } else { b']' });
    let mut depth = 0usize;
    for (k, &b) in line.iter().enumerate().skip(open) {
        if b == o {
            depth += 1;
        } else if b == c {
            depth -= 1;
            if depth == 0 {
                return Some(k + 1);
            }
        }
    }
    None
}

fn skip_ws(line: &[u8], mut p: usize) -> usize {
    while p < line.len() && (line[p] == b' ' || line[p] == b'\t') {
        p += 1;
    }
    p
}

/// The part of `func [ (recv) ] name [ [T any] ] (params) results` holding
/// byte `at` of `line`, which opens (after indentation) with `func`.
fn func_part(line: &[u8], at: usize) -> FuncPart {
    let mut p = skip_ws(line, 0) + 4;
    p = skip_ws(line, p);
    if p < line.len() && line[p] == b'(' {
        match close_of(line, p) {
            Some(e) if at < e => return FuncPart::Receiver,
            Some(e) => p = skip_ws(line, e),
            None => return FuncPart::Receiver,
        }
    }
    let s = p;
    while p < line.len() && word_byte(line[p]) {
        p += 1;
    }
    if at >= s && at < p {
        return FuncPart::Name;
    }
    p = skip_ws(line, p);
    if p < line.len() && line[p] == b'[' {
        match close_of(line, p) {
            Some(e) if at < e => return FuncPart::TypeParams,
            Some(e) => p = skip_ws(line, e),
            None => return FuncPart::TypeParams,
        }
    }
    if p < line.len() && line[p] == b'(' {
        match close_of(line, p) {
            Some(e) if at < e => return FuncPart::Params,
            Some(_) => {}
            None => return FuncPart::Params,
        }
    }
    FuncPart::Rest
}

/// Whole-word occurrences of `name` in `src`, each as (line start, line
/// end, offset in the line).
fn word_lines<'s>(src: &'s [u8], name: &'s str) -> impl Iterator<Item = (usize, usize, usize)> + 's {
    let n = name.as_bytes();
    let mut from = 0;
    std::iter::from_fn(move || {
        while from + n.len() <= src.len() {
            let i = from + memfind(&src[from..], n)?;
            let end = i + n.len();
            from = i + 1;
            if (i > 0 && word_byte(src[i - 1])) || (end < src.len() && word_byte(src[end])) {
                continue;
            }
            let ls = src[..i].iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
            let le = src[end..].iter().position(|&b| b == b'\n').map(|p| end + p).unwrap_or(src.len());
            return Some((ls, le, i - ls));
        }
        None
    })
}

/// Whether Go source `src` can answer an index read of `name` (see
/// plans/perf/scoped-graph.md, section path). The Go definitions are
/// top-level functions, methods and interface methods, and the other reads
/// are interface `name`, a construction of `name` (`name{`, `pkg.name{`,
/// `name[T]{`, `new(name)`) and a function returning `name`. Each puts a
/// whole-word `name`:
/// - first on its line (an interface method, a grouped type spec, a result
///   on a line of its own),
/// - on a line opening with `type`, or with `)` (the results after a
///   parameter list that spans lines),
/// - on a line opening with `func`, outside the parameter list,
/// - right before `{` or `[`, not right after `]` (a slice or map literal
///   constructs no named type),
/// - right after `new(`, or `new(pkg.`.
///
/// A file with no such occurrence declares, constructs and returns nothing
/// called `name`, so no read of `name` is answered from it.
pub(crate) fn go_could_declare(src: &[u8], name: &str) -> bool {
    if name.is_empty() || !name.bytes().all(word_byte) {
        return true;
    }
    for (ls, le, at) in word_lines(src, name) {
        let line = &src[ls..le];
        let lead = line[..at].trim_ascii_start();
        if lead.is_empty() || lead.starts_with(b"type") || lead.starts_with(b")") {
            return true;
        }
        if lead.starts_with(b"func") && func_part(line, at) != FuncPart::Params {
            return true;
        }
        let next = line[at + name.len()..].trim_ascii_start();
        let prev_close = at > 0 && line[at - 1] == b']';
        if (next.starts_with(b"{") && !prev_close) || next.starts_with(b"[") {
            return true;
        }
        let before = &line[..at];
        let before = match before.strip_suffix(b".") {
            Some(b) => &b[..b.iter().rposition(|&c| !word_byte(c)).map(|p| p + 1).unwrap_or(0)],
            None => before,
        };
        if before.ends_with(b"new(") {
            return true;
        }
    }
    false
}

/// Whether Go source `src` can declare `name` the way `flow::Module` reads
/// it: a type, struct, `var` or `const`, or the name of a function, method
/// or interface method. Each puts `name` first on its line, on a line
/// opening with `type`, `var` or `const`, or as the name on a `func` line.
/// Whether Go source `src` can define something named `name`: a function
/// or method (the name on a `func` line) or an interface method (first on
/// its line).
pub(crate) fn go_could_name(src: &[u8], name: &str) -> bool {
    if name.is_empty() || !name.bytes().all(word_byte) {
        return true;
    }
    word_lines(src, name).any(|(ls, le, at)| {
        let line = &src[ls..le];
        let lead = line[..at].trim_ascii_start();
        lead.is_empty() || (lead.starts_with(b"func") && func_part(line, at) == FuncPart::Name)
    })
}

pub(crate) fn go_could_bind(src: &[u8], name: &str) -> bool {
    if name.is_empty() || !name.bytes().all(word_byte) {
        return true;
    }
    for (ls, le, at) in word_lines(src, name) {
        let line = &src[ls..le];
        let lead = line[..at].trim_ascii_start();
        if lead.is_empty() || lead.starts_with(b"type") || lead.starts_with(b"var") || lead.starts_with(b"const") {
            return true;
        }
        if lead.starts_with(b"func") && func_part(line, at) == FuncPart::Name {
            return true;
        }
    }
    false
}

fn memfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        match hay[i..hay.len() - needle.len() + 1].iter().position(|&b| b == first) {
            None => return None,
            Some(p) => {
                i += p;
                if &hay[i..i + needle.len()] == needle {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

/// Whether `hay` holds `name` with no identifier byte touching an end of it
/// that is itself an identifier byte.
pub(crate) fn holds_bounded(hay: &[u8], name: &str) -> bool {
    holds_any(hay, &[name])
}

/// `holds_bounded` for any of `names`, validating the text once.
pub(crate) fn holds_any(hay: &[u8], names: &[&str]) -> bool {
    let text = std::str::from_utf8(hay).ok();
    names.iter().any(|name| {
        let n = name.as_bytes();
        let Some(first) = name.chars().next() else { return true };
        let (check_before, check_after) = (word_byte(n[0]), word_byte(n[n.len() - 1]));
        let mut from = 0;
        while from + n.len() <= hay.len() {
            let found = match text {
                // The standard library's substring search, far faster than
                // comparing a window at every offset.
                Some(s) => s[from..].find(*name),
                None => hay[from..].windows(n.len()).position(|w| w == n),
            };
            let Some(k) = found else { return false };
            let i = from + k;
            let end = i + n.len();
            let before = !check_before || i == 0 || !word_byte(hay[i - 1]);
            let after = !check_after || end == hay.len() || !word_byte(hay[end]);
            if before && after {
                return true;
            }
            // Past the first character, so the next search starts on a
            // character boundary.
            from = i + first.len_utf8();
        }
        false
    })
}

/// One call site as the flow engine reads it: `CallSite` with its file, and
/// its caller and target as `FlowGraph` definition keys.
#[derive(Debug, Clone)]
pub struct Site {
    pub file: String,
    pub target: Option<usize>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub args: Vec<(usize, usize)>,
}

/// The call graph as the flow engine (`path` with `=>`, `==>`, `<=`, `<==`)
/// reads it: the sites of a call, the callers of a definition, the
/// definitions of a name and the definition at a line. Definitions are keys
/// handed out by the graph. A lazy graph answers each question from the
/// files that can change the answer, loading and resolving them on first
/// use, so the answer equals the whole-folder graph's:
/// - the sites of a call: its file, resolved for the function holding it,
///   against an index grown until no name it asks is uncovered;
/// - the callers of a definition named N: every file holding N, resolved for
///   the calls that can reach N;
/// - the definitions named N, or interface N: the files holding N.
///
/// Keys of a lazy graph are handed out in the order definitions are first
/// met, so every answer that lists definitions orders them as the whole
/// index does (job order, then file order) rather than by key.
pub struct FlowGraph {
    kind: FlowKind,
}

enum FlowKind {
    Whole(WholeFlow),
    Lazy(RefCell<LazyFlow>),
}

struct WholeFlow {
    defs: Vec<Rc<super::Def>>,
    by_name: HashMap<String, Vec<usize>>,
    sites: Vec<Rc<Site>>,
    ranges: HashMap<(String, usize, usize), Vec<usize>>,
    callers: HashMap<usize, Vec<usize>>,
}

/// What a lazy graph has resolved in one file: its sites by emission
/// position (function, call, target), and which functions and callee names
/// it was resolved for.
#[derive(Default)]
struct FileSites {
    sites: std::collections::BTreeMap<(usize, usize, usize), Rc<Site>>,
    lines: HashSet<usize>,
    names: HashSet<String>,
}

struct LazyFlow {
    jobs: Vec<(String, &'static str)>,
    words: Rc<Words>,
    job_of: HashMap<String, usize>,
    files: Vec<State>,
    /// Names whose every file that can answer any read of them is loaded.
    covered: HashSet<String>,
    /// Names whose every file that can define them is loaded.
    named: HashSet<String>,
    /// The index over the loaded files with each entry's (job, position in
    /// its file), rebuilt when a file is loaded.
    index: Option<(super::DefIndex, Vec<(usize, usize)>)>,
    defs: Vec<Rc<super::Def>>,
    key_of: HashMap<(usize, usize), usize>,
    /// Each key's (job, position), for ordering.
    place: Vec<(usize, usize)>,
    resolved: HashMap<usize, FileSites>,
}

impl FlowGraph {
    pub fn whole(g: &CallGraph) -> FlowGraph {
        let defs: Vec<Rc<super::Def>> = g.index.defs.iter().map(|d| Rc::new(d.clone())).collect();
        let mut sites = Vec::new();
        let mut ranges: HashMap<(String, usize, usize), Vec<usize>> = HashMap::new();
        let mut callers: HashMap<usize, Vec<usize>> = HashMap::new();
        for s in &g.sites {
            let Some(d) = g.index.defs.get(s.caller) else { continue };
            let i = sites.len();
            ranges.entry((d.id.file.clone(), s.start_byte, s.end_byte)).or_default().push(i);
            if let Some((t, _)) = s.target {
                callers.entry(t).or_default().push(i);
            }
            sites.push(Rc::new(Site {
                file: d.id.file.clone(),
                target: s.target.map(|(t, _)| t),
                start_byte: s.start_byte,
                end_byte: s.end_byte,
                args: s.args.clone(),
            }));
        }
        FlowGraph { kind: FlowKind::Whole(WholeFlow { defs, by_name: g.index.by_name.clone(), sites, ranges, callers }) }
    }

    pub fn lazy(jobs: &[(String, &'static str)], words: Rc<Words>) -> FlowGraph {
        let mut job_of: HashMap<String, usize> = HashMap::new();
        let mut files = Vec::with_capacity(jobs.len());
        for (i, (f, _)) in jobs.iter().enumerate() {
            if job_of.contains_key(f.as_str()) {
                files.push(State::Skipped);
            } else {
                job_of.insert(f.clone(), i);
                files.push(State::Unread);
            }
        }
        let jobs = jobs.to_vec();
        FlowGraph {
            kind: FlowKind::Lazy(RefCell::new(LazyFlow {
                jobs,
                words,
                job_of,
                files,
                covered: HashSet::new(),
                named: HashSet::new(),
                index: None,
                defs: Vec::new(),
                key_of: HashMap::new(),
                place: Vec::new(),
                resolved: HashMap::new(),
            })),
        }
    }

    /// The definition behind key `i`.
    pub fn def(&self, i: usize) -> Rc<super::Def> {
        match &self.kind {
            FlowKind::Whole(w) => w.defs[i].clone(),
            FlowKind::Lazy(l) => l.borrow().defs[i].clone(),
        }
    }

    /// The targets of the sites at (start, end) in `file`, in site order,
    /// unresolved ones left out. None when no site is there.
    pub fn site_targets(&self, file: &str, start: usize, end: usize) -> Option<Vec<usize>> {
        match &self.kind {
            FlowKind::Whole(w) => {
                let ids = w.ranges.get(&(file.to_string(), start, end))?;
                Some(ids.iter().filter_map(|&i| w.sites[i].target).collect())
            }
            FlowKind::Lazy(l) => l.borrow_mut().site_targets(file, start, end),
        }
    }

    /// Sites calling `def`, in file then document order.
    pub fn callers_of(&self, def: usize) -> Vec<Rc<Site>> {
        match &self.kind {
            FlowKind::Whole(w) => {
                let mut v = w.callers.get(&def).cloned().unwrap_or_default();
                v.sort_by_key(|&i| (w.sites[i].file.clone(), w.sites[i].start_byte));
                v.dedup();
                v.into_iter().map(|i| w.sites[i].clone()).collect()
            }
            FlowKind::Lazy(l) => l.borrow_mut().callers_of(def),
        }
    }

    /// The first definition, in index order, declared in `file` at `line`
    /// that `keep` accepts.
    pub fn def_at(&self, file: &str, line: usize, keep: impl Fn(&super::Def) -> bool) -> Option<usize> {
        let fits = |d: &super::Def| d.id.file == file && d.id.line == line && keep(d);
        match &self.kind {
            FlowKind::Whole(w) => w.defs.iter().position(|d| fits(d)),
            FlowKind::Lazy(l) => {
                let mut l = l.borrow_mut();
                let &j = l.job_of.get(file)?;
                l.load(j);
                let k = match &l.files[j] {
                    State::Loaded(ld) => ld.defs.iter().position(fits)?,
                    _ => return None,
                };
                Some(l.key(j, k))
            }
        }
    }

    /// The definitions named `name`, in index order.
    pub fn by_name(&self, name: &str) -> Vec<usize> {
        match &self.kind {
            FlowKind::Whole(w) => w.by_name.get(name).cloned().unwrap_or_default(),
            FlowKind::Lazy(l) => l.borrow_mut().by_name(name),
        }
    }

    /// Whether some interface named `name` declares a method.
    pub fn is_interface(&self, name: &str) -> bool {
        let is = |d: &super::Def| matches!(&d.kind, super::DefKind::InterfaceMethod { interface } if interface == name);
        match &self.kind {
            FlowKind::Whole(w) => w.defs.iter().any(|d| is(d)),
            FlowKind::Lazy(l) => {
                let mut l = l.borrow_mut();
                l.cover(&[name.to_string()]);
                l.words.holders(name).into_iter().any(|j| match &l.files[j] {
                    State::Loaded(ld) => ld.defs.iter().any(is),
                    _ => false,
                })
            }
        }
    }
}

impl LazyFlow {
    fn load(&mut self, j: usize) {
        if !matches!(self.files[j], State::Unread) {
            return;
        }
        let (file, lang) = &self.jobs[j];
        self.files[j] = match crate::parsed::get(file, lang) {
            Some(p) => State::Loaded(Loaded::read(file, &p)),
            None => State::Skipped,
        };
        self.index = None;
    }

    fn cover(&mut self, names: &[String]) {
        for n in names {
            if self.covered.insert(n.clone()) {
                for j in self.words.declarers(n) {
                    self.load(j);
                }
            }
        }
    }

    /// Load every file that can define one of `names`.
    fn cover_named(&mut self, names: &[String]) {
        for n in names {
            if !self.covered.contains(n) && self.named.insert(n.clone()) {
                for j in self.words.namers(n) {
                    self.load(j);
                }
            }
        }
    }

    fn key(&mut self, j: usize, k: usize) -> usize {
        if let Some(&i) = self.key_of.get(&(j, k)) {
            return i;
        }
        let State::Loaded(l) = &self.files[j] else { unreachable!("a key is only made for a loaded file") };
        let i = self.defs.len();
        self.defs.push(Rc::new(l.defs[k].clone()));
        self.key_of.insert((j, k), i);
        self.place.push((j, k));
        i
    }

    fn by_name(&mut self, name: &str) -> Vec<usize> {
        self.cover_named(&[name.to_string()]);
        let mut out = Vec::new();
        for j in self.words.holders(name) {
            let ks: Vec<usize> = match &self.files[j] {
                State::Loaded(l) => (0..l.defs.len()).filter(|&k| l.defs[k].name == name).collect(),
                _ => continue,
            };
            for k in ks {
                out.push(self.key(j, k));
            }
        }
        out
    }

    fn ensure_index(&mut self) {
        if self.index.is_some() {
            return;
        }
        let mut owner = Vec::new();
        let mut loaded = Vec::new();
        for (j, s) in self.files.iter().enumerate() {
            if let State::Loaded(l) = s {
                owner.extend((0..l.defs.len()).map(|k| (j, k)));
                loaded.push(l);
            }
        }
        self.index = Some((graph::index_of(&loaded), owner));
    }

    /// Resolve job `j` for the functions declared at `lines` and the calls
    /// that can reach a definition named in `names`, adding the sites.
    fn resolve(&mut self, j: usize, lines: &HashSet<usize>, names: &HashSet<String>) {
        self.load(j);
        let fs = self.resolved.entry(j).or_default();
        let lines: HashSet<usize> = lines.difference(&fs.lines).copied().collect();
        let names: HashSet<String> = names.difference(&fs.names).cloned().collect();
        if lines.is_empty() && names.is_empty() {
            return;
        }
        fs.lines.extend(lines.iter().copied());
        fs.names.extend(names.iter().cloned());
        let sites = loop {
            self.ensure_index();
            let (index, _) = self.index.as_ref().expect("built");
            let State::Loaded(l) = &self.files[j] else { return };
            index.record();
            let want = Want { names: &names, lines: Some(&lines) };
            let (_, sites) = l.resolve(index, Some(&want));
            let (named, other) = index.take_asked_split();
            let mut new_named: Vec<String> =
                named.into_iter().filter(|n| !self.covered.contains(n) && !self.named.contains(n)).collect();
            let mut new: Vec<String> = other.into_iter().filter(|n| !self.covered.contains(n)).collect();
            if new.is_empty() && new_named.is_empty() {
                break sites;
            }
            new.sort();
            new_named.sort();
            self.cover(&new);
            self.cover_named(&new_named);
        };
        let positions = match &self.files[j] {
            State::Loaded(Loaded { model: super::FileModel::Go(scope), .. }) => scope.call_positions().1,
            _ => HashMap::new(),
        };
        let file = self.jobs[j].0.clone();
        let mut last: Option<((usize, usize), usize)> = None;
        let mut made = Vec::new();
        for s in sites {
            let Some(&(f, c)) = positions.get(&(s.start_byte, s.end_byte)) else { continue };
            // Interface dispatch gives one site per target, consecutively.
            let k = match last {
                Some((p, k)) if p == (f, c) => k + 1,
                _ => 0,
            };
            last = Some(((f, c), k));
            made.push(((f, c, k), s));
        }
        for (pos, s) in made {
            if self.resolved[&j].sites.contains_key(&pos) {
                continue;
            }
            let target = s.target.map(|(t, _)| {
                let (tj, tk) = self.index.as_ref().expect("built").1[t];
                self.key(tj, tk)
            });
            let site = Site { file: file.clone(), target, start_byte: s.start_byte, end_byte: s.end_byte, args: s.args };
            self.resolved.get_mut(&j).expect("entry").sites.insert(pos, Rc::new(site));
        }
    }

    fn site_targets(&mut self, file: &str, start: usize, end: usize) -> Option<Vec<usize>> {
        let &j = self.job_of.get(file)?;
        self.load(j);
        let (lines, at) = match &self.files[j] {
            State::Loaded(Loaded { model: super::FileModel::Go(scope), .. }) => scope.call_positions(),
            _ => return None,
        };
        let &(f, c) = at.get(&(start, end))?;
        self.resolve(j, &HashSet::from([lines[f]]), &HashSet::new());
        let fs = &self.resolved[&j];
        let here: Vec<&Rc<Site>> = fs.sites.range((f, c, 0)..=(f, c, usize::MAX)).map(|(_, s)| s).collect();
        if here.is_empty() {
            return None;
        }
        Some(here.iter().filter_map(|s| s.target).collect())
    }

    fn callers_of(&mut self, def: usize) -> Vec<Rc<Site>> {
        let name = self.defs[def].name.clone();
        self.cover(&[name.clone()]);
        let names = HashSet::from([name.clone()]);
        let mut out = Vec::new();
        for j in self.words.holders(&name) {
            // A call to `name` is in any file holding it, declaring or not.
            self.load(j);
            if !matches!(self.files[j], State::Loaded(_)) {
                continue;
            }
            self.resolve(j, &HashSet::new(), &names);
            if let Some(fs) = self.resolved.get(&j) {
                out.extend(fs.sites.values().filter(|s| s.target == Some(def)).cloned());
            }
        }
        out.sort_by(|a, b| (&a.file, a.start_byte).cmp(&(&b.file, b.start_byte)));
        out
    }
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
