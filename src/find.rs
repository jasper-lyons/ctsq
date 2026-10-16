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
use crate::config::queries::{self, Detect, NamedQuery, Target};
use crate::files;
use crate::footer;
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
                format!("{}:{}: {}", q.source, q.line, q.name),
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

    /// `errors` makes a substring key match an error's format string through
    /// its placeholders as well (`error_key_matches`).
    fn matches(&self, key: &str, errors: bool) -> bool {
        match self {
            KeyFilter::Any => true,
            KeyFilter::Sub(s) if errors => error_key_matches(s, key),
            KeyFilter::Sub(s) => key.to_lowercase().contains(s),
            KeyFilter::Re(r) => r.is_match(key),
        }
    }
}

/// The verbs and interpolations of a format string: `%s %v %d %w %q %x %f %T
/// %+v` and friends, `${...}` in a JS template and `#{...}` in a Ruby string.
fn placeholders() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"%[+#\-]?\d*(?:\.\d+)?[svdwqxfTtgep]|\$\{[^}]*\}|#\{[^}]*\}").unwrap()
    })
}

/// Whether a message typed by the user finds an error whose key is the format
/// string `fmt`. It does when the user's text is a case-insensitive substring
/// of the format string, or when the format string with every placeholder
/// turned into a wildcard matches somewhere in the user's text (a message
/// copied from a log has the verbs filled in). Concatenation is not followed.
/// A format string with under four literal characters is too loose for the
/// second rule.
fn error_key_matches(user: &str, fmt: &str) -> bool {
    let fmt = fmt.replace("\\\"", "\"");
    if fmt.to_lowercase().contains(&user.to_lowercase()) {
        return true;
    }
    let mut pattern = String::new();
    let mut literal = 0;
    let mut at = 0;
    for m in placeholders().find_iter(&fmt) {
        let lit = &fmt[at..m.start()];
        literal += lit.chars().filter(|c| !c.is_whitespace()).count();
        pattern.push_str(&regex::escape(lit));
        pattern.push_str(".*?");
        at = m.end();
    }
    let lit = &fmt[at..];
    literal += lit.chars().filter(|c| !c.is_whitespace()).count();
    pattern.push_str(&regex::escape(lit));
    literal >= 4 && regex::Regex::new(&format!("(?is){}", pattern)).is_ok_and(|r| r.is_match(user))
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
type Compiled = Vec<(&'static str, Box<dyn lang::Lang>, compiler::CompiledQuery, Vec<Vec<String>>)>;

/// The literal texts a regex name filter cannot match without: one of them
/// has to occur in the node's text. Only the plain shapes give any, a literal
/// or an alternation of literals, optionally anchored and grouped
/// (`Error$`, `^(Get|Post)$`). Anything else (classes, repeats, flags) gives
/// None and the filter is not used to skip files.
fn regex_literals(src: &str) -> Option<Vec<String>> {
    let s = src.strip_prefix('^').unwrap_or(src);
    let s = s.strip_suffix('$').unwrap_or(s);
    let s = match s.strip_prefix('(') {
        Some(inner) => inner.strip_prefix("?:").unwrap_or(inner).strip_suffix(')')?,
        None => s,
    };
    let alts: Vec<String> = s.split('|').map(str::to_string).collect();
    let plain = |a: &String| !a.is_empty() && a.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' ' | ':'));
    alts.iter().all(plain).then_some(alts)
}

/// The name filters in the query that name literal text. Each group is the
/// alternatives of one filter, and a file has to hold one of each group's
/// texts, so a file that fails any group cannot match and is never parsed. An
/// exact name is a group of one. Most files in a repo fall out here.
fn needles(q: &crate::query::ast::Query, out: &mut Vec<Vec<String>>) {
    use crate::query::ast::{NameMatch, Selector, SelectorNode};
    fn sel(s: &Selector, out: &mut Vec<Vec<String>>) {
        match &s.node {
            SelectorNode::Bare(a) => match &a.name_match {
                Some(NameMatch::Exact(n)) if !n.is_empty() => out.push(vec![n.clone()]),
                Some(NameMatch::Regex(r)) => {
                    if let Some(alts) = regex_literals(r) {
                        out.push(alts);
                    }
                }
                _ => {}
            },
            SelectorNode::Group { query, .. } => needles(query, out),
        }
        for f in &s.fields {
            if let Some(inner) = &f.inner {
                needles(inner, out);
            }
        }
    }
    sel(&q.head, out);
    for (_, s) in &q.tail {
        sel(s, out);
    }
}

use crate::resolve::holds;

