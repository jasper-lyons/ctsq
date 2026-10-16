//! Named queries, read from `queries/<framework>.toml` files in the same
//! layers as language mechanics: repo `.ctsq/`, then config, then cache, then
//! built-in. A query a higher layer defines for the same framework replaces the
//! lower one's query of the same name. The other queries of the lower file stay.
//!
//! File shape:
//!
//! ```toml
//! framework = "react-router"
//! langs     = ["tsx", "ts", "jsx", "js"]
//! detect    = { import = ["react-router", "react-router-dom"] }
//!
//! [route]
//! match  = ["<selector>", "<selector>"]   # one selector or several
//! target = "def"                          # def, enclosing or self
//! ```
//!
//! `rewrite`, `paths` and `key_from_path` (query keys) and the `default-export`
//! target are recognised and rejected as not supported yet, so a file written
//! for a later ctsq says what is missing rather than failing on an unknown key.

use super::{ConfigError, Roots, BUILTIN_QUERIES};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a match points at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// Resolve the `@target` capture's name with `def`.
    Def,
    /// The nearest named scope holding the match.
    Enclosing,
    /// The match itself (its `@target` capture when it has one).
    Match,
}

/// When a framework's queries apply. Empty lists mean no rule of that kind.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detect {
    /// Any file of the query's languages imports one of these.
    pub import: Vec<String>,
    /// One of these paths exists under the search root.
    pub file: Vec<String>,
    /// Named in package.json, go.mod or Gemfile at the search root.
    pub dependency: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedQuery {
    /// The table name: `route`, `text`, `error`.
    pub name: String,
    pub framework: String,
    /// Canonical language names (`typescript`, `tsx`, `javascript`).
    pub langs: Vec<String>,
    /// None: applies whenever files of `langs` exist.
    pub detect: Option<Detect>,
    /// Selectors, each run on its own. A row from any of them is a match.
    pub matches: Vec<String>,
    pub target: Target,
    /// The file as printed: a path for a file on disk, or
    /// `built-in queries/<file>.toml`.
    pub source: String,
    /// 1-based line of the query's `[name]` table in the file.
    pub line: usize,
}

/// Keys a query table may hold that a later ctsq fills in.
const NOT_YET: &[&str] = &["rewrite", "paths", "key_from_path"];

fn err(label: &str, line: Option<usize>, key: Option<&str>, msg: &str) -> ConfigError {
    ConfigError::new(label, line, key.map(str::to_string), msg)
}

/// A string or an array of strings, as a list.
fn strings(label: &str, key: &str, v: &toml::Value) -> Result<Vec<String>, ConfigError> {
    match v {
        toml::Value::String(s) => Ok(vec![s.clone()]),
        toml::Value::Array(a) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| err(label, None, Some(key), "expected a string or a list of strings"))
            })
            .collect(),
        _ => Err(err(label, None, Some(key), "expected a string or a list of strings")),
    }
}

/// The line a `[name]` table header sits on, 1-based. 1 when not found.
fn table_line(text: &str, name: &str) -> usize {
    let quoted = format!("[\"{}\"]", name);
    let plain = format!("[{}]", name);
    text.lines()
        .position(|l| {
            let t = l.split('#').next().unwrap_or("").trim();
            t == plain || t == quoted
        })
        .map(|i| i + 1)
        .unwrap_or(1)
}

fn canonical_lang(name: &str) -> Option<&'static str> {
    match name {
        "jsx" => Some("javascript"),
        other => crate::compiler::lang::lang_for_name_canonical(other),
    }
}

