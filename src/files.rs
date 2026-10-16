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
