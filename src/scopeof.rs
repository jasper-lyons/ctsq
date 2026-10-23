//! `ctsq scopeof path:line [path:line ...]`: from a location to the definition
//! that holds it. It is the other direction from `def`, which goes from a name
//! to a location.
//!
//! Each coordinate prints a heading `path:LINE  in  path:START-END  signature`
//! and then the scope's lines as they sit in the file. A line outside every
//! named scope prints `path:LINE  at top level` and a window of the file.

use crate::compiler::lang;
use crate::files;
use crate::miss;
use crate::scope::{self, Scope};

/// One `path:line` or `path:line:col` argument.
#[derive(Debug, Clone, PartialEq)]
pub struct Coord {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 1-based byte column, when the argument carried one.
    pub col: Option<usize>,
}

/// Parse one argument. The path is everything before the last one or two
/// numeric fields.
pub fn parse_coord(arg: &str) -> Result<Coord, String> {
    let bad = || format!("not a coordinate: {}, expected path:LINE or path:LINE:COL", arg);
    let num = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
    if let Some((rest, col)) = arg.rsplit_once(':') {
        if let Some((path, line)) = rest.rsplit_once(':') {
            if let (Some(l), Some(c)) = (num(line), num(col)) {
                if !path.is_empty() {
                    return Ok(Coord { path: path.to_string(), line: l, col: Some(c) });
                }
            }
        }
        if let Some(l) = num(col) {
            if !rest.is_empty() {
                return Ok(Coord { path: rest.to_string(), line: l, col: None });
            }
        }
    }
    Err(bad())
}

/// What the flags asked for.
#[derive(Debug, Clone)]
pub struct Options {
    pub up: usize,
    pub siblings: bool,
    pub callers: bool,
    pub block: bool,
    pub budget: usize,
    pub lang: Option<String>,
    pub no_ignore: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { up: 0, siblings: false, callers: false, block: false, budget: 150, lang: None, no_ignore: false }
    }
}

/// What a run printed, and the exit code: 0 when any coordinate resolved.
#[derive(Debug, Default)]
pub struct Report {
    pub out: String,
    pub err: String,
    pub code: i32,
}

/// One coordinate's answer.
#[derive(Debug, Clone)]
pub struct Hit {
    pub coord: Coord,
    /// The scope holding the line, or the innermost block with `--block`.
    /// None at the top level, where the window is cut from the whole file.
    pub scope: Option<Scope>,
    /// The first and last line shown, 1-based and inclusive.
    pub shown: (usize, usize),
    /// The lines the window was cut from: the scope's, or the whole file's.
    pub span: (usize, usize),
    /// The shown lines, as they sit in the file.
    pub lines: Vec<String>,
    /// With `--siblings`, the named scopes beside the scope.
    pub siblings: Option<Vec<Scope>>,
    /// With `--callers`, one level of callers of the scope.
    pub callers: Option<crate::calltree::CallTrees>,
    /// `--up` asked past the outermost scope: how many enclosing scopes there are.
    pub only: Option<usize>,
}

/// Each coordinate's answer, in order. An error is the stderr line for a
/// coordinate that did not resolve.
pub fn build(coords: &[Coord], opts: &Options, hide: &files::HideOpts) -> Vec<Result<Hit, String>> {
    coords.iter().map(|c| one(c, opts, hide)).collect()
}

/// The text `scopeof` prints for `hits`: blocks separated by a blank line,
/// the errors on stderr, code 0 when any coordinate resolved.
pub fn render(hits: &[Result<Hit, String>]) -> Report {
    let mut r = Report::default();
    let mut resolved = 0usize;
    for h in hits {
        match h {
            Ok(h) => {
                if resolved > 0 {
                    r.out.push('\n');
                }
                resolved += 1;
                r.out.push_str(&render_hit(h));
            }
            Err(e) => {
                r.err.push_str(e);
                r.err.push('\n');
            }
        }
    }
    r.code = if resolved > 0 { 0 } else { 1 };
    r
}

