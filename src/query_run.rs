//! The `-q` search without its printing. A `Run` compiles the query once per
//! language and answers one file at a time with the rows that survive the
//! attribute filters and test hiding. `main` prints them.

use crate::compiler::{self, lang};
use crate::files;
use crate::footer;
use crate::needle;
use crate::parsed;
use crate::query;
use crate::runner;
use crate::semantic;
use std::collections::HashMap;

/// A file's text and, when the query has patterns, its tree: from the run's
/// cache for a file, unparsed text for stdin.
pub enum Fetched {
    File(std::rc::Rc<parsed::ParsedFile>),
    /// Read but not parsed: stdin, or a query with no pattern for the language.
    Text(parsed::Source),
}

impl Fetched {
    pub fn source(&self) -> &[u8] {
        match self {
            Fetched::File(p) => &p.source,
            Fetched::Text(s) => s,
        }
    }

    /// The source as a shared handle, so a row can keep it.
    pub fn shared_source(&self) -> parsed::Source {
        match self {
            Fetched::File(p) => p.source.clone(),
            Fetched::Text(s) => s.clone(),
        }
    }

    pub fn tree(&self) -> Option<&tree_sitter::Tree> {
        match self {
            Fetched::File(p) => p.tree.as_ref(),
            Fetched::Text(_) => None,
        }
    }

    /// `runner::run_rows`, over the cached tree when there is one.
    pub fn rows(&self, compiled: &compiler::CompiledQuery, l: &dyn lang::Lang, body: bool) -> Result<Vec<runner::MatchRow>, String> {
        match self.tree() {
            Some(t) => runner::run_rows_on(compiled, l, self.source(), t, body),
            None => runner::run_rows(compiled, l, self.source(), body),
        }
    }
}

/// `file` from the cache, parsed with `lang_name`'s grammar unless `text_only`
/// (a query with no pattern matches nothing and needs no tree).
pub fn fetch(file: &str, lang_name: &'static str, text_only: bool) -> std::io::Result<Fetched> {
    let source = parsed::try_source(file)?;
    if text_only {
        return Ok(Fetched::Text(source));
    }
    Ok(match parsed::get(file, lang_name) {
        Some(p) => Fetched::File(p),
        None => Fetched::Text(source),
    })
}

/// Whether `file` is readable and lacks the bytes of `needles`, so a query
/// cannot match in it. Stdin and unreadable files are not skipped, the loop
/// reports the unreadable ones.
pub fn lacks_needles(needles: &needle::Needles, file: &str) -> bool {
    if needles.is_none() || file == "-" {
        return false;
    }
    match parsed::try_source(file) {
        Ok(src) => !needles.holds(&src),
        Err(_) => false,
    }
}

/// Drop the rows `hider` hides, counting them in `hidden`. With `per_line` a
/// hidden row counts once per line, the way `-q` prints one line per match.
/// Without it every row counts, the way `refs` prints every row. `tree` is
/// the file's parse when the caller holds it, so test scopes reuse it.
pub fn hide_rows(
    hider: &mut files::Hider,
    file: &str,
    source: &[u8],
    tree: Option<&tree_sitter::Tree>,
    rows: Vec<runner::MatchRow>,
    per_line: bool,
    hidden: &mut footer::Hidden,
) -> Vec<runner::MatchRow> {
    if rows.is_empty() {
        return rows;
    }
    let count = |cut: &[usize]| {
        if per_line {
            cut.iter().collect::<std::collections::HashSet<_>>().len()
        } else {
            cut.len()
        }
    };
    if let Some(why) = hider.file(file) {
        let lines: Vec<usize> = rows.iter().map(|r| r.line).collect();
        hidden.count(why, count(&lines));
        return vec![];
    }
    let scopes = hider.scope_lines_in(file, Some(source), tree);
    if scopes.is_empty() {
        return rows;
    }
    let mut cut = Vec::new();
    let kept = rows
        .into_iter()
        .filter(|r| {
            let inside = scopes.iter().any(|(a, b)| *a <= r.line && r.line <= *b);
            if inside {
                cut.push(r.line);
            }
            !inside
        })
        .collect();
    hidden.count(files::Hide::Test, count(&cut));
    kept
}