fn compile_all(q: &NamedQuery) -> Result<Compiled, Report> {
    let mut out: Compiled = Vec::new();
    for m in &q.matches {
        let ast = crate::query::parse(m).map_err(|e| config_error(q, m, &e.to_string()))?;
        let mut must = Vec::new();
        needles(&ast, &mut must);
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
    extra: &'a [Name],
}

impl Ask<'_> {
    fn key_ok(&self, q: &NamedQuery, key: &str) -> bool {
        self.filter.matches(key, q.name == "error")
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
    for (file, lang_name) in &jobs {
        if !compiled.iter().any(|(l, ..)| l == lang_name) {
            continue;
        }
        let Ok(source) = std::fs::read(file) else { continue };
        let mine: Vec<_> = compiled
            .iter()
            .filter(|(l, _, _, must)| l == lang_name && must.iter().all(|alts| alts.iter().any(|n| holds(&source, n))))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&mine[0].1.ts_language()).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&source, None) else { continue };
        for (_, lb, c, _) in mine {
            let Ok(rows) = runner::run_rows_on(c, lb.as_ref(), &source, &tree, false) else { continue };
            for r in rows {
                let cap = |n: &str| r.captures.iter().find(|c| c.name == n);
                let key = match cap("key") {
                    Some(k) => tidy_key(&k.text),
                    None => r.src_line.clone().unwrap_or_default(),
                };
                let key = match &q.key_rewrite {
                    Some(rw) => queries::apply_rewrite(rw, &key),
                    None => key,
                };
                let key_ok = ask.key_ok(q, &key);
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
                    Target::Def => {
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
                    Target::DefaultExport => {
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
                    hits.push(Hit { file: file.clone(), line, key, target, target_loc });
                }
            }
        }
    }
    hits.sort_by(|a, b| (&a.file, a.line, &a.key).cmp(&(&b.file, b.line, &b.key)));
    Ok(hits)
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
        let key_ok = ask.key_ok(q, &key);
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
        hits.push(Hit { file: file.clone(), line: 1, key, target, target_loc });
    }
    hits.sort_by(|a, b| (&a.key, &a.file).cmp(&(&b.key, &b.file)));
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
    let ask = Ask { filter: &filter, extra: &extra };
    let owned: Vec<NamedQuery> = mine.iter().map(|q| (*q).clone()).collect();
    let why = verdicts(&owned, scan);
    let mut hider = files::Hider::new(hide);
    let root = scan.root.display().to_string();
    hider.add_jobs(&root, &scan.jobs.clone());
    let mut hidden = footer::Hidden::default();

    let mut tried: Vec<(String, String)> = Vec::new();
    let mut not_detected: Vec<(String, String)> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut counts: Vec<String> = Vec::new();
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
            continue;
        }
        counts.push(format!(
            "{} {} ({}, from {})",
            hits.len(),
            footer::plural(name, hits.len()),
            q.framework,
            q.source
        ));
        for h in hits {
            rows.push(vec![format!("{}:{}", shown(&scan.prefix, &h.file), h.line), h.key, h.target, h.target_loc]);
        }
    }

    let shown_n = rows.len();
    if shown_n == 0 && !hidden.any() {
        let mut out = crate::miss::find_report(name, key, &tried, &not_detected);
        for n in &notes {
            out.push_str(n);
            out.push('\n');
        }
        return Report { out, code: 1 };
    }
    let mut out = columns(&rows);
    for c in &counts {
        out.push_str(c);
        out.push('\n');
    }
    for n in &notes {
        out.push_str(n);
        out.push('\n');
    }
    if hidden.any() {
        // The footer's own count line repeats what the count lines said.
        let f = footer::footer(shown_n, name, &hidden);
        if shown_n == 0 {
            out.push_str(&f);
        } else if let Some(last) = f.lines().last() {
            out.push_str(last);
            out.push('\n');
        }
    }
    Report { out, code: if shown_n == 0 { 1 } else { 0 } }
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
    if r.code == 2 {
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
        assert_eq!(hidden.out, "0 routes\nhidden: 1 in tests (--tests)\n");
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
            "frontend/src/routesConfig.ts:16  /library      Library  frontend/src/routes/library.tsx:3\n\
             frontend/src/routesConfig.ts:17  /library/:id  Asset    frontend/src/routes/library/[id].tsx:1\n\
             2 routes (solid-routes, from .ctsq/queries/solid-routes.toml)\n"
        );
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
             4 routes (chi, from built-in queries/go-chi.toml)\n"
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
    fn the_built_in_solid_query_misses_the_lazy_shape_and_says_what_it_tried() {
        let dir = fixture("solid-mavexa");
        let qs = queries::load_with(&Roots { repo: None, config: None, cache: None }, &dir).unwrap();
        let r = report(&qs, "route", Some("/library"), &mut Scan::new(&dir, false), &hide());
        assert_eq!(r.code, 1);
        assert_eq!(
            r.out,
            "no route matches /library here (tried solid-routes from built-in queries/solid-routes.toml)\n\
             not detected here: chi (built-in queries/go-chi.toml), net-http (built-in queries/go-http.toml), next-app (built-in queries/next-app.toml), rails-routes (built-in queries/rails-routes.toml), react-router (built-in queries/react-router.toml)\n"
        );
    }

    #[test]
    fn the_listing_names_each_query_its_source_line_and_why() {
        let dir = fixture("solid");
        let r = list_report(&queries_in(&dir), &mut Scan::new(&dir, false));
        assert_eq!(r.code, 0);
        assert_eq!(
            r.out,
            "built-in queries/jsx-component.toml:8: component  jsx-component  (always)\n\
             built-in queries/error-js.toml:7: error           js-error       (always)\n\
             built-in queries/jsx-label.toml:8: label          jsx-label      (always)\n\
             built-in queries/solid-routes.toml:9: route       solid-routes   (import @solidjs/router in src/App.tsx)\n\
             built-in queries/jsx-text.toml:7: text            jsx-text       (always)\n\
             not detected here: error (go-error), error (ruby-error), route (chi), route (net-http), route (next-app), route (rails-routes), route (react-router)\n"
        );
        let dir = fixture("solid-mavexa");
        let r = list_report(&queries_in(&dir), &mut Scan::new(&dir, false));
        assert!(
            r.out.contains(".ctsq/queries/solid-routes.toml:10: route         solid-routes   (import @solidjs/router in frontend/src/routesConfig.ts)\n"),
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
            "src/Card.tsx:6    Welcome back         Card    src/Card.tsx:3\n\
             src/Card.tsx:14   All rights reserved  Footer  src/Card.tsx:14\n\
             src/Card.tsx:18   Panel                Panel   src/Card.tsx:16\n\
             src/Legacy.jsx:2  Old page             Legacy  src/Legacy.jsx:1\n\
             4 texts (jsx-text, from built-in queries/jsx-text.toml)\n"
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
        assert_eq!(go.out, "go/load.go:14  failed to load pipeline %s: %w  Load  go/load.go:10\n1 error (go-error, from built-in queries/error-go.toml)\n");
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

    #[test]
    fn error_key_matching_rules() {
        assert!(error_key_matches("failed to load pipeline 7: boom", "failed to load pipeline %s: %w"));
        assert!(error_key_matches("render 3 blocks (&{a})", "render %d blocks (%+v)"));
        assert!(error_key_matches("bad field 9", "Bad field ${text}"));
        assert!(error_key_matches("got \"x\" here", "got %q here"));
        assert!(error_key_matches("pipeline is empty (id=3)", "pipeline is empty"), "the format string anywhere in the log line");
        assert!(error_key_matches("load pipeline", "failed to load pipeline %s"), "a substring of the literal text");
        assert!(!error_key_matches("totally different", "failed to load pipeline %s"));
        assert!(!error_key_matches("anything at all", "%w"), "all placeholder, too loose");
        assert!(!error_key_matches("a b", "x: %s"), "under four literal characters");
        assert!(!error_key_matches("failed to load pipeline abc", "\"failed to load \" + name"), "concatenation is not followed");
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
        assert!(!r.out.contains("zz-copy"), "the second framework found nothing new: {}", r.out);
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
        let mut n = Vec::new();
        needles(&ast, &mut n);
        assert_eq!(n, vec![vec!["path".to_string()]], "a regex that is not plain literals gives no needle");
    }

    #[test]
    fn a_regex_of_plain_literals_gives_alternatives_and_others_give_none() {
        let lits = |s: &str| regex_literals(s);
        assert_eq!(lits("Error$"), Some(vec!["Error".to_string()]));
        assert_eq!(lits("^(Get|Post)$"), Some(vec!["Get".to_string(), "Post".to_string()]));
        assert_eq!(lits("^(?:aria-label|title)$"), Some(vec!["aria-label".to_string(), "title".to_string()]));
        for none in ["^[A-Z]", "\\S", "a.b", "(?i)foo", "a*", "", "(a)(b)"] {
            assert_eq!(lits(none), None, "{}", none);
        }
        let ast = crate::query::parse("(call_expression.function(selector_expression.field(field_identifier#/^(Get|Post)$/)))").unwrap();
        let mut n = Vec::new();
        needles(&ast, &mut n);
        assert_eq!(n, vec![vec!["Get".to_string(), "Post".to_string()]]);
    }

    #[test]
    fn output_has_no_semicolons_or_em_dashes() {
        for name in ["solid", "react-router", "solid-mavexa", "go-http", "go-chi", "rails", "next-app"] {
            let r = run(name, None, &hide());
            assert!(!r.out.contains(';') && !r.out.contains('\u{2014}'), "{}", r.out);
        }
    }
}
