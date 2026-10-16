//! Name-matching fallback resolver for languages without a dedicated
//! resolver. Preserves the original calltree behavior: call sites are
//! extracted with the abstract query engine and matched to definitions by
//! bare name; only a project-wide unique name resolves, everything else
//! stays external.

use super::{Confidence, Def, DefId, DefIndex, DefKind, Resolver, ResolvedCall};
use crate::compiler::{self, lang};

pub struct NameResolver {
    lang_name: &'static str,
}

impl NameResolver {
    pub fn new(lang_name: &str) -> Self {
        let lang_name = lang::lang_for_name_canonical(lang_name).unwrap_or("c");
        NameResolver { lang_name }
    }
}

impl Resolver for NameResolver {
    fn collect_defs(&self, file: &str, _source: &[u8]) -> Vec<Def> {
        crate::tree::collect_function_locs(file, self.lang_name)
            .into_iter()
            .map(|(name, line)| Def {
                qualified: vec![name.clone()],
                name,
                kind: DefKind::Function,
                ret: None,
                id: DefId { file: file.to_string(), line },
            })
            .collect()
    }

    fn resolve_calls(&self, file: &str, source: &[u8], index: &DefIndex) -> Vec<ResolvedCall> {
        let Some(lang_box) = lang::for_name(self.lang_name) else { return vec![] };
        let mine: Vec<(usize, &Def)> = index.defs.iter().enumerate().filter(|(_, d)| d.id.file == file).collect();
        if mine.is_empty() {
            return vec![];
        }
        // One parse serves every function in the file.
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&lang_box.ts_language()).is_err() {
            return vec![];
        }
        let Some(tree) = parser.parse(source, None) else { return vec![] };
        // One query run serves every function too, when the query with any
        // name is the per-name query with a match-anything name predicate:
        // its rows filtered by name are that name's rows.
        let any = AnyName::new(lang_box.as_ref());
        let mut named: Option<Vec<(crate::runner::MatchRow, String)>> = None;
        let mut out = Vec::new();
        for (caller, def) in mine {
            let query_str = format!("(*function#{} @f).body((&function @c))", def.name);
            let Ok(ast) = crate::query::parse(&query_str) else { continue };
            let compiled = compiler::compile(&ast, lang_box.as_ref());
            if compiled.is_empty() { continue; }
            let matches: Vec<String> = match any.as_ref().filter(|a| a.covers(&compiled, &def.name)) {
                Some(a) => {
                    let rows = named.get_or_insert_with(|| {
                        crate::runner::run_rows_named(&a.query, lang_box.as_ref(), source, &tree).unwrap_or_default()
                    });
                    rows.iter().filter(|(_, n)| *n == def.name).map(|(r, _)| r.node_text.clone()).collect()
                }
                None => {
                    let Ok(rows) = crate::runner::run_rows_on(&compiled, lang_box.as_ref(), source, &tree, false)
                    else { continue };
                    rows.into_iter().map(|r| r.node_text).collect()
                }
            };
            for m in matches {
                let (Some(callee), Some(line)) = (parse_capture("c", &m), parse_line("c", &m))
                else { continue };
                let target = match index.by_name.get(&callee).map(|v| v.as_slice()) {
                    Some([single]) => Some((*single, Confidence::Exact)),
                    _ => None,
                };
                out.push(ResolvedCall { caller, callee_display: callee, line, target });
            }
        }
        out
    }
}

/// The call-site query with a match-anything function name, kept only when it
/// differs from the per-name query in its name predicates alone.
struct AnyName {
    query: compiler::CompiledQuery,
    /// The per-name query's outer pattern with a placeholder for the name.
    outer_with: String,
    inner: String,
    scope_field: String,
}

const PLACEHOLDER: &str = "ctsqanyname";