/// The node-text form has to say which file it came from, or a multi-line hit
/// sends the reader back to the file to find it. `<match>` is a placeholder for
/// exactly that, so the path replaces it. The capture form (`@f:4: …, @c:6: …`)
/// has no slot of its own and takes the path as a prefix.
pub fn with_path(display: &str, node_text: &str) -> String {
    match node_text.strip_prefix("<match>") {
        Some(rest) => format!("{}{}", display, rest),
        None => format!("{}:{}", display, node_text),
    }
}

/// Whether a sibling pair `A ~ B` cannot match in `fetched`, because A or B has no match alone.
fn gated_out(fetched: &Fetched, gate: Option<&(compiler::CompiledQuery, compiler::CompiledQuery)>, l: &dyn lang::Lang) -> bool {
    match (fetched.tree(), gate) {
        (Some(t), Some((a, b))) => !runner::has_match(a, l, fetched.source(), t) || !runner::has_match(b, l, fetched.source(), t),
        _ => false,
    }
}

/// What one file gave the query.
pub enum Outcome {
    /// The rows left after attribute filters and hiding, with the file's text
    /// for printing source lines, context and scopes. Empty when nothing matched.
    Rows { source: parsed::Source, rows: Vec<runner::MatchRow> },
    /// The file could not be read: the error as `Failed to read FILE: ERROR` shows it.
    ReadError(String),
    /// The query failed on the file: the error as `Runner error in FILE: ERROR` shows it.
    RunnerError(String),
}

/// One file's answer.
pub struct FileRows {
    /// The file as given, `-` for stdin.
    pub file: String,
    pub lang: &'static str,
    pub outcome: Outcome,
}

impl FileRows {
    /// The path rows print with: `<stdin>` for `-`.
    pub fn display(&self) -> &str {
        if self.file == "-" { "<stdin>" } else { self.file.as_str() }
    }
}

type Compiled = (Box<dyn lang::Lang>, compiler::CompiledQuery, Option<(compiler::CompiledQuery, compiler::CompiledQuery)>);

/// One `-q` run over files given one at a time. The query must already have
/// passed `compiler::validate` and its checks for every language it meets.
pub struct Run<'a> {
    ast: &'a query::ast::Query,
    attrs: &'a [query::ast::AttrMatch],
    body: bool,
    needles: needle::Needles,
    compiled: HashMap<&'static str, Compiled>,
    stdin: Option<parsed::Source>,
    /// What hiding dropped so far.
    pub hidden: footer::Hidden,
}