/// Every named query in one file. `label` is the file as it will be printed.
pub fn parse_file(label: &str, text: &str) -> Result<Vec<NamedQuery>, ConfigError> {
    let table = super::parse_table(label, text)?;
    let framework = match table.get("framework") {
        Some(toml::Value::String(s)) if !s.is_empty() => s.clone(),
        Some(_) => return Err(err(label, None, Some("framework"), "expected a non-empty string")),
        None => return Err(err(label, None, None, "missing `framework = \"<name>\"`")),
    };
    let langs = match table.get("langs") {
        Some(v) => strings(label, "langs", v)?,
        None => return Err(err(label, None, None, "missing `langs = [...]`")),
    };
    if langs.is_empty() {
        return Err(err(label, None, Some("langs"), "needs at least one language"));
    }
    let mut canon: Vec<String> = Vec::new();
    for l in &langs {
        match canonical_lang(l) {
            Some(c) => {
                if !canon.iter().any(|x| x == c) {
                    canon.push(c.to_string());
                }
            }
            None => return Err(err(label, None, Some("langs"), &format!("unknown language \"{}\"", l))),
        }
    }
    let detect = match table.get("detect") {
        None => None,
        Some(toml::Value::Table(t)) => {
            let mut d = Detect::default();
            for (k, v) in t {
                let list = strings(label, &format!("detect.{}", k), v)?;
                match k.as_str() {
                    "import" => d.import = list,
                    "file" => d.file = list,
                    "dependency" => d.dependency = list,
                    other => {
                        return Err(err(
                            label,
                            None,
                            Some("detect"),
                            &format!("unknown detect rule `{}`, the rules are import, file and dependency", other),
                        ))
                    }
                }
            }
            Some(d)
        }
        Some(_) => return Err(err(label, None, Some("detect"), "expected a table such as { import = [\"x\"] }")),
    };

    let mut out = Vec::new();
    for (name, v) in &table {
        if matches!(name.as_str(), "framework" | "langs" | "detect") {
            continue;
        }
        let line = table_line(text, name);
        let toml::Value::Table(q) = v else {
            return Err(err(
                label,
                Some(line),
                Some(name),
                "unknown key, a file holds framework, langs, detect and one [table] per query",
            ));
        };
        let mut matches = Vec::new();
        let mut target = None;
        for (k, v) in q {
            match k.as_str() {
                "match" => matches = strings(label, name, v)?,
                "target" => {
                    target = Some(match v.as_str() {
                        Some("def") => Target::Def,
                        Some("enclosing") => Target::Enclosing,
                        Some("self") => Target::Match,
                        Some("default-export") => {
                            return Err(err(label, Some(line), Some(name), "target \"default-export\" is not supported yet"))
                        }
                        _ => {
                            return Err(err(label, Some(line), Some(name), "target must be \"def\", \"enclosing\" or \"self\""))
                        }
                    })
                }
                k if NOT_YET.contains(&k) => {
                    return Err(err(label, Some(line), Some(name), &format!("`{}` is not supported yet", k)))
                }
                other => {
                    return Err(err(
                        label,
                        Some(line),
                        Some(name),
                        &format!("unknown key `{}`, a query holds match and target", other),
                    ))
                }
            }
        }
        if matches.is_empty() {
            return Err(err(label, Some(line), Some(name), "missing `match = \"<selector>\"`"));
        }
        for m in &matches {
            if let Err(e) = crate::query::parse(m) {
                return Err(err(label, Some(line), Some(name), &format!("match `{}`: {}", m, e)));
            }
        }
        let Some(target) = target else {
            return Err(err(label, Some(line), Some(name), "missing `target`, one of \"def\", \"enclosing\" or \"self\""));
        };
        out.push(NamedQuery {
            name: name.clone(),
            framework: framework.clone(),
            langs: canon.clone(),
            detect: detect.clone(),
            matches,
            target,
            source: label.to_string(),
            line,
        });
    }
    Ok(out)
}

/// The `queries/` directory of each on-disk layer, highest priority first.
pub fn dirs(roots: &Roots) -> Vec<PathBuf> {
    roots.ordered().into_iter().map(|r| r.join("queries")).collect()
}

