//! The repo glossary, `.ctsq/glossary.toml` at the repo root. It maps product
//! words to the names they have in code:
//!
//! ```toml
//! "pipeline page" = ["Pipeline"]
//! "brief sync"    = ["sync_merged", "cmd_sync"]
//! "supplies"      = ["/resource|stock/"]
//! ```
//!
//! Keys match case-insensitively as whole phrases. A value is a name, or a
//! `/regex/` name filter. Only the repo layer is read: the glossary is written
//! by people or agents for one repo and never inferred.
//!
//! `find`, `def` and the miss reports ask `Glossary::lookup` and print the line
//! `Entry::note` returns to say they used it.

use super::{ConfigError, Roots};
use std::path::Path;

/// One mapped name: exact text, or a regex over names.
#[derive(Debug, Clone)]
pub enum Name {
    Exact(String),
    Re(regex::Regex),
}

impl Name {
    fn parse(raw: &str) -> Result<Name, String> {
        if raw.len() >= 2 && raw.starts_with('/') && raw.ends_with('/') {
            regex::Regex::new(&raw[1..raw.len() - 1])
                .map(Name::Re)
                .map_err(|e| format!("the regex {} does not compile: {}", raw, e.to_string().lines().last().unwrap_or("").trim()))
        } else {
            Ok(Name::Exact(raw.to_string()))
        }
    }

