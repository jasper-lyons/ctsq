#![allow(dead_code)]
//! Language mechanics and outside-call models, loaded from layered TOML files.
//!
//! Lookup order, highest priority first:
//!   1. repo `.ctsq/`
//!   2. `$XDG_CONFIG_HOME/ctsq` else `~/.config/ctsq`
//!   3. `$XDG_CACHE_HOME/ctsq` else `~/.cache/ctsq`
//!   4. built-in defaults compiled into the binary
//! Under each root: `languages/<lang>.toml`, `models/<lang>/*.toml` and
//! `queries/<framework>.toml` (named queries, see `queries`).
//!
//! Public API for later packages:
//!   `load(repo_root, lang) -> Result<LangConfig, ConfigError>`
//!   `load_with(&Roots, repo_root, lang)` for tests and explicit roots
//!   `LangConfig { mechanics, models, go_version, module_path }`
//!   `LangConfig::model(qualified_name) -> Option<&Model>`
//!   `gomod::{module_path, go_version, find_go_mod, is_outside}`

pub mod gomod;
pub mod mechanics;
pub mod models;
pub mod queries;

use gomod::GoVersion;
pub use mechanics::Mechanics;
#[allow(unused_imports)]
pub use models::{Model, Pass, Slot};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// A problem in a config file. Callers print it and exit 2.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub file: String,
    pub line: Option<usize>,
    pub key: Option<String>,
    pub message: String,
}

