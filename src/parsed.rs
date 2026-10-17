//! One read and one parse per file per run, shared by every consumer.
//!
//! ctsq is single-threaded, so the caches are thread-local maps handing out
//! `Rc` handles. A handle keeps its tree alive for as long as a walk borrows
//! nodes from it, without a lifetime in every signature.
//!
//! The maps hold weak references, so an entry lives as long as anyone holds
//! its handle (a `TreeStore`, a loop over the file), and a `get` while it is
//! alive is a hit. On top of that the most recently used entries are kept
//! alive by the cache itself, up to `KEEP_BYTES` of estimated memory, so a
//! file revisited soon after is not parsed again. Keeping every tree instead
//! cost 10 to 16 times the reference binary's peak memory on Mavexa (`-q`,
//! `tree`), since a tree takes about 24 times its source in memory.
//!
//! Files are keyed by the path string exactly as the job gives it. A tree is
//! keyed by that path plus the lang name, which is already refined per file
//! (ts vs tsx), so one path parsed with two grammars is two entries over one
//! read of its text. Text with no path (stdin, `example` snippets, template
//! bodies, inline test sources) goes through `parse_text`, uncached.
//!
//! With `CTSQ_PARSE_STATS=1` the process prints
//! `parsed N files, M cache hits, K text reads` to stderr at exit.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A file's bytes, shared between the text cache and every parse of it.
/// `Rc<Vec<u8>>` rather than `Rc<[u8]>` so `TreeStore` consumers that call
/// `.source.as_slice()` keep compiling; `&pf.source` coerces to `&[u8]`.
pub type Source = Rc<Vec<u8>>;

/// One file parsed with one grammar.
pub struct ParsedFile {
    pub lang: &'static str,
    pub source: Source,
    /// None when the language has no grammar here or the parse failed.
    pub tree: Option<tree_sitter::Tree>,
}

/// Counters for `CTSQ_PARSE_STATS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Files parsed through `get` (cache misses that parsed).
    pub parses: usize,
    /// `get` calls answered from the cache.
    pub hits: usize,
    /// Files read from disk.
    pub text_reads: usize,
}

// Atomics rather than thread-locals so the exit hook can read them after
// thread-local storage may already be gone.
static PARSES: AtomicUsize = AtomicUsize::new(0);
static HITS: AtomicUsize = AtomicUsize::new(0);
static TEXT_READS: AtomicUsize = AtomicUsize::new(0);

/// Estimated memory the cache itself keeps alive, in bytes: text counts its
/// length, a parsed file `TREE_WEIGHT` times its source length.
const KEEP_BYTES: usize = 4 << 20;
/// A parsed file's memory over its source length, tree and text together
/// (measured on Mavexa: 11.5MB of source, about 270MB of trees).
const TREE_WEIGHT: usize = 25;

