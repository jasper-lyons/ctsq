//! `ctsq find`: named queries (see `config::queries`) run against the current
//! directory.
//!
//! `ctsq find` lists the queries that apply here and why. `ctsq find <query>
//! [key]` runs one query for every framework it applies to and prints each
//! match as `path:LINE  key  target  target-path:LINE`, then a count line per
//! framework naming the file the query came from.
//!
//! Both build their whole output as a string with an exit code, so tests
//! assert on it directly: `list_report` and `report`.

use crate::compiler::{self, lang};
use crate::config::glossary::{Glossary, Name};
use crate::config::queries::{self, Detect, KeyMatch, NamedQuery, Target};
use crate::files;
use crate::footer;
use crate::needle::Needles;
use crate::resolve::DefLookup;
use crate::runner;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The output and exit code of a `find` call. A code of 2 is a usage or
/// config error and its text belongs on stderr.
#[derive(Debug)]
pub struct Report {
    pub out: String,
    pub code: i32,
}

/// The files under the search root, read at most once each.
pub struct Scan {
    root: PathBuf,
    /// What every walked path starts with, cut off when printing.
    prefix: String,
    jobs: Vec<(String, &'static str)>,
    texts: HashMap<String, Option<String>>,
}

impl Scan {
    pub fn new(root: &Path, no_ignore: bool) -> Scan {
        let r = root.display().to_string();
        let jobs = files::resolve_jobs(&r, None, no_ignore);
        let prefix = format!("{}/", r.trim_end_matches('/'));
        Scan { root: root.to_path_buf(), prefix, jobs, texts: HashMap::new() }
    }

    fn jobs_for(&self, langs: &[String]) -> Vec<(String, &'static str)> {
        self.jobs.iter().filter(|(_, l)| langs.iter().any(|x| x == l)).cloned().collect()
    }

    fn text(&mut self, file: &str) -> Option<&str> {
        self.texts
            .entry(file.to_string())
            .or_insert_with(|| std::fs::read_to_string(file).ok())
            .as_deref()
    }
}

/// A path as printed: relative to the search root, so a walk of `.` loses
/// the `./` in front.
fn shown<'a>(prefix: &str, p: &'a str) -> &'a str {
    p.strip_prefix(prefix).unwrap_or(p)
}

