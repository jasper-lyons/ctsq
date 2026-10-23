//! `ctsq refs NAME... [PATH]...`: every identifier spelled NAME. `build`
//! returns the rows, `render` formats them the way the command prints.

use crate::compiler::{self, lang};
use crate::files;
use crate::footer;
use crate::miss;
use crate::needle;
use crate::parsed;
use crate::query;
use crate::query_run::{fetch, hide_rows, lacks_needles, with_path};
use crate::resolve::{self, NamesOut};
use crate::runner;
use crate::scope;
use std::collections::HashMap;

/// One reference.
pub struct RefRow {
    pub file: String,
    pub lang: &'static str,
    /// The match: `line`, `start_byte` and `node_text` (`<match>:LINE: text`).
    pub row: runner::MatchRow,
    /// The file's text, for scope rows.
    pub source: parsed::Source,
}

/// One name's references, in file order.
pub struct NameRefs {
    pub name: String,
    pub rows: Vec<RefRow>,
}

/// What `refs` found. Rows past `--max-matches` are already left out and
/// counted by the cap.
pub struct Refs {
    pub names: Vec<NameRefs>,
    /// Names with no reference shown and none hidden.
    pub missing: Vec<String>,
    /// Read and runner errors, each a whole stderr line.
    pub errors: Vec<String>,
    pub hidden: footer::Hidden,
}

impl Refs {
    pub fn shown(&self) -> usize {
        self.names.iter().map(|n| n.rows.len()).sum()
    }
}