/// A key in the recently-used list: a path's text (lang None) or its parse.
type Key = (String, Option<&'static str>);

/// A strong handle the cache holds only to keep an entry alive, never reads.
#[allow(dead_code)]
enum Kept {
    Text(Source),
    Tree(Rc<ParsedFile>),
}

#[derive(Default)]
struct Cache {
    text: HashMap<String, Weak<Vec<u8>>>,
    trees: HashMap<(String, &'static str), Weak<ParsedFile>>,
    /// Most recently used last, each with its weight.
    recent: VecDeque<(Key, Kept, usize)>,
    kept: usize,
}

impl Cache {
    /// Move `key` to the most recent end, if it is in the list.
    fn touch(&mut self, key: &Key) {
        if let Some(i) = self.recent.iter().position(|(k, _, _)| k == key) {
            if i + 1 != self.recent.len() {
                let e = self.recent.remove(i).expect("index in range");
                self.recent.push_back(e);
            }
        }
    }

    fn keep(&mut self, key: Key, value: Kept, weight: usize) {
        if let Some(i) = self.recent.iter().position(|(k, _, _)| *k == key) {
            let (_, _, w) = self.recent.remove(i).expect("index in range");
            self.kept -= w;
        }
        self.recent.push_back((key, value, weight));
        self.kept += weight;
        // The newest entry stays even when it alone is over the budget.
        while self.kept > KEEP_BYTES && self.recent.len() > 1 {
            let (_, _, w) = self.recent.pop_front().expect("not empty");
            self.kept -= w;
        }
    }

    /// Drop a path's text-only entry, once a parse of it holds the text.
    fn forget_text(&mut self, path: &str) {
        if let Some(i) = self.recent.iter().position(|((p, l), _, _)| l.is_none() && p == path) {
            let (_, _, w) = self.recent.remove(i).expect("index in range");
            self.kept -= w;
        }
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// The bytes of `path`, read from disk only when no live handle holds them.
/// The error is the read's own, so callers can print it as they did before
/// the cache. A failed read is not cached.
pub fn try_source(path: &str) -> std::io::Result<Source> {
    let live = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let s = c.text.get(path).and_then(Weak::upgrade)?;
        c.touch(&(path.to_string(), None));
        Some(s)
    });
    if let Some(s) = live {
        return Ok(s);
    }
    let bytes = std::fs::read(path)?;
    TEXT_READS.fetch_add(1, Ordering::Relaxed);
    let s: Source = Rc::new(bytes);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.text.insert(path.to_string(), Rc::downgrade(&s));
        c.keep((path.to_string(), None), Kept::Text(s.clone()), s.len());
    });
    Ok(s)
}

/// The bytes of `path`. None when it cannot be read.
pub fn source(path: &str) -> Option<Source> {
    try_source(path).ok()
}

/// `path` parsed with the grammar for `lang`, parsed only when no live handle
/// holds that parse. None when the file cannot be read. The entry's `tree` is
/// None when `lang` has no grammar or the parse failed.
pub fn get(path: &str, lang: &'static str) -> Option<Rc<ParsedFile>> {
    let live = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let p = c.trees.get(&(path.to_string(), lang)).and_then(Weak::upgrade)?;
        c.touch(&(path.to_string(), Some(lang)));
        Some(p)
    });
    if let Some(p) = live {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Some(p);
    }
    let source = source(path)?;
    let tree = parse_text(lang, &source);
    PARSES.fetch_add(1, Ordering::Relaxed);    let weight = source.len().saturating_mul(TREE_WEIGHT);
    let p = Rc::new(ParsedFile { lang, source, tree });
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.trees.insert((path.to_string(), lang), Rc::downgrade(&p));
        c.forget_text(path);
        c.keep((path.to_string(), Some(lang)), Kept::Tree(p.clone()), weight);
    });
    Some(p)
}

/// Parse text that has no path, uncached and not counted.
pub fn parse_text(lang: &str, bytes: &[u8]) -> Option<tree_sitter::Tree> {
    let l = crate::compiler::lang::for_name(lang)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&l.ts_language()).ok()?;
    parser.parse(bytes, None)
}

pub fn stats() -> Stats {
    Stats {
        parses: PARSES.load(Ordering::Relaxed),
        hits: HITS.load(Ordering::Relaxed),
        text_reads: TEXT_READS.load(Ordering::Relaxed),
    }
}

