use crate::compiler::lang;
use ignore::WalkBuilder;

/// Files under `dir` whose extension is in `exts`, plus files with no extension
/// whose shebang line names a language one of `exts` belongs to (a Ruby script
/// called `brief`). The `.git` directory is skipped: its objects have no
/// extension and would each be opened for a shebang.
pub fn collect_files(dir: &str, exts: &[&str], no_ignore: bool) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(dir)
        .git_ignore(!no_ignore)
        .git_global(!no_ignore)
        .git_exclude(!no_ignore)
        .ignore(!no_ignore)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for result in walker {
        if let Ok(entry) = result {
            let path = entry.path();
            if path.is_file() {
                let Some(s) = path.to_str() else { continue };
                match path.extension().and_then(|e| e.to_str()) {
                    Some(ext) => {
                        if exts.contains(&ext) {
                            files.push(s.to_string());
                        }
                    }
                    None => {
                        if let Some(l) = lang::lang_for_shebang_file(s) {
                            if lang::extensions_for_name(l).iter().any(|e| exts.contains(e)) {
                                files.push(s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    files.sort();
    files
}

/// `resolve_jobs` for a command that takes several paths: a path that does not
/// exist prints `no such file: X (did you mean Y)` on stderr and contributes no
/// jobs, so the other paths still run. Any other failure behaves as in `resolve_jobs`.
pub fn resolve_jobs_lenient(path: &str, lang_name: Option<&str>, no_ignore: bool) -> Vec<(String, &'static str)> {
    if matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
        eprintln!("{}", crate::miss::no_such_file(path));
        return Vec::new();
    }
    resolve_jobs(path, lang_name, no_ignore)
}

/// Resolve (file_path, lang_name) pairs from a path (file or directory).
/// `--lang ts` takes `.ts` and `.tsx` files and parses each with its own
/// grammar. A file with no extension takes its language from its shebang line.
pub fn resolve_jobs(path: &str, lang_name: Option<&str>, no_ignore: bool) -> Vec<(String, &'static str)> {
    let requested = match lang_name {
        Some(ln) => match lang::lang_for_name_canonical(ln) {
            Some(c) => Some(c),
            None => {
                eprintln!("Unknown language: {}. Supported: c, cpp, javascript, typescript, tsx, python, ruby, rust, go", ln);
                std::process::exit(1);
            }
        },
        None => None,
    };
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{}", crate::miss::no_such_file(path));
            std::process::exit(1);
        }
        Err(e) => { eprintln!("Failed to access {}: {}", path, e); std::process::exit(1); }
    };
    if meta.is_dir() {
        let exts = lang_name.map(lang::extensions_for_name)
            .unwrap_or_else(lang::all_known_extensions);
        collect_files(path, exts, no_ignore).into_iter().filter_map(|f| {
            let ln = match requested {
                Some(c) => lang::refine_for_path(c, &f),
                None => lang::lang_for_path(&f)?,
            };
            Some((f, ln))
        }).collect()
    } else {
        let ln = match requested {
            Some(c) => Some(lang::refine_for_path(c, path)),
            None => lang::lang_for_path(path),
        };
        match ln {
            Some(ln) => vec![(path.to_string(), ln)],
            None => {
                match std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
                    Some(ext) => eprintln!("Cannot detect language from extension .{}; use --lang", ext),
                    None if lang::shebang_is_shell(&lang::first_line_of(path)) => {
                        eprintln!("Cannot detect language for {}: shell scripts have no grammar here; use --lang to force one", path)
                    }
                    None => eprintln!("Cannot detect language for {}: no extension and no recognised shebang; use --lang", path),
                }
                std::process::exit(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hiding tests and excluded paths
// ---------------------------------------------------------------------------

/// Why a file or a match is hidden from output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hide {
    /// A test file, or a test scope inside a file (`--tests` shows it).
    Test,
    /// A path an `--exclude` glob names.
    Excluded,
}

/// The two global flags that decide what is hidden.
#[derive(Debug, Clone, Default)]
pub struct HideOpts {
    /// `--tests`: hide nothing for being a test.
    pub tests: bool,
    /// `--exclude GLOB`, repeatable.
    pub exclude: Vec<String>,
}

/// One language's `[tests]` table, with its globs compiled.
struct LangTests {
    files: Vec<regex::Regex>,
    dirs: Vec<String>,
    /// Scope selectors compiled for this language. A selector the language has
    /// no kind for is left out, with one note on stderr.
    scopes: Vec<(Box<dyn lang::Lang>, crate::compiler::CompiledQuery)>,
}

/// Decides, per file and per line, what a command hides, and remembers the
/// search root each file was found under.
///
/// A test path is judged relative to the search root given on the command line,
/// so `ctsq -q X tests/` searches inside tests/ as usual. A file named directly
/// on the command line is never hidden as a file, though test scopes inside it
/// still are. Use: `add_jobs(root, &jobs)` once per path argument, then `file`
/// for a whole file and `site` for one line, or `keep` to split a list.
pub struct Hider {
    show_tests: bool,
    exclude: Vec<(bool, regex::Regex)>,
    /// file -> (search root, language)
    jobs: std::collections::HashMap<String, (String, &'static str)>,
    tests: std::collections::HashMap<(std::path::PathBuf, &'static str), std::rc::Rc<LangTests>>,
    scopes: std::collections::HashMap<String, Vec<(usize, usize)>>,
    /// file -> verdict of `file`, so a match in the file does not re-judge it
    verdicts: std::collections::HashMap<String, Option<Hide>>,
    /// search root -> repo root, where the config lookup starts
    repos: std::collections::HashMap<String, std::path::PathBuf>,
}

impl Hider {
    pub fn new(opts: &HideOpts) -> Hider {
        Hider {
            show_tests: opts.tests,
            exclude: opts.exclude.iter().map(|g| (g.contains('/'), glob_regex(g))).collect(),
            jobs: Default::default(),
            tests: Default::default(),
            scopes: Default::default(),
            verdicts: Default::default(),
            repos: Default::default(),
        }
    }

    /// A Hider that hides nothing, for tests that want every match.
    #[cfg(test)]
    pub fn none() -> Hider {
        Hider::new(&HideOpts { tests: true, exclude: vec![] })
    }

    /// Record the jobs one path argument produced, so each file is judged
    /// against that root.
    pub fn add_jobs(&mut self, root: &str, jobs: &[(String, &'static str)]) {
        for (f, l) in jobs {
            self.jobs.entry(f.clone()).or_insert_with(|| (root.to_string(), *l));
        }
    }

    fn tests_for(&mut self, root: &str, lang_name: &'static str) -> std::rc::Rc<LangTests> {
        let repo = self
            .repos
            .entry(root.to_string())
            .or_insert_with(|| crate::config::find_repo_root(std::path::Path::new(root)))
            .clone();
        let key = (repo.clone(), lang_name);
        if let Some(t) = self.tests.get(&key) {
            return t.clone();
        }
        let table = match crate::config::load_tests(&repo, lang_name) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(2);
            }
        };
        let mut scopes = Vec::new();
        for sel in &table.scopes {
            let compiled = crate::query::parse(sel).ok().and_then(|ast| {
                let l = lang::for_name(lang_name)?;
                crate::compiler::validate(&ast, l.as_ref(), lang_name).ok()?;
                let c = crate::compiler::compile(&ast, l.as_ref());
                Some((l, c))
            });
            match compiled {
                Some(c) => scopes.push(c),
                None => eprintln!("[tests] scope '{}' does not compile for {}, skipped", sel, lang_name),
            }
        }
        let t = std::rc::Rc::new(LangTests {
            files: table.files.iter().map(|g| glob_regex(g)).collect(),
            dirs: table.dirs.clone(),
            scopes,
        });
        self.tests.insert(key, t.clone());
        t
    }

    /// Why `file` is hidden as a whole, if it is. A file no `add_jobs` call
    /// recorded, and a file named directly on the command line, are never hidden.
    pub fn file(&mut self, file: &str) -> Option<Hide> {
        if let Some(v) = self.verdicts.get(file) {
            return *v;
        }
        let v = self.file_verdict(file);
        self.verdicts.insert(file.to_string(), v);
        v
    }

    fn file_verdict(&mut self, file: &str) -> Option<Hide> {
        let (root, lang_name) = self.jobs.get(file)?.clone();
        if file == "-" || std::path::Path::new(&root).is_file() {
            return None;
        }
        let rel = relative(&root, file);
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        let Some((name, dirs)) = segs.split_last() else { return None };
        if self.exclude.iter().any(|(slash, re)| {
            if *slash { re.is_match(&rel) } else { segs.iter().any(|s| re.is_match(s)) }
        }) {
            return Some(Hide::Excluded);
        }
        if self.show_tests {
            return None;
        }
        let t = self.tests_for(&root, lang_name);
        let by_name = t.files.iter().any(|re| re.is_match(name) || re.is_match(&rel));
        let by_dir = dirs.iter().any(|d| t.dirs.iter().any(|td| td == d));
        (by_name || by_dir).then_some(Hide::Test)
    }

    /// Whether `file` has test scopes to look for, so callers can skip reading it.
    fn has_scopes(&mut self, file: &str) -> bool {
        if self.show_tests {
            return false;
        }
        let Some((root, lang_name)) = self.jobs.get(file).cloned() else { return false };
        lang_name == "rust" || !self.tests_for(&root, lang_name).scopes.is_empty()
    }

    /// Test scopes in `file` as inclusive 1-based line ranges, cached per file.
    /// `source` saves a read when the caller already holds the bytes.
    pub fn scope_lines(&mut self, file: &str, source: Option<&[u8]>) -> Vec<(usize, usize)> {
        self.scope_lines_in(file, source, None)
    }

    /// `scope_lines` with a tree the caller already parsed from the file with
    /// its grammar, so a Rust file is not parsed a second time.
    pub fn scope_lines_in(
        &mut self,
        file: &str,
        source: Option<&[u8]>,
        tree: Option<&tree_sitter::Tree>,
    ) -> Vec<(usize, usize)> {
        if !self.has_scopes(file) {
            return vec![];
        }
        if let Some(s) = self.scopes.get(file) {
            return s.clone();
        }
        let owned;
        let source = match source {
            Some(s) => s,
            None => match std::fs::read(file) {
                Ok(s) => {
                    owned = s;
                    &owned
                }
                Err(_) => return vec![],
            },
        };
        let (root, lang_name) = self.jobs[file].clone();
        let t = self.tests_for(&root, lang_name);
        let mut ranges = Vec::new();
        let mut add_rows = |rows: Vec<crate::runner::MatchRow>| {
            for r in rows {
                ranges.push((line_at(source, r.start_byte), line_at(source, r.end_byte.saturating_sub(1).max(r.start_byte))));
            }
        };
        if lang_name == "rust" {
            // A Rust file with neither `mod test` nor `cfg(` has no test scope,
            // so it is not parsed. Otherwise one parse serves every scope.
            if contains(source, b"mod test") || contains(source, b"cfg(") {
                let parsed;
                let tree = match tree {
                    Some(t) => Some(t),
                    None => {
                        parsed = parse_rust(source);
                        parsed.as_ref()
                    }
                };
                if let Some(tree) = tree {
                    for (l, c) in &t.scopes {
                        if let Ok(rows) = crate::runner::run_rows_on(c, l.as_ref(), source, tree, false) {
                            add_rows(rows);
                        }
                    }
                    ranges.extend(rust_cfg_test_ranges(tree, source));
                }
            }
        } else {
            for (l, c) in &t.scopes {
                if let Ok(rows) = crate::runner::run_rows(c, l.as_ref(), source, false) {
                    add_rows(rows);
                }
            }
        }
        ranges.sort_unstable();
        ranges.dedup();
        self.scopes.insert(file.to_string(), ranges.clone());
        ranges
    }

    /// Why the match at `file:line` is hidden, if it is: the whole file, or a
    /// test scope around the line.
    pub fn site(&mut self, file: &str, line: usize) -> Option<Hide> {
        if let Some(h) = self.file(file) {
            return Some(h);
        }
        self.scope_lines(file, None)
            .iter()
            .any(|(a, b)| *a <= line && line <= *b)
            .then_some(Hide::Test)
    }

    /// Split `items` into the ones shown and a count of the ones hidden, by the
    /// `file:line` each one sits at.
    pub fn keep<T>(
        &mut self,
        items: Vec<T>,
        at: impl Fn(&T) -> (String, usize),
        hidden: &mut crate::footer::Hidden,
    ) -> Vec<T> {
        let mut out = Vec::new();
        for it in items {
            let (f, l) = at(&it);
            match self.site(&f, l) {
                Some(h) => hidden.count(h, 1),
                None => out.push(it),
            }
        }
        out
    }
}

/// `file` relative to `root`, with `/` separators. A file outside the root is
/// judged by its own path.
fn relative(root: &str, file: &str) -> String {
    let p = std::path::Path::new(file);
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.to_string_lossy().replace('\\', "/")
}

/// 1-based line holding byte `at`.
fn line_at(source: &[u8], at: usize) -> usize {
    source[..at.min(source.len())].iter().filter(|b| **b == b'\n').count() + 1
}

/// A glob as an anchored regex. `*` and `?` stay inside one path segment, `**`
/// crosses segments, `{a,b}` is either, `[...]` is a character class.
pub fn glob_regex(glob: &str) -> regex::Regex {
    let chars: Vec<char> = glob.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    let mut braces = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    re.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    re.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '{' => {
                braces += 1;
                re.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            ',' if braces > 0 => re.push('|'),
            '[' => match chars[i..].iter().position(|ch| *ch == ']') {
                Some(end) if end > 1 => {
                    let class: String = chars[i + 1..i + end].iter().collect();
                    let class = class.strip_prefix('!').map(|r| format!("^{}", r)).unwrap_or(class);
                    re.push_str(&format!("[{}]", class.replace('\\', "\\\\")));
                    i += end + 1;
                    continue;
                }
                _ => re.push_str("\\["),
            },
            _ => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    regex::Regex::new(&re).unwrap_or_else(|_| regex::Regex::new(&format!("^{}$", regex::escape(glob))).unwrap())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    crate::resolve::holds(haystack, std::str::from_utf8(needle).unwrap_or(""))
}

fn parse_rust(source: &[u8]) -> Option<tree_sitter::Tree> {
    let l = lang::for_name("rust")?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&l.ts_language()).ok()?;
    parser.parse(source, None)
}

/// Rust items under `#[cfg(test)]`, from the attribute line to the item's end.
/// Found in code because a selector has no way to say "the item after this
/// attribute" (BUILD.md A09).
fn rust_cfg_test_ranges(tree: &tree_sitter::Tree, source: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let root = tree.root_node();
    // An attribute that names `cfg` has those bytes, so look the attribute up
    // from each occurrence instead of walking every node of the tree.
    let text = String::from_utf8_lossy(source);
    let mut seen: Vec<usize> = Vec::new();
    for (at, _) in text.match_indices("cfg") {
        let Some(mut node) = root.descendant_for_byte_range(at, at + 3) else { continue };
        while node.kind() != "attribute_item" {
            match node.parent() {
                Some(p) => node = p,
                None => break,
            }
        }
        if node.kind() != "attribute_item" || seen.contains(&node.start_byte()) {
            continue;
        }
        seen.push(node.start_byte());
        let attr: String = node.utf8_text(source).unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
        if attr != "#[cfg(test)]" {
            continue;
        }
        let mut item = node.next_named_sibling();
        while let Some(n) = item {
            if !matches!(n.kind(), "attribute_item" | "line_comment" | "block_comment") {
                break;
            }
            item = n.next_named_sibling();
        }
        if let Some(item) = item {
            out.push((node.start_position().row + 1, item.end_position().row + 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-files-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_directory_walk_picks_up_extensionless_scripts_by_shebang() {
        let d = scratch("walk");
        fs::write(d.join("brief"), "#!/usr/bin/env ruby\ndef run; end\n").unwrap();
        fs::write(d.join("serve"), "#!/usr/bin/python3\ndef run(): pass\n").unwrap();
        fs::write(d.join("tool"), "#!/usr/bin/env node\nfunction run() {}\n").unwrap();
        fs::write(d.join("deploy"), "#!/bin/bash\necho hi\n").unwrap();
        fs::write(d.join("README"), "plain text\n").unwrap();
        let jobs = resolve_jobs(d.to_str().unwrap(), None, true);
        let names: Vec<(String, &str)> = jobs
            .iter()
            .map(|(f, l)| (f.rsplit('/').next().unwrap().to_string(), *l))
            .collect();
        assert_eq!(
            names,
            vec![
                ("brief".to_string(), "ruby"),
                ("serve".to_string(), "python"),
                ("tool".to_string(), "javascript"),
            ]
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn lang_filter_keeps_only_matching_shebang_scripts() {
        let d = scratch("filter");
        fs::write(d.join("brief"), "#!/usr/bin/env ruby\n").unwrap();
        fs::write(d.join("serve"), "#!/usr/bin/env python\n").unwrap();
        let jobs = resolve_jobs(d.to_str().unwrap(), Some("ruby"), true);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].1, "ruby");
        assert!(jobs[0].0.ends_with("brief"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_named_extensionless_file_takes_its_shebang_language() {
        let d = scratch("named");
        let f = d.join("brief");
        fs::write(&f, "#!/usr/bin/env ruby\n").unwrap();
        let jobs = resolve_jobs(f.to_str().unwrap(), None, true);
        assert_eq!(jobs, vec![(f.to_str().unwrap().to_string(), "ruby")]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn ts_walks_take_tsx_files_with_the_tsx_grammar() {
        let d = scratch("tsx");
        fs::write(d.join("a.ts"), "const a = 1;\n").unwrap();
        fs::write(d.join("b.tsx"), "const B = () => <div/>;\n").unwrap();
        fs::write(d.join("c.js"), "var c;\n").unwrap();
        let jobs = resolve_jobs(d.to_str().unwrap(), Some("ts"), true);
        let langs: Vec<&str> = jobs.iter().map(|(_, l)| *l).collect();
        assert_eq!(langs, vec!["typescript", "tsx"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_path_contributes_no_jobs_and_does_not_exit() {
        let d = scratch("missing");
        fs::write(d.join("a.js"), "var a;\n").unwrap();
        let gone = d.join("b.js");
        assert!(resolve_jobs_lenient(gone.to_str().unwrap(), None, true).is_empty());
        assert_eq!(resolve_jobs_lenient(d.to_str().unwrap(), None, true).len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    fn hider_over(d: &std::path::Path, opts: &HideOpts) -> (Hider, Vec<(String, &'static str)>) {
        let root = d.to_str().unwrap();
        let jobs = resolve_jobs(root, None, true);
        let mut h = Hider::new(opts);
        h.add_jobs(root, &jobs);
        (h, jobs)
    }

    fn name_of(f: &str, d: &std::path::Path) -> String {
        relative(d.to_str().unwrap(), f)
    }

    #[test]
    fn test_files_are_judged_by_name_and_directory_per_language() {
        let d = scratch("hide");
        for (p, src) in [
            ("pkg/server.go", "package pkg\n"),
            ("pkg/server_test.go", "package pkg\n"),
            ("pkg/testdata/x.go", "package x\n"),
            ("web/App.tsx", "const a = 1;\n"),
            ("web/App.test.tsx", "const a = 1;\n"),
            ("web/__tests__/b.ts", "const b = 1;\n"),
            ("py/test_a.py", "a = 1\n"),
            ("py/a.py", "a = 1\n"),
            ("py/conftest.py", "a = 1\n"),
            ("src/lib.rs", "fn a() {}\n"),
            ("tests/cli.rs", "fn a() {}\n"),
            ("rb/a_spec.rb", "x = 1\n"),
        ] {
            let f = d.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, src).unwrap();
        }
        let (mut h, jobs) = hider_over(&d, &HideOpts::default());
        let mut hidden: Vec<String> = jobs
            .iter()
            .filter(|(f, _)| h.file(f) == Some(Hide::Test))
            .map(|(f, _)| name_of(f, &d))
            .collect();
        hidden.sort();
        assert_eq!(
            hidden,
            vec![
                "pkg/server_test.go",
                "pkg/testdata/x.go",
                "py/conftest.py",
                "py/test_a.py",
                "rb/a_spec.rb",
                "tests/cli.rs",
                "web/App.test.tsx",
                "web/__tests__/b.ts",
            ]
        );
        let (mut all, jobs) = hider_over(&d, &HideOpts { tests: true, exclude: vec![] });
        assert!(jobs.iter().all(|(f, _)| all.file(f).is_none()), "--tests hides no file");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_test_directory_given_as_the_root_is_searched_normally() {
        let d = scratch("root");
        fs::create_dir_all(d.join("tests")).unwrap();
        fs::write(d.join("tests/cli.rs"), "fn a() {}\n").unwrap();
        let (mut h, jobs) = hider_over(&d.join("tests"), &HideOpts::default());
        assert_eq!(jobs.len(), 1);
        assert_eq!(h.file(&jobs[0].0), None, "the segment above the root does not count");
        // Named directly, even a test file is shown.
        let f = d.join("tests/cli.rs");
        let mut named = Hider::new(&HideOpts::default());
        named.add_jobs(f.to_str().unwrap(), &resolve_jobs(f.to_str().unwrap(), None, true));
        assert_eq!(named.file(f.to_str().unwrap()), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exclude_globs_hide_paths_relative_to_the_root() {
        let d = scratch("exclude");
        for p in ["vendor/a.go", "gen/b.pb.go", "main.go", "deep/vendor/c.go"] {
            let f = d.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, "package x\n").unwrap();
        }
        let opts = HideOpts { tests: false, exclude: vec!["vendor".into(), "gen/*.pb.go".into()] };
        let (mut h, jobs) = hider_over(&d, &opts);
        let mut ex: Vec<String> = jobs
            .iter()
            .filter(|(f, _)| h.file(f) == Some(Hide::Excluded))
            .map(|(f, _)| name_of(f, &d))
            .collect();
        ex.sort();
        assert_eq!(ex, vec!["deep/vendor/c.go", "gen/b.pb.go", "vendor/a.go"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn globs_keep_star_inside_one_segment() {
        assert!(glob_regex("*_test.go").is_match("a_test.go"));
        assert!(!glob_regex("*_test.go").is_match("pkg/a_test.go"));
        assert!(glob_regex("**/*.go").is_match("a/b/c.go"));
        assert!(glob_regex("**/*.go").is_match("c.go"));
        assert!(glob_regex("*.{ts,tsx}").is_match("a.tsx"));
        assert!(glob_regex("test_?.py").is_match("test_a.py"));
        assert!(glob_regex("[ab].go").is_match("b.go") && !glob_regex("[ab].go").is_match("c.go"));
        assert!(glob_regex("a.b").is_match("a.b") && !glob_regex("a.b").is_match("axb"));
    }

    #[test]
    fn a_tree_the_caller_parsed_gives_the_same_scopes_and_cfg_in_strings_is_not_a_scope() {
        let d = scratch("scopes-tree");
        let src = "const S: &str = \"#[cfg(test)]\";\n// #[cfg(test)]\n#[cfg( test )]\n#[allow(dead_code)]\nfn helper() {\n}\n\nmod m {\n    #[cfg(test)]\n    fn inner() {}\n}\n";
        fs::write(d.join("lib.rs"), src).unwrap();
        let (mut h, jobs) = hider_over(&d, &HideOpts::default());
        let f = &jobs[0].0;
        let tree = parse_rust(src.as_bytes()).unwrap();
        let with_tree = h.scope_lines_in(f, Some(src.as_bytes()), Some(&tree));
        assert_eq!(with_tree, vec![(3, 6), (9, 10)]);
        let (mut h2, _) = hider_over(&d, &HideOpts::default());
        assert_eq!(h2.scope_lines(f, None), with_tree);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rust_test_scopes_are_mod_tests_and_cfg_test_items() {
        let d = scratch("scopes");
        let src = "fn real() {}\n\n#[cfg(test)]\nfn helper() {\n}\n\nmod tests {\n    fn t() {}\n}\n\n#[cfg(test)]\nmod other {\n    fn u() {}\n}\n";
        fs::write(d.join("lib.rs"), src).unwrap();
        let (mut h, jobs) = hider_over(&d, &HideOpts::default());
        let f = &jobs[0].0;
        assert_eq!(h.scope_lines(f, None), vec![(3, 5), (7, 9), (11, 14)]);
        assert_eq!(h.site(f, 1), None);
        assert_eq!(h.site(f, 8), Some(Hide::Test));
        assert_eq!(h.site(f, 13), Some(Hide::Test));
        let mut hidden = crate::footer::Hidden::default();
        let kept = h.keep(vec![(f.clone(), 1), (f.clone(), 4), (f.clone(), 8)], |(f, l)| (f.clone(), *l), &mut hidden);
        assert_eq!(kept.len(), 1);
        assert_eq!(hidden.tests, 2);
        let (mut all, _) = hider_over(&d, &HideOpts { tests: true, exclude: vec![] });
        assert!(all.scope_lines(f, None).is_empty(), "--tests finds no scopes");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_rust_file_without_test_markers_is_not_parsed_for_scopes_and_verdicts_are_cached() {
        let d = scratch("noscopes");
        fs::write(d.join("plain.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        fs::create_dir_all(d.join("tests")).unwrap();
        fs::write(d.join("tests/cli.rs"), "fn t() {}\n").unwrap();
        let (mut h, jobs) = hider_over(&d, &HideOpts::default());
        let plain = jobs.iter().find(|(f, _)| f.ends_with("plain.rs")).unwrap().0.clone();
        let named = jobs.iter().find(|(f, _)| f.ends_with("tests/cli.rs")).unwrap().0.clone();
        assert!(h.scope_lines(&plain, None).is_empty());
        assert!(contains(b"mod tests {}", b"mod test") && contains(b"#[cfg(test)]", b"cfg("));
        assert!(!contains(b"fn a() {}", b"cfg("));
        assert_eq!(h.file(&named), Some(Hide::Test));
        assert!(h.verdicts.contains_key(&named), "the verdict is remembered");
        assert_eq!(h.site(&named, 1), Some(Hide::Test));
        assert_eq!(h.site(&plain, 1), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn js_walks_take_jsx_files() {
        let d = scratch("jsx");
        fs::write(d.join("a.js"), "var a;\n").unwrap();
        fs::write(d.join("b.jsx"), "const B = () => <div/>;\n").unwrap();
        let jobs = resolve_jobs(d.to_str().unwrap(), Some("js"), true);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|(_, l)| *l == "javascript"));
        let _ = fs::remove_dir_all(&d);
    }
}
