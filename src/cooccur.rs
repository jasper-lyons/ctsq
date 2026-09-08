//! `files` — which files satisfy every one of several queries at once.
//!
//! This is the one shape `grep -rl X | xargs grep -l Y` had and ctsq did not.
//! Nothing new runs here: each query goes through the normal compile-and-run
//! path, per file, and a file is reported only when every query matched in it.

use crate::compiler::{self, lang};
use crate::files;
use crate::query;
use crate::runner;
use std::collections::HashMap;

/// Files where every query in `queries` matches at least once, each with the
/// per-query match count in query order.
pub fn matching_files(
    queries: &[String],
    jobs: &[(String, &'static str)],
) -> Result<Vec<(String, Vec<usize>)>, String> {
    let mut asts = Vec::new();
    for q in queries {
        asts.push(query::parse(q).map_err(|e| format!("Parse error in '{}': {}", q, e))?);
    }

    // (lang, query index) -> compiled. Compiling is the expensive part and a
    // directory usually holds many files of the same language.
    let mut cache: HashMap<(&'static str, usize), compiler::CompiledQuery> = HashMap::new();
    let mut langs: HashMap<&'static str, Box<dyn lang::Lang>> = HashMap::new();

    let mut out = Vec::new();
    for (file, lang_name) in jobs {
        let lang_box = langs
            .entry(lang_name)
            .or_insert_with(|| lang::for_name(lang_name).expect("lang_name always valid here"));
        let Ok(source) = std::fs::read(file) else { continue };

        let mut counts = Vec::with_capacity(asts.len());
        for (i, ast) in asts.iter().enumerate() {
            let compiled = cache
                .entry((lang_name, i))
                .or_insert_with(|| compiler::compile(ast, lang_box.as_ref()));
            let n = runner::run(compiled, lang_box.as_ref(), &source, false)
                .map(|m| m.len())
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            counts.push(n);
        }
        if counts.len() == asts.len() {
            out.push((file.clone(), counts));
        }
    }
    Ok(out)
}

pub fn run(queries: &[String], path: &str, lang_name: Option<&str>, no_ignore: bool) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let hits = match matching_files(queries, &jobs) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    if hits.is_empty() {
        println!(
            "no file matches all {} queries in {} ({} files scanned)",
            queries.len(),
            path,
            jobs.len()
        );
        std::process::exit(1);
    }

    for (file, counts) in hits {
        let counts: Vec<String> = counts.iter().map(|c| c.to_string()).collect();
        println!("{}  {}", file, counts.join(" "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures(sub: &str) -> String {
        format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), sub)
    }

    fn jobs_for(sub: &str) -> Vec<(String, &'static str)> {
        files::resolve_jobs(&fixtures(sub), None, false)
    }

    #[test]
    fn a_file_needs_every_query_to_be_listed() {
        let jobs = jobs_for("example.go");
        let both = vec!["id#NewServer".to_string(), "id#Start".to_string()];
        let hits = matching_files(&both, &jobs).unwrap();
        assert_eq!(hits.len(), 1, "both names live in example.go: {:?}", hits);

        let one_missing = vec!["id#NewServer".to_string(), "id#not_in_this_file".to_string()];
        let hits = matching_files(&one_missing, &jobs).unwrap();
        assert!(hits.is_empty(), "one query missing must exclude the file: {:?}", hits);
    }

    #[test]
    fn counts_are_reported_per_query_in_order() {
        let jobs = jobs_for("example.go");
        let qs = vec!["id#NewServer".to_string(), "id#host".to_string()];
        let hits = matching_files(&qs, &jobs).unwrap();
        assert_eq!(hits.len(), 1, "{:?}", hits);
        let (_, counts) = &hits[0];
        assert_eq!(counts.len(), 2, "one count per query: {:?}", counts);
        assert!(counts.iter().all(|c| *c > 0), "{:?}", counts);
    }

    #[test]
    fn a_directory_narrows_to_the_files_holding_all_the_names() {
        let jobs = jobs_for(".");
        assert!(jobs.len() > 1, "fixture directory should hold several files");
        let qs = vec!["id#NewServer".to_string(), "id#Start".to_string()];
        let hits = matching_files(&qs, &jobs).unwrap();
        assert!(
            hits.iter().all(|(f, _)| f.ends_with(".go")),
            "only the Go fixture holds both: {:?}",
            hits
        );
    }

    #[test]
    fn a_bad_query_is_an_error_not_a_panic() {
        let jobs = jobs_for("example.go");
        assert!(matching_files(&["(((".to_string()], &jobs).is_err());
    }
}