impl<'a> Run<'a> {
    /// `every_file` keeps files that lack the query's names (`--show-query`
    /// prints for each). `stdin` is the text read for a `-` file.
    pub fn new(
        ast: &'a query::ast::Query,
        attrs: &'a [query::ast::AttrMatch],
        body: bool,
        every_file: bool,
        stdin: Option<Vec<u8>>,
    ) -> Run<'a> {
        // A file without the query's names cannot match, so it is never parsed.
        let needles = if every_file { needle::Needles::none() } else { needle::Needles::from_query(ast) };
        Run { ast, attrs, body, needles, compiled: HashMap::new(), stdin: stdin.map(std::rc::Rc::new), hidden: footer::Hidden::default() }
    }

    /// The query compiled for `lang_name`.
    pub fn compiled(&mut self, lang_name: &'static str) -> &compiler::CompiledQuery {
        &self.entry(lang_name).1
    }

    fn entry(&mut self, lang_name: &'static str) -> &Compiled {
        let ast = self.ast;
        self.compiled.entry(lang_name).or_insert_with(|| {
            let l = lang::for_name(lang_name).expect("lang_name always valid here");
            let c = compiler::compile(ast, l.as_ref());
            let g = compiler::sibling_gate(ast, l.as_ref());
            (l, c, g)
        })
    }

    /// The rows of one file, or None when the file lacks the query's names
    /// and was skipped unread.
    pub fn file(&mut self, hider: &mut files::Hider, file: &str, lang_name: &'static str) -> Option<FileRows> {
        if lacks_needles(&self.needles, file) {
            return None;
        }
        let body = self.body;
        let attrs = self.attrs;
        let stdin = self.stdin.clone();
        self.entry(lang_name);
        let (lang_box, compiled, gate) = &self.compiled[lang_name];
        // A file's text and tree come from the run's cache, so test hiding
        // and anything after reuse this parse. Stdin has no path to key on.
        let fetched = if file == "-" {
            Fetched::Text(stdin.expect("stdin pre-read"))
        } else {
            match fetch(file, lang_name, compiled.is_empty()) {
                Ok(f) => f,
                Err(e) => return Some(FileRows { file: file.to_string(), lang: lang_name, outcome: Outcome::ReadError(e.to_string()) }),
            }
        };
        let source = fetched.source();
        let result = (if gated_out(&fetched, gate.as_ref(), lang_box.as_ref()) { Ok(vec![]) } else { fetched.rows(compiled, lang_box.as_ref(), body) }).map(|rows| {
            let rows = semantic::attrs::filter_rows(rows, attrs, lang_name, &lang_box.ts_language(), source);
            hide_rows(hider, file, source, fetched.tree(), rows, true, &mut self.hidden)
        });
        let outcome = match result {
            Ok(rows) => Outcome::Rows { source: fetched.shared_source(), rows },
            Err(e) => Outcome::RunnerError(e),
        };
        Some(FileRows { file: file.to_string(), lang: lang_name, outcome })
    }
}

/// Every file's rows for `ast` over `jobs`, with what hiding dropped. Files
/// that lack the query's names are left out.
pub fn rows(
    ast: &query::ast::Query,
    attrs: &[query::ast::AttrMatch],
    jobs: &[(String, &'static str)],
    hider: &mut files::Hider,
    body: bool,
) -> (Vec<FileRows>, footer::Hidden) {
    let mut run = Run::new(ast, attrs, body, false, None);
    let files: Vec<FileRows> = jobs.iter().filter_map(|(f, l)| run.file(hider, f, l)).collect();
    (files, run.hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_back_per_file_and_a_test_file_is_hidden_once_per_line() {
        let root = "fixtures/hidden";
        let jobs = files::resolve_jobs(root, None, true);
        let mut hider = files::Hider::new(&Default::default());
        hider.add_jobs(root, &jobs);
        let ast = query::parse("id#HiddenLimit").unwrap();
        let (got, hidden) = rows(&ast, &[], &jobs, &mut hider, false);
        // lib.rs lacks the name and is never read
        assert!(got.iter().all(|f| !f.file.ends_with("lib.rs")));
        let main = got.iter().find(|f| f.file.ends_with("/limit.go")).expect("limit.go answered");
        assert_eq!((main.display(), main.lang), ("fixtures/hidden/limit.go", "go"));
        let Outcome::Rows { rows, .. } = &main.outcome else { panic!("limit.go has rows") };
        let lines: Vec<(usize, Option<&str>)> = rows.iter().map(|r| (r.line, r.src_line.as_deref())).collect();
        assert_eq!(lines, vec![(5, Some("const HiddenLimit = 3")), (7, Some("var doubled = HiddenLimit * 2"))]);
        let test = got.iter().find(|f| f.file.ends_with("limit_test.go")).expect("test file answered");
        assert!(matches!(&test.outcome, Outcome::Rows { rows, .. } if rows.is_empty()));
        // three hits on two lines
        assert_eq!(hidden.tests, 2);
        assert_eq!(with_path("a.go", "<match>:3: x"), "a.go:3: x");
    }
}