/// All coordinates, blocks separated by a blank line: `build` then `render`.
pub fn report(coords: &[Coord], opts: &Options, hide: &files::HideOpts) -> Report {
    render(&build(coords, opts, hide))
}

/// The `A-B` lines of `lo..=hi` to show, `budget` of them centred on `center`.
fn window(center: usize, lo: usize, hi: usize, budget: usize) -> (usize, usize) {
    if hi + 1 - lo <= budget {
        return (lo, hi);
    }
    let mut a = center.saturating_sub(budget / 2).max(lo);
    let mut b = a + budget - 1;
    if b > hi {
        b = hi;
        a = b + 1 - budget;
    }
    (a, b)
}

/// The footer for a window that cut the body: the shown lines, the span they
/// were cut from, and the flag value that shows all of it.
fn showing(a: usize, b: usize, lo: usize, hi: usize) -> String {
    format!("showing lines {}-{} of {}-{}, --budget {} for all\n", a, b, lo, hi, hi + 1 - lo)
}

fn render_hit(h: &Hit) -> String {
    let c = &h.coord;
    let mut out = String::new();
    match &h.scope {
        None => out.push_str(&format!("{}:{}  at top level\n", c.path, c.line)),
        Some(s) => out.push_str(&format!("{}:{}  in  {}:{}-{}  {}\n", c.path, c.line, s.path, s.start_line, s.end_line, s.signature)),
    }
    for l in &h.lines {
        out.push_str(l);
        out.push('\n');
    }
    if let Some(sibs) = &h.siblings {
        if sibs.is_empty() {
            out.push_str("siblings: none\n");
        } else {
            out.push_str("siblings:\n");
            for x in sibs {
                out.push_str(&format!("{}:{}: {}\n", x.path, x.start_line, x.signature));
            }
        }
    }
    if let (Some(t), Some(s)) = (&h.callers, &h.scope) {
        let (text, _) = crate::calltree::render_one(t, &[], None);
        let header = format!("{}()\n", s.name);
        let rest = text.strip_prefix(&header).unwrap_or(&text);
        if rest.is_empty() {
            out.push_str("callers: none\n");
        } else {
            out.push_str(&format!("callers:\n{}", rest));
        }
    }
    let (a, b) = h.shown;
    if h.shown != h.span {
        out.push_str(&showing(a, b, h.span.0, h.span.1));
    }
    if let Some(n) = h.only {
        out.push_str(&format!("{}:{}: only {} enclosing {}\n", c.path, c.line, n, if n == 1 { "scope" } else { "scopes" }));
    }
    out
}