impl ConfigError {
    pub fn new(file: &str, line: Option<usize>, key: Option<String>, message: &str) -> ConfigError {
        ConfigError { file: file.to_string(), line, key, message: message.to_string() }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(l) = self.line {
            write!(f, ":{}", l)?;
        }
        write!(f, ": ")?;
        if let Some(k) = &self.key {
            write!(f, "[{}] ", k)?;
        }
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

/// The three on-disk roots, highest priority first. None means unavailable.
#[derive(Debug, Clone)]
pub struct Roots {
    pub repo: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub cache: Option<PathBuf>,
}

impl Roots {
    /// Pure constructor, no environment access.
    pub fn from_vars(
        repo_root: &Path,
        home: Option<&str>,
        xdg_config: Option<&str>,
        xdg_cache: Option<&str>,
    ) -> Roots {
        let pick = |xdg: Option<&str>, fallback: &str| -> Option<PathBuf> {
            match xdg.filter(|s| !s.is_empty()) {
                Some(x) => Some(PathBuf::from(x).join("ctsq")),
                None => home.filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join(fallback).join("ctsq")),
            }
        };
        Roots {
            repo: Some(repo_root.join(".ctsq")),
            config: pick(xdg_config, ".config"),
            cache: pick(xdg_cache, ".cache"),
        }
    }

    pub fn from_env(repo_root: &Path) -> Roots {
        let get = |k: &str| std::env::var(k).ok();
        Roots::from_vars(
            repo_root,
            get("HOME").as_deref(),
            get("XDG_CONFIG_HOME").as_deref(),
            get("XDG_CACHE_HOME").as_deref(),
        )
    }

    /// Existing roots in priority order.
    fn ordered(&self) -> Vec<&PathBuf> {
        [&self.repo, &self.config, &self.cache].into_iter().flatten().collect()
    }
}

struct BuiltinModel {
    lang: &'static str,
    label: &'static str,
    version: Option<(u32, u32)>,
    text: &'static str,
}

const BUILTIN_MODELS: &[BuiltinModel] = &[BuiltinModel {
    lang: "go",
    label: "stdlib@1.23.toml",
    version: Some((1, 23)),
    text: include_str!("../../config/models/go/stdlib@1.23.toml"),
}];

const BUILTIN_LANGUAGES: &[(&str, &str)] = &[
    ("go", include_str!("../../config/languages/go.toml")),
    ("gotmpl", include_str!("../../config/languages/gotmpl.toml")),
    ("typescript", include_str!("../../config/languages/typescript.toml")),
    ("tsx", include_str!("../../config/languages/tsx.toml")),
    ("javascript", include_str!("../../config/languages/javascript.toml")),
    ("python", include_str!("../../config/languages/python.toml")),
    ("ruby", include_str!("../../config/languages/ruby.toml")),
    ("rust", include_str!("../../config/languages/rust.toml")),
    ("c", include_str!("../../config/languages/c.toml")),
    ("cpp", include_str!("../../config/languages/cpp.toml")),
];

/// Built-in named queries, `queries/<file>` beside `languages/`.
const BUILTIN_QUERIES: &[(&str, &str)] = &[
    ("go-chi.toml", include_str!("../../config/queries/go-chi.toml")),
    ("go-http.toml", include_str!("../../config/queries/go-http.toml")),
    ("next-app.toml", include_str!("../../config/queries/next-app.toml")),
    ("rails-routes.toml", include_str!("../../config/queries/rails-routes.toml")),
    ("react-router.toml", include_str!("../../config/queries/react-router.toml")),
    ("solid-routes.toml", include_str!("../../config/queries/solid-routes.toml")),
];

/// The merged `[tests]` table for `lang`: built-in, then cache, config and repo
/// layers, higher layers winning per key. Lighter than `load_with`: no go.mod,
/// no models, and no other table is read. A language with no file anywhere has
/// no tests table, which is not an error.
pub fn load_tests_with(roots: &Roots, lang: &str) -> Result<mechanics::Tests, ConfigError> {
    let mut merged = toml::Table::new();
    if let Some((_, text)) = BUILTIN_LANGUAGES.iter().find(|(l, _)| *l == lang) {
        let label = format!("<built-in>/languages/{}.toml", lang);
        if let Some(toml::Value::Table(t)) = parse_table(&label, text)?.get("tests") {
            merge_tables(&mut merged, t);
        }
    }
    if !(lang.is_empty() || lang.contains(['/', '\\']) || lang.starts_with('.')) {
        for root in roots.ordered().into_iter().rev() {
            let p = root.join("languages").join(format!("{}.toml", lang));
            if p.is_file() {
                let label = p.display().to_string();
                if let Some(toml::Value::Table(t)) = parse_table(&label, &read(&p)?)?.get("tests") {
                    merge_tables(&mut merged, t);
                }
            }
        }
    }
    merged
        .try_into()
        .map_err(|e: toml::de::Error| ConfigError::new(&format!("languages/{}.toml", lang), None, Some("tests".into()), e.message().trim()))
}

/// `load_tests_with` with roots from the environment, cached once per repo root
/// and language for the run.
pub fn load_tests(repo_root: &Path, lang: &str) -> Result<mechanics::Tests, ConfigError> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, String), mechanics::Tests>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (repo_root.to_path_buf(), lang.to_string());
    if let Some(t) = cache.lock().unwrap().get(&key) {
        return Ok(t.clone());
    }
    let t = load_tests_with(&Roots::from_env(repo_root), lang)?;
    cache.lock().unwrap().insert(key, t.clone());
    Ok(t)
}

/// Models for qualified names, first definition wins.
#[derive(Debug, Default)]
pub struct Models {
    map: HashMap<String, Model>,
}

impl Models {
    pub fn get(&self, qualified_name: &str) -> Option<&Model> {
        self.map.get(qualified_name)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Model)> {
        self.map.iter()
    }
}

#[derive(Debug)]
pub struct LangConfig {
    pub lang: String,
    pub mechanics: Mechanics,
    pub models: Models,
    /// Version from the `go` line of the nearest go.mod, if any.
    pub go_version: Option<GoVersion>,
    /// Module path from the nearest go.mod, if any.
    pub module_path: Option<String>,
    /// Model sources in priority order with the number of models each supplied.
    pub sources: Vec<(String, usize)>,
}

impl LangConfig {
    pub fn model(&self, qualified_name: &str) -> Option<&Model> {
        self.models.get(qualified_name)
    }
}

/// Load with roots taken from the environment.
pub fn load(repo_root: &Path, lang: &str) -> Result<LangConfig, ConfigError> {
    load_with(&Roots::from_env(repo_root), repo_root, lang)
}