/// A path as printed: relative to `cwd` when it is under it.
fn shown(p: &Path, cwd: &Path) -> String {
    p.strip_prefix(cwd).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

/// Every named query, merged over the layers. Sorted by query name, then
/// framework, so listings are stable.
pub fn load_with(roots: &Roots, cwd: &Path) -> Result<Vec<NamedQuery>, ConfigError> {
    let mut merged: BTreeMap<(String, String), NamedQuery> = BTreeMap::new();
    let mut absorb = |qs: Vec<NamedQuery>| {
        for q in qs {
            merged.insert((q.name.clone(), q.framework.clone()), q);
        }
    };
    for (file, text) in BUILTIN_QUERIES {
        absorb(parse_file(&format!("built-in queries/{}", file), text)?);
    }
    for dir in dirs(roots).into_iter().rev() {
        for p in super::list_toml(&dir) {
            let text = super::read(&p)?;
            absorb(parse_file(&shown(&p, cwd), &text)?);
        }
    }
    Ok(merged.into_values().collect())
}

/// `load_with` with roots from the environment and the repo root above `cwd`.
pub fn load(cwd: &Path) -> Result<Vec<NamedQuery>, ConfigError> {
    let root = super::find_repo_root(cwd);
    load_with(&Roots::from_env(&root), cwd)
}

/// The `queries:` block of `ctsq config path`.
pub fn path_report(roots: &Roots) -> String {
    let show = |p: Option<&PathBuf>| match p {
        Some(p) => p.join("queries").display().to_string(),
        None => "(unavailable, HOME is not set)".to_string(),
    };
    let builtin: Vec<&str> = BUILTIN_QUERIES.iter().map(|(f, _)| *f).collect();
    format!(
        "queries:\n1. repo      {}\n2. config    {}\n3. cache     {}\n4. built-in  {}\n",
        show(roots.repo.as_ref()),
        show(roots.config.as_ref()),
        show(roots.cache.as_ref()),
        builtin.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-queries-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const FILE: &str = "framework = \"demo\"\nlangs = [\"ts\", \"jsx\"]\ndetect = { import = \"demo-lib\" }\n\n# routes\n[route]\nmatch = \"(pair.value(string @key))\"\ntarget = \"self\"\n\n[label]\nmatch = [\"(string @key)\", \"(id @key)\"]\ntarget = \"enclosing\"\n";

    #[test]
    fn a_file_gives_one_query_per_table_with_its_line() {
        let qs = parse_file("x.toml", FILE).unwrap();
        assert_eq!(qs.len(), 2);
        let route = qs.iter().find(|q| q.name == "route").unwrap();
        assert_eq!(route.framework, "demo");
        assert_eq!(route.langs, vec!["typescript", "javascript"], "aliases are canonical");
        assert_eq!(route.detect.as_ref().unwrap().import, vec!["demo-lib"]);
        assert_eq!(route.target, Target::Match);
        assert_eq!(route.line, 6);
        let label = qs.iter().find(|q| q.name == "label").unwrap();
        assert_eq!(label.matches.len(), 2);
        assert_eq!(label.target, Target::Enclosing);
        assert_eq!(label.line, 10);
    }

    #[test]
    fn later_fields_are_named_as_not_supported_yet() {
        for key in ["rewrite = [\"a\", \"b\"]", "paths = \"app/**\"", "key_from_path = \"x\""] {
            let text = format!("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"(id @key)\"\ntarget = \"def\"\n{}\n", key);
            let e = parse_file("f.toml", &text).unwrap_err();
            assert!(e.message.contains("not supported yet"), "{}: {}", key, e);
            assert_eq!(e.line, Some(3), "{}", e);
        }
        let e = parse_file("f.toml", "framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id\"\ntarget = \"default-export\"\n").unwrap_err();
        assert!(e.message.contains("not supported yet"), "{}", e);
    }

    #[test]
    fn clearly_wrong_files_are_errors_that_name_the_problem() {
        let cases = [
            ("langs = [\"ts\"]\n", "framework"),
            ("framework = \"f\"\n", "langs"),
            ("framework = \"f\"\nlangs = [\"cobol\"]\n", "cobol"),
            ("framework = \"f\"\nlangs = [\"ts\"]\ndetect = { imports = [\"x\"] }\n", "imports"),
            ("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id\"\n", "target"),
            ("framework = \"f\"\nlangs = [\"ts\"]\n[route]\ntarget = \"def\"\n", "match"),
            ("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id\"\ntarget = \"up\"\n", "target must be"),
            ("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id\"\ntarget = \"def\"\nmatchs = 1\n", "matchs"),
            ("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id#(\"\ntarget = \"def\"\n", "expected a name"),
            ("framework = \"f\"\nlangs = [\"ts\"]\nstray = 1\n", "unknown key"),
        ];
        for (text, want) in cases {
            let e = parse_file("f.toml", text).unwrap_err();
            assert!(e.to_string().contains(want), "{:?}: {}", text, e);
        }
    }

    #[test]
    fn a_higher_layer_replaces_the_same_query_of_the_same_framework_only() {
        let d = tmp();
        let roots = Roots::from_vars(&d, Some(d.join("home").to_str().unwrap()), None, None);
        let cache = roots.cache.clone().unwrap();
        write(&cache.join("queries/demo.toml"), FILE);
        write(
            &d.join(".ctsq/queries/mine.toml"),
            "framework = \"demo\"\nlangs = [\"tsx\"]\n\n[route]\nmatch = \"(id @key)\"\ntarget = \"def\"\n",
        );
        let qs = load_with(&roots, &d).unwrap();
        let demo: Vec<&NamedQuery> = qs.iter().filter(|q| q.framework == "demo").collect();
        assert_eq!(demo.len(), 2, "{:?}", demo);
        let route = demo.iter().find(|q| q.name == "route").unwrap();
        assert_eq!(route.source, ".ctsq/queries/mine.toml", "relative to the search root");
        assert_eq!(route.langs, vec!["tsx"]);
        assert!(route.detect.is_none());
        let label = demo.iter().find(|q| q.name == "label").unwrap();
        assert!(label.source.ends_with("queries/demo.toml"), "the lower layer keeps its other query: {}", label.source);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn built_in_packs_load_and_name_their_source() {
        let d = tmp();
        let roots = Roots::from_vars(&d, Some(d.join("home").to_str().unwrap()), None, None);
        let qs = load_with(&roots, &d).unwrap();
        let frameworks: Vec<&str> = qs.iter().filter(|q| q.name == "route").map(|q| q.framework.as_str()).collect();
        assert_eq!(frameworks, vec!["react-router", "solid-routes"]);
        assert!(qs.iter().all(|q| q.source.starts_with("built-in queries/")), "{:?}", qs);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_path_report_lists_a_queries_directory_per_layer() {
        let r = Roots::from_vars(Path::new("/r"), Some("/h"), None, None);
        let s = path_report(&r);
        assert!(s.starts_with("queries:\n"), "{}", s);
        assert!(s.contains("/r/.ctsq/queries") && s.contains("/h/.config/ctsq/queries") && s.contains("/h/.cache/ctsq/queries"), "{}", s);
        assert!(s.contains("solid-routes.toml") && s.contains("react-router.toml"), "{}", s);
    }
}