/// The references of each name across `jobs`. An error is the whole output a
/// call that cannot start gives: a name that is not a query (code 2), or no
/// file to search (code 1).
pub fn build(names: &[String], jobs: &[(String, &'static str)], hider: &mut files::Hider) -> Result<Refs, NamesOut> {
    let mut asts = Vec::new();
    for name in names {
        let query_str = format!("id#{}", name);
        match query::parse(&query_str) {
            Ok(q) => asts.push(q),
            Err(e) => return Err(NamesOut { out: String::new(), err: miss::query_error_report(&query_str, &e), code: 2 }),
        }
    }
    if jobs.is_empty() {
        return Err(NamesOut { out: String::new(), err: resolve::not_found_line(names, jobs), code: 1 });
    }
    let mut r = Refs { names: Vec::new(), missing: Vec::new(), errors: Vec::new(), hidden: footer::Hidden::default() };
    for (name, ast) in names.iter().zip(&asts) {
        let mut name_hidden = footer::Hidden::default();
        let mut rows = Vec::new();
        let mut compiled_cache: HashMap<&'static str, (Box<dyn lang::Lang>, compiler::CompiledQuery)> = HashMap::new();
        let needles = needle::Needles::from_query(ast);
        for (file, lang_name) in jobs {
            if lacks_needles(&needles, file) {
                continue;
            }
            let (lang_box, compiled) = compiled_cache.entry(lang_name).or_insert_with(|| {
                let l = lang::for_name(lang_name).expect("lang_name always valid here");
                let c = compiler::compile(ast, l.as_ref());
                (l, c)
            });
            let fetched = match fetch(file, lang_name, compiled.is_empty()) {
                Ok(f) => f,
                Err(e) => {
                    r.errors.push(format!("Failed to read {}: {}", file, e));
                    continue;
                }
            };
            let source = fetched.source();
            match fetched.rows(compiled, lang_box.as_ref(), false) {
                Ok(found) => {
                    for m in hide_rows(hider, file, source, fetched.tree(), found, false, &mut name_hidden) {
                        if !footer::take(file) {
                            continue;
                        }
                        rows.push(RefRow { file: file.clone(), lang: lang_name, row: m, source: fetched.shared_source() });
                    }
                }
                Err(e) => r.errors.push(format!("Runner error in {}: {}", file, e)),
            }
        }
        if rows.is_empty() && !name_hidden.any() {
            r.missing.push(name.clone());
        }
        resolve::merge_hidden(&mut r.hidden, &name_hidden);
        r.names.push(NameRefs { name: name.clone(), rows });
    }
    Ok(r)
}

/// The text `refs` prints: a `path:LINE: text` row per reference, its scope
/// row under it with `scope`, then the count and footer. Stdout is empty when
/// nothing was shown, and the code is 1 then.
pub fn render(r: &Refs, where_: &str, jobs: &[(String, &'static str)], scope_opts: Option<scope::Opts>) -> NamesOut {
    let mut out = String::new();
    let mut err = String::new();
    for e in &r.errors {
        err.push_str(e);
        err.push('\n');
    }
    let mut scopes = scope::Scopes::new();
    for n in &r.names {
        for x in &n.rows {
            out.push_str(&with_path(&x.file, &x.row.node_text));
            out.push('\n');
            if let Some(o) = scope_opts {
                if let Some(s) = scopes.hit(&x.file, x.lang, &x.source, x.row.start_byte) {
                    out.push_str(&scope::render(&s, &x.source, o.body, ""));
                }
            }
        }
    }
    let shown = r.shown();
    if shown == 0 && r.hidden.any() {
        let names: Vec<&str> = r.names.iter().map(|n| n.name.as_str()).collect();
        err.push_str(&footer::nothing_shown(&format!("no reference to {}", names.join(", ")), where_, jobs.len(), &r.hidden));
    } else if shown > 0 {
        out.push_str(&footer::tail(shown, "reference", &r.hidden, scope_opts.is_some()));
    }
    err.push_str(&resolve::not_found_line(&r.missing, jobs));
    NamesOut { out, err, code: if shown == 0 { 1 } else { 0 } }
}

/// `build`, the cap settled into the footer, then `render`.
pub fn report(
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    hider: &mut files::Hider,
    scope_opts: Option<scope::Opts>,
) -> NamesOut {
    match build(names, jobs, hider) {
        Ok(mut r) => {
            footer::settle(&mut r.hidden);
            render(&r, where_, jobs, scope_opts)
        }
        Err(o) => o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden_fixture(tests: bool) -> (Vec<(String, &'static str)>, files::Hider) {
        let root = "fixtures/hidden";
        let jobs = files::resolve_jobs(root, None, true);
        let mut hider = files::Hider::new(&files::HideOpts { tests, exclude: vec![] });
        hider.add_jobs(root, &jobs);
        (jobs, hider)
    }

    #[test]
    fn rows_name_each_reference_and_a_test_file_counts_every_row() {
        let (jobs, mut hider) = hidden_fixture(false);
        let names = vec!["HiddenLimit".to_string(), "Nowhere".to_string()];
        let r = build(&names, &jobs, &mut hider).ok().expect("refs built");
        let rows: Vec<(&str, usize)> = r.names[0].rows.iter().map(|x| (x.file.as_str(), x.row.line)).collect();
        assert_eq!(rows, vec![("fixtures/hidden/limit.go", 5), ("fixtures/hidden/limit.go", 7)]);
        assert!(r.names[1].rows.is_empty());
        assert_eq!(r.missing, vec!["Nowhere".to_string()]);
        // one row per line in limit_test.go, every row counts
        assert_eq!(r.hidden.tests, 2);
        let out = render(&r, "fixtures/hidden", &jobs, None);
        assert_eq!(out.code, 0);
        assert!(out.out.starts_with("fixtures/hidden/limit.go:5: \"HiddenLimit\"\n"), "{}", out.out);
        assert!(out.err.starts_with("not found: Nowhere"), "{}", out.err);
    }

    #[test]
    fn a_name_that_is_not_a_query_is_a_usage_error() {
        let (jobs, mut hider) = hidden_fixture(true);
        let e = build(&["a)".to_string()], &jobs, &mut hider).err().expect("bad name");
        assert_eq!((e.code, e.out.as_str()), (2, ""));
    }
}