fn one(c: &Coord, o: &Options, hide: &files::HideOpts) -> Result<Hit, String> {
    let source = match std::fs::read(&c.path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(miss::no_such_file(&c.path)),
        Err(e) => return Err(format!("Failed to read {}: {}", c.path, e)),
    };
    let lang_name = match &o.lang {
        Some(l) => lang::lang_for_name_canonical(l).map(|n| lang::refine_for_path(n, &c.path)),
        None => lang::lang_for_path(&c.path),
    };
    let Some(lang_name) = lang_name else {
        return Err(format!("Cannot detect language for {}, use --lang", c.path));
    };
    let text = String::from_utf8_lossy(&source);
    let lines: Vec<&str> = text.lines().collect();
    if c.line > lines.len() {
        return Err(format!("{}:{}: past the end of the file ({} lines)", c.path, c.line, lines.len()));
    }
    let Some(tree) = scope::parse(lang_name, &source) else {
        return Err(format!("Failed to parse {}", c.path));
    };
    let cut = |a: usize, b: usize| lines[a - 1..b].iter().map(|l| l.to_string()).collect::<Vec<String>>();

    // A signature line starts before the scope's own node (`export`, `const x =`,
    // `type`), so the last character of the line is tried as well and the deeper
    // answer wins.
    let bytes: Vec<usize> = match c.col {
        Some(col) => vec![scope::byte_at(&source, c.line, col)],
        None => {
            let (first, last) = scope::line_bytes(&source, c.line);
            vec![first, last]
        }
    };
    let mut chain: Vec<Scope> = Vec::new();
    for &b in &bytes {
        let cand = scope::enclosing(&c.path, lang_name, &source, &tree, b);
        if cand.first().map(|s| s.start_byte) > chain.first().map(|s| s.start_byte) {
            chain = cand;
        }
    }

    let mut only = None;
    let selected: Option<Scope> = if o.block {
        let mut best: Option<Scope> = None;
        for &b in &bytes {
            if let Some(s) = scope::innermost_block(&c.path, &source, &tree, b) {
                if best.as_ref().map(|x| x.start_byte) < Some(s.start_byte) {
                    best = Some(s);
                }
            }
        }
        best.map(|mut s| {
            s.signature = lines[s.start_line - 1].trim().to_string();
            s
        })
    } else if chain.is_empty() {
        None
    } else {
        if o.up >= chain.len() && o.up > 0 {
            only = Some(chain.len());
        }
        Some(chain[o.up.min(chain.len() - 1)].clone())
    };

    let Some(s) = selected else {
        // Top level: a window of the file around the line.
        let (a, b) = window(c.line, 1, lines.len(), o.budget);
        return Ok(Hit {
            coord: c.clone(),
            scope: None,
            shown: (a, b),
            span: (1, lines.len()),
            lines: cut(a, b),
            siblings: None,
            callers: None,
            only: None,
        });
    };

    let (a, b) = window(c.line.clamp(s.start_line, s.end_line), s.start_line, s.end_line, o.budget);
    let named = s.kind != "block";
    let siblings = (o.siblings && named).then(|| scope::siblings(&c.path, lang_name, &source, &tree, &s));
    let callers = (o.callers && named).then(|| callers(&s, &c.path, o, hide));
    Ok(Hit {
        coord: c.clone(),
        shown: (a, b),
        span: (s.start_line, s.end_line),
        lines: cut(a, b),
        siblings,
        callers,
        only,
        scope: Some(s),
    })
}

