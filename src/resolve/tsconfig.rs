//! Minimal tsconfig.json / jsconfig.json reader for import-alias resolution.
//!
//! Parses JSONC (comments + trailing commas), follows `extends`, and applies
//! `compilerOptions.baseUrl` + `paths` so that non-relative TS/JS imports like
//! `~/lib/api` or `@app/thing` resolve to real files. Heuristic only in the
//! sense that it ignores module-resolution mode nuances; the alias mapping
//! itself is read exactly from the config.

use super::imports::{is_jsts, normalize_lexical, FileInfo, JS_EXTS};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ── JSONC value + parser ──────────────────────────────────────────────────────

enum Json {
    Null,
    Bool,   // value irrelevant — we only read strings/arrays/objects
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self { Json::Str(s) => Some(s), _ => None }
    }
    fn as_array(&self) -> Option<&[Json]> {
        match self { Json::Arr(a) => Some(a), _ => None }
    }
}

struct P { s: Vec<char>, i: usize }

impl P {
    fn new(src: &str) -> Self { P { s: src.chars().collect(), i: 0 } }
    fn peek(&self) -> Option<char> { self.s.get(self.i).copied() }
    fn at(&self, k: usize) -> Option<char> { self.s.get(self.i + k).copied() }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() { self.i += 1; }
        c
    }

    /// Skip whitespace and `//` / `/* */` comments (JSONC).
    fn ws(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => { self.i += 1; }
                Some('/') if self.at(1) == Some('/') => {
                    while let Some(c) = self.peek() { self.i += 1; if c == '\n' { break; } }
                }
                Some('/') if self.at(1) == Some('*') => {
                    self.i += 2;
                    while self.i < self.s.len() {
                        if self.s[self.i] == '*' && self.at(1) == Some('/') { self.i += 2; break; }
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match self.peek()? {
            '{' => self.object(),
            '[' => self.array(),
            '"' => Some(Json::Str(self.string()?)),
            't' => { self.keyword("true")?; Some(Json::Bool) }
            'f' => { self.keyword("false")?; Some(Json::Bool) }
            'n' => { self.keyword("null")?; Some(Json::Null) }
            _ => self.number(),
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.bump(); // {
        let mut out = Vec::new();
        loop {
            self.ws();
            match self.peek()? {
                '}' => { self.bump(); break; }
                ',' => { self.bump(); }        // tolerate leading / trailing commas
                '"' => {
                    let k = self.string()?;
                    self.ws();
                    if self.peek() == Some(':') { self.bump(); }
                    let v = self.value()?;
                    out.push((k, v));
                }
                _ => return None,
            }
        }
        Some(Json::Obj(out))
    }

    fn array(&mut self) -> Option<Json> {
        self.bump(); // [
        let mut out = Vec::new();
        loop {
            self.ws();
            match self.peek()? {
                ']' => { self.bump(); break; }
                ',' => { self.bump(); }
                _ => out.push(self.value()?),
            }
        }
        Some(Json::Arr(out))
    }

    fn string(&mut self) -> Option<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump()? {
                '"' => break,
                '\\' => match self.bump()? {
                    'n' => out.push('\n'), 't' => out.push('\t'), 'r' => out.push('\r'),
                    'b' => out.push('\u{8}'), 'f' => out.push('\u{c}'),
                    'u' => {
                        let mut h = String::new();
                        for _ in 0..4 { h.push(self.bump()?); }
                        if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                            out.push(ch);
                        }
                    }
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        Some(out)
    }

    fn keyword(&mut self, kw: &str) -> Option<()> {
        for c in kw.chars() { if self.bump()? != c { return None; } }
        Some(())
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || "+-.eE".contains(c) { self.i += 1; } else { break; }
        }
        let s: String = self.s[start..self.i].iter().collect();
        s.parse::<f64>().ok().map(|_| Json::Num) // validate, but the value is unused
    }
}

fn parse_jsonc(src: &str) -> Option<Json> {
    P::new(src).value()
}

// ── Config model + discovery ──────────────────────────────────────────────────

#[derive(Clone)]
struct Cfg {
    /// Directory containing the tsconfig (absolute).
    dir: PathBuf,
    /// `compilerOptions.baseUrl`, made absolute (None when unset).
    base_url: Option<PathBuf>,
    /// `compilerOptions.paths`: (pattern, [targets]).
    paths: Vec<(String, Vec<String>)>,
}

pub struct TsConfig {
    configs: Vec<Cfg>,
}

impl TsConfig {
    /// Discover and parse every tsconfig/jsconfig governing the scanned JS/TS files.
    pub fn build(files: &[FileInfo]) -> TsConfig {
        let mut walked: HashSet<PathBuf> = HashSet::new();
        let mut loaded: HashSet<PathBuf> = HashSet::new();
        let mut configs = Vec::new();
        for f in files {
            if !is_jsts(f.lang) { continue; }
            let mut cur = Path::new(&f.canonical).parent().map(Path::to_path_buf);
            while let Some(dir) = cur {
                if !walked.insert(dir.clone()) { break; } // ancestors already scanned
                for name in ["tsconfig.json", "jsconfig.json"] {
                    let p = dir.join(name);
                    if p.is_file() && loaded.insert(p.clone()) {
                        if let Some(cfg) = load_cfg(&p, 0) { configs.push(cfg); }
                    }
                }
                cur = dir.parent().map(Path::to_path_buf);
            }
        }
        TsConfig { configs }
    }

    /// Resolve a non-relative specifier through the nearest governing config.
    /// `Some(hits)` means a config applied (empty ⇒ it's an external package);
    /// `None` means no config governs this file — the caller may fall back.
    pub fn resolve(&self, importer: &Path, spec: &str, known: &HashSet<String>) -> Option<Vec<String>> {
        let cfg = self.configs.iter()
            .filter(|c| importer.starts_with(&c.dir))
            .max_by_key(|c| c.dir.components().count())?;

        let base = cfg.base_url.clone().unwrap_or_else(|| cfg.dir.clone());
        let mut hits = Vec::new();

        // Most specific matching `paths` pattern wins (longest literal prefix).
        let best = cfg.paths.iter()
            .filter_map(|(pat, targets)| match_pattern(pat, spec).map(|cap| (prefix_len(pat), targets, cap)))
            .max_by_key(|(plen, _, _)| *plen);

        if let Some((_, targets, captured)) = best {
            for t in targets {
                let cand = normalize_lexical(&base.join(substitute(t, &captured)));
                probe_into(&cand, known, &mut hits);
            }
            hits.sort();
            hits.dedup();
            return Some(hits);
        }

        // No path alias matched. If baseUrl is set, non-relative bare imports
        // resolve against it; otherwise the specifier is an external package.
        if cfg.base_url.is_some() {
            let cand = normalize_lexical(&base.join(spec));
            probe_into(&cand, known, &mut hits);
            hits.sort();
            hits.dedup();
        }
        Some(hits)
    }
}

/// Load one config, merging anything it `extends` (extends resolved first, then
/// this file overrides). Depth-guarded against extends cycles.
fn load_cfg(path: &Path, depth: usize) -> Option<Cfg> {
    if depth > 8 { return None; }
    let src = std::fs::read_to_string(path).ok()?;
    let json = parse_jsonc(&src)?;
    let dir = path.parent()?.to_path_buf();

    let mut base_url = None;
    let mut paths: Vec<(String, Vec<String>)> = Vec::new();

    if let Some(ext) = json.get("extends").and_then(Json::as_str) {
        if let Some(parent) = resolve_extends(&dir, ext).and_then(|p| load_cfg(&p, depth + 1)) {
            base_url = parent.base_url;
            paths = parent.paths;
        }
    }

    if let Some(co) = json.get("compilerOptions") {
        if let Some(b) = co.get("baseUrl").and_then(Json::as_str) {
            base_url = Some(normalize_lexical(&dir.join(b)));
        }
        if let Some(Json::Obj(entries)) = co.get("paths") {
            let mut np = Vec::new();
            for (pat, val) in entries {
                let targets: Vec<String> = val.as_array()
                    .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                np.push((pat.clone(), targets));
            }
            if !np.is_empty() { paths = np; } // child paths replace, not merge
        }
    }

    Some(Cfg { dir, base_url, paths })
}

/// Resolve a relative `extends` target (packages in node_modules are ignored).
fn resolve_extends(dir: &Path, ext: &str) -> Option<PathBuf> {
    if !ext.starts_with('.') { return None; }
    let mut p = normalize_lexical(&dir.join(ext));
    if p.extension().is_none() { p.set_extension("json"); }
    Some(p)
}

/// Literal-prefix length of a `paths` pattern (whole string when non-wildcard),
/// used to pick the most specific match.
fn prefix_len(pat: &str) -> usize {
    pat.find('*').unwrap_or(pat.len())
}

/// Match `spec` against a `paths` pattern; returns the `*` capture (or "" for
/// an exact, wildcard-free pattern).
fn match_pattern(pat: &str, spec: &str) -> Option<String> {
    match pat.find('*') {
        Some(star) => {
            let (pre, post) = (&pat[..star], &pat[star + 1..]);
            if spec.len() >= pre.len() + post.len() && spec.starts_with(pre) && spec.ends_with(post) {
                Some(spec[pre.len()..spec.len() - post.len()].to_string())
            } else {
                None
            }
        }
        None => if pat == spec { Some(String::new()) } else { None },
    }
}

/// Substitute the captured `*` back into a target template.
fn substitute(target: &str, captured: &str) -> String {
    match target.find('*') {
        Some(star) => format!("{}{}{}", &target[..star], captured, &target[star + 1..]),
        None => target.to_string(),
    }
}

/// Probe `<base>`, `<base>.{ext}`, and `<base>/index.{ext}` in the scanned set.
fn probe_into(base: &Path, known: &HashSet<String>, hits: &mut Vec<String>) {
    let mut add = |p: PathBuf| {
        let s = p.to_string_lossy().into_owned();
        if known.contains(&s) && !hits.contains(&s) { hits.push(s); }
    };
    add(base.to_path_buf());
    for ext in JS_EXTS { add(base.with_extension(ext)); }
    for ext in JS_EXTS { add(base.join("index").with_extension(ext)); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        let src = "{\n  // line comment\n  \"a\": 1, /* block */ \"b\": [\"x\", \"y\",],\n}";
        let j = parse_jsonc(src).expect("parse");
        assert!(j.get("a").is_some());
        assert_eq!(j.get("b").and_then(Json::as_array).map(|a| a.len()), Some(2));
    }

    #[test]
    fn pattern_match_and_substitute() {
        assert_eq!(match_pattern("~/*", "~/lib/api"), Some("lib/api".to_string()));
        assert_eq!(match_pattern("@app/*", "~/x"), None);
        assert_eq!(match_pattern("exact", "exact"), Some(String::new()));
        assert_eq!(match_pattern("exact", "other"), None);
        assert_eq!(substitute("src/shared/*", "thing"), "src/shared/thing");
        assert!(prefix_len("@app/*") > prefix_len("@/*"), "more specific pattern wins");
    }
}
