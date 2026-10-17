//! One read and one parse per file per run, shared by every consumer.
//!
//! ctsq is single-threaded, so the caches are thread-local maps handing out
//! `Rc` handles. A handle keeps its tree alive for as long as a walk borrows
//! nodes from it, without a lifetime in every signature. Nothing is evicted:
//! entries live until the process exits.
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
use std::collections::HashMap;
use std::rc::Rc;
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

thread_local! {
    static TEXT: RefCell<HashMap<String, Source>> = RefCell::new(HashMap::new());
    static TREES: RefCell<HashMap<(String, &'static str), Rc<ParsedFile>>> = RefCell::new(HashMap::new());
}

/// The bytes of `path`, read from disk on the first call only. The error is
/// the read's own, so callers can print it as they did before the cache.
/// A failed read is not cached.
pub fn try_source(path: &str) -> std::io::Result<Source> {
    if let Some(s) = TEXT.with(|t| t.borrow().get(path).cloned()) {
        return Ok(s);
    }
    let bytes = std::fs::read(path)?;
    TEXT_READS.fetch_add(1, Ordering::Relaxed);
    let s: Source = Rc::new(bytes);
    TEXT.with(|t| t.borrow_mut().insert(path.to_string(), s.clone()));
    Ok(s)
}

/// The bytes of `path`, read once per run. None when it cannot be read.
pub fn source(path: &str) -> Option<Source> {
    try_source(path).ok()
}

/// `path` parsed with the grammar for `lang`, parsed on the first call only.
/// None when the file cannot be read. The entry's `tree` is None when `lang`
/// has no grammar or the parse failed.
pub fn get(path: &str, lang: &'static str) -> Option<Rc<ParsedFile>> {
    let key = (path.to_string(), lang);
    if let Some(p) = TREES.with(|t| t.borrow().get(&key).cloned()) {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Some(p);
    }
    let source = source(path)?;
    let tree = parse_text(lang, &source);
    PARSES.fetch_add(1, Ordering::Relaxed);
    let p = Rc::new(ParsedFile { lang, source, tree });
    TREES.with(|t| t.borrow_mut().insert(key, p.clone()));
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