pub fn load_with(roots: &Roots, repo_root: &Path, lang: &str) -> Result<LangConfig, ConfigError> {
    if lang.is_empty() || lang.contains(['/', '\\']) || lang.starts_with('.') {
        return Err(ConfigError::new("<args>", None, None, &format!("bad language name \"{}\"", lang)));
    }
    let (go_version, module_path) = if lang == "go" {
        (gomod::go_version(repo_root), gomod::module_path(repo_root))
    } else {
        (None, None)
    };

    // Mechanics: merge tables, lowest priority first, so higher layers win per key.
    let mut layers: Vec<(String, toml::Table)> = Vec::new();
    if let Some((_, text)) = BUILTIN_LANGUAGES.iter().find(|(l, _)| *l == lang) {
        let label = format!("<built-in>/languages/{}.toml", lang);
        layers.push((label.clone(), parse_table(&label, text)?));
    }
    for root in roots.ordered().into_iter().rev() {
        let p = root.join("languages").join(format!("{}.toml", lang));
        if p.is_file() {
            let text = read(&p)?;
            layers.push((p.display().to_string(), parse_table(&p.display().to_string(), &text)?));
        }
    }
    if layers.is_empty() {
        return Err(ConfigError::new(
            "<config>",
            None,
            None,
            &format!("unknown language \"{}\": no languages/{}.toml found", lang, lang),
        ));
    }
    let mut merged = toml::Table::new();
    for (_, t) in &layers {
        merge_tables(&mut merged, t);
    }
    let mechanics: Mechanics = merged
        .try_into()
        .map_err(|e: toml::de::Error| ConfigError::new("<merged mechanics>", None, None, e.message().trim()))?;

    // Models: priority order, first definition of a name wins.
    let mut models = Models::default();
    let mut sources = Vec::new();
    let mut absorb = |label: &str, text: &str| -> Result<(), ConfigError> {
        let mut n = 0;
        for (k, m) in models::parse_file(label, text)? {
            if !models.map.contains_key(&k) {
                models.map.insert(k, m);
                n += 1;
            }
        }
        sources.push((label.to_string(), n));
        Ok(())
    };
    for root in roots.ordered() {
        let dir = root.join("models").join(lang);
        for path in select_versioned(list_toml(&dir), go_version) {
            let text = read(&path)?;
            absorb(&path.display().to_string(), &text)?;
        }
    }
    let builtin: Vec<(String, Option<(u32, u32)>, &str)> = BUILTIN_MODELS
        .iter()
        .filter(|b| b.lang == lang)
        .map(|b| (b.label.to_string(), b.version, b.text))
        .collect();
    let items: Vec<(String, Option<GoVersion>)> = builtin
        .iter()
        .map(|(l, v, _)| (l.clone(), v.map(|(major, minor)| GoVersion { major, minor })))
        .collect();
    for name in select_versions(&items, go_version) {
        let (_, _, text) = builtin.iter().find(|(l, _, _)| *l == name).unwrap();
        absorb(&format!("<built-in>/models/{}/{}", lang, name), text)?;
    }

    Ok(LangConfig { lang: lang.to_string(), mechanics, models, go_version, module_path, sources })
}

fn read(p: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(p)
        .map_err(|e| ConfigError::new(&p.display().to_string(), None, None, &format!("cannot read: {}", e)))
}

fn parse_table(label: &str, text: &str) -> Result<toml::Table, ConfigError> {
    text.parse::<toml::Table>().map_err(|e| {
        let line = e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        ConfigError::new(label, line, None, e.message().trim())
    })
}