impl AnyName {
    fn new(lang: &dyn lang::Lang) -> Option<AnyName> {
        let compile = |sel: &str| {
            let q = format!("(*function#{} @f).body((&function @c))", sel);
            crate::query::parse(&q).ok().map(|ast| compiler::compile(&ast, lang))
        };
        let with = compile(PLACEHOLDER)?;
        let query = compile("/./")?;
        let (
            compiler::CompiledQuery::Scoped { outer: ow, scope_field: fw, inner: iw },
            compiler::CompiledQuery::Scoped { outer: oa, scope_field: fa, inner: ia },
        ) = (&with, &query)
        else {
            return None;
        };
        let eq = regex::Regex::new(&format!(r#"\(#eq\? @(_n\d+) "{}"\)"#, PLACEHOLDER)).ok()?;
        let (ow, iw) = (numbered(ow), numbered(iw));
        if fw != fa || iw != numbered(ia) || eq.replace_all(&ow, r#"(#match? @$1 ".")"#) != numbered(oa) || ow.contains("#match?") {
            return None;
        }
        Some(AnyName { outer_with: ow, inner: iw, scope_field: fw.clone(), query })
    }

    /// Whether `compiled`, the query for function `name`, is this query with
    /// `name` in place of the match-anything predicate.
    fn covers(&self, compiled: &compiler::CompiledQuery, name: &str) -> bool {
        match compiled {
            compiler::CompiledQuery::Scoped { outer, scope_field, inner } => {
                *scope_field == self.scope_field
                    && numbered(inner) == self.inner
                    && numbered(outer) == self.outer_with.replace(&format!("\"{}\"", PLACEHOLDER), &format!("\"{}\"", name))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_any_name_query_covers_the_per_name_query_in_every_fallback_language() {
        for l in ["c", "cpp", "javascript", "typescript", "tsx", "python", "ruby"] {
            let lang = lang::for_name(l).unwrap();
            let any = AnyName::new(lang.as_ref()).unwrap_or_else(|| panic!("no any-name query for {}", l));
            let q = crate::query::parse("(*function#main @f).body((&function @c))").unwrap();
            assert!(any.covers(&compiler::compile(&q, lang.as_ref()), "main"), "{}", l);
            assert!(!any.covers(&compiler::compile(&q, lang.as_ref()), "other"), "{}", l);
        }
        assert_eq!(numbered("(x @_n24) @_root3 (#eq? @_n24 \"a\")"), "(x @_n0) @_root1 (#eq? @_n0 \"a\")");
    }

    #[test]
    fn one_query_run_gives_the_rows_each_per_name_run_gives() {
        let src = b"function a() { g(); h(); }\nconst b = () => g(), c = () => g();\nclass K { a() { k(); } }\n";
        let lang = lang::for_name("javascript").unwrap();
        let tree = crate::scope::parse("javascript", src).unwrap();
        let any = AnyName::new(lang.as_ref()).unwrap();
        let rows = crate::runner::run_rows_named(&any.query, lang.as_ref(), src, &tree).unwrap();
        for name in ["a", "b", "c", "K"] {
            let q = crate::query::parse(&format!("(*function#{} @f).body((&function @c))", name)).unwrap();
            let per = crate::runner::run_rows_on(&compiler::compile(&q, lang.as_ref()), lang.as_ref(), src, &tree, false).unwrap();
            let per: Vec<String> = per.into_iter().map(|r| r.node_text).collect();
            let one: Vec<String> = rows.iter().filter(|(_, n)| n == name).map(|(r, _)| r.node_text.clone()).collect();
            assert_eq!(one, per, "{}", name);
        }
    }
}

/// `pattern` with its internal capture names (`@_n24`, `@_root3`) renumbered
/// in order of first appearance. The compiler numbers them from a counter that
/// runs across compiles, so two compiles of one selector differ only there.
fn numbered(pattern: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"@(_[A-Za-z]+)(\d+)").unwrap());
    let mut seen: Vec<String> = Vec::new();
    re.replace_all(pattern, |c: &regex::Captures| {
        let k = match seen.iter().position(|s| s == &c[0]) {
            Some(k) => k,
            None => {
                seen.push(c[0].to_string());
                seen.len() - 1
            }
        };
        format!("@{}{}", &c[1], k)
    })
    .into_owned()
}

/// Extract the text of a named capture from a runner output line.
/// Runner format: `@name:LINE: "text"` or `@name:LINE: text`
pub(crate) fn parse_capture(name: &str, m: &str) -> Option<String> {
    let prefix = format!("@{}:", name);
    for part in m.split(", ") {
        let part = part.trim();
        if part.starts_with(&prefix) {
            // @name:LINE: "text"
            let after_line = part[prefix.len()..].splitn(2, ": ").nth(1)?;
            let text = after_line.trim_matches('"');
            // call_expression text is `foo(args...)` — take only the callee name
            let name = text.split('(').next().unwrap_or(text).trim();
            // For method calls `obj.method` take only the method name
            let name = name.split('.').last().unwrap_or(name).trim();
            if name.is_empty() { continue; }
            return Some(name.to_string());
        }
    }
    None
}

pub(crate) fn parse_line(name: &str, m: &str) -> Option<usize> {
    let prefix = format!("@{}:", name);
    for part in m.split(", ") {
        let part = part.trim();
        if part.starts_with(&prefix) {
            let after = &part[prefix.len()..];
            let line_str = after.splitn(2, ':').next()?;
            return line_str.parse().ok();
        }
    }
    None
}
