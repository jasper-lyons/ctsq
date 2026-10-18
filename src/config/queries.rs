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
//! Further keys of a query table:
//!
//! ```toml
//! rewrite       = ["(\\w+)#(\\w+)", "${1:pascal}Controller#$2"]  # the @target text, before resolution
//! paths         = "app/**/page.{tsx,jsx,ts,js}"   # file routes: each matching file is a match
//! key_from_path = "^app(/.*)?/page\\.\\w+$"          # the key is group 1, or / when empty
//! target        = "default-export"                # the module's default export
//! ```
//!
//! Keys that turn each row's key into a pattern the user's key is tested
//! against, so a pasted URL or a log line finds the row that produced it:
//!
//! ```toml
//! key_pattern     = [["/:(\\w+)\\?", "(?:/(?P<$1>[^/]+))?"], [":(\\w+)", "(?P<$1>[^/]+)"]]
//! key_input       = [["^[a-z]+://[^/]+", ""], ["[?#].*$", ""], ["(.)/$", "$1"]]
//! key_text        = [["\\\\\"", "\""]]   # the row key as the substring test sees it
//! key_match       = "whole"              # or "within": the pattern anywhere in the user's key
//! key_min_literal = 4                    # fewer literal characters: no pattern for that row
//! ```
//!
//! `key_pattern` is scanned over a row's key left to right. At each position
//! the rules are tried in order and the first whose regex matches there (a
//! non-empty match starting exactly at that position) emits its fragment, with
//! `$1`, `${1}`, `$name` and `$0` expanded from that rule's match, and the scan
//! resumes after the match. `^` and `$` in a rule mean the start and end of the
//! whole key. A character no rule matches is emitted regex-escaped and counts
//! as one literal character (whitespace does not count). A named group that
//! occurs twice gets `_2`, `_3` on the later ones. The pattern is compiled
//! case-insensitively (`(?is)`), anchored `^...$` for `whole`.
//!
//! `key_input` is applied to the user's key, in order, every match replaced
//! (`$1` expands), before it is tested against the patterns. `key_text` is
//! applied the same way to each row's key before the substring test and before
//! the pattern is built. A `/regex/` key sees the key as printed and is never
//! tested against patterns.
//!
//! With `key_pattern` and `whole`, rows whose pattern matched print first, the
//! most literal characters first, and a pattern's named groups print as
//! `name=value` after the row when the row did not also match as text. With
//! `within`, rows keep the source order.

use super::{ConfigError, Roots, BUILTIN_QUERIES};
use std::collections::{BTreeMap, HashMap};
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
    /// Resolve the `@target` (a module specifier, or the file itself for a
    /// `paths` query) to a file and name that file's default export.
    DefaultExport,
}

/// How a row's pattern meets the user's key.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum KeyMatch {
    /// The whole key, anchored `^...$`.
    #[default]
    Whole,
    /// Anywhere in the key, as a log line holds a message.
    Within,
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
    /// `[regex, replacement]` applied to the `@target` text before `def` runs.
    pub rewrite: Option<(String, String)>,
    /// `[regex, replacement]` applied to the key (after its quotes are dropped).
    pub key_rewrite: Option<(String, String)>,
    /// A glob, relative to the search root: each matching file is a match.
    pub paths: Option<String>,
    /// With `paths`: a regex over the relative path, the key is group 1.
    pub key_from_path: Option<String>,
    /// `[[regex, fragment], ...]`: how a row's key becomes a pattern. Empty:
    /// rows match by text only.
    pub key_pattern: Vec<(String, String)>,
    /// `[[regex, replacement], ...]` applied to the user's key before it is
    /// tested against the patterns.
    pub key_input: Vec<(String, String)>,
    /// `[[regex, replacement], ...]` applied to a row's key before the
    /// substring test and the pattern build. The printed key is unchanged.
    pub key_text: Vec<(String, String)>,
    pub key_match: KeyMatch,
    /// A row whose pattern has fewer literal characters gets no pattern.
    pub key_min_literal: usize,
    /// The file as printed: a path for a file on disk, or
    /// `built-in queries/<file>.toml`.
    pub source: String,
    /// 1-based line of the query's `[name]` table in the file.
    pub line: usize,
}

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

fn single(label: &str, line: usize, name: &str, key: &str, v: &toml::Value) -> Result<String, ConfigError> {
    v.as_str().map(str::to_string).ok_or_else(|| err(label, Some(line), Some(name), &format!("`{}` is a string", key)))
}

