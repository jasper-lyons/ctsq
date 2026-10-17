//! Reports for a search that found nothing, and for a query that never ran.
//!
//! A miss says what was searched, and where the name does occur if it occurs
//! under another reading: an `id#` query that matches no identifier reruns the
//! name over string literals, JSX text and comments, and a definition query
//! that matches nothing looks for the same name under the other definition
//! kinds. A query error prints the query with a caret under the fault.
//!
//! `report`, `query_error_report` and `no_such_file` return strings so tests
//! can assert on the whole output. `report_for` does the discovery and calls
//! `report`.

use crate::compiler::{self, lang};
use crate::query::ast::*;
use crate::needle::Needles;
use crate::runner;
use std::collections::HashSet;

/// How many hits one found-as block shows before it says how many more.
const MAX_ROWS: usize = 10;

/// What kind of miss a query is, which picks the wording and the fallback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MissKind {
    /// A lone `id#name`.
    Identifier,
    /// A lone `*kind#name`.
    Definition,
    Other,
}

/// Hits for the same name read as text instead of as an identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundBlock {
    /// `string literal`, `JSX text` or `comment`.
    pub label: &'static str,
    /// `path:line: source line`, one per hit.
    pub rows: Vec<String>,
    /// A query that matches these rows, without the `ctsq -q` around it.
    pub try_query: String,
    /// `-l` values the try line needs because the kind exists only in some
    /// grammars (`jsx_text`). One try line per value; empty means no `-l`.
    pub langs: Vec<&'static str>,
}

/// The same name defined under another definition kind.
#[derive(Debug, Clone, PartialEq)]
pub struct DefinedAs {
    pub kind: &'static str,
    pub name: String,
    /// `path:line`, one per place.
    pub hits: Vec<String>,
    pub try_query: String,
}

/// Single quotes around a query for the `try:` line, with any quote inside it
/// closed and reopened so the line pastes into a shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn report(
    kind: MissKind,
    query: &str,
    scope: &str,
    files: usize,
    found: &[FoundBlock],
    defined: &[DefinedAs],
) -> String {
    let noun = match kind {
        MissKind::Identifier => "no identifier matches",
        MissKind::Definition => "no definition matches",
        MissKind::Other => "no match for",
    };
    let plural = if files == 1 { "file" } else { "files" };
    let mut out = format!("{} {} in {} ({} {})\n", noun, query, scope, files, plural);
    if files == 0 {
        out.push_str("no file under that path has an extension ctsq reads, or all of them are ignored (--no-ignore searches ignored files too)\n");
    }
    for block in found {
        out.push_str(&format!("found as {}:\n", block.label));
        for row in block.rows.iter().take(MAX_ROWS) {
            out.push_str(row);
            out.push('\n');
        }
        if block.rows.len() > MAX_ROWS {
            out.push_str(&format!("({} more)\n", block.rows.len() - MAX_ROWS));
        }
    }
    for d in defined {
        for hit in &d.hits {
            out.push_str(&format!("{} is defined as a {} here: {}\n", d.name, d.kind, hit));
        }
    }
    for block in found {
        if block.langs.is_empty() {
            out.push_str(&format!("try: ctsq -q {} {}\n", shell_quote(&block.try_query), scope));
        }
        for l in &block.langs {
            out.push_str(&format!("try: ctsq -l {} -q {} {}\n", l, shell_quote(&block.try_query), scope));
        }
    }
    for d in defined {
        out.push_str(&format!("try: ctsq -q {} {}\n", shell_quote(&d.try_query), scope));
    }
    out
}

/// A query error: the message, the query, a caret under the fault, then the fix
/// when one is known. The query is indented two spaces and the caret sits under
/// the character at `pos`.
pub fn query_error_report(query: &str, e: &QueryError) -> String {
    let cut = e.pos.min(query.len());
    let cut = (0..=cut).rev().find(|i| query.is_char_boundary(*i)).unwrap_or(0);
    let col = query[..cut].chars().count();
    let mut out = format!("query error: {}\n  {}\n  {}^\n", e, query, " ".repeat(col));
    if let Some(h) = &e.hint {
        out.push_str(h);
        out.push('\n');
    }
    out
}