/// When `CTSQ_PARSE_STATS` is set, print the counters to stderr at exit,
/// including exits through `std::process::exit`.
pub fn report_stats_at_exit() {
    if std::env::var_os("CTSQ_PARSE_STATS").is_none_or(|v| v.is_empty() || v == "0") {
        return;
    }
    #[cfg(unix)]
    {
        extern "C" fn print_stats() {
            let s = stats();
            eprintln!("parsed {} files, {} cache hits, {} text reads", s.parses, s.hits, s.text_reads);
        }
        unsafe {
            libc::atexit(print_stats);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-parsed-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // Tests run on their own threads, so the thread-local caches start empty
    // per test. The counters are process-wide, so assertions use deltas, and
    // only ones that hold whatever other tests do at the same time: each test
    // counts its own work as at least the delta it expects.

    #[test]
    fn a_second_get_is_a_hit_on_the_same_tree() {
        let d = scratch("hit");
        let f = d.join("a.rs");
        std::fs::write(&f, "fn a() {}\n").unwrap();
        let p = f.to_str().unwrap();
        let before = stats();
        let a = get(p, "rust").unwrap();
        let b = get(p, "rust").unwrap();
        assert!(Rc::ptr_eq(&a, &b), "one parse, shared");
        assert!(a.tree.is_some());
        let after = stats();
        assert!(after.parses >= before.parses + 1);
        assert!(after.hits >= before.hits + 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn two_grammars_for_one_path_are_two_entries_over_one_read() {
        let d = scratch("grammars");
        let f = d.join("a.ts");
        std::fs::write(&f, "const a = 1;\n").unwrap();
        let p = f.to_str().unwrap();
        let ts = get(p, "typescript").unwrap();
        let tsx = get(p, "tsx").unwrap();
        assert!(!Rc::ptr_eq(&ts, &tsx), "a parse per grammar");
        assert_eq!(ts.lang, "typescript");
        assert_eq!(tsx.lang, "tsx");
        assert!(Rc::ptr_eq(&ts.source, &tsx.source), "the text is read once and shared");
        assert!(Rc::ptr_eq(&ts.source, &source(p).unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn text_is_read_from_disk_once() {
        let d = scratch("once");
        let f = d.join("a.go");
        std::fs::write(&f, "package a\n").unwrap();
        let p = f.to_str().unwrap();
        let first = source(p).unwrap();
        // A change on disk is not seen: the run keeps the bytes it read first.
        std::fs::write(&f, "package b\n").unwrap();
        let second = source(p).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(second.as_slice(), b"package a\n");
        drop((first, second));
        // Nobody holds it, but it is recent, so the cache still does.
        assert_eq!(source(p).unwrap().as_slice(), b"package a\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recent_entries_stay_alive_within_the_budget_and_old_ones_go() {
        let d = scratch("budget");
        // Each file weighs a third of the budget once parsed.
        let len = KEEP_BYTES / TREE_WEIGHT / 3;
        let body = format!("// {}\n", "x".repeat(len));
        let paths: Vec<String> = (0..5)
            .map(|i| {
                let f = d.join(format!("f{}.rs", i));
                std::fs::write(&f, &body).unwrap();
                f.to_str().unwrap().to_string()
            })
            .collect();
        let weak: Vec<Weak<ParsedFile>> = paths.iter().map(|p| Rc::downgrade(&get(p, "rust").unwrap())).collect();
        assert!(weak[0].upgrade().is_none(), "the oldest is dropped once over budget");
        assert!(weak[4].upgrade().is_some() && weak[3].upgrade().is_some(), "the newest are kept");
        // A held handle keeps an entry a hit, whatever the budget.
        let held = get(&paths[1], "rust").unwrap();
        for p in &paths[2..] {
            get(p, "rust").unwrap();
        }
        assert!(Rc::ptr_eq(&held, &get(&paths[1], "rust").unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stats_count_parses_hits_and_reads() {
        let d = scratch("stats");
        let f = d.join("a.py");
        std::fs::write(&f, "x = 1\n").unwrap();
        let p = f.to_str().unwrap();
        let before = stats();
        get(p, "python").unwrap();
        get(p, "python").unwrap();
        get(p, "python").unwrap();
        let after = stats();
        assert!(after.parses > before.parses);
        assert!(after.hits >= before.hits + 2);
        assert!(after.text_reads > before.text_reads);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_file_is_none_and_not_cached() {
        let d = scratch("missing");
        let f = d.join("gone.rs");
        let p = f.to_str().unwrap();
        assert!(get(p, "rust").is_none());
        assert_eq!(try_source(p).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        std::fs::write(&f, "fn a() {}\n").unwrap();
        assert!(get(p, "rust").is_some(), "a failed read is retried");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_text_parses_without_a_path_and_an_unknown_lang_is_none() {
        assert!(parse_text("go", b"package a\n").is_some());
        assert!(parse_text("cobol", b"x").is_none());
    }
}
