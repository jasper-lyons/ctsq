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
        let mut out = Vec::new();
        for (caller, def) in index.defs.iter().enumerate().filter(|(_, d)| d.id.file == file) {
            let query_str = format!("(*function#{} @f).body((&function @c))", def.name);
            let Ok(ast) = crate::query::parse(&query_str) else { continue };
            let compiled = compiler::compile(&ast, lang_box.as_ref());
            if compiled.is_empty() { continue; }
            let Ok(matches) = crate::runner::run(&compiled, lang_box.as_ref(), source, false)
            else { continue };
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