fn merge_tables(base: &mut toml::Table, over: &toml::Table) {
    for (k, v) in over {
        match (base.get_mut(k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge_tables(b, o),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

fn list_toml(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

/// Split `name@1.23.toml` into (`name`, Some(1.23)). Unversioned files give None.
fn split_version(file_name: &str) -> (String, Option<GoVersion>) {
    let stem = file_name.strip_suffix(".toml").unwrap_or(file_name);
    match stem.split_once('@') {
        Some((name, ver)) => match GoVersion::parse(ver) {
            Some(v) => (name.to_string(), Some(v)),
            None => (stem.to_string(), None),
        },
        None => (stem.to_string(), None),
    }
}

/// Of the versioned files sharing a name, keep the highest version at or below
/// `want` (nearest lower). If every version is above `want`, keep the lowest.
/// With no `want`, keep the highest. Unversioned files are always kept.
/// Returns the chosen file names.
fn select_versions(items: &[(String, Option<GoVersion>)], want: Option<GoVersion>) -> Vec<String> {
    let mut out = Vec::new();
    let mut groups: HashMap<String, Vec<(GoVersion, &String)>> = HashMap::new();
    for (file, _) in items {
        match split_version(file) {
            (name, Some(v)) => groups.entry(name).or_default().push((v, file)),
            (_, None) => out.push(file.clone()),
        }
    }
    let mut names: Vec<_> = groups.keys().cloned().collect();
    names.sort();
    for name in names {
        let mut g = groups.remove(&name).unwrap();
        g.sort_by_key(|(v, _)| *v);
        let pick = match want {
            None => g.last(),
            Some(w) => g.iter().rev().find(|(v, _)| *v <= w).or_else(|| g.first()),
        };
        out.push(pick.unwrap().1.clone());
    }
    out.sort();
    out
}

fn select_versions_on_paths(paths: Vec<PathBuf>, want: Option<GoVersion>) -> Vec<PathBuf> {
    let items: Vec<(String, Option<GoVersion>)> = paths
        .iter()
        .map(|p| (p.file_name().unwrap().to_string_lossy().to_string(), None))
        .collect();
    let chosen = select_versions(&items, want);
    paths
        .into_iter()
        .filter(|p| chosen.contains(&p.file_name().unwrap().to_string_lossy().to_string()))
        .collect()
}

fn select_versioned(paths: Vec<PathBuf>, want: Option<GoVersion>) -> Vec<PathBuf> {
    select_versions_on_paths(paths, want)
}

/// Nearest ancestor of `start` holding `.ctsq` or `.git`, else `start`.
pub fn find_repo_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".ctsq").is_dir() || dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return start.to_path_buf();
        }
    }
}

/// `ctsq config path` output: the lookup locations in priority order.
pub fn path_report(roots: &Roots) -> String {
    let show = |p: &Option<PathBuf>| match p {
        Some(p) => p.display().to_string(),
        None => "(unavailable, HOME is not set)".to_string(),
    };
    format!(
        "1. repo      {}\n2. config    {}\n3. cache     {}\n4. built-in  (compiled into ctsq)\n",
        show(&roots.repo),
        show(&roots.config),
        show(&roots.cache)
    )
}

/// `ctsq config dump <lang>` output: merged mechanics and model counts.
pub fn dump_report(cfg: &LangConfig) -> String {
    let mut s = format!("language: {}\n", cfg.lang);
    if let Some(v) = cfg.go_version {
        s.push_str(&format!("go version: {}\n", v));
    }
    if let Some(m) = &cfg.module_path {
        s.push_str(&format!("module path: {}\n", m));
    }
    s.push_str("\n[mechanics]\n");
    s.push_str(&toml::to_string(&cfg.mechanics).unwrap_or_else(|e| format!("# cannot render: {}\n", e)));
    s.push_str(&format!("\nmodels: {}\n", cfg.models.len()));
    for (label, n) in &cfg.sources {
        s.push_str(&format!("  {} ({})\n", label, n));
    }
    s
}

/// Entry for `ctsq config path|dump`. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_repo_root(&cwd);
    match args.first().map(String::as_str) {
        Some("path") if args.len() == 1 => {
            let roots = Roots::from_env(&root);
            print!("{}{}", path_report(&roots), queries::path_report(&roots));
            0
        }
        // The same aliases as -l, so `dump ts` reads typescript.toml.
        Some("dump") if args.len() == 2 => match load(
            &root,
            crate::compiler::lang::lang_for_name_canonical(&args[1]).unwrap_or(&args[1]),
        ) {
            Ok(cfg) => {
                print!("{}", dump_report(&cfg));
                0
            }
            Err(e) => {
                eprintln!("error: {}", e);
                2
            }
        },
        _ => {
            eprintln!("usage: ctsq config path | ctsq config dump <lang>");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-config-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn roots(d: &Path) -> Roots {
        Roots::from_vars(d, Some(d.join("home").to_str().unwrap()), None, None)
    }

    #[test]
    fn built_in_go_loads() {
        let d = tmp();
        let c = load_with(&roots(&d), &d, "go").unwrap();
        assert!(c.model("strings.ToUpper").is_some());
        assert!(c.model("(*html/template.Template).Execute").unwrap().template.is_some());
        assert!(c.model("builtin.len").unwrap().clear);
        assert_eq!(c.model("os.Getenv").unwrap().source, Some(Slot::Return));
        assert!(c.mechanics.nodes.call.contains(&"call_expression".to_string()));
        assert_eq!(c.mechanics.outside.module_file, "go.mod");
        assert_eq!(c.mechanics.nodes.return_, vec!["return_statement".to_string()]);
        assert!(c.models.len() > 50);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn repo_model_overrides_built_in() {
        let d = tmp();
        write(
            &d.join(".ctsq/models/go/app.toml"),
            "[\"strings.ToUpper\"]\nclear = true\n\n[\"my/pkg.Do\"]\npass = [\"0 -> return\"]\n",
        );
        let c = load_with(&roots(&d), &d, "go").unwrap();
        let m = c.model("strings.ToUpper").unwrap();
        assert!(m.clear);
        assert!(m.pass.is_empty());
        assert!(m.origin.contains("app.toml"));
        assert!(c.model("my/pkg.Do").is_some());
        assert!(c.model("strings.ToLower").is_some());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn config_dir_ranks_below_repo_above_cache() {
        let d = tmp();
        let r = roots(&d);
        let name = "[\"x.F\"]\nsource = \"return\"\n";
        write(&r.cache.clone().unwrap().join("models/go/a.toml"), "[\"x.F\"]\nclear = true\n");
        write(&r.config.clone().unwrap().join("models/go/a.toml"), name);
        let c = load_with(&r, &d, "go").unwrap();
        assert_eq!(c.model("x.F").unwrap().source, Some(Slot::Return));
        write(&d.join(".ctsq/models/go/a.toml"), "[\"x.F\"]\nwhole = true\n");
        let c = load_with(&r, &d, "go").unwrap();
        assert!(c.model("x.F").unwrap().whole);
        std::fs::remove_dir_all(&d).ok();
    }

    fn ver_files(want: Option<&str>) -> Vec<String> {
        let items: Vec<(String, Option<GoVersion>)> = ["stdlib@1.21.toml", "stdlib@1.23.toml", "app.toml"]
            .iter()
            .map(|s| (s.to_string(), None))
            .collect();
        select_versions(&items, want.and_then(GoVersion::parse))
    }

    #[test]
    fn go_version_selects_stdlib_file() {
        assert_eq!(ver_files(Some("1.23.4")), vec!["app.toml", "stdlib@1.23.toml"]);
        assert_eq!(ver_files(Some("1.22")), vec!["app.toml", "stdlib@1.21.toml"]);
        assert_eq!(ver_files(Some("1.25")), vec!["app.toml", "stdlib@1.23.toml"]);
        assert_eq!(ver_files(Some("1.19")), vec!["app.toml", "stdlib@1.21.toml"]);
        assert_eq!(ver_files(None), vec!["app.toml", "stdlib@1.23.toml"]);
    }

    #[test]
    fn go_mod_version_drives_built_in_choice() {
        let d = tmp();
        write(&d.join("go.mod"), "module example.com/m\n\ngo 1.23.4\n");
        let c = load_with(&roots(&d), &d, "go").unwrap();
        assert_eq!(c.go_version, GoVersion::parse("1.23"));
        assert_eq!(c.module_path.as_deref(), Some("example.com/m"));
        assert!(c.sources.iter().any(|(l, n)| l.ends_with("stdlib@1.23.toml") && *n > 0));
        write(&d.join("go.mod"), "module example.com/m\n\ngo 1.25\n");
        let c = load_with(&roots(&d), &d, "go").unwrap();
        assert!(c.sources.iter().any(|(l, _)| l.ends_with("stdlib@1.23.toml")));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn mechanics_override_merges_per_key() {
        let d = tmp();
        write(&d.join(".ctsq/languages/go.toml"), "[nodes]\nloop = [\"for_statement\", \"range_clause\"]\n");
        let c = load_with(&roots(&d), &d, "go").unwrap();
        assert_eq!(c.mechanics.nodes.loop_.len(), 2);
        assert!(!c.mechanics.nodes.call.is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn malformed_file_reports_file_line_key() {
        let d = tmp();
        write(&d.join(".ctsq/models/go/bad.toml"), "[\"ok.F\"]\nclear = true\n\n[\"bad.F\"]\npass = [\"zzz\"]\n");
        let e = load_with(&roots(&d), &d, "go").unwrap_err();
        assert!(e.file.ends_with("bad.toml"));
        assert_eq!(e.key.as_deref(), Some("bad.F"));
        assert!(e.line.is_some());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unknown_language_is_an_error() {
        let d = tmp();
        assert!(load_with(&roots(&d), &d, "cobol").is_err());
        assert!(load_with(&roots(&d), &d, "../x").is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn every_language_has_built_in_test_defaults() {
        let d = tmp();
        let r = roots(&d);
        let go = load_tests_with(&r, "go").unwrap();
        assert_eq!(go.files, vec!["*_test.go"]);
        assert_eq!(go.dirs, vec!["testdata"]);
        let ts = load_tests_with(&r, "typescript").unwrap();
        assert_eq!(ts.files, vec!["*.test.*", "*.spec.*"]);
        assert_eq!(ts.dirs, vec!["__tests__", "test", "tests", "e2e"]);
        assert_eq!(load_tests_with(&r, "tsx").unwrap(), ts);
        assert_eq!(load_tests_with(&r, "javascript").unwrap(), ts);
        let py = load_tests_with(&r, "python").unwrap();
        assert_eq!(py.files, vec!["test_*.py", "*_test.py", "conftest.py"]);
        assert_eq!(load_tests_with(&r, "ruby").unwrap().dirs, vec!["test", "spec"]);
        let rs = load_tests_with(&r, "rust").unwrap();
        assert!(rs.files.is_empty());
        assert_eq!(rs.scopes, vec!["module#tests"]);
        assert_eq!(load_tests_with(&r, "cpp").unwrap(), load_tests_with(&r, "c").unwrap());
        assert_eq!(load_tests_with(&r, "gotmpl").unwrap(), Default::default());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_repo_tests_table_overrides_per_key() {
        let d = tmp();
        write(&d.join(".ctsq/languages/go.toml"), "[tests]\ndirs = [\"fixtures\"]\n");
        let t = load_tests_with(&roots(&d), "go").unwrap();
        assert_eq!(t.dirs, vec!["fixtures"]);
        assert_eq!(t.files, vec!["*_test.go"], "a key the repo leaves out keeps the built-in");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn dump_shows_the_merged_tests_table() {
        let d = tmp();
        let s = dump_report(&load_with(&roots(&d), &d, "typescript").unwrap());
        assert!(s.contains("[tests]\n"), "{}", s);
        assert!(s.contains("\"*.spec.*\""), "{}", s);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn roots_follow_xdg_then_home() {
        let r = Roots::from_vars(Path::new("/r"), Some("/h"), None, Some("/c"));
        assert_eq!(r.config, Some(PathBuf::from("/h/.config/ctsq")));
        assert_eq!(r.cache, Some(PathBuf::from("/c/ctsq")));
        assert_eq!(r.repo, Some(PathBuf::from("/r/.ctsq")));
    }

    #[test]
    fn path_report_lists_four_locations() {
        let r = Roots::from_vars(Path::new("/r"), Some("/h"), None, None);
        let s = path_report(&r);
        assert_eq!(s.lines().count(), 4);
        assert!(s.contains("/r/.ctsq") && s.contains("/h/.config/ctsq") && s.contains("/h/.cache/ctsq") && s.contains("built-in"));
    }
}
