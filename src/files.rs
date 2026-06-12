use crate::compiler::lang;
use ignore::WalkBuilder;

pub fn collect_files(dir: &str, exts: &[&str], no_ignore: bool) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(dir)
        .git_ignore(!no_ignore)
        .git_global(!no_ignore)
        .git_exclude(!no_ignore)
        .ignore(!no_ignore)
        .hidden(false)
        .build();
    for result in walker {
        if let Ok(entry) = result {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if exts.contains(&ext) {
                        if let Some(s) = path.to_str() {
                            files.push(s.to_string());
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
pub fn resolve_jobs(path: &str, lang_name: Option<&str>, no_ignore: bool) -> Vec<(String, &'static str)> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => { eprintln!("Failed to access {}: {}", path, e); std::process::exit(1); }
    };
    if meta.is_dir() {
        let exts = lang_name.map(lang::extensions_for_name)
            .unwrap_or_else(lang::all_known_extensions);
        collect_files(path, exts, no_ignore).into_iter().filter_map(|f| {
            let ext = std::path::Path::new(&f).extension()
                .and_then(|e| e.to_str()).unwrap_or("");
            let ln = lang_name.and_then(lang::lang_for_name_canonical)
                .or_else(|| lang::lang_for_extension(ext))?;
            Some((f, ln))
        }).collect()
    } else {
        let ln = lang_name.and_then(lang::lang_for_name_canonical)
            .or_else(|| {
                let ext = std::path::Path::new(path).extension()
                    .and_then(|e| e.to_str()).unwrap_or("");
                lang::lang_for_extension(ext)
            });
        match ln {
            Some(ln) => vec![(path.to_string(), ln)],
            None => { eprintln!("Cannot detect language; use --lang"); std::process::exit(1); }
        }
    }
}