/// A `[[regex, text], ...]` list, each regex compiled to check it.
fn rule_list(label: &str, line: usize, name: &str, key: &str, v: &toml::Value) -> Result<Vec<(String, String)>, ConfigError> {
    let shape = || err(label, Some(line), Some(name), &format!("`{}` is [[\"<regex>\", \"<text>\"], ...]", key));
    let toml::Value::Array(items) = v else { return Err(shape()) };
    let mut out = Vec::new();
    for item in items {
        let toml::Value::Array(pair) = item else { return Err(shape()) };
        let (Some(re), Some(text), 2) = (pair.first().and_then(|x| x.as_str()), pair.get(1).and_then(|x| x.as_str()), pair.len())
        else {
            return Err(shape());
        };
        if let Err(e) = regex::Regex::new(re) {
            return Err(err(
                label,
                Some(line),
                Some(name),
                &format!("`{}` regex `{}` does not compile: {}", key, re, e.to_string().lines().last().unwrap_or("").trim()),
            ));
        }
        out.push((re.to_string(), text.to_string()));
    }
    Ok(out)
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
        let mut rewrite = None;
        let mut key_rewrite = None;
        let mut paths = None;
        let mut key_from_path = None;
        let mut key_pattern = Vec::new();
        let mut key_input = Vec::new();
        let mut key_text = Vec::new();
        let mut key_match = KeyMatch::Whole;
        let mut key_min_literal = 0;
        for (k, v) in q {
            match k.as_str() {
                "match" => matches = strings(label, name, v)?,
                "target" => {
                    target = Some(match v.as_str() {
                        Some("def") => Target::Def,
                        Some("enclosing") => Target::Enclosing,
                        Some("self") => Target::Match,
                        Some("default-export") => Target::DefaultExport,
                        _ => {
                            return Err(err(
                                label,
                                Some(line),
                                Some(name),
                                "target must be \"def\", \"enclosing\", \"self\" or \"default-export\"",
                            ))
                        }
                    })
                }
                "rewrite" => {
                    let pair = strings(label, name, v)?;
                    if pair.len() != 2 {
                        return Err(err(label, Some(line), Some(name), "`rewrite` is [\"<regex>\", \"<replacement>\"]"));
                    }
                    if let Err(e) = regex::Regex::new(&pair[0]) {
                        return Err(err(
                            label,
                            Some(line),
                            Some(name),
                            &format!("`rewrite` regex does not compile: {}", e.to_string().lines().last().unwrap_or("").trim()),
                        ));
                    }
                    rewrite = Some((pair[0].clone(), pair[1].clone()));
                }
                "key_rewrite" => {
                    let pair = strings(label, name, v)?;
                    if pair.len() != 2 || regex::Regex::new(&pair[0]).is_err() {
                        return Err(err(label, Some(line), Some(name), "`key_rewrite` is [\"<regex>\", \"<replacement>\"]"));
                    }
                    key_rewrite = Some((pair[0].clone(), pair[1].clone()));
                }
                "paths" => paths = Some(single(label, line, name, "paths", v)?),
                "key_from_path" => {
                    let re = single(label, line, name, "key_from_path", v)?;
                    if let Err(e) = regex::Regex::new(&re) {
                        return Err(err(
                            label,
                            Some(line),
                            Some(name),
                            &format!("`key_from_path` regex does not compile: {}", e.to_string().lines().last().unwrap_or("").trim()),
                        ));
                    }
                    key_from_path = Some(re);
                }
                "key_pattern" => key_pattern = rule_list(label, line, name, k, v)?,
                "key_input" => key_input = rule_list(label, line, name, k, v)?,
                "key_text" => key_text = rule_list(label, line, name, k, v)?,
                "key_match" => {
                    key_match = match v.as_str() {
                        Some("whole") => KeyMatch::Whole,
                        Some("within") => KeyMatch::Within,
                        _ => return Err(err(label, Some(line), Some(name), "key_match must be \"whole\" or \"within\"")),
                    }
                }
                "key_min_literal" => {
                    key_min_literal = match v.as_integer() {
                        Some(n) if n >= 0 => n as usize,
                        _ => return Err(err(label, Some(line), Some(name), "key_min_literal is a whole number")),
                    }
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
        if matches.is_empty() && paths.is_none() {
            return Err(err(label, Some(line), Some(name), "missing `match = \"<selector>\"`"));
        }
        if paths.is_some() != key_from_path.is_some() {
            return Err(err(label, Some(line), Some(name), "`paths` and `key_from_path` go together"));
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
            rewrite,
            key_rewrite,
            paths,
            key_from_path,
            key_pattern,
            key_input,
            key_text,
            key_match,
            key_min_literal,
            source: label.to_string(),
            line,
        });
    }
    Ok(out)
}

/// `Pascal` case of `text`: words split at anything not a letter or digit, each
/// starting upper case (`user_profiles` gives `UserProfiles`).
fn pascal(text: &str) -> String {
    let mut out = String::new();
    for w in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let mut cs = w.chars();
        if let Some(f) = cs.next() {
            out.extend(f.to_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

fn camel(text: &str) -> String {
    let p = pascal(text);
    let mut cs = p.chars();
    match cs.next() {
        Some(f) => f.to_lowercase().chain(cs).collect(),
        None => p,
    }
}

/// Expand `$1`, `${1}`, `${1:pascal}` and `${1:camel}` against `caps`.
fn expand(caps: &regex::Captures, repl: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = repl.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] != '$' || i + 1 >= cs.len() {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        let (spec, next) = if cs[i + 1] == '{' {
            match cs[i + 2..].iter().position(|&c| c == '}') {
                Some(end) => (cs[i + 2..i + 2 + end].iter().collect::<String>(), i + 3 + end),
                None => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            }
        } else {
            let digits: String = cs[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                out.push('$');
                i += 1;
                continue;
            }
            let n = digits.len();
            (digits, i + 1 + n)
        };
        let (num, filter) = spec.split_once(':').unwrap_or((&spec, ""));
        let group = num.parse::<usize>().ok().and_then(|n| caps.get(n)).map(|m| m.as_str()).unwrap_or("");
        out.push_str(&match filter {
            "pascal" => pascal(group),
            "camel" => camel(group),
            _ => group.to_string(),
        });
        i = next;
    }
    out
}

/// Replace the first match of the rewrite's regex in `text` with its
/// replacement. Text the regex does not match is returned as it is.
pub fn apply_rewrite(rw: &(String, String), text: &str) -> String {
    let Ok(re) = regex::Regex::new(&rw.0) else { return text.to_string() };
    match re.captures(text) {
        Some(c) => {
            let m = c.get(0).unwrap();
            format!("{}{}{}", &text[..m.start()], expand(&c, &rw.1), &text[m.end()..])
        }
        None => text.to_string(),
    }
}

/// Apply each `[regex, replacement]` in order, every match replaced. A rule
/// whose regex does not compile (parse_file rejects those) is skipped.
pub fn apply_all(rules: &[(String, String)], text: &str) -> String {
    let mut out = text.to_string();
    for (re, repl) in rules {
        if let Ok(r) = regex::Regex::new(re) {
            out = r.replace_all(&out, repl.as_str()).into_owned();
        }
    }
    out
}

/// A row key turned into a regex by `KeyScanner::scan`.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyRegex {
    /// The regex source, not anchored, without flags.
    pub source: String,
    /// Characters no rule matched, emitted escaped, whitespace not counted.
    pub literal: usize,
}

/// A query's compiled `key_pattern` rules.
pub struct KeyScanner {
    rules: Vec<(regex::Regex, String)>,
}

impl KeyScanner {
    /// None when there are no rules.
    pub fn new(rules: &[(String, String)]) -> Option<KeyScanner> {
        let rules: Vec<(regex::Regex, String)> =
            rules.iter().filter_map(|(re, f)| regex::Regex::new(re).ok().map(|r| (r, f.clone()))).collect();
        (!rules.is_empty()).then_some(KeyScanner { rules })
    }

    /// Scan `key` left to right: at each position the first rule matching
    /// there emits its expanded fragment, any other character is emitted
    /// escaped. Repeated group names get a suffix.
    pub fn scan(&self, key: &str) -> KeyRegex {
        let mut out = String::new();
        let mut literal = 0;
        // The earliest position each rule can next match at, None for never:
        // a search from one position finds the leftmost match, so no rule is
        // searched again before the match it already found.
        let mut next: Vec<Option<usize>> = vec![Some(0); self.rules.len()];
        let mut pos = 0;
        while pos < key.len() {
            let mut hit: Option<(usize, String)> = None;
            for (i, (re, frag)) in self.rules.iter().enumerate() {
                match next[i] {
                    None => continue,
                    Some(s) if s > pos => continue,
                    _ => {}
                }
                let Some(c) = re.captures_at(key, pos) else {
                    next[i] = None;
                    continue;
                };
                let m = c.get(0).unwrap();
                if m.start() > pos {
                    next[i] = Some(m.start());
                    continue;
                }
                if m.end() == pos {
                    // An empty match emits nothing and would never advance.
                    next[i] = Some(pos + 1);
                    continue;
                }
                let mut f = String::new();
                c.expand(frag, &mut f);
                hit = Some((m.end(), f));
                break;
            }
            match hit {
                Some((end, f)) => {
                    out.push_str(&f);
                    pos = end;
                }
                None => {
                    let ch = key[pos..].chars().next().unwrap();
                    let mut buf = [0u8; 4];
                    out.push_str(&regex::escape(ch.encode_utf8(&mut buf)));
                    if !ch.is_whitespace() {
                        literal += 1;
                    }
                    pos += ch.len_utf8();
                }
            }
        }
        KeyRegex { source: dedupe_groups(&out), literal }
    }
}

/// Rename the second and later named groups of one name to `name_2`,
/// `name_3`, so the regex compiles. Escaped parentheses are not groups.
fn dedupe_groups(src: &str) -> String {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = String::new();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        let escaped = i > 0 && src[..i].bytes().rev().take_while(|&c| c == b'\\').count() % 2 == 1;
        let open = if rest.starts_with("(?P<") {
            4
        } else if rest.starts_with("(?<") && !rest.starts_with("(?<=") && !rest.starts_with("(?<!") {
            3
        } else {
            0
        };
        if open > 0 && !escaped {
            let name: String = rest[open..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() && rest[open + name.len()..].starts_with('>') {
                let n = seen.entry(name.clone()).or_insert(0);
                *n += 1;
                out.push_str(&rest[..open]);
                if *n == 1 {
                    out.push_str(&name);
                } else {
                    out.push_str(&format!("{}_{}", name, n));
                }
                i += open + name.len();
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
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
    fn rewrite_paths_and_the_default_export_target_parse() {
        let text = "framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"(id @key)\"\ntarget = \"def\"\nrewrite = [\"(\\\\w+)#(\\\\w+)\", \"${1:pascal}#$2\"]\n\n[file]\npaths = \"app/**/page.tsx\"\nkey_from_path = \"^app(/.*)?/page\\\\.\\\\w+$\"\ntarget = \"default-export\"\n";
        let qs = parse_file("f.toml", text).unwrap();
        let route = qs.iter().find(|q| q.name == "route").unwrap();
        assert_eq!(route.rewrite.as_ref().unwrap().1, "${1:pascal}#$2");
        let file = qs.iter().find(|q| q.name == "file").unwrap();
        assert_eq!(file.target, Target::DefaultExport);
        assert!(file.matches.is_empty() && file.paths.as_deref() == Some("app/**/page.tsx"));
        assert!(file.key_from_path.is_some());
    }

    #[test]
    fn rewrite_expands_groups_and_case_filters() {
        let rw = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(apply_rewrite(&rw("(\\w+)#(\\w+)", "${1:pascal}Controller#$2"), "library#index"), "LibraryController#index");
        assert_eq!(apply_rewrite(&rw("(\\w+)#(\\w+)", "${1:pascal}Controller#$2"), "user_profiles#show"), "UserProfilesController#show");
        assert_eq!(apply_rewrite(&rw("^(\\w+)$", "${1:camel}!"), "User_profiles"), "userProfiles!");
        assert_eq!(apply_rewrite(&rw("a(b)", "<$1${1}>"), "xabz"), "x<bb>z");
        assert_eq!(apply_rewrite(&rw("^q$", "r"), "other"), "other", "no match leaves the text");
    }

    fn scanner(rules: &[(&str, &str)]) -> KeyScanner {
        let owned: Vec<(String, String)> = rules.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        KeyScanner::new(&owned).unwrap()
    }

    const COLON: &[(&str, &str)] = &[(r"/:(\w+)\?", r"(?:/(?P<$1>[^/]+))?"), (r":(\w+)", r"(?P<$1>[^/]+)"), (r"\*", ".*")];

    #[test]
    fn a_character_no_rule_matches_is_escaped_and_counted() {
        let s = scanner(COLON);
        let k = s.scan("a.b(c){d}");
        assert_eq!(k.source, r"a\.b\(c\)\{d\}");
        assert_eq!(k.literal, 9);
        let k = s.scan("x y");
        assert_eq!((k.source.as_str(), k.literal), ("x y", 2), "whitespace is not a literal character");
    }

    #[test]
    fn every_occurrence_is_replaced_and_named_groups_expand() {
        let s = scanner(COLON);
        let k = s.scan("/api/org/:org_id/team/:team_id");
        assert_eq!(k.source, r"/api/org/(?P<org_id>[^/]+)/team/(?P<team_id>[^/]+)");
        assert_eq!(k.literal, "/api/org//team/".len(), "rule matches do not count");
        let re = regex::Regex::new(&format!("^(?:{})$", k.source)).unwrap();
        let c = re.captures("/api/org/mavexa/team/hr").unwrap();
        assert_eq!((&c["org_id"], &c["team_id"]), ("mavexa", "hr"));
        let k = s.scan("/settings/:tab?");
        assert_eq!(k.source, r"/settings(?:/(?P<tab>[^/]+))?", "the first rule matching at a position wins");
        assert_eq!(s.scan("*").literal, 0);
    }

    #[test]
    fn caret_and_dollar_in_a_rule_mean_the_ends_of_the_whole_key() {
        let s = scanner(&[("^/", "(?:[A-Z]+ )?/"), ("/$", "(?:/.*)?")]);
        assert_eq!(s.scan("/a/b/").source, r"(?:[A-Z]+ )?/a/b(?:/.*)?");
    }

    #[test]
    fn specificity_counts_only_the_escaped_characters() {
        let s = scanner(COLON);
        assert!(s.scan("/events/new").literal > s.scan("/events/:id").literal);
    }

    #[test]
    fn a_repeated_group_name_gets_a_suffix_so_the_regex_compiles() {
        let s = scanner(COLON);
        let k = s.scan("/a/:id/b/:id/c/:id");
        assert_eq!(k.source, r"/a/(?P<id>[^/]+)/b/(?P<id_2>[^/]+)/c/(?P<id_3>[^/]+)");
        assert!(regex::Regex::new(&k.source).is_ok());
        assert_eq!(dedupe_groups(r"\(?P<x>y\)(?P<x>z)"), r"\(?P<x>y\)(?P<x>z)", "an escaped paren is no group");
    }

    #[test]
    fn apply_all_replaces_every_match_of_each_rule_in_order() {
        let rules = vec![("^[a-z]+://[^/]*".to_string(), String::new()), ("[?#].*$".to_string(), String::new()), ("(.)/$".to_string(), "$1".to_string())];
        assert_eq!(apply_all(&rules, "https://x.com/a/b/?q=1"), "/a/b");
        assert_eq!(apply_all(&rules, "/"), "/", "a lone slash stays");
        assert_eq!(apply_all(&[("o".to_string(), "0".to_string())], "foo"), "f00");
    }

    #[test]
    fn key_rules_parse_and_bad_ones_are_errors() {
        let base = "framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"(id @key)\"\ntarget = \"def\"\n";
        let ok = format!("{}key_pattern = [[':(\\w+)', '(?P<$1>[^/]+)']]\nkey_input = [['/$', '']]\nkey_text = [['a', 'b']]\nkey_match = \"within\"\nkey_min_literal = 4\n", base);
        let q = parse_file("f.toml", &ok).unwrap().remove(0);
        assert_eq!(q.key_pattern, vec![(r":(\w+)".to_string(), "(?P<$1>[^/]+)".to_string())]);
        assert_eq!((q.key_match, q.key_min_literal, q.key_input.len(), q.key_text.len()), (KeyMatch::Within, 4, 1, 1));
        for (extra, want) in [
            ("key_pattern = ['a', 'b']", "key_pattern"),
            ("key_pattern = [['a']]", "key_pattern"),
            ("key_input = [['(', 'b']]", "does not compile"),
            ("key_match = \"part\"", "key_match"),
            ("key_min_literal = -1", "key_min_literal"),
        ] {
            let e = parse_file("f.toml", &format!("{}{}\n", base, extra)).unwrap_err();
            assert!(e.message.contains(want), "{}: {}", extra, e);
        }
    }

    #[test]
    fn bad_rewrite_and_lone_paths_are_errors() {
        for (extra, want) in [
            ("rewrite = [\"a\"]", "rewrite"),
            ("rewrite = [\"(\", \"b\"]", "does not compile"),
            ("paths = \"app/**\"", "go together"),
        ] {
            let text = format!("framework = \"f\"\nlangs = [\"ts\"]\n[route]\nmatch = \"(id @key)\"\ntarget = \"def\"\n{}\n", extra);
            let e = parse_file("f.toml", &text).unwrap_err();
            assert!(e.message.contains(want), "{}: {}", extra, e);
            assert_eq!(e.line, Some(3), "{}", e);
        }
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
        assert_eq!(frameworks, vec!["chi", "net-http", "next-app", "rails-routes", "react-router", "solid-routes"]);
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