/// One level of callers of `s`, from the calltree walk. It searches the current
/// directory when the file sits under it, else the file's own directory.
fn callers(s: &Scope, file: &str, o: &Options, hide: &files::HideOpts) -> crate::calltree::CallTrees {
    let p = std::path::Path::new(file);
    let root = if p.is_relative() && !file.starts_with("..") {
        ".".to_string()
    } else {
        p.parent().and_then(|d| d.to_str()).filter(|d| !d.is_empty()).unwrap_or(".").to_string()
    };
    let jobs = files::resolve_jobs_lenient(&root, o.lang.as_deref(), o.no_ignore);
    let mut hider = files::Hider::new(hide);
    hider.add_jobs(&root, &jobs);
    crate::calltree::build(crate::calltree::Direction::Up, &[s.name.clone()], &jobs, 1, &mut hider)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq_scopeof_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(d: &std::path::Path, name: &str, src: &str) -> String {
        let p = d.join(name);
        std::fs::write(&p, src).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn at(path: &str, line: usize) -> Coord {
        Coord { path: path.to_string(), line, col: None }
    }

    fn run(c: &[Coord], o: &Options) -> Report {
        report(c, o, &files::HideOpts::default())
    }

    const RS: &str = "const A: u8 = 1;\nstruct S;\nimpl S {\n    fn m(&self) {\n        let c = |x| helper(x);\n    }\n    fn n(&self) {}\n}\nfn helper(x: u8) {}\n";

    #[test]
    fn coordinates_parse_with_and_without_a_column() {
        assert_eq!(parse_coord("src/a.rs:30").unwrap(), Coord { path: "src/a.rs".into(), line: 30, col: None });
        assert_eq!(parse_coord("src/a.rs:30:7").unwrap(), Coord { path: "src/a.rs".into(), line: 30, col: Some(7) });
        assert_eq!(parse_coord("a:b/c.rs:4").unwrap().path, "a:b/c.rs");
    }

    #[test]
    fn arguments_that_are_not_coordinates_name_the_expected_form() {
        for bad in ["src/a.rs", "src/a.rs:x", ":30", "a.rs:0", "a.rs:3:0", "a.rs:"] {
            let e = parse_coord(bad).unwrap_err();
            assert_eq!(e, format!("not a coordinate: {}, expected path:LINE or path:LINE:COL", bad));
        }
    }

    #[test]
    fn a_line_in_a_closure_returns_the_named_parent_with_its_body() {
        let d = dir("closure");
        let f = put(&d, "a.rs", RS);
        let r = run(&[at(&f, 5)], &Options::default());
        assert_eq!(r.code, 0);
        assert_eq!(r.out, format!("{f}:5  in  {f}:4-6  fn m(&self) {{\n    fn m(&self) {{\n        let c = |x| helper(x);\n    }}\n"));
        assert_eq!(r.err, "");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_signature_line_returns_that_function() {
        let d = dir("sig");
        let f = put(&d, "a.rs", RS);
        let r = run(&[at(&f, 7)], &Options::default());
        assert!(r.out.starts_with(&format!("{f}:7  in  {f}:7-7  fn n(&self) {{}}\n")), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_column_narrows_the_byte() {
        let d = dir("col");
        let f = put(&d, "a.rs", "fn outer() {\n    fn inner() { go(); }\n}\n");
        // column 1 sits in the indentation, which only outer holds
        let r = run(&[Coord { path: f.clone(), line: 2, col: Some(1) }], &Options::default());
        assert!(r.out.starts_with(&format!("{f}:2  in  {f}:1-3  fn outer() {{\n")), "{}", r.out);
        let r = run(&[Coord { path: f.clone(), line: 2, col: Some(20) }], &Options::default());
        assert!(r.out.starts_with(&format!("{f}:2  in  {f}:2-2  fn inner() {{ go(); }}\n")), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn up_climbs_to_the_impl_and_stops_at_the_outermost() {
        let d = dir("up");
        let f = put(&d, "a.rs", RS);
        let r = run(&[at(&f, 5)], &Options { up: 1, ..Options::default() });
        assert!(r.out.starts_with(&format!("{f}:5  in  {f}:3-8  impl S {{\n")), "{}", r.out);
        assert!(!r.out.contains("only"), "{}", r.out);
        let r = run(&[at(&f, 5)], &Options { up: 4, ..Options::default() });
        assert!(r.out.starts_with(&format!("{f}:5  in  {f}:3-8  impl S {{\n")), "{}", r.out);
        assert!(r.out.ends_with(&format!("{f}:5: only 2 enclosing scopes\n")), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn siblings_follow_the_body_as_path_line_signature_rows() {
        let d = dir("sib");
        let f = put(&d, "a.rs", RS);
        let r = run(&[at(&f, 5)], &Options { siblings: true, ..Options::default() });
        assert!(r.out.ends_with(&format!("    }}\nsiblings:\n{f}:7: fn n(&self) {{}}\n")), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn callers_list_one_level_of_callers_of_the_scope() {
        let d = dir("callers");
        let f = put(&d, "calc.rs", "fn add(a: i32) -> i32 { a }\nfn twice(a: i32) -> i32 { add(a) + add(a) }\nfn lonely() {}\n");
        let r = run(&[at(&f, 1)], &Options { callers: true, ..Options::default() });
        assert!(r.out.contains("\ncallers:\n└── twice()  [calc.rs:2]\n"), "{}", r.out);
        let r = run(&[at(&f, 3)], &Options { callers: true, ..Options::default() });
        assert!(r.out.ends_with("callers: none\n"), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn block_returns_the_innermost_block_headed_by_its_first_line() {
        let d = dir("block");
        let src = "fn f(x: u8) {\n    if x > 1 {\n        go();\n    }\n}\n";
        let f = put(&d, "a.rs", src);
        let r = run(&[at(&f, 3)], &Options { block: true, ..Options::default() });
        assert_eq!(r.out, format!("{f}:3  in  {f}:2-4  if x > 1 {{\n    if x > 1 {{\n        go();\n    }}\n"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_scope_over_budget_prints_a_window_centred_on_the_line_and_a_footer() {
        let d = dir("budget");
        let mut src = String::from("fn big() {\n");
        for i in 0..18 {
            src.push_str(&format!("    step{}();\n", i));
        }
        src.push_str("}\n");
        let f = put(&d, "a.rs", &src);
        // the scope is lines 1-20, line 10 is step8
        let r = run(&[at(&f, 10)], &Options { budget: 4, ..Options::default() });
        assert_eq!(
            r.out,
            format!("{f}:10  in  {f}:1-20  fn big() {{\n    step6();\n    step7();\n    step8();\n    step9();\nshowing lines 8-11 of 1-20, --budget 20 for all\n")
        );
        // near the end the window is clamped to the scope
        let r = run(&[at(&f, 20)], &Options { budget: 3, ..Options::default() });
        assert!(r.out.ends_with("    step16();\n    step17();\n}\nshowing lines 18-20 of 1-20, --budget 20 for all\n"), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_line_outside_every_scope_is_top_level_with_a_window_of_the_file() {
        let d = dir("top");
        let f = put(&d, "a.rs", RS);
        let r = run(&[at(&f, 1)], &Options::default());
        assert!(r.out.starts_with(&format!("{f}:1  at top level\nconst A: u8 = 1;\n")), "{}", r.out);
        assert!(!r.out.contains("showing"), "{}", r.out);
        let r = run(&[at(&f, 1)], &Options { budget: 3, ..Options::default() });
        assert_eq!(r.out, format!("{f}:1  at top level\nconst A: u8 = 1;\nstruct S;\nimpl S {{\nshowing lines 1-3 of 1-9, --budget 9 for all\n"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_file_and_a_line_past_the_end_go_to_stderr_and_the_rest_continue() {
        let d = dir("miss");
        let f = put(&d, "a.rs", RS);
        let gone = d.join("gone.rs").to_str().unwrap().to_string();
        let r = run(&[at(&gone, 1), at(&f, 99), at(&f, 5), at(&f, 7)], &Options::default());
        assert_eq!(r.code, 0);
        assert!(r.err.contains(&format!("no such file: {gone}")), "{}", r.err);
        assert!(r.err.contains(&format!("{f}:99: past the end of the file (9 lines)\n")), "{}", r.err);
        // two resolved coordinates are separated by one blank line
        assert!(r.out.contains("    }\n\n"), "{}", r.out);
        assert_eq!(r.out.matches("  in  ").count(), 2);
        let r = run(&[at(&gone, 1), at(&f, 99)], &Options::default());
        assert_eq!((r.code, r.out.as_str()), (1, ""));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_exported_const_arrow_is_found_from_its_signature_line() {
        let d = dir("tsx");
        let f = put(&d, "a.tsx", "import x from 'y'\n\nexport const Editor = (p: P) => {\n  return 1\n}\n");
        let r = run(&[at(&f, 3)], &Options::default());
        assert!(r.out.starts_with(&format!("{f}:3  in  {f}:3-5  export const Editor = (p: P) => {{\n")), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_output_has_no_semicolons_or_em_dashes_of_its_own() {
        assert!(!showing(1, 2, 1, 9).contains(';') && !showing(1, 2, 1, 9).contains('\u{2014}'));
        assert_eq!(window(10, 1, 20, 4), (8, 11));
        assert_eq!(window(1, 1, 20, 4), (1, 4));
        assert_eq!(window(20, 1, 20, 4), (17, 20));
        assert_eq!(window(5, 3, 6, 150), (3, 6));
    }
}