    /// Whether `name` is this mapped name.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Name::Exact(n) => n == name,
            Name::Re(r) => r.is_match(name),
        }
    }

    /// Whether free text, such as a row's key, holds this name as a whole word.
    pub fn found_in(&self, text: &str) -> bool {
        match self {
            Name::Exact(n) => {
                let word = |c: char| c.is_alphanumeric() || c == '_';
                text.match_indices(n.as_str()).any(|(i, m)| {
                    let before = text[..i].chars().next_back().is_none_or(|c| !word(c));
                    let after = text[i + m.len()..].chars().next().is_none_or(|c| !word(c));
                    before && after
                })
            }
            Name::Re(r) => r.is_match(text),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// The key as written in the file.
    pub term: String,
    /// The values as written, for printing.
    pub raw: Vec<String>,
    pub names: Vec<Name>,
    /// 1-based line of the key in the file.
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Glossary {
    /// The file as printed.
    pub file: String,
    pub entries: Vec<Entry>,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

impl Entry {
    /// `.ctsq/glossary.toml:3: "pipeline page" also searched as Pipeline`
    pub fn note(&self, file: &str) -> String {
        format!("{}:{}: \"{}\" also searched as {}", file, self.line, self.term, self.raw.join(", "))
    }
}

impl Glossary {
    /// The entries whose term `phrase` is. With `whole` false the term only has
    /// to occur in `phrase` as a whole phrase (`the pipeline page` holds
    /// `pipeline page`, `pipelines` does not hold `pipeline`).
    pub fn lookup(&self, phrase: &str, whole: bool) -> Vec<&Entry> {
        let hay = words(phrase);
        self.entries
            .iter()
            .filter(|e| {
                let t = words(&e.term);
                if t.is_empty() {
                    return false;
                }
                if whole {
                    hay == t
                } else {
                    hay.windows(t.len()).any(|w| w == t.as_slice())
                }
            })
            .collect()
    }
}

/// The line a key sits on, 1-based. 1 when not found.
fn key_line(text: &str, key: &str) -> usize {
    let forms = [format!("\"{}\"", key), format!("'{}'", key), key.to_string()];
    text.lines()
        .position(|l| {
            let t = l.trim_start();
            forms.iter().any(|f| t.starts_with(f.as_str()) && t[f.len()..].trim_start().starts_with('='))
        })
        .map(|i| i + 1)
        .unwrap_or(1)
}

pub fn parse(label: &str, text: &str) -> Result<Glossary, ConfigError> {
    let table = super::parse_table(label, text)?;
    let mut entries = Vec::new();
    for (term, v) in &table {
        let line = key_line(text, term);
        let bad = |m: &str| ConfigError::new(label, Some(line), Some(term.clone()), m);
        let raw: Vec<String> = match v {
            toml::Value::String(s) => vec![s.clone()],
            toml::Value::Array(a) => {
                let mut out = Vec::new();
                for x in a {
                    match x.as_str() {
                        Some(s) => out.push(s.to_string()),
                        None => return Err(bad("expected a name or a list of names")),
                    }
                }
                out
            }
            _ => return Err(bad("expected a name or a list of names")),
        };
        if raw.is_empty() || raw.iter().any(|r| r.is_empty()) {
            return Err(bad("needs at least one non-empty name"));
        }
        if words(term).is_empty() {
            return Err(bad("the key has no letters or digits"));
        }
        let mut names = Vec::new();
        for r in &raw {
            names.push(Name::parse(r).map_err(|m| bad(&m))?);
        }
        entries.push(Entry { term: term.clone(), raw, names, line });
    }
    entries.sort_by_key(|e| e.line);
    Ok(Glossary { file: label.to_string(), entries })
}

/// The glossary in the repo layer of `roots`, None when there is no file.
/// `cwd` makes the printed path relative.
pub fn load_with(roots: &Roots, cwd: &Path) -> Result<Option<Glossary>, ConfigError> {
    let Some(repo) = &roots.repo else { return Ok(None) };
    let p = repo.join("glossary.toml");
    if !p.is_file() {
        return Ok(None);
    }
    let text = super::read(&p)?;
    let label = p.strip_prefix(cwd).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
    parse(&label, &text).map(Some)
}

/// `load_with` for the repo above `cwd`.
pub fn load(cwd: &Path) -> Result<Option<Glossary>, ConfigError> {
    let root = super::find_repo_root(cwd);
    load_with(&Roots::from_env(&root), cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# product words\n\"pipeline page\" = [\"Pipeline\"]\n\"brief sync\"    = [\"sync_merged\", \"cmd_sync\"]\n\"supplies\"      = [\"/resource|stock/\"]\n";

    #[test]
    fn a_file_gives_one_entry_per_key_with_its_line() {
        let g = parse(".ctsq/glossary.toml", TEXT).unwrap();
        let lines: Vec<(&str, usize)> = g.entries.iter().map(|e| (e.term.as_str(), e.line)).collect();
        assert_eq!(lines, vec![("pipeline page", 2), ("brief sync", 3), ("supplies", 4)]);
        assert_eq!(g.entries[1].raw, vec!["sync_merged", "cmd_sync"]);
        assert!(matches!(g.entries[2].names[0], Name::Re(_)));
    }

    #[test]
    fn keys_match_as_whole_phrases_without_regard_to_case() {
        let g = parse("g.toml", TEXT).unwrap();
        assert_eq!(g.lookup("Pipeline Page", true).len(), 1);
        assert_eq!(g.lookup("pipeline", true).len(), 0, "a part of the phrase is not the phrase");
        assert_eq!(g.lookup("the pipeline page is slow", false).len(), 1);
        assert_eq!(g.lookup("the pipeline pages", false).len(), 0, "pages is not page");
        assert_eq!(g.lookup("the pipeline page is slow", true).len(), 0, "def needs the whole name");
        assert_eq!(g.lookup("brief-sync", true).len(), 1, "punctuation between words does not matter");
    }

    #[test]
    fn names_and_regex_names_match() {
        let g = parse("g.toml", TEXT).unwrap();
        let sync = &g.entries[1].names;
        assert!(sync[0].matches("sync_merged") && !sync[0].matches("sync_merged2"));
        let re = &g.entries[2].names[0];
        assert!(re.matches("stockLevel") && re.matches("resources") && !re.matches("Pipeline"));
        assert!(sync[0].found_in("run sync_merged now") && !sync[0].found_in("sync_merged_all"));
    }

    #[test]
    fn a_note_starts_with_the_file_and_line() {
        let g = parse("g.toml", TEXT).unwrap();
        assert_eq!(g.entries[0].note(".ctsq/glossary.toml"), ".ctsq/glossary.toml:2: \"pipeline page\" also searched as Pipeline");
        assert_eq!(g.entries[1].note("g"), "g:3: \"brief sync\" also searched as sync_merged, cmd_sync");
    }

    #[test]
    fn bad_values_name_the_file_line_and_key() {
        for (text, want) in [
            ("\"a\" = 3\n", "expected a name"),
            ("\"a\" = []\n", "at least one"),
            ("\"a\" = [\"/(/\"]\n", "does not compile"),
            ("\"--\" = [\"x\"]\n", "no letters"),
        ] {
            let e = parse("g.toml", text).unwrap_err();
            assert!(e.to_string().contains(want), "{}: {}", text, e);
            assert_eq!(e.line, Some(1), "{}", e);
        }
    }
}
