//! `types` subcommand — a reference table mapping each abstract ("meta") node
//! type to the concrete tree-sitter node(s) it matches, per language. Reads the
//! same `Lang::resolve` mappings the compiler uses, so it never drifts.

use crate::compiler::lang::{for_name, Lang};
use crate::query::ast::Sigil;
use std::collections::HashSet;

/// The fixed abstract-type vocabulary, in a sensible reading order.
/// Also the classification priority order for `example` (query-by-example).
pub const ABSTRACT_TYPES: &[&str] = &[
    "id", "function", "call", "class", "module", "type", "var", "variable",
    "const", "param", "import", "literal", "block", "if", "for", "while",
    "switch", "op", "field", "template_action",
    // Aliases and wider names, appended so `example` classifies by the earlier ones first.
    "method", "string", "comment",
];

/// Every language, in canonical form (tsx included — it adds JSX call forms).
const ALL_LANGS: &[&str] = &[
    "c", "cpp", "javascript", "typescript", "tsx", "python", "ruby", "rust", "go", "gotmpl",
];

pub fn run(langs: &[String], lang_flag: Option<&str>) {
    // Pick targets: explicit positionals, else the global --lang, else all.
    let targets: Vec<String> = if !langs.is_empty() {
        langs.to_vec()
    } else if let Some(l) = lang_flag {
        vec![l.to_string()]
    } else {
        ALL_LANGS.iter().map(|s| s.to_string()).collect()
    };

    println!("Abstract type → tree-sitter node(s). Sigils: *type = definitions, &type = references.");

    for name in &targets {
        let Some(lang) = for_name(name) else {
            eprintln!("unknown language: {}", name);
            continue;
        };
        println!();
        println!("{}", name);
        for ty in ABSTRACT_TYPES {
            println!("  {:<9} {}", ty, cell(lang.as_ref(), ty));
        }
    }
}

/// The right-hand column for one abstract type: the shared node list, or the
/// def/ref split (`*…`/`&…`) when the sigil changes the mapping.
fn cell(lang: &dyn Lang, ty: &str) -> String {
    let defs = names(lang, ty, Sigil::Def);
    let refs = names(lang, ty, Sigil::Ref);
    match (defs, refs) {
        (None, None) => "(passes through as a raw node type)".to_string(),
        (Some(d), Some(r)) if d.is_empty() && r.is_empty() => "—".to_string(),
        (Some(d), Some(r)) if d == r => d.join("  "),
        (d, r) => {
            let mut parts = Vec::new();
            let (d, r) = (d.unwrap_or_default(), r.unwrap_or_default());
            if !d.is_empty() { parts.push(format!("*{}", d.join(" "))); }
            if !r.is_empty() { parts.push(format!("&{}", r.join(" "))); }
            if parts.is_empty() { "—".to_string() } else { parts.join("   ") }
        }
    }
}

/// Distinct concrete node names an abstract type maps to under one sigil.
/// `None` means the type is unknown to this language (compiler passes it through).
fn names(lang: &dyn Lang, ty: &str, sigil: Sigil) -> Option<Vec<String>> {
    lang.resolve(ty, Some(&sigil)).map(|vs| {
        let mut seen = HashSet::new();
        vs.iter().map(|v| v.node.to_string()).filter(|n| seen.insert(n.clone())).collect()
    })
}