/// A `ctsq find <query> [key]` that matched nothing: the query, the key, each
/// framework tried with the file its query came from, and the frameworks whose
/// query exists but did not detect here.
pub fn find_report(
    query: &str,
    key: Option<&str>,
    tried: &[(String, String)],
    not_detected: &[(String, String)],
) -> String {
    let what = match key {
        Some(k) => format!("no {} matches {} here", query, k),
        None => format!("no {} found here", query),
    };
    let mut out = if tried.is_empty() {
        format!("{}, no framework with a {} query applies\n", what, query)
    } else {
        let list: Vec<String> = tried.iter().map(|(f, s)| format!("{} from {}", f, s)).collect();
        format!("{} (tried {})\n", what, list.join(", "))
    };
    if !not_detected.is_empty() {
        let list: Vec<String> = not_detected.iter().map(|(f, s)| format!("{} ({})", f, s)).collect();
        out.push_str(&format!("not detected here: {}\n", list.join(", ")));
    }
    out
}

// ---------------------------------------------------------------------------
// Missing paths
// ---------------------------------------------------------------------------

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur.push(sub.min(prev[j] + 1).min(cur[j - 1] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The sibling of `path` whose name is closest to the last segment, if one is close.
pub fn did_you_mean(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let (dir, name) = match trimmed.rsplit_once('/') {
        Some((d, n)) => (if d.is_empty() { "/" } else { d }, n),
        None => (".", trimmed),
    };
    if name.is_empty() {
        return None;
    }
    let low = name.to_lowercase();
    let limit = (name.chars().count() / 3).max(1);
    let mut best: Option<(usize, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Some(cand) = entry.file_name().to_str().map(str::to_string) else { continue };
        let cl = cand.to_lowercase();
        let d = if cl == low {
            0
        } else if low.len() >= 3 && cl.len() >= 3 && (cl.starts_with(&low) || low.starts_with(&cl)) {
            1
        } else {
            edit_distance(&low, &cl)
        };
        if d <= limit && best.as_ref().is_none_or(|(bd, bn)| (d, &cand) < (*bd, bn)) {
            best = Some((d, cand));
        }
    }
    let (_, cand) = best?;
    Some(match trimmed.rsplit_once('/') {
        Some((d, _)) => format!("{}/{}", d, cand),
        None => cand,
    })
}

/// The stderr line for a path argument that does not exist.
pub fn no_such_file(path: &str) -> String {
    match did_you_mean(path) {
        Some(y) => format!("no such file: {} (did you mean {})", path, y),
        None => format!("no such file: {}", path),
    }
}

// ---------------------------------------------------------------------------
// Fallbacks
// ---------------------------------------------------------------------------

/// The shape of query the fallbacks understand: one atom, nothing else.
fn lone_atom(ast: &Query) -> Option<&Atom> {
    if !ast.tail.is_empty() || !ast.head.fields.is_empty() || !ast.head.not.is_empty() {
        return None;
    }
    match &ast.head.node {
        SelectorNode::Bare(a) => Some(a),
        SelectorNode::Group { .. } => None,
    }
}

pub fn kind_of(ast: &Query) -> MissKind {
    match lone_atom(ast) {
        Some(a) if a.name_match.is_some() => match (a.node_type.as_deref(), &a.sigil) {
            (Some("id"), None) => MissKind::Identifier,
            (Some(_), Some(Sigil::Def)) => MissKind::Definition,
            _ => MissKind::Other,
        },
        _ => MissKind::Other,
    }
}

/// The regex source a name filter stands for when read as a substring.
fn as_regex(nm: &NameMatch) -> String {
    match nm {
        NameMatch::Exact(s) => regex::escape(s),
        NameMatch::Regex(r) => r.clone(),
    }
}

fn kind_query(sigil: Option<Sigil>, kind: &str, nm: NameMatch) -> Query {
    Query {
        head: Selector {
            node: SelectorNode::Bare(Atom {
                sigil,
                node_type: Some(kind.to_string()),
                name_match: Some(nm),
                attrs: vec![],
            }),
            fields: vec![],
            not: vec![],
        },
        tail: vec![],
    }
}

/// Rows of `ast` over every readable job, as `path:line: source line`. A job whose
/// language has no such kind, or whose pattern tree-sitter rejects, contributes none.
///
/// `needles` are the texts every match holds (the name filter's), so a file
/// without those bytes is neither parsed nor searched.
fn rows_over(ast: &Query, jobs: &[(String, &'static str)], needles: &Needles) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut cache: std::collections::HashMap<&'static str, Option<(Box<dyn lang::Lang>, compiler::CompiledQuery)>> =
        std::collections::HashMap::new();
    for (file, lang_name) in jobs {
        if file == "-" {
            continue;
        }
        let slot = cache.entry(lang_name).or_insert_with(|| {
            let l = lang::for_name(lang_name)?;
            if compiler::validate(ast, l.as_ref(), lang_name).is_err() {
                return None;
            }
            let c = compiler::compile(ast, l.as_ref());
            if c.is_empty() {
                return None;
            }
            Some((l, c))
        });
        let Some((l, c)) = slot else { continue };
        let Ok(source) = std::fs::read(file) else { continue };
        if !needles.holds(&source) {
            continue;
        }
        let Ok(rows) = runner::run_rows(c, l.as_ref(), &source, false) else { continue };
        for r in rows {
            let text = r.src_line.unwrap_or(r.node_text);
            out.push((file.clone(), r.line, text));
        }
    }
    out
}

/// Rerun the name of a lone `id#name` over text: string literals, JSX text, comments.
pub fn id_fallback(ast: &Query, jobs: &[(String, &'static str)]) -> Vec<FoundBlock> {
    let Some(atom) = lone_atom(ast) else { return vec![] };
    let Some(nm) = &atom.name_match else { return vec![] };
    let pattern = as_regex(nm);
    let mut blocks = Vec::new();
    for (label, kind) in [("string literal", "string"), ("JSX text", "jsx_text"), ("comment", "comment")] {
        let q = kind_query(None, kind, NameMatch::Regex(pattern.clone()));
        let mut seen = HashSet::new();
        // A case-sensitive match of the name holds its literal bytes.
        let hits = rows_over(&q, jobs, &Needles::from_name(nm));
        // jsx_text is a kind of the javascript and tsx grammars only, so its
        // try line selects the grammar the hits came from.
        let mut langs: Vec<&'static str> = Vec::new();
        if kind == "jsx_text" {
            for (f, _, _) in &hits {
                if let Some((_, l)) = jobs.iter().find(|(jf, _)| jf == f) {
                    if !langs.contains(l) {
                        langs.push(l);
                    }
                }
            }
            langs.sort_unstable();
        }
        let rows: Vec<String> = hits
            .into_iter()
            .map(|(f, line, text)| format!("{}:{}: {}", f, line, text))
            .filter(|r| seen.insert(r.clone()))
            .collect();
        if !rows.is_empty() {
            blocks.push(FoundBlock {
                label,
                rows,
                try_query: format!("{}#/{}/", kind, pattern.replace('/', "\\/")),
                langs,
            });
        }
    }
    blocks
}

/// The kinds that introduce a name, which a definition miss checks in turn.
const DEFINITION_KINDS: &[&str] = &["function", "type", "class", "module", "variable"];

/// For a lone `*kind#name` that matched nothing, the other definition kinds the name has.
pub fn definition_fallback(ast: &Query, jobs: &[(String, &'static str)]) -> Vec<DefinedAs> {
    let Some(atom) = lone_atom(ast) else { return vec![] };
    let (Some(asked), Some(nm)) = (atom.node_type.as_deref(), &atom.name_match) else { return vec![] };
    let asked = match asked {
        "method" => "function",
        "const" | "var" => "variable",
        other => other,
    };
    let name = match nm {
        NameMatch::Exact(s) => s.clone(),
        NameMatch::Regex(r) => format!("/{}/", r),
    };
    let mut reported: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for kind in DEFINITION_KINDS.iter().copied().filter(|k| *k != asked) {
        let q = kind_query(Some(Sigil::Def), kind, nm.clone());
        let mut hits = Vec::new();
        let needles = Needles::from_name(&nm);
        for (f, line, _) in rows_over(&q, jobs, &needles) {
            let hit = format!("{}:{}", f, line);
            if reported.insert(hit.clone()) {
                hits.push(hit);
            }
        }
        hits.truncate(3);
        if !hits.is_empty() {
            let try_query = compiler::render_atom(&Atom {
                sigil: Some(Sigil::Def),
                node_type: Some(kind.to_string()),
                name_match: Some(nm.clone()),
                attrs: vec![],
            });
            out.push(DefinedAs { kind, name: name.clone(), hits, try_query });
        }
    }
    out
}

/// The whole miss report for a query that matched nothing in `jobs`.
pub fn report_for(ast: &Query, query: &str, scope: &str, jobs: &[(String, &'static str)]) -> String {
    let kind = kind_of(ast);
    let (found, defined) = match kind {
        MissKind::Identifier => (id_fallback(ast, jobs), vec![]),
        MissKind::Definition => (vec![], definition_fallback(ast, jobs)),
        MissKind::Other => (vec![], vec![]),
    };
    report(kind, query, scope, jobs.len(), &found, &defined)
}

/// `report_for`, plus the repo glossary: when the missed name is a glossary
/// term, the same kind of query runs for each name it maps to. What they found
/// is shown with a `try:` line each, and a last line names the glossary file
/// and line that was used.
pub fn report_for_glossary(
    ast: &Query,
    query: &str,
    scope: &str,
    jobs: &[(String, &'static str)],
    glossary: Option<&crate::config::glossary::Glossary>,
) -> String {
    let mut out = report_for(ast, query, scope, jobs);
    let Some(g) = glossary else { return out };
    let Some(atom) = lone_atom(ast) else { return out };
    let (Some(kind), Some(NameMatch::Exact(asked))) = (atom.node_type.as_deref(), &atom.name_match) else { return out };
    let mut notes = Vec::new();
    for entry in g.lookup(asked, true) {
        for (raw, name) in entry.raw.iter().zip(&entry.names) {
            let nm = match name {
                crate::config::glossary::Name::Exact(n) => NameMatch::Exact(n.clone()),
                crate::config::glossary::Name::Re(r) => NameMatch::Regex(r.as_str().to_string()),
            };
            let q = kind_query(atom.sigil.clone(), kind, nm.clone());
            let mut seen = HashSet::new();
            let rows: Vec<String> = rows_over(&q, jobs, &Needles::from_name(&nm))
                .into_iter()
                .map(|(f, line, text)| format!("{}:{}: {}", f, line, text))
                .filter(|r| seen.insert(r.clone()))
                .collect();
            if rows.is_empty() {
                continue;
            }
            out.push_str(&format!("found as {}:\n", raw));
            for r in rows.iter().take(MAX_ROWS) {
                out.push_str(r);
                out.push('\n');
            }
            if rows.len() > MAX_ROWS {
                out.push_str(&format!("({} more)\n", rows.len() - MAX_ROWS));
            }
            let try_query = compiler::render_atom(&Atom {
                sigil: atom.sigil.clone(),
                node_type: Some(kind.to_string()),
                name_match: Some(nm),
                attrs: vec![],
            });
            out.push_str(&format!("try: ctsq -q {} {}\n", shell_quote(&try_query), scope));
        }
        notes.push(entry.note(&g.file));
    }
    for n in notes {
        out.push_str(&n);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-miss-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn jobs_of(files: &[(&std::path::Path, &'static str)]) -> Vec<(String, &'static str)> {
        files.iter().map(|(p, l)| (p.to_str().unwrap().to_string(), *l)).collect()
    }

    /// Run a bare query over one source and return the 1-based lines it hits.
    fn lines_hit(query: &str, lang_name: &str, source: &str) -> Vec<usize> {
        let ast = crate::query::parse(query).unwrap();
        let l = lang::for_name(lang_name).unwrap();
        let c = compiler::compile(&ast, l.as_ref());
        runner::run_rows(&c, l.as_ref(), source.as_bytes(), false)
            .unwrap()
            .iter()
            .map(|r| r.line)
            .collect()
    }

    #[test]
    fn the_report_has_the_output_format_shape() {
        let found = vec![FoundBlock {
            label: "string literal",
            rows: vec!["frontend/src/Chat.tsx:118: <p class=\"hint\">Still reading your files</p>".into()],
            try_query: "string#/Still reading your files/".into(),
            langs: vec![],
        }];
        let text = report(MissKind::Identifier, "id#/Still reading your files/", "frontend/src", 212, &found, &[]);
        assert_eq!(
            text,
            "no identifier matches id#/Still reading your files/ in frontend/src (212 files)\n\
             found as string literal:\n\
             frontend/src/Chat.tsx:118: <p class=\"hint\">Still reading your files</p>\n\
             try: ctsq -q 'string#/Still reading your files/' frontend/src\n"
        );
    }

    #[test]
    fn each_kind_found_gets_its_own_block_and_try_line() {
        let found = vec![
            FoundBlock { label: "string literal", rows: vec!["a.rs:1: x".into()], try_query: "string#/x/".into(), langs: vec![] },
            FoundBlock { label: "comment", rows: vec!["a.rs:2: // x".into()], try_query: "comment#/x/".into(), langs: vec![] },
        ];
        let text = report(MissKind::Identifier, "id#x", "src", 1, &found, &[]);
        assert!(text.starts_with("no identifier matches id#x in src (1 file)\n"), "{}", text);
        assert!(text.contains("found as string literal:\na.rs:1: x\nfound as comment:\na.rs:2: // x\n"), "{}", text);
        assert!(text.contains("try: ctsq -q 'string#/x/' src\ntry: ctsq -q 'comment#/x/' src\n"), "{}", text);
    }

    #[test]
    fn a_definition_miss_names_the_kind_it_is_defined_as() {
        let d = vec![DefinedAs {
            kind: "variable",
            name: "config".into(),
            hits: vec!["src/a.ts:3".into()],
            try_query: "*variable#config".into(),
        }];
        let text = report(MissKind::Definition, "*function#config", "src", 4, &[], &d);
        assert_eq!(
            text,
            "no definition matches *function#config in src (4 files)\n\
             config is defined as a variable here: src/a.ts:3\n\
             try: ctsq -q '*variable#config' src\n"
        );
    }

    #[test]
    fn a_plain_miss_keeps_the_no_match_wording() {
        let text = report(MissKind::Other, "function#x y", "src/", 2, &[], &[]);
        assert_eq!(text, "no match for function#x y in src/ (2 files)\n");
    }

    #[test]
    fn zero_files_says_why() {
        let text = report(MissKind::Other, "id#x", "docs", 0, &[], &[]);
        assert!(text.starts_with("no match for id#x in docs (0 files)\n"), "{}", text);
        assert!(text.contains("extension ctsq reads"), "{}", text);
    }

    #[test]
    fn a_try_line_quotes_a_query_holding_a_single_quote() {
        let found = vec![FoundBlock { label: "comment", rows: vec!["a.c:1: x".into()], try_query: "comment#/it's/".into(), langs: vec![] }];
        let text = report(MissKind::Identifier, "id#x", "src", 1, &found, &[]);
        assert!(text.contains("try: ctsq -q 'comment#/it'\\''s/' src"), "{}", text);
    }

    #[test]
    fn long_blocks_are_cut_with_a_count() {
        let rows: Vec<String> = (1..=13).map(|i| format!("a.rs:{}: x", i)).collect();
        let found = vec![FoundBlock { label: "comment", rows, try_query: "comment#/x/".into(), langs: vec![] }];
        let text = report(MissKind::Identifier, "id#x", "src", 1, &found, &[]);
        assert!(text.contains("a.rs:10: x\n(3 more)\n"), "{}", text);
        assert!(!text.contains("a.rs:11:"), "{}", text);
    }

    #[test]
    fn an_id_miss_finds_the_name_in_a_string_literal_and_the_try_line_matches() {
        let d = scratch("str");
        let f = d.join("a.rs");
        let src = "fn main() {\n    println!(\"Cannot detect the thing\");\n}\n";
        fs::write(&f, src).unwrap();
        let ast = crate::query::parse("id#/Cannot detect/").unwrap();
        let jobs = jobs_of(&[(&f, "rust")]);
        let blocks = id_fallback(&ast, &jobs);
        assert_eq!(blocks.len(), 1, "{:?}", blocks);
        assert_eq!(blocks[0].label, "string literal");
        assert_eq!(blocks[0].rows, vec![format!("{}:2: println!(\"Cannot detect the thing\");", f.display())]);
        assert_eq!(lines_hit(&blocks[0].try_query, "rust", src), vec![2], "{}", blocks[0].try_query);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_plain_name_is_a_case_sensitive_substring_of_the_text() {
        let d = scratch("sub");
        let f = d.join("a.py");
        let src = "x = 'Still reading your files...'\n# still reading is lower case\n";
        fs::write(&f, src).unwrap();
        let jobs = jobs_of(&[(&f, "python")]);
        let ast = crate::query::parse("id#\"Still reading\"").unwrap();
        let blocks = id_fallback(&ast, &jobs);
        assert_eq!(blocks.len(), 1, "{:?}", blocks);
        assert_eq!(blocks[0].label, "string literal");
        assert_eq!(blocks[0].rows.len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_id_miss_finds_comments_and_jsx_text_each_in_its_own_block() {
        let d = scratch("jsx");
        let f = d.join("Chat.tsx");
        let src = "// Still reading your files\nexport const Chat = () => <p>Still reading your files</p>;\n";
        fs::write(&f, src).unwrap();
        let jobs = jobs_of(&[(&f, "tsx")]);
        let ast = crate::query::parse("id#/Still reading your files/").unwrap();
        let blocks = id_fallback(&ast, &jobs);
        let labels: Vec<&str> = blocks.iter().map(|b| b.label).collect();
        assert_eq!(labels, vec!["JSX text", "comment"], "{:?}", blocks);
        for b in &blocks {
            assert!(!lines_hit(&b.try_query, "tsx", src).is_empty(), "{} must match: {}", b.label, b.try_query);
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_with_a_slash_or_quote_still_gives_a_try_line_that_matches() {
        let d = scratch("esc");
        let f = d.join("a.js");
        let src = "const u = 'a/b \"c\"';\n";
        fs::write(&f, src).unwrap();
        let jobs = jobs_of(&[(&f, "javascript")]);
        let ast = crate::query::parse("id#\"a/b\"").unwrap();
        let blocks = id_fallback(&ast, &jobs);
        assert_eq!(blocks.len(), 1, "{:?}", blocks);
        assert_eq!(lines_hit(&blocks[0].try_query, "javascript", src), vec![1], "{}", blocks[0].try_query);
        let ast = crate::query::parse("id#\"c\\\"\"").unwrap();
        let blocks = id_fallback(&ast, &jobs);
        assert_eq!(blocks.len(), 1, "{:?}", blocks);
        assert_eq!(lines_hit(&blocks[0].try_query, "javascript", src), vec![1], "{}", blocks[0].try_query);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn only_a_lone_id_query_gets_the_text_fallback() {
        let ast = crate::query::parse("function#x id#y").unwrap();
        assert_eq!(kind_of(&ast), MissKind::Other);
        assert!(id_fallback(&ast, &[]).is_empty());
        assert_eq!(kind_of(&crate::query::parse("id#y").unwrap()), MissKind::Identifier);
        assert_eq!(kind_of(&crate::query::parse("*function#y").unwrap()), MissKind::Definition);
        assert_eq!(kind_of(&crate::query::parse("&function#y").unwrap()), MissKind::Other);
        assert_eq!(kind_of(&crate::query::parse("id").unwrap()), MissKind::Other);
    }

    #[test]
    fn a_definition_miss_finds_the_other_kind_and_the_try_line_matches() {
        let d = scratch("def");
        let f = d.join("a.ts");
        let src = "export const config = { a: 1 };\n";
        fs::write(&f, src).unwrap();
        let ast = crate::query::parse("*function#config").unwrap();
        let jobs = jobs_of(&[(&f, "typescript")]);
        let defined = definition_fallback(&ast, &jobs);
        assert_eq!(defined.len(), 1, "{:?}", defined);
        assert_eq!(defined[0].kind, "variable");
        assert_eq!(defined[0].hits, vec![format!("{}:1", f.display())]);
        assert_eq!(lines_hit(&defined[0].try_query, "typescript", src), vec![1], "{}", defined[0].try_query);
        let text = report_for(&ast, "*function#config", "src", &jobs);
        assert!(text.starts_with("no definition matches *function#config in src (1 file)\n"), "{}", text);
        assert!(text.contains(&format!("config is defined as a variable here: {}:1\n", f.display())), "{}", text);
        assert!(text.ends_with("try: ctsq -q '*variable#config' src\n"), "{}", text);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_definition_miss_with_nothing_elsewhere_is_just_the_header() {
        let d = scratch("none");
        let f = d.join("a.go");
        fs::write(&f, "package main\n\nfunc main() {}\n").unwrap();
        let ast = crate::query::parse("*function#nothing").unwrap();
        let text = report_for(&ast, "*function#nothing", "x", &jobs_of(&[(&f, "go")]));
        assert_eq!(text, "no definition matches *function#nothing in x (1 file)\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_error_report_is_the_output_format_block() {
        let e = crate::query::parse("id#/ui/page/").unwrap_err();
        let text = query_error_report("id#/ui/page/", &e);
        assert_eq!(
            text,
            "query error: unexpected '/' at position 6\n\
             \x20 id#/ui/page/\n\
             \x20       ^\n\
             a '/' inside a regex name filter must be escaped: id#/ui\\/page/\n"
        );
    }

    #[test]
    fn the_caret_counts_characters_not_bytes() {
        let e = QueryError::new("bad", 3);
        let text = query_error_report("é é)", &e);
        assert!(text.contains("\n  é é)\n  ") && text.contains("   ^\n"), "{}", text);
    }

    #[test]
    fn a_missing_path_suggests_a_close_sibling() {
        let d = scratch("path");
        fs::write(d.join("main.rs"), "").unwrap();
        fs::write(d.join("files.rs"), "").unwrap();
        let typo = format!("{}/mian.rs", d.display());
        assert_eq!(no_such_file(&typo), format!("no such file: {} (did you mean {}/main.rs)", typo, d.display()));
        let far = format!("{}/zzzzzzzz.rs", d.display());
        assert_eq!(no_such_file(&far), format!("no such file: {}", far));
        let case = format!("{}/Main.rs", d.display());
        assert!(no_such_file(&case).contains("(did you mean ") && no_such_file(&case).contains("main.rs"), "{}", no_such_file(&case));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_find_miss_names_the_query_key_frameworks_and_sources() {
        let tried = vec![("solid-routes".to_string(), "built-in queries/solid-routes.toml".to_string())];
        let missing = vec![("react-router".to_string(), "built-in queries/react-router.toml".to_string())];
        assert_eq!(
            find_report("route", Some("/nope"), &tried, &missing),
            "no route matches /nope here (tried solid-routes from built-in queries/solid-routes.toml)\n\
             not detected here: react-router (built-in queries/react-router.toml)\n"
        );
        assert_eq!(
            find_report("route", None, &[], &missing),
            "no route found here, no framework with a route query applies\n\
             not detected here: react-router (built-in queries/react-router.toml)\n"
        );
    }

    #[test]
    fn a_missing_directory_has_no_suggestion() {
        assert_eq!(no_such_file("/no/such/dir/zzz"), "no such file: /no/such/dir/zzz");
    }
}