/// The pattern that finds an import of `name` in a file of `lang`, in any of
/// the language's import forms. A subpath import (`name/sub`) counts.
fn import_regex(lang: &str, name: &str) -> Option<regex::Regex> {
    let n = regex::escape(name);
    let src = match lang {
        "typescript" | "tsx" | "javascript" => {
            format!(r#"(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)['"]{}(?:/[^'"]*)?['"]"#, n)
        }
        "go" => format!(r#""{}(?:/[^"]*)?""#, n),
        "python" => format!(r"(?m)^\s*(?:from|import)\s+{}(?:\.|\s|$)", n),
        "ruby" => format!(r#"\brequire(?:_relative)?\s*\(?\s*['"]{}(?:/[^'"]*)?['"]"#, n),
        "rust" => format!(r"(?:\buse\s+|\bextern\s+crate\s+){}\b", n),
        "c" | "cpp" => format!(r#"#\s*include\s*[<"]{}"#, n),
        _ => return None,
    };
    regex::Regex::new(&src).ok()
}

/// Why `detect` holds here, or None when it does not.
fn detected(d: &Detect, langs: &[String], scan: &mut Scan) -> Option<String> {
    for f in &d.file {
        if scan.root.join(f).exists() {
            return Some(format!("file {}", f));
        }
    }
    for dep in &d.dependency {
        let n = regex::escape(dep);
        let checks = [
            ("package.json", format!(r#""{}"\s*:"#, n)),
            ("go.mod", format!(r"(?m)(?:^|[\s(]){}(?:\s|/|$)", n)),
            ("Gemfile", format!(r#"\bgem\s+['"]{}['"]"#, n)),
        ];
        for (file, pat) in checks {
            let Ok(text) = std::fs::read_to_string(scan.root.join(file)) else { continue };
            if regex::Regex::new(&pat).is_ok_and(|r| r.is_match(&text)) {
                return Some(format!("dependency {} in {}", dep, file));
            }
        }
    }
    if !d.import.is_empty() {
        for (file, lang_name) in scan.jobs_for(langs) {
            let res: Vec<(String, regex::Regex)> = d
                .import
                .iter()
                .filter_map(|n| import_regex(lang_name, n).map(|r| (n.clone(), r)))
                .collect();
            let Some(text) = scan.text(&file) else { continue };
            if let Some((n, _)) = res.iter().find(|(_, r)| r.is_match(text)) {
                return Some(format!("import {} in {}", n, shown(&scan.prefix, &file)));
            }
        }
    }
    None
}

/// Why `q` applies here, or None.
fn applies(q: &NamedQuery, scan: &mut Scan) -> Option<String> {
    match &q.detect {
        Some(d) => detected(d, &q.langs, scan),
        None => (!scan.jobs_for(&q.langs).is_empty()).then(|| "always".to_string()),
    }
}

/// Why each query applies, computed once per framework (every query of one
/// framework file shares its `detect`).
fn verdicts(queries: &[NamedQuery], scan: &mut Scan) -> Vec<Option<String>> {
    let mut seen: HashMap<(String, String), Option<String>> = HashMap::new();
    queries
        .iter()
        .map(|q| {
            seen.entry((q.framework.clone(), q.source.clone()))
                .or_insert_with(|| applies(q, scan))
                .clone()
        })
        .collect()
}

/// Rows padded into columns two spaces apart. The last column is not padded.
/// Whether a def target's text is a quoted JS/TS module specifier, as in
/// `lazy(() => import("~/routes/x"))`, which names a file, not a definition.
fn is_module_specifier(lang_name: &str, text: Option<&str>) -> bool {
    matches!(lang_name, "javascript" | "typescript" | "tsx")
        && text.is_some_and(|t| t.starts_with(['\'', '"', '`']) && t.contains('/'))
}

/// Longest key shown in the padded key column.
const KEY_MAX: usize = 40;

/// A key cut to `KEY_MAX` characters with `...` at the end, for display only.
fn cap_key(key: &str) -> String {
    if key.chars().count() <= KEY_MAX {
        return key.to_string();
    }
    let head: String = key.chars().take(KEY_MAX - 3).collect();
    format!("{}...", head)
}

fn columns(rows: &[Vec<String>]) -> String {
    let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; n];
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut out = String::new();
    for r in rows {
        let mut line = String::new();
        for (i, c) in r.iter().enumerate() {
            if i + 1 == r.len() {
                line.push_str(c);
            } else {
                line.push_str(c);
                line.push_str(&" ".repeat(widths[i] - c.chars().count() + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// `ctsq find` with no arguments: every named query that applies here.
pub fn list_report(queries: &[NamedQuery], scan: &mut Scan) -> Report {
    let why = verdicts(queries, scan);
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for (q, w) in queries.iter().zip(&why) {
        match w {
            Some(w) => rows.push(vec![
                format!("{}:{}:", q.source, q.line),
                q.name.clone(),
                q.framework.clone(),
                format!("({})", w),
            ]),
            None => missing.push(format!("{} ({})", q.name, q.framework)),
        }
    }
    let mut out = columns(&rows);
    if rows.is_empty() {
        out.push_str("no named query applies here\n");
    }
    if !missing.is_empty() {
        out.push_str(&format!("not detected here: {}\n", missing.join(", ")));
    }
    Report { out, code: 0 }
}

/// A text a file must hold, see `KeyFilter::file_needle`.
enum KeyNeedle {
    /// Lowercase ASCII letters and digits, found whatever the case.
    Fold(String),
}

impl KeyNeedle {
    fn holds(&self, source: &[u8]) -> bool {
        let KeyNeedle::Fold(n) = self;
        let n = n.as_bytes();
        let first = n[0];
        let found = source.len() >= n.len()
            && source
                .windows(n.len())
                .any(|w| w[0].to_ascii_lowercase() == first && w.eq_ignore_ascii_case(n));
        // The Kelvin sign and the dotted capital I lowercase to ASCII letters.
        found || crate::needle::holds(source, "\u{212A}") || crate::needle::holds(source, "\u{130}")
    }
}

/// How a key filter matches: a case-insensitive substring, or a regex when
/// written `/.../`.
enum KeyFilter {
    Any,
    Sub(String),
    Re(regex::Regex),
}

impl KeyFilter {
    fn new(key: Option<&str>) -> Result<KeyFilter, String> {
        match key {
            None => Ok(KeyFilter::Any),
            Some(k) if k.len() >= 2 && k.starts_with('/') && k.ends_with('/') => regex::Regex::new(&k[1..k.len() - 1])
                .map(KeyFilter::Re)
                .map_err(|e| format!("the key regex {} does not compile: {}", k, e.to_string().lines().last().unwrap_or("").trim())),
            Some(k) => Ok(KeyFilter::Sub(k.to_lowercase())),
        }
    }

    /// What a file must hold for any row of it to pass this filter, as groups
    /// of which one text must occur: None when nothing is provable. A key row
    /// is the source text of a node (quotes trimmed, whitespace runs joined), so
    /// a run of letters and digits of the key occurs in the file unbroken. A
    /// substring key is case-insensitive, so its longest such run is searched
    /// ignoring ASCII case. A regex key counts only when it is a literal or an
    /// alternation of literals without spaces. `loose` (the query has
    /// `key_pattern` or `key_text`) gives none for a substring key: a row also
    /// matches by pattern, or by a key the text rules changed, so the user's
    /// text need not occur in the file (a pasted URL never does).
    fn file_needle(&self, loose: bool) -> Option<KeyNeedle> {
        match self {
            KeyFilter::Any => None,
            KeyFilter::Sub(_) if loose => None,
            KeyFilter::Sub(s) => {
                let run = s.split(|c: char| !c.is_ascii_alphanumeric()).max_by_key(|r| r.len())?;
                (!run.is_empty()).then(|| KeyNeedle::Fold(run.to_string()))
            }
            KeyFilter::Re(_) => None,
        }
    }

    /// The text test. `text` is the row key after `key_text`, `printed` the
    /// key as printed, which a `/regex/` key sees.
    fn matches(&self, text: &str, printed: &str) -> bool {
        match self {
            KeyFilter::Any => true,
            KeyFilter::Sub(s) => text.to_lowercase().contains(s),
            KeyFilter::Re(r) => r.is_match(printed),
        }
    }
}

/// A row that matched by its key's pattern.
#[derive(Debug, Clone, PartialEq)]
struct PatternHit {
    /// Literal characters in the pattern: more is more specific.
    literal: usize,
    /// `name=value` for each named group that took part, space-separated.
    params: String,
}

/// One query's key test: the text filter, and the pattern `key_pattern`
/// builds from each row's key (see `config::queries`).
struct KeyMatcher<'a> {
    filter: &'a KeyFilter,
    key_text: &'a [(String, String)],
    scanner: Option<queries::KeyScanner>,
    /// The user's key after `key_input`. None: no pattern is tested (no
    /// `key_pattern`, no key, or a `/regex/` key on a `within` query).
    input: Option<String>,
    mode: KeyMatch,
    min_literal: usize,
    /// Each row key's compiled pattern with its literal count and the name to
    /// print for each group, None when it has none.
    cache: HashMap<String, Option<(regex::Regex, usize, HashMap<String, String>)>>,
    /// `path:line: [name]` of the query, for the warning about a pattern that
    /// does not compile.
    label: String,
    warned: bool,
}

impl<'a> KeyMatcher<'a> {
    fn new(q: &'a NamedQuery, filter: &'a KeyFilter, key: Option<&str>) -> KeyMatcher<'a> {
        let scanner = queries::KeyScanner::new(&q.key_pattern);
        // A `/regex/` key on a whole-key query is also a pasted path: `/events/7/`
        // finds `/events/:id`.
        let input = match (filter, key, &scanner) {
            (KeyFilter::Sub(_), Some(k), Some(_)) => Some(queries::apply_all(&q.key_input, k)),
            (KeyFilter::Re(_), Some(k), Some(_)) if q.key_match == KeyMatch::Whole => Some(queries::apply_all(&q.key_input, k)),
            _ => None,
        };
        KeyMatcher {
            filter,
            key_text: &q.key_text,
            scanner,
            input,
            mode: q.key_match,
            min_literal: q.key_min_literal,
            cache: HashMap::new(),
            label: format!("{}:{}: [{}]", q.source, q.line, q.name),
            warned: false,
        }
    }

    /// Whether a row key passes, and how when by pattern.
    fn test(&mut self, key: &str) -> (bool, Option<PatternHit>) {
        let text = if self.key_text.is_empty() { key.to_string() } else { queries::apply_all(self.key_text, key) };
        let by_text = self.filter.matches(&text, key);
        let (Some(input), Some(scanner)) = (&self.input, &self.scanner) else { return (by_text, None) };
        let (mode, min) = (self.mode, self.min_literal);
        let (label, warned) = (&self.label, &mut self.warned);
        let compiled = self.cache.entry(text.clone()).or_insert_with(|| {
            let k = scanner.scan(&text);
            // A whole-key pattern with no parameter or splat matches one URL
            // only, so the root route `/` needs no literal character.
            if k.literal < min && (mode != KeyMatch::Whole || k.wild) {
                return None;
            }
            let src = match mode {
                KeyMatch::Whole => format!("(?is)^(?:{})$", k.source),
                KeyMatch::Within => format!("(?is){}", k.source),
            };
            match regex::Regex::new(&src) {
                Ok(r) => Some((r, k.literal, k.names.into_iter().collect())),
                Err(e) => {
                    if !*warned {
                        *warned = true;
                        eprintln!(
                            "warning: {} the key `{}` gives a pattern that does not compile, the key is matched as text only: {}",
                            label,
                            key,
                            e.to_string().lines().last().unwrap_or("").trim()
                        );
                    }
                    None
                }
            }
        });
        let Some((re, literal, names)) = compiled else { return (by_text, None) };
        let Some(caps) = re.captures(input) else { return (by_text, None) };
        // A key that also matched as text filled in nothing (it holds the
        // route's own `{id}`), so only a pattern-only match shows values.
        let params: Vec<String> = if by_text {
            Vec::new()
        } else {
            re.capture_names()
                .flatten()
                .filter_map(|n| caps.name(n).map(|m| format!("{}={}", names.get(n).map(String::as_str).unwrap_or(n), m.as_str())))
                .collect()
        };
        (true, Some(PatternHit { literal: *literal, params: params.join(" ") }))
    }
}

/// Pattern matches first, the most literal characters first, the order
/// otherwise kept. Only for a `whole` query with `key_pattern`: a `within`
/// query keeps the source order.
fn order_by_pattern(q: &NamedQuery, hits: &mut [Hit]) {
    if q.key_pattern.is_empty() || q.key_match != KeyMatch::Whole {
        return;
    }
    hits.sort_by_key(|h| match &h.pattern {
        Some(p) => (0, std::cmp::Reverse(p.literal)),
        None => (1, std::cmp::Reverse(0)),
    });
}

/// A key as printed: its quotes dropped, a message over several lines on one.
fn tidy_key(raw: &str) -> String {
    let k = unquote(raw);
    if k.contains(['\n', '\r']) {
        k.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        k.to_string()
    }
}

/// A string literal's text without its quotes. Anything else as it is.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\'', '`'] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// The name `def` looks up for a target's text: `<About />` gives `About`,
/// `About` gives `About`, `pages.About` gives `pages.About`.
fn target_name(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim().trim_start_matches('<');
    let name: String = first
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | ':'))
        .collect();
    if name.is_empty() { first.to_string() } else { name }
}

/// How many leading directories two paths share.
fn shared_dirs(a: &str, b: &str) -> usize {
    let dirs = |p: &str| -> Vec<String> {
        let mut v: Vec<String> = p.split('/').map(str::to_string).collect();
        v.pop();
        v
    };
    dirs(a).iter().zip(dirs(b).iter()).take_while(|(x, y)| x == y).count()
}

/// One printed match.
struct Hit {
    file: String,
    line: usize,
    key: String,
    target: String,
    target_loc: String,
    /// Set when the row matched by its key's pattern.
    pattern: Option<PatternHit>,
}

/// A named query that does not compile is the query file's fault: name the
/// file, the table line and the selector.
fn config_error(q: &NamedQuery, m: &str, why: &str) -> Report {
    Report {
        out: format!("{}:{}: [{}] match `{}`: {}\n", q.source, q.line, q.name, m, why),
        code: 2,
    }
}

/// Compile each of `q`'s selectors for each language its files are in. A
/// selector whose node kinds one grammar lacks is skipped for that grammar,
/// but it has to compile for at least one of the query's languages.
type Compiled = Vec<(&'static str, Box<dyn lang::Lang>, compiler::CompiledQuery, Needles)>;

fn compile_all(q: &NamedQuery) -> Result<Compiled, Report> {
    let mut out: Compiled = Vec::new();
    for m in &q.matches {
        let ast = crate::query::parse(m).map_err(|e| config_error(q, m, &e.to_string()))?;
        let must = Needles::from_query(&ast);
        let mut first_err = None;
        let mut any = false;
        let mut langs: Vec<Box<dyn lang::Lang>> = Vec::new();
        for l in &q.langs {
            let Some(name) = lang::lang_for_name_canonical(l) else { continue };
            let Some(lb) = lang::for_name(name) else { continue };
            if let Err(e) = compiler::validate(&ast, lb.as_ref(), name) {
                first_err.get_or_insert(e);
                continue;
            }
            if let Err(e) = compiler::check_compiles(&ast, lb.as_ref(), name, m) {
                first_err.get_or_insert(e.to_string());
                continue;
            }
            any = true;
            langs.push(lb);
            let lb = lang::for_name(name).unwrap();
            let c = compiler::compile(&ast, lb.as_ref());
            out.push((name, lb, c, must.clone()));
        }
        if !any {
            return Err(config_error(q, m, &first_err.unwrap_or_else(|| "no language to run it on".into())));
        }
        let refs: Vec<&dyn lang::Lang> = langs.iter().map(|b| b.as_ref()).collect();
        if let Err(e) = compiler::check_fields(&ast, &refs, m) {
            return Err(config_error(q, m, &e.to_string()));
        }
    }
    Ok(out)
}

/// What a run looks for: the key filter, and the names a glossary term adds. A
/// row that fails the key still shows when its key holds, or its target is, one
/// of the added names.
struct Ask<'a> {
    filter: &'a KeyFilter,
    /// The key as the user typed it.
    key: Option<&'a str>,
    extra: &'a [Name],
}

impl Ask<'_> {
    fn matcher<'b>(&'b self, q: &'b NamedQuery) -> KeyMatcher<'b> {
        KeyMatcher::new(q, self.filter, self.key)
    }

    /// Whether a row the key filter dropped is kept by the glossary.
    fn glossary_keeps(&self, key: &str, target: &str) -> bool {
        self.extra.iter().any(|n| n.found_in(key) || n.matches(target))
    }
}

/// Run one named query over the files of its languages.
fn run_query(
    q: &NamedQuery,
    scan: &mut Scan,
    ask: &Ask,
    hider: &mut files::Hider,
    hidden: &mut footer::Hidden,
) -> Result<Vec<Hit>, Report> {
    if q.paths.is_some() {
        return run_paths(q, scan, ask, hider, hidden);
    }
    let compiled = compile_all(q)?;
    let jobs = scan.jobs_for(&q.langs);
    let prefix = scan.prefix.clone();
    let shown = |p: &str| shown(&prefix, p).to_string();
    let mut lookup: Option<DefLookup> = None;
    let mut specs: Option<crate::resolve::imports::JsSpecResolver> = None;
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    // A file lacking the key's text has no row that passes the key filter, and
    // nothing else keeps a row: not the glossary, not a rewritten key.
    let key_needle = if ask.extra.is_empty() && q.key_rewrite.is_none() {
        ask.filter.file_needle(!q.key_pattern.is_empty() || !q.key_text.is_empty())
    } else {
        None
    };
    let mut km = ask.matcher(q);
    for (file, lang_name) in &jobs {
        if !compiled.iter().any(|(l, ..)| l == lang_name) {
            continue;
        }
        let Ok(source) = std::fs::read(file) else { continue };
        if let Some(k) = &key_needle {
            if !k.holds(&source) {
                continue;
            }
        }
        let mine: Vec<_> = compiled
            .iter()
            .filter(|(l, _, _, must)| l == lang_name && must.holds(&source))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&mine[0].1.ts_language()).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&source, None) else { continue };
        let mut file_rows: Vec<(runner::MatchRow, String)> = Vec::new();
        for (_, lb, c, _) in mine {
            let Ok(rows) = runner::run_rows_on(c, lb.as_ref(), &source, &tree, false) else { continue };
            for r in rows {
                let key = match r.captures.iter().find(|c| c.name == "key") {
                    Some(k) => tidy_key(&k.text),
                    None => r.src_line.clone().unwrap_or_default(),
                };
                let key = match &q.key_rewrite {
                    Some(rw) => queries::apply_rewrite(rw, &key),
                    None => key,
                };
                file_rows.push((r, key));
            }
        }
        if q.key_nest {
            nest_keys(&mut file_rows, &tree);
        }
        {
            for (r, key) in file_rows {
                let cap = |n: &str| r.captures.iter().find(|c| c.name == n);
                let (key_ok, pattern) = km.test(&key);
                if !key_ok && ask.extra.is_empty() {
                    continue;
                }
                let line = cap("key").map(|k| k.line).unwrap_or(r.line);
                let why_hidden = hider.site(file, line);
                if key_ok {
                    if let Some(h) = why_hidden {
                        hidden.count(h, 1);
                        continue;
                    }
                }
                let tcap = cap("target");
                let (target, target_loc) = match q.target {
                    // A def target whose text is a quoted module specifier (a lazy
                    // import) is resolved like default-export: the module's
                    // default export serves the route.
                    Target::Def if !is_module_specifier(lang_name, tcap.map(|t| t.text.as_str())) => {
                        // No @target: the @key node names the target too (`resources :gigs`).
                        let text = tcap.or_else(|| cap("key")).map(|t| t.text.as_str()).unwrap_or("");
                        let name = match &q.rewrite {
                            Some(rw) => queries::apply_rewrite(rw, unquote(text)).trim_end_matches('#').to_string(),
                            None => target_name(text),
                        };
                        let l = lookup.get_or_insert_with(|| DefLookup::new(&jobs));
                        let pick = pick_def(l, &name, file, hider);
                        let loc = match pick {
                            Some((f, line)) => format!("{}:{}", shown(&f), line),
                            None => "?".to_string(),
                        };
                        (name, loc)
                    }
                    Target::Def | Target::DefaultExport => {
                        let spec = {
                            let t = tcap.map(|t| t.text.as_str()).unwrap_or("");
                            let t = unquote(t.lines().next().unwrap_or(""));
                            match &q.rewrite {
                                Some(rw) => queries::apply_rewrite(rw, t),
                                None => t.to_string(),
                            }
                        };
                        let r = specs.get_or_insert_with(|| crate::resolve::imports::JsSpecResolver::new(&jobs));
                        match nearest(r.resolve(file, &spec), file) {
                            Some(target_file) => export_of(&target_file, &jobs, &prefix, &spec),
                            None => (spec, "?".to_string()),
                        }
                    }
                    Target::Enclosing => {
                        let byte = tcap.map(|t| t.start_byte).unwrap_or(r.start_byte);
                        match crate::scope::nearest(file, lang_name, &source, &tree, byte) {
                            Some(s) => (s.name.clone(), format!("{}:{}", shown(file), s.start_line)),
                            None => ("(top level)".to_string(), "?".to_string()),
                        }
                    }
                    Target::Match => match tcap {
                        Some(t) => (
                            unquote(t.text.lines().next().unwrap_or("")).to_string(),
                            format!("{}:{}", shown(file), t.line),
                        ),
                        None => (r.src_line.clone().unwrap_or_default(), format!("{}:{}", shown(file), r.line)),
                    },
                };
                if !key_ok {
                    if !ask.glossary_keeps(&key, &target) {
                        continue;
                    }
                    if let Some(h) = why_hidden {
                        hidden.count(h, 1);
                        continue;
                    }
                }
                if seen.insert((file.clone(), line, key.clone(), target.clone())) {
                    hits.push(Hit { file: file.clone(), line, key, target, target_loc, pattern });
                }
            }
        }
    }
    hits.sort_by(|a, b| (&a.file, a.line, &a.key).cmp(&(&b.file, b.line, &b.key)));
    order_by_pattern(q, &mut hits);
    Ok(hits)
}

/// The node a row spans for `key_nest`: the smallest node holding all its
/// captures (the route object, or the JSX opening tag), widened from an
/// opening tag to its whole element so the element's children are inside.
fn row_span(r: &runner::MatchRow, tree: &tree_sitter::Tree) -> Option<(usize, usize)> {
    let start = r.captures.iter().map(|c| c.start_byte).min()?;
    let end = r.captures.iter().map(|c| c.end_byte).max()?;
    let mut n = tree.root_node().descendant_for_byte_range(start, end)?;
    if n.kind().ends_with("_opening_element") {
        n = n.parent().unwrap_or(n);
    }
    Some((n.start_byte(), n.end_byte()))
}

/// `a` and `b` joined with exactly one `/` between them.
fn join_path(a: &str, b: &str) -> String {
    let tail = b.trim_start_matches('/');
    if tail.is_empty() {
        return if a.is_empty() { "/".to_string() } else { a.to_string() };
    }
    format!("{}/{}", a.trim_end_matches('/'), tail)
}

/// Decision 09: each row whose key lies inside the span of other rows of the
/// same query in this file takes their keys, outermost first, in front of its
/// own. A row's own key, and rows on the same key node, do not count.
fn nest_keys(rows: &mut [(runner::MatchRow, String)], tree: &tree_sitter::Tree) {
    let key_at = |r: &runner::MatchRow| r.captures.iter().find(|c| c.name == "key").map(|c| (c.start_byte, c.end_byte));
    // One parent per key node, however many selectors matched it.
    let mut parents: Vec<((usize, usize), (usize, usize), String)> = Vec::new();
    for (r, key) in rows.iter() {
        let (Some(k), Some(span)) = (key_at(r), row_span(r, tree)) else { continue };
        if !parents.iter().any(|(pk, ..)| *pk == k) {
            parents.push((k, span, key.clone()));
        }
    }
    let joined: Vec<Option<String>> = rows
        .iter()
        .map(|(r, key)| {
            let k = key_at(r)?;
            let mut outer: Vec<&((usize, usize), (usize, usize), String)> = parents
                .iter()
                .filter(|(pk, span, _)| *pk != k && span.0 <= k.0 && k.1 <= span.1 && !(pk.0 <= k.0 && k.1 <= pk.1))
                .collect();
            if outer.is_empty() {
                return None;
            }
            // Outermost first: the widest span starts first.
            outer.sort_by_key(|(_, span, _)| (span.0, std::cmp::Reverse(span.1)));
            let mut out = String::new();
            for (_, _, pkey) in outer {
                out = if out.is_empty() { pkey.clone() } else { join_path(&out, pkey) };
            }
            Some(join_path(&out, key))
        })
        .collect();
    for ((_, key), j) in rows.iter_mut().zip(joined) {
        if let Some(j) = j {
            *key = j;
        }
    }
}

/// The file nearest `from` (longest shared directory), then the first by name.
fn nearest(mut files: Vec<String>, from: &str) -> Option<String> {
    files.sort();
    files.into_iter().rev().max_by_key(|f| shared_dirs(from, f))
}

/// The definition `name` points at, nearest `from`, a shown one before a hidden
/// one. `Ctrl#action` (a Rails target) finds the controller, then the action
/// inside the controller's file, and falls back to the controller.
fn pick_def(l: &mut DefLookup, name: &str, from: &str, hider: &mut files::Hider) -> Option<(String, usize)> {
    let mut best = |defs: Vec<(String, usize, crate::resolve::Confidence)>| -> Option<(String, usize)> {
        defs.iter()
            .min_by_key(|(f, line, _)| (hider.site(f, *line).is_some(), std::cmp::Reverse(shared_dirs(from, f))))
            .map(|(f, line, _)| (f.clone(), *line))
    };
    let Some((class, action)) = name.split_once('#') else {
        return best(l.find(name));
    };
    let (cf, cl) = best(l.find(class))?;
    if !action.is_empty() {
        let in_file: Vec<_> = l.find(action).into_iter().filter(|(f, _, _)| *f == cf).collect();
        if let Some(found) = best(in_file) {
            return Some(found);
        }
    }
    Some((cf, cl))
}

/// The name and `path:LINE` of the default export of `file`.
fn export_of(file: &str, jobs: &[(String, &'static str)], prefix: &str, spec: &str) -> (String, String) {
    let lang_name = jobs.iter().find(|(f, _)| f == file).map(|(_, l)| *l).unwrap_or("typescript");
    let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(spec).to_string();
    match default_export(file, lang_name) {
        Some((name, line)) => (name.unwrap_or(stem), format!("{}:{}", shown(prefix, file), line)),
        None => (stem, format!("{}:1", shown(prefix, file))),
    }
}

/// The default export of a JS or TS file: its name (None when anonymous) and
/// line. `export default function X`, `export default X` (the line of X's
/// declaration when it is in the file) and `export { X as default }` name X.
fn default_export(file: &str, lang_name: &str) -> Option<(Option<String>, usize)> {
    let source = std::fs::read(file).ok()?;
    let lb = lang::for_name(lang_name)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lb.ts_language()).ok()?;
    let tree = parser.parse(&source, None)?;
    let root = tree.root_node();
    let text = |n: tree_sitter::Node| String::from_utf8_lossy(&source[n.byte_range()]).to_string();
    // The line a top-level declaration of `name` starts on.
    let declared = |name: &str| -> Option<usize> {
        let mut c = root.walk();
        for top in root.children(&mut c) {
            let d = if top.kind() == "export_statement" {
                match top.child_by_field_name("declaration") {
                    Some(d) => d,
                    None => continue,
                }
            } else {
                top
            };
            let named = match d.kind() {
                "function_declaration" | "class_declaration" | "generator_function_declaration" => {
                    d.child_by_field_name("name").is_some_and(|n| text(n) == name)
                }
                "lexical_declaration" | "variable_declaration" => {
                    let mut c2 = d.walk();
                    let found = d.children(&mut c2).any(|v| {
                        v.kind() == "variable_declarator" && v.child_by_field_name("name").is_some_and(|n| text(n) == name)
                    });
                    found
                }
                _ => false,
            };
            if named {
                return Some(top.start_position().row + 1);
            }
        }
        None
    };
    let mut c = root.walk();
    for top in root.children(&mut c) {
        if top.kind() != "export_statement" {
            continue;
        }
        let line = top.start_position().row + 1;
        let mut c2 = top.walk();
        let is_default = top.children(&mut c2).any(|k| k.kind() == "default");
        if is_default {
            if let Some(d) = top.child_by_field_name("declaration") {
                let name = d.child_by_field_name("name").map(text);
                return Some((name, line));
            }
            if let Some(v) = top.child_by_field_name("value") {
                if v.kind() == "identifier" {
                    let n = text(v);
                    let l = declared(&n).unwrap_or(line);
                    return Some((Some(n), l));
                }
                return Some((None, line));
            }
        }
        let mut c3 = top.walk();
        for clause in top.children(&mut c3).filter(|k| k.kind() == "export_clause") {
            let mut c4 = clause.walk();
            for sp in clause.children(&mut c4).filter(|k| k.kind() == "export_specifier") {
                if sp.child_by_field_name("alias").is_some_and(|a| text(a) == "default") {
                    let n = sp.child_by_field_name("name").map(text)?;
                    let l = declared(&n).unwrap_or(line);
                    return Some((Some(n), l));
                }
            }
        }
    }
    None
}

/// A `paths` query: each file under the search root matching the glob is a
/// match. The key comes from `key_from_path`, the target is the file's
/// default export. Route groups, directories written `(name)`, are not part
/// of a URL and are dropped from the key.
fn run_paths(
    q: &NamedQuery,
    scan: &mut Scan,
    ask: &Ask,
    hider: &mut files::Hider,
    hidden: &mut footer::Hidden,
) -> Result<Vec<Hit>, Report> {
    let bad = |why: String| Report { out: format!("{}:{}: [{}] {}\n", q.source, q.line, q.name, why), code: 2 };
    let glob = q.paths.as_deref().unwrap_or("");
    let mut ob = ignore::overrides::OverrideBuilder::new(&scan.root);
    ob.add(glob).map_err(|e| bad(format!("paths `{}`: {}", glob, e)))?;
    let ov = ob.build().map_err(|e| bad(format!("paths `{}`: {}", glob, e)))?;
    let re = regex::Regex::new(q.key_from_path.as_deref().unwrap_or("")).map_err(|e| bad(format!("key_from_path: {}", e)))?;
    let jobs = scan.jobs_for(&q.langs);
    let prefix = scan.prefix.clone();
    let mut hits = Vec::new();
    let mut km = ask.matcher(q);
    for (file, _) in &jobs {
        let rel = shown(&prefix, file);
        if !ov.matched(rel, false).is_whitelist() {
            continue;
        }
        let Some(caps) = re.captures(rel) else { continue };
        let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let segs: Vec<&str> = raw.split('/').filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')'))).collect();
        let mut key = if segs.is_empty() { "/".to_string() } else { format!("/{}", segs.join("/")) };
        if let Some(rw) = &q.key_rewrite {
            key = queries::apply_rewrite(rw, &key);
        }
        let (key_ok, pattern) = km.test(&key);
        if !key_ok && ask.extra.is_empty() {
            continue;
        }
        let why_hidden = hider.site(file, 1);
        if key_ok {
            if let Some(h) = why_hidden {
                hidden.count(h, 1);
                continue;
            }
        }
        let (target, target_loc) = export_of(file, &jobs, &prefix, rel);
        if !key_ok {
            if !ask.glossary_keeps(&key, &target) {
                continue;
            }
            if let Some(h) = why_hidden {
                hidden.count(h, 1);
                continue;
            }
        }
        hits.push(Hit { file: file.clone(), line: 1, key, target, target_loc, pattern });
    }
    hits.sort_by(|a, b| (&a.key, &a.file).cmp(&(&b.key, &b.file)));
    order_by_pattern(q, &mut hits);
    Ok(hits)
}

/// `ctsq find <query> [key]`.
#[cfg(test)]
pub fn report(queries: &[NamedQuery], name: &str, key: Option<&str>, scan: &mut Scan, hide: &files::HideOpts) -> Report {
    report_with(queries, name, key, scan, hide, None)
}

/// `report` with a repo glossary: a key that is, or holds as a whole phrase, a
/// glossary term also finds the rows whose key holds, or whose target is, one
/// of the term's names. A line says so, last before any footer.
pub fn report_with(
    queries: &[NamedQuery],
    name: &str,
    key: Option<&str>,
    scan: &mut Scan,
    hide: &files::HideOpts,
    glossary: Option<&Glossary>,
) -> Report {
    let mine: Vec<&NamedQuery> = queries.iter().filter(|q| q.name == name).collect();
    if mine.is_empty() {
        let mut names: Vec<&str> = queries.iter().map(|q| q.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        return Report {
            out: format!("no named query '{}', the queries are: {}\n", name, names.join(", ")),
            code: 2,
        };
    }
    let filter = match KeyFilter::new(key) {
        Ok(f) => f,
        Err(e) => return Report { out: format!("{}\n", e), code: 2 },
    };
    let mut extra: Vec<Name> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if let (Some(g), Some(k)) = (glossary, key) {
        // A /regex/ key is a filter, not a phrase.
        if !(k.len() >= 2 && k.starts_with('/') && k.ends_with('/')) {
            for e in g.lookup(k, false) {
                extra.extend(e.names.iter().cloned());
                notes.push(e.note(&g.file));
            }
        }
    }
    let ask = Ask { filter: &filter, key, extra: &extra };
    let owned: Vec<NamedQuery> = mine.iter().map(|q| (*q).clone()).collect();
    let why = verdicts(&owned, scan);
    let mut hider = files::Hider::new(hide);
    let root = scan.root.display().to_string();
    hider.add_jobs(&root, &scan.jobs.clone());
    let mut hidden = footer::Hidden::default();

    let mut tried: Vec<(String, String)> = Vec::new();
    let mut not_detected: Vec<(String, String)> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut counts: Vec<(usize, String)> = Vec::new();
    let mut printed: HashSet<(String, usize, String, String)> = HashSet::new();
    for (q, w) in owned.iter().zip(&why) {
        if w.is_none() {
            not_detected.push((q.framework.clone(), q.source.clone()));
            continue;
        }
        tried.push((q.framework.clone(), q.source.clone()));
        let hits = match run_query(q, scan, &ask, &mut hider, &mut hidden) {
            Ok(h) => h,
            Err(r) => return r,
        };
        // A row two frameworks both found prints once, under the first.
        let hits: Vec<Hit> = hits
            .into_iter()
            .filter(|h| printed.insert((h.file.clone(), h.line, h.key.clone(), h.target.clone())))
            .collect();
        if hits.is_empty() {
            counts.push((0, format!("0 {} ({}, from {})", footer::plural(name, 0), q.framework, q.source)));
            continue;
        }
        // The count line counts the rows printed, so --max-matches shows in it.
        let before = rows.len();
        for h in hits {
            if !footer::take(&h.file) {
                continue;
            }
            let mut row = vec![format!("{}:{}", shown(&scan.prefix, &h.file), h.line), cap_key(&h.key), h.target, h.target_loc];
            if let Some(p) = h.pattern.filter(|p| !p.params.is_empty()) {
                row.push(p.params);
            }
            rows.push(row);
        }
        let n = rows.len() - before;
        counts.push((n, format!(
            "{} {} ({}, from {})",
            n,
            footer::plural(name, n),
            q.framework,
            q.source
        )));
    }
    footer::settle(&mut hidden);

    let shown_n = rows.len();
    if shown_n == 0 && !hidden.any() {
        let mut out = crate::miss::find_report(name, key, &tried, &not_detected);
        for n in &notes {
            out.push_str(n);
            out.push('\n');
        }
        return Report { out, code: 1 };
    }
    if shown_n == 0 {
        let phrase = match key {
            Some(k) => format!("no {} to {}", name, k),
            None => format!("no {}", name),
        };
        let out = footer::nothing_shown(&phrase, &scan.root.display().to_string(), scan.jobs.len(), &hidden);
        return Report { out, code: 1 };
    }
    let mut out = columns(&rows);
    // Frameworks that matched nothing are listed too, but only beside one that
    // matched, so the reader sees they were tried.
    let any_matched = counts.iter().any(|(n, _)| *n > 0);
    for (n, c) in &counts {
        if *n == 0 && !any_matched {
            continue;
        }
        out.push_str(c);
        out.push('\n');
    }
    for n in &notes {
        out.push_str(n);
        out.push('\n');
    }
    if hidden.any() {
        // The footer's own count line repeats what the count lines said.
        out.push_str(&footer::hidden_lines(&hidden));
    }
    Report { out, code: 0 }
}

/// Entry for the CLI: loads the queries for the current directory and runs
/// the list or one query. Prints and returns the exit code.
pub fn run(name: Option<&str>, key: Option<&str>, no_ignore: bool, hide: &files::HideOpts) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let queries = match crate::config::queries::load(&cwd) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("error: {}", e);
            return 2;
        }
    };
    let glossary = match crate::config::glossary::load(&cwd) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {}", e);
            return 2;
        }
    };
    let mut scan = Scan::new(Path::new("."), no_ignore);
    let r = match name {
        None => list_report(&queries, &mut scan),
        Some(n) => report_with(&queries, n, key, &mut scan, hide, glossary.as_ref()),
    };
    // A miss, a usage error and an everything-hidden report go to stderr.
    if r.code != 0 {
        eprint!("{}", r.out);
    } else {
        print!("{}", r.out);
    }
    r.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{queries, Roots};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!("{}/fixtures/queries/{}", env!("CARGO_MANIFEST_DIR"), name))
    }

    /// The queries a run in `dir` sees, with no user config or cache layer.
    fn queries_in(dir: &Path) -> Vec<NamedQuery> {
        let home = std::env::temp_dir().join(format!("ctsq-find-nohome-{}", std::process::id()));
        let roots = Roots::from_vars(dir, Some(home.to_str().unwrap()), None, None);
        queries::load_with(&roots, dir).unwrap()
    }

    fn run(name: &str, key: Option<&str>, opts: &files::HideOpts) -> Report {
        let dir = fixture(name);
        let qs = queries_in(&dir);
        report(&qs, "route", key, &mut Scan::new(&dir, false), opts)
    }

    fn hide() -> files::HideOpts {
        files::HideOpts { tests: false, exclude: vec![] }
    }

    #[test]
    fn solid_routes_in_jsx_and_config_objects_resolve_their_components() {
        let r = run("solid", None, &hide());
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(
            r.out,
            "src/App.tsx:7    /            Home      src/pages/Home.tsx:1\n\
             src/App.tsx:8    /about       About     src/pages/About.tsx:1\n\
             src/routes.ts:5  /settings    Settings  src/pages/Settings.tsx:1\n\
             src/routes.ts:6  /about/team  Nowhere   ?\n\
             4 routes (solid-routes, from built-in queries/solid-routes.toml)\n\
             hidden: 1 in tests (--tests)\n"
        );
    }

    #[test]
    fn a_key_is_a_case_insensitive_substring_or_a_slash_regex() {
        let r = run("solid", Some("/ABOUT"), &hide());
        let rows: Vec<&str> = r.out.lines().filter(|l| l.starts_with("src/")).collect();
        assert_eq!(rows.len(), 2, "{}", r.out);
        assert!(r.out.contains("1 in tests"), "the test route matched the key too: {}", r.out);
        let r = run("solid", Some("/^/about$/"), &hide());
        assert!(r.out.starts_with("src/App.tsx:8  /about  About  src/pages/About.tsx:1\n1 route ("), "{}", r.out);
        let bad = run("solid", Some("/(/"), &hide());
        assert_eq!(bad.code, 2, "{}", bad.out);
    }

    #[test]
    fn tests_shows_the_hidden_route() {
        let r = run("solid", Some("test-only"), &files::HideOpts { tests: true, exclude: vec![] });
        assert!(r.out.starts_with("src/routes.test.ts:3  /about/test-only  Home  src/pages/Home.tsx:1\n"), "{}", r.out);
        let hidden = run("solid", Some("test-only"), &hide());
        assert_eq!(hidden.code, 1);
        assert!(
            hidden.out.starts_with("no route to test-only outside tests in ")
                && hidden.out.ends_with("\nhidden: 1 in tests (--tests)\n"),
            "{}",
            hidden.out
        );
    }

    #[test]
    fn react_router_objects_and_jsx_resolve_their_components() {
        let r = run("react-router", None, &hide());
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(
            r.out,
            "src/App.tsx:6   /contact  Contact  src/pages.tsx:17\n\
             src/App.tsx:7   /legacy   Legacy   src/pages.tsx:21\n\
             src/main.tsx:5  /         Root     src/pages.tsx:1\n\
             src/main.tsx:6  /about    About    src/pages.tsx:5\n\
             src/main.tsx:7  /team     Layout   src/pages.tsx:9\n\
             5 routes (react-router, from built-in queries/react-router.toml)\n"
        );
    }

    #[test]
    fn a_repo_query_file_overrides_the_built_in_for_a_lazy_import_shape() {
        let r = run("solid-mavexa", Some("/library"), &hide());
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(
            r.out,
            "frontend/src/routesConfig.ts:16  /w/:workspaceID/library      Library  frontend/src/routes/library.tsx:3\n\
             frontend/src/routesConfig.ts:17  /w/:workspaceID/library/:id  Asset    frontend/src/routes/library/[id].tsx:1\n\
             2 routes (solid-routes, from .ctsq/queries/solid-routes.toml)\n"
        );
        // The override leaves the key fields out, so it takes the built-in's,
        // key_nest and key_pattern among them.
        let r = run("solid-mavexa", Some("https://app.example.com/w/abc/library/7"), &hide());
        assert!(
            r.out.starts_with("frontend/src/routesConfig.ts:17  /w/:workspaceID/library/:id  Asset  frontend/src/routes/library/[id].tsx:1  workspaceID=abc id=7\n"),
            "{}",
            r.out
        );
    }

    #[test]
    fn nested_keys_join_with_one_slash() {
        assert_eq!(join_path("/w/:id", "/start"), "/w/:id/start");
        assert_eq!(join_path("/org/:org", "team/:team"), "/org/:org/team/:team");
        assert_eq!(join_path("/", "about"), "/about");
        assert_eq!(join_path("/a/", "/"), "/a/");
        assert_eq!(join_path("/a", ""), "/a");
        assert_eq!(join_path("", "/"), "/");
    }

    /// Whether `user` finds a row whose key is `key` through the built-in
    /// pack `file`, and the params it shows.
    fn pack_test(file: &str, user: &str, key: &str) -> (bool, Option<String>) {
        let text = std::fs::read_to_string(format!("{}/config/queries/{}", env!("CARGO_MANIFEST_DIR"), file)).unwrap();
        let q = queries::parse_file("p", &text).unwrap().remove(0);
        let f = KeyFilter::new(Some(user)).unwrap();
        let (ok, hit) = KeyMatcher::new(&q, &f, Some(user)).test(key);
        (ok, hit.map(|h| h.params))
    }

    #[test]
    fn a_route_with_no_parameter_needs_no_literal_character_but_a_catch_all_does() {
        assert!(pack_test("solid-routes.toml", "https://x/", "/").0);
        assert!(!pack_test("solid-routes.toml", "https://x/a", "*").0);
        assert!(!pack_test("solid-routes.toml", "https://x/a", "/*").0);
        assert!(pack_test("go-http.toml", "https://x/", "/{$}").0);
        assert!(!pack_test("go-http.toml", "https://x/a", "/{$}").0);
        assert!(pack_test("go-http.toml", "https://x/", "GET /{$}").0);
        assert!(pack_test("next-app.toml", "https://x/", "/").0);
        assert!(!pack_test("go-http.toml", "https://x/a", "/").0, "the root no longer takes every path");
    }

    #[test]
    fn a_slash_regex_key_is_also_a_pasted_path_on_a_whole_key_query() {
        assert_eq!(pack_test("solid-routes.toml", "/events/7/", "/events/:id"), (true, Some("id=7".to_string())));
        assert!(!pack_test("error-go.toml", "/x 7 y/", "x %d y").0, "a within query keeps a regex key a regex");
    }

    #[test]
    fn go_routes_take_any_case_method_and_head_for_get() {
        assert!(pack_test("go-http.toml", "get https://x/a/1", "GET /a/{id}").0);
        assert!(pack_test("go-http.toml", "HEAD /a/1", "GET /a/{id}").0);
        assert!(!pack_test("go-http.toml", "HEAD /a/1", "POST /a/{id}").0);
        assert!(pack_test("go-chi.toml", "/q/12", "/q/{id:\\\\d+}").0, "the Go string is unescaped");
        assert!(!pack_test("go-chi.toml", "/q/ab", "/q/{id:\\\\d+}").0);
    }

    #[test]
    fn splat_routes_also_take_the_bare_prefix() {
        for key in ["/docs/*rest", "/docs/*"] {
            assert!(pack_test("solid-routes.toml", "https://x/docs", key).0, "{}", key);
            assert!(pack_test("react-router.toml", "https://x/docs/", key).0, "{}", key);
            assert!(pack_test("react-router.toml", "https://x/docs/a/b", key).0, "{}", key);
        }
        assert_eq!(pack_test("solid-routes.toml", "/docs/a/b", "/docs/*rest").1.as_deref(), Some("rest=a/b"));
    }

    #[test]
    fn go_net_http_routes_resolve_their_handlers_and_keep_the_method() {
        let r = run("go-http", None, &hide());
        assert_eq!(
            r.out,
            "main.go:20  GET /api/healthz       healthz  main.go:9\n\
             main.go:21  POST /api/auth/login   login    main.go:11\n\
             main.go:22  POST /api/auth/logout  logout   main.go:13\n\
             main.go:30  /                      index    main.go:17\n\
             4 routes (net-http, from built-in queries/go-http.toml)\n"
        );
    }

    #[test]
    fn chi_routes_resolve_functions_and_method_values() {
        let r = run("go-chi", None, &hide());
        assert_eq!(
            r.out,
            "main.go:20  /users       listUsers   main.go:9\n\
             main.go:21  /users       createUser  main.go:11\n\
             main.go:22  /users/{id}  Show        main.go:15\n\
             main.go:24  /users/{id}  createUser  main.go:11\n\
             4 routes (chi, from built-in queries/go-chi.toml)\n\
             0 routes (net-http, from built-in queries/go-http.toml)\n"
        );
    }

    #[test]
    fn rails_routes_rewrite_to_a_controller_and_action_and_print_a_question_mark_when_missing() {
        let r = run("rails", None, &hide());
        assert_eq!(
            r.out,
            "config/routes.rb:2  /about    StaticController#about     app/controllers/static_controller.rb:2\n\
             config/routes.rb:3  /contact  StaticController#contact   app/controllers/static_controller.rb:5\n\
             config/routes.rb:4  /login    SessionsController#create  app/controllers/sessions_controller.rb:3\n\
             config/routes.rb:5  /missing  GhostsController#boo       ?\n\
             config/routes.rb:6  /gigs     GigsController             app/controllers/gigs_controller.rb:1\n\
             5 routes (rails-routes, from built-in queries/rails-routes.toml)\n"
        );
    }

    #[test]
    fn next_app_files_are_routes_keyed_by_directory_with_groups_dropped() {
        let r = run("next-app", None, &hide());
        assert_eq!(
            r.out,
            "app/page.tsx:1                      /             Home      app/page.tsx:1\n\
             app/about/page.tsx:1                /about        About     app/about/page.tsx:1\n\
             app/blog/[slug]/page.tsx:1          /blog/[slug]  BlogPost  app/blog/[slug]/page.tsx:1\n\
             app/(marketing)/pricing/page.tsx:1  /pricing      page      app/(marketing)/pricing/page.tsx:1\n\
             4 routes (next-app, from built-in queries/next-app.toml)\n"
        );
        let r = run("next-app", Some("/blog"), &hide());
        assert!(r.out.starts_with("app/blog/[slug]/page.tsx:1  /blog/[slug]  BlogPost  "), "{}", r.out);
    }

    #[test]
    fn default_exports_are_named_by_declaration_local_or_file_stem() {
        let d = std::env::temp_dir().join(format!("ctsq-find-defexp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let cases = [
            ("a.tsx", "export default function A() {}\n", Some("A")),
            ("b.tsx", "const x = 1;\nconst B = () => 1;\nexport default B;\n", Some("B")),
            ("c.tsx", "export default () => 1;\n", None),
            ("d.tsx", "function D() {}\nexport { D as default };\n", Some("D")),
        ];
        for (f, src, want) in cases {
            std::fs::write(d.join(f), src).unwrap();
            let got = default_export(d.join(f).to_str().unwrap(), "tsx").map(|(n, _)| n);
            assert_eq!(got, Some(want.map(str::to_string)), "{}", f);
        }
        assert_eq!(default_export(d.join("b.tsx").to_str().unwrap(), "tsx").unwrap().1, 2, "the local's line");
        std::fs::write(d.join("e.tsx"), "export const e = 1;\n").unwrap();
        assert_eq!(default_export(d.join("e.tsx").to_str().unwrap(), "tsx"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_built_in_solid_query_resolves_the_lazy_import_shape_through_default_export() {
        let dir = fixture("solid-mavexa");
        let qs = queries::load_with(&Roots { repo: None, config: None, cache: None }, &dir).unwrap();
        let r = report(&qs, "route", Some("/library"), &mut Scan::new(&dir, false), &hide());
        assert_eq!(r.code, 0, "{}", r.out);
        assert!(
            r.out.contains("/library      Library  frontend/src/routes/library.tsx:3\n"),
            "{}",
            r.out
        );
        assert!(r.out.contains("(solid-routes, from built-in queries/solid-routes.toml)"), "{}", r.out);
    }

    #[test]
    fn a_framework_that_matched_nothing_still_gets_a_zero_count_line_beside_one_that_did() {
        let d = std::env::temp_dir().join(format!("ctsq-find-zero-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".ctsq/queries")).unwrap();
        std::fs::write(d.join("a.go"), "package a\nimport \"errors\"\nfunc F() error { return errors.New(\"same thing\") }\n").unwrap();
        let copy = std::fs::read_to_string(format!("{}/config/queries/error-go.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let copy = copy.replace("go-error", "zz-copy").replace("errors.New", "errors.Nothing");
        std::fs::write(d.join(".ctsq/queries/zz-copy.toml"), copy).unwrap();
        let r = run_named_in(&d, "error", None);
        assert!(r.out.contains("1 error (go-error,"), "{}", r.out);
        assert!(r.out.contains("0 errors (zz-copy, from "), "{}", r.out);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_long_key_is_cut_to_forty_characters_with_three_dots_for_display_only() {
        assert_eq!(cap_key("short"), "short");
        let long = "GET /api/workspaces/{workspaceID}/library/items/{itemID}";
        let cut = cap_key(long);
        assert_eq!(cut.chars().count(), 40, "{}", cut);
        assert!(cut.ends_with("...") && !cut.contains('\u{2026}'), "{}", cut);
        assert!(long.starts_with(&cut[..37]));
        assert_eq!(cap_key(&"x".repeat(40)), "x".repeat(40));
    }

    #[test]
    fn the_listing_names_each_query_its_source_line_and_why() {
        let dir = fixture("solid");
        let r = list_report(&queries_in(&dir), &mut Scan::new(&dir, false));
        assert_eq!(r.code, 0);
        assert_eq!(
            r.out,
            "built-in queries/jsx-component.toml:8:  component  jsx-component  (always)\n\
             built-in queries/error-js.toml:7:       error      js-error       (always)\n\
             built-in queries/jsx-label.toml:8:      label      jsx-label      (always)\n\
             built-in queries/solid-routes.toml:9:   route      solid-routes   (import @solidjs/router in src/App.tsx)\n\
             built-in queries/jsx-text.toml:7:       text       jsx-text       (always)\n\
             not detected here: error (go-error), error (ruby-error), route (chi), route (net-http), route (next-app), route (rails-routes), route (react-router)\n"
        );
        let dir = fixture("solid-mavexa");
        let r = list_report(&queries_in(&dir), &mut Scan::new(&dir, false));
        assert!(
            r.out.contains(".ctsq/queries/solid-routes.toml:10:     route      solid-routes   (import @solidjs/router in frontend/src/routesConfig.ts)\n"),
            "{}",
            r.out
        );
    }

    #[test]
    fn an_unknown_query_name_is_a_usage_error_listing_the_names() {
        let dir = fixture("solid");
        let r = report(&queries_in(&dir), "routes", None, &mut Scan::new(&dir, false), &hide());
        assert_eq!(r.code, 2);
        assert_eq!(r.out, "no named query 'routes', the queries are: component, error, label, route, text\n");
    }

    fn run_named(name: &str, query: &str, key: Option<&str>) -> Report {
        let dir = fixture(name);
        let qs = queries_in(&dir);
        report(&qs, query, key, &mut Scan::new(&dir, false), &hide())
    }

    #[test]
    fn jsx_text_labels_and_components_name_the_component_that_renders_them() {
        let r = run_named("jsx", "text", None);
        assert_eq!(
            r.out,
            "src/Card.tsx:6    Welcome back         Card          src/Card.tsx:3\n\
             src/Card.tsx:14   All rights reserved  Footer        src/Card.tsx:14\n\
             src/Card.tsx:18   Panel                Panel         src/Card.tsx:16\n\
             src/Legacy.jsx:2  Old page             Legacy        src/Legacy.jsx:1\n\
             src/Live.tsx:4    Live preview         renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:4    Offline              renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:5    Streaming now        renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:8    Plain literal        renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:9    Plain template       renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:10   Nested string        renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:10   Nested template      renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:10   Template branch      renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:11   Binary string        renderStatus  src/Live.tsx:1\n\
             src/Live.tsx:12   Binary template      renderStatus  src/Live.tsx:1\n\
             14 texts (jsx-text, from built-in queries/jsx-text.toml)\n"
        );
        let r = run_named("jsx", "label", Some("person"));
        assert_eq!(
            r.out,
            "src/Card.tsx:7  Find a person  Card  src/Card.tsx:3\n\
             1 label (jsx-label, from built-in queries/jsx-label.toml)\n"
        );
        let r = run_named("jsx", "label", None);
        let mut at = 0;
        for key in ["Profile card", "Find a person", "Search people", "Avatar", "Save changes", "Stores the card", "Old helper"] {
            // title, aria-label as {"string"}, placeholder, alt, label, tooltip and helperText
            let found = r.out[at..].find(key);
            assert!(found.is_some(), "{} missing or out of order: {}", key, r.out);
            at += found.unwrap();
        }
        assert!(r.out.contains("7 labels ("), "{}", r.out);
        let r = run_named("jsx", "component", None);
        let names: Vec<&str> = r.out.lines().filter(|l| l.starts_with("src/")).map(|l| l.split_whitespace().nth(1).unwrap()).collect();
        assert_eq!(names, vec!["Card", "Footer", "Panel", "Spacer", "Legacy"], "helper and NotAComponent are not components: {}", r.out);
    }

    #[test]
    fn an_error_key_matches_through_placeholders_in_each_language() {
        let go = run_named("errors", "error", Some("failed to load pipeline abc-1: open x: no such file"));
        assert_eq!(go.out, "go/load.go:14  failed to load pipeline %s: %w  Load  go/load.go:10\n1 error (go-error, from built-in queries/error-go.toml)\n0 errors (js-error, from built-in queries/error-js.toml)\n0 errors (ruby-error, from built-in queries/error-ruby.toml)\n");
        let go = run_named("errors", "error", Some("render 3 blocks ({1 2})"));
        assert!(go.out.starts_with("go/load.go:18  render %d blocks (%+v)  Render  go/load.go:17\n"), "{}", go.out);
        let ts = run_named("errors", "error", Some("bad field 7"));
        assert!(ts.out.starts_with("web/api.ts:3  Bad field ${text}  parse  web/api.ts:1\n"), "{}", ts.out);
        let rb = run_named("errors", "error", Some("Slot 4 is taken"));
        assert!(rb.out.starts_with("rb/gig.rb:3  Slot #{slot} is taken  book  rb/gig.rb:2\n"), "{}", rb.out);
        let plain = run_named("errors", "error", Some("nothing to parse"));
        assert!(plain.out.starts_with("web/api.ts:2  Nothing to parse  parse  web/api.ts:1\n"), "a literal substring, any case: {}", plain.out);
        let top = run_named("errors", "error", Some("pipeline is empty"));
        assert!(top.out.starts_with("go/load.go:8  pipeline is empty  (top level)  ?\n"), "{}", top.out);
        let miss = run_named("errors", "error", Some("something unrelated happened"));
        assert_eq!(miss.code, 1, "{}", miss.out);
        let all = run_named("errors", "error", None);
        assert_eq!(all.out.lines().filter(|l| l.contains(" errors (") || l.contains(" error (")).count(), 3, "{}", all.out);
    }

    /// Whether `user` finds the error whose key is `fmt`, through the rules of
    /// the built-in Go error pack.
    fn error_matches(user: &str, fmt: &str) -> bool {
        let text = std::fs::read_to_string(format!("{}/config/queries/error-go.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let q = queries::parse_file("g", &text).unwrap().remove(0);
        let f = KeyFilter::new(Some(user)).unwrap();
        let (ok, _) = KeyMatcher::new(&q, &f, Some(user)).test(fmt);
        ok
    }

    #[test]
    fn error_key_matching_rules() {
        assert!(error_matches("failed to load pipeline 7: boom", "failed to load pipeline %s: %w"));
        assert!(error_matches("render 3 blocks (&{a})", "render %d blocks (%+v)"));
        assert!(error_matches("bad field 9", "Bad field ${text}"));
        assert!(error_matches("got \"x\" here", "got %q here"));
        assert!(error_matches("pipeline is empty (id=3)", "pipeline is empty"), "the format string anywhere in the log line");
        assert!(error_matches("load pipeline", "failed to load pipeline %s"), "a substring of the literal text");
        assert!(!error_matches("totally different", "failed to load pipeline %s"));
        assert!(!error_matches("anything at all", "%w"), "all placeholder, too loose");
        assert!(!error_matches("a b", "x: %s"), "under four literal characters");
        assert!(!error_matches("failed to load pipeline abc", "\"failed to load \" + name"), "concatenation is not followed");
    }

    #[test]
    fn a_glossary_term_as_key_also_finds_rows_by_target_name_and_says_so() {
        let dir = fixture("../glossary");
        let qs = queries_in(&dir);
        let g = crate::config::glossary::load_with(&Roots::from_vars(&dir, None, None, None), &dir).unwrap().unwrap();
        let run = |key: &str| report_with(&qs, "error", Some(key), &mut Scan::new(&dir, false), &hide(), Some(&g));
        let r = run("pipeline page");
        assert_eq!(
            r.out,
            "src/pipeline.go:6  cannot start the run  Pipeline  src/pipeline.go:5\n\
             1 error (go-error, from built-in queries/error-go.toml)\n\
             .ctsq/glossary.toml:2: \"pipeline page\" also searched as Pipeline\n"
        );
        assert!(run("the Pipeline Page is down").out.contains("src/pipeline.go:6"), "a term inside a longer phrase");
        let r = run("supplies");
        assert!(r.out.starts_with("src/pipeline.go:14  the shelf ran dry  stockLevel  "), "a regex name: {}", r.out);
        assert!(r.out.ends_with(".ctsq/glossary.toml:4: \"supplies\" also searched as /resource|stock/\n"), "{}", r.out);
        let none = report(&qs, "error", Some("pipeline page"), &mut Scan::new(&dir, false), &hide());
        assert_eq!(none.code, 1, "without the glossary nothing matches: {}", none.out);
        let miss = run("brief sync today");
        assert!(miss.out.contains("src/pipeline.go:10  nothing was merged  sync_merged"), "{}", miss.out);
        let unrelated = run("zzz nothing");
        assert_eq!(unrelated.code, 1);
        assert!(!unrelated.out.contains("glossary"), "no term, no line: {}", unrelated.out);
    }

    #[test]
    fn a_row_two_frameworks_both_find_prints_once_under_the_first() {
        let dir = std::env::temp_dir().join(format!("ctsq-find-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ctsq/queries")).unwrap();
        std::fs::write(dir.join("a.go"), "package a\nimport \"errors\"\nfunc F() error { return errors.New(\"same thing\") }\n").unwrap();
        let copy = std::fs::read_to_string(format!("{}/config/queries/error-go.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(dir.join(".ctsq/queries/zz-copy.toml"), copy.replace("go-error", "zz-copy")).unwrap();
        let r = run_named_in(&dir, "error", None);
        assert_eq!(r.out.lines().filter(|l| l.starts_with("a.go:3")).count(), 1, "{}", r.out);
        assert!(r.out.contains("1 error (go-error,"), "{}", r.out);
        assert!(!r.out.contains("1 error (zz-copy"), "the second framework found nothing new: {}", r.out);
        assert!(r.out.contains("0 errors (zz-copy, from "), "{}", r.out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn run_named_in(dir: &Path, query: &str, key: Option<&str>) -> Report {
        let qs = queries_in(dir);
        report(&qs, query, key, &mut Scan::new(dir, false), &hide())
    }

    #[test]
    fn detect_rules_for_files_dependencies_and_no_detect() {
        let d = std::env::temp_dir().join(format!("ctsq-find-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("config")).unwrap();
        std::fs::write(d.join("config/routes.rb"), "x").unwrap();
        std::fs::write(d.join("package.json"), "{ \"dependencies\": { \"react-router-dom\": \"6\" } }").unwrap();
        std::fs::write(d.join("a.ts"), "export const a = 1\n").unwrap();
        let mut scan = Scan::new(&d, false);
        let langs = vec!["typescript".to_string()];
        let det = |file: &[&str], dep: &[&str]| Detect {
            import: vec![],
            file: file.iter().map(|s| s.to_string()).collect(),
            dependency: dep.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(detected(&det(&["config/routes.rb"], &[]), &langs, &mut scan).as_deref(), Some("file config/routes.rb"));
        assert_eq!(
            detected(&det(&[], &["react-router-dom"]), &langs, &mut scan).as_deref(),
            Some("dependency react-router-dom in package.json")
        );
        assert_eq!(detected(&det(&["nope"], &["react"]), &langs, &mut scan), None, "react is not react-router-dom");
        let mut q = queries_in(&fixture("solid")).into_iter().find(|q| q.framework == "solid-routes").unwrap();
        q.detect = None;
        assert_eq!(applies(&q, &mut scan).as_deref(), Some("always"));
        q.langs = vec!["go".into()];
        assert_eq!(applies(&q, &mut scan), None, "no Go files here");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn imports_are_found_in_each_language_form() {
        for (lang, src) in [
            ("tsx", "import { Route } from \"@solidjs/router\""),
            ("typescript", "import type { R } from '@solidjs/router/types'"),
            ("javascript", "const r = require('@solidjs/router')"),
            ("javascript", "const r = await import(\"@solidjs/router\")"),
        ] {
            assert!(import_regex(lang, "@solidjs/router").unwrap().is_match(src), "{}: {}", lang, src);
        }
        assert!(!import_regex("tsx", "react-router").unwrap().is_match("import x from 'react-router-dom'"));
        assert!(import_regex("go", "github.com/go-chi/chi").unwrap().is_match("import \"github.com/go-chi/chi/v5\""));
        assert!(import_regex("python", "flask").unwrap().is_match("from flask import Flask"));
        assert!(import_regex("ruby", "sinatra").unwrap().is_match("require 'sinatra'"));
    }

    #[test]
    fn a_selector_that_compiles_nowhere_is_a_config_error_naming_the_file() {
        let mut q = queries_in(&fixture("solid")).into_iter().find(|q| q.framework == "solid-routes").unwrap();
        q.matches = vec!["(jsx_attribute @a)".into()];
        q.langs = vec!["typescript".into()];
        let e = compile_all(&q).err().unwrap();
        assert_eq!(e.code, 2);
        assert!(
            e.out.starts_with("built-in queries/solid-routes.toml:9: [route] match `(jsx_attribute @a)`: unknown kind 'jsx_attribute'"),
            "{}",
            e.out
        );
    }

    #[test]
    fn a_file_without_the_exact_names_is_never_parsed() {
        let ast = crate::query::parse("(pair.key(property_identifier#path).value(string @key)) ~ (id#/x.y/ @t)").unwrap();
        let n = Needles::from_query(&ast);
        assert_eq!(n.groups().to_vec(), vec![vec!["path".to_string()]], "a regex that is not plain literals gives no needle");
        assert!(n.holds(b"x path y") && !n.holds(b"nothing"));
    }

    #[test]
    fn output_has_no_semicolons_or_em_dashes() {
        for name in ["solid", "react-router", "solid-mavexa", "go-http", "go-chi", "rails", "next-app"] {
            let r = run(name, None, &hide());
            assert!(!r.out.contains(';') && !r.out.contains('\u{2014}'), "{}", r.out);
        }
    }

    #[test]
    fn a_substring_key_needs_its_longest_run_in_the_file_whatever_the_case() {
        let f = KeyFilter::new(Some("Sign-in now")).unwrap();
        let n = f.file_needle(false).unwrap();
        assert!(n.holds(b"<p>please SIGN-IN</p>") && n.holds(b"a sIgN b"));
        assert!(!n.holds(b"nothing here"));
        assert!(n.holds("\u{212A}".as_bytes()), "the Kelvin sign lowercases to k");
        assert!(f.file_needle(true).is_none(), "an error key also matches through placeholders");
        assert!(KeyFilter::new(Some("/Wel/")).unwrap().file_needle(false).is_none());
        assert!(KeyFilter::new(None).unwrap().file_needle(false).is_none());
        assert!(KeyFilter::new(Some("- ")).unwrap().file_needle(false).is_none());
    }
}
