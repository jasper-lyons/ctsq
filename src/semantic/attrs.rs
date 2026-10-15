//! Attribute filtering: `[from=...]`, `[in=...]` and the dot shorthand
//! `[qual=...]`, applied to `-q` rows after tree-sitter matching.
//!
//! Each row's primary node is found again by byte range in a fresh parse of
//! the file. From that node the stage finds the name identifier, its
//! qualifier (`log` in `log.Printf`), the import that qualifier (or the name
//! itself) is bound by, and the nearest enclosing named declaration.

use crate::query::ast::{AttrMatch, AttrOp, Query, Selector, SelectorNode};
use crate::resolve::imports::import_resolver_for;
use crate::runner::MatchRow;
use std::collections::HashMap;
use tree_sitter::{Node, Parser};

/// The attribute keys `-q` understands.
const KNOWN_KEYS: &[&str] = &["from", "in", "qual"];

/// Attributes of the query's last selector. Attributes written anywhere else
/// (an earlier selector, a group, a field inner query, a `:not`) are an error,
/// because a `-q` row only carries the last selector's node.
pub fn last_selector_attrs(query: &Query) -> Result<Vec<AttrMatch>, String> {
    let (last, earlier): (&Selector, Vec<&Selector>) = match query.tail.last() {
        Some((_, sel)) => {
            let mut e = vec![&query.head];
            e.extend(query.tail[..query.tail.len() - 1].iter().map(|(_, s)| s));
            (sel, e)
        }
        None => (&query.head, vec![]),
    };
    let misplaced = earlier.iter().any(|s| selector_has_attrs(s))
        || last.fields.iter().any(|f| f.inner.as_ref().is_some_and(|q| query_has_attrs(q)))
        || last.not.iter().any(selector_has_attrs)
        || matches!(&last.node, SelectorNode::Group { query, .. } if query_has_attrs(query));
    if misplaced {
        return Err("attributes are only supported on the last selector in -q".into());
    }
    let attrs = match &last.node {
        SelectorNode::Bare(atom) => atom.attrs.clone(),
        SelectorNode::Group { .. } => vec![],
    };
    for a in &attrs {
        if !KNOWN_KEYS.contains(&a.key.as_str()) {
            return Err(format!(
                "unknown attribute '{}', known attributes: {}",
                a.key,
                KNOWN_KEYS.join(", ")
            ));
        }
        if a.op == AttrOp::Regex {
            regex::Regex::new(&a.value)
                .map_err(|e| format!("bad regex in attribute '{}': {}", a.key, e))?;
        }
    }
    Ok(attrs)
}

fn query_has_attrs(q: &Query) -> bool {
    selector_has_attrs(&q.head) || q.tail.iter().any(|(_, s)| selector_has_attrs(s))
}

fn selector_has_attrs(sel: &Selector) -> bool {
    let own = match &sel.node {
        SelectorNode::Bare(atom) => !atom.attrs.is_empty(),
        SelectorNode::Group { query, .. } => query_has_attrs(query),
    };
    own || sel.fields.iter().any(|f| f.inner.as_ref().is_some_and(|q| query_has_attrs(q)))
        || sel.not.iter().any(selector_has_attrs)
}

/// Reject attributes a language cannot answer, before any file is read.
pub fn check_lang(attrs: &[AttrMatch], lang_name: &str) -> Result<(), String> {
    if attrs.iter().any(|a| a.key == "from") && !supports_from(lang_name) {
        return Err(format!("attribute from is not supported for {}", lang_name));
    }
    Ok(())
}

fn supports_from(lang_name: &str) -> bool {
    matches!(lang_name, "go" | "javascript" | "typescript" | "tsx" | "python" | "rust")
}

/// Keep the rows whose primary node satisfies every attribute.
pub fn filter_rows(
    rows: Vec<MatchRow>,
    attrs: &[AttrMatch],
    lang_name: &str,
    ts_lang: &tree_sitter::Language,
    source: &[u8],
) -> Vec<MatchRow> {
    if attrs.is_empty() {
        return rows;
    }
    let mut parser = Parser::new();
    if parser.set_language(ts_lang).is_err() {
        return vec![];
    }
    let Some(tree) = parser.parse(source, None) else { return vec![] };
    let root = tree.root_node();
    let imports = file_imports(root, source, lang_name);
    rows.into_iter()
        .filter(|r| {
            let Some(node) = root.descendant_for_byte_range(r.start_byte, r.end_byte) else {
                return false;
            };
            let facts = Facts::of(node, root, source, lang_name, &imports);
            attrs.iter().all(|a| facts.satisfies(a))
        })
        .collect()
}

/// Local name to import origin for one file.
fn file_imports(root: Node, source: &[u8], lang_name: &str) -> HashMap<String, String> {
    match lang_name {
        "go" => crate::resolve::go::go_imports(root, source),
        "javascript" | "typescript" | "tsx" | "python" | "rust" => {
            let lang: &'static str = match lang_name {
                "javascript" => "javascript",
                "typescript" => "typescript",
                "tsx" => "tsx",
                "python" => "python",
                _ => "rust",
            };
            let mut out = HashMap::new();
            for raw in import_resolver_for(lang).imports(source) {
                for b in &raw.bindings {
                    out.entry(b.clone()).or_insert_with(|| raw.specifier.clone());
                }
            }
            out
        }
        _ => HashMap::new(),
    }
}

/// What one matched node tells the attributes.
struct Facts {
    /// Root identifier of the qualifier (`log` in `log.Printf`), when there is one.
    qual_root: Option<String>,
    /// Whole qualifier text (`os.path` in `os.path.join`).
    qual_text: Option<String>,
    /// Import origin of the qualifier root, or of the name itself when unqualified.
    from: Option<String>,
    /// Whether `qual_root` is a name some import binds.
    qual_is_import: bool,
    /// Nearest enclosing named declaration, or for a Go member access through
    /// a typed variable (`u.Email`) the variable's type.
    within: Option<String>,
}

impl Facts {
    fn of(
        node: Node,
        root: Node,
        source: &[u8],
        lang_name: &str,
        imports: &HashMap<String, String>,
    ) -> Facts {
        let name = name_node(node, 0).unwrap_or(node);
        let qualifier = qualifier_of(name);
        let qual_root = qualifier.and_then(root_ident).map(|n| text(n, source).to_string());
        let qual_text = qualifier.map(|q| text(q, source).to_string());
        let (from, qual_is_import) = match &qual_root {
            Some(q) => (imports.get(q).cloned(), imports.contains_key(q)),
            None => (imports.get(text(name, source)).cloned(), false),
        };
        // `u.Email` in Go belongs to u's type, not to the code around it, and
        // `u.Contact.Email` to the type of `u.Contact`.
        let typed_qualifier = match (qualifier, lang_name) {
            (Some(q), "go") if !qual_is_import => Some(go_operand_type(q, root, source, name.start_position().row + 1)),
            _ => None,
        };
        let within = match typed_qualifier {
            Some(Some(t)) => Some(t),
            // A chain whose type is unknown belongs to no declaration.
            Some(None) if qualifier.is_some_and(|q| q.kind() == "selector_expression") => None,
            _ => enclosing_decl(name, source),
        };
        Facts { qual_root, qual_text, from, qual_is_import, within }
    }

    fn satisfies(&self, a: &AttrMatch) -> bool {
        match a.key.as_str() {
            "from" => self.from.as_deref().is_some_and(|v| op_matches(&a.op, &a.value, v)),
            "in" => self.within.as_deref().is_some_and(|v| op_matches(&a.op, &a.value, v)),
            "qual" => {
                let via_in = self.within.as_deref().is_some_and(|v| op_matches(&a.op, &a.value, v));
                let via_import = self.qual_is_import
                    && (self.qual_root.as_deref().is_some_and(|v| op_matches(&a.op, &a.value, v))
                        || self.qual_text.as_deref().is_some_and(|v| op_matches(&a.op, &a.value, v)));
                via_in || via_import
            }
            _ => false,
        }
    }
}

fn op_matches(op: &AttrOp, want: &str, have: &str) -> bool {
    match op {
        AttrOp::Eq => have == want,
        AttrOp::Prefix => have.starts_with(want),
        AttrOp::Contains => have.contains(want),
        AttrOp::Suffix => have.ends_with(want),
        AttrOp::Regex => regex::Regex::new(want).is_ok_and(|re| re.is_match(have)),
    }
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn is_ident(kind: &str) -> bool {
    kind.ends_with("identifier") || kind == "constant"
}

/// The identifier a matched node is named by: the node itself when it is an
/// identifier, the called name of a call, the member of a member access, or
/// the `name` field of a declaration.
fn name_node(node: Node, depth: usize) -> Option<Node> {
    if depth > 4 {
        return None;
    }
    let kind = node.kind();
    if is_ident(kind) {
        return Some(node);
    }
    let member = match kind {
        "call_expression" => node.child_by_field_name("function"),
        "call" => node.child_by_field_name("method").or_else(|| node.child_by_field_name("function")),
        "selector_expression" | "field_expression" => node.child_by_field_name("field"),
        "member_expression" => node.child_by_field_name("property"),
        "attribute" => node.child_by_field_name("attribute"),
        "scoped_identifier" | "qualified_identifier" | "scoped_type_identifier" => {
            node.child_by_field_name("name")
        }
        "generic_function" => node.child_by_field_name("function"),
        _ => None,
    };
    if let Some(m) = member {
        return name_node(m, depth + 1);
    }
    if let Some(n) = node.child_by_field_name("name") {
        return name_node(n, depth + 1);
    }
    // `type_declaration` holds a `type_spec`, `lexical_declaration` a declarator.
    let mut cur = node.walk();
    let found = node.named_children(&mut cur).find(|c| c.child_by_field_name("name").is_some());
    found.and_then(|c| name_node(c, depth + 1))
}

/// The qualifier in front of a member name: `log` for `Printf` in `log.Printf`.
fn qualifier_of(name: Node) -> Option<Node> {
    let parent = name.parent()?;
    let (member_field, qual_field) = match parent.kind() {
        "selector_expression" => ("field", "operand"),
        "member_expression" => ("property", "object"),
        "attribute" => ("attribute", "object"),
        "scoped_identifier" | "scoped_type_identifier" => ("name", "path"),
        "field_expression" => ("field", if parent.child_by_field_name("value").is_some() { "value" } else { "argument" }),
        "qualified_identifier" => ("name", "scope"),
        "call" => ("method", "receiver"),
        _ => return None,
    };
    let member = parent.child_by_field_name(member_field)?;
    if member.id() != name.id() {
        return None;
    }
    parent.child_by_field_name(qual_field)
}

/// Type of a Go operand: the variable's type for `u`, and for a chain
/// `u.Contact` the declared type of the field `Contact` on u's type, from the
/// struct declarations in this file. None when any step is unknown.
fn go_operand_type(op: Node, root: Node, source: &[u8], line: usize) -> Option<String> {
    match op.kind() {
        "identifier" => {
            crate::resolve::go::binding_type(root, source, "", text(op, source), line, &crate::resolve::DefIndex::default())
        }
        "selector_expression" => {
            let inner = go_operand_type(op.child_by_field_name("operand")?, root, source, line)?;
            let f = op.child_by_field_name("field")?;
            crate::resolve::go::go_field_type(root, source, &inner, text(f, source))
        }
        "parenthesized_expression" | "unary_expression" => {
            go_operand_type(op.named_child(0)?, root, source, line)
        }
        _ => None,
    }
}

/// Leftmost identifier of a qualifier chain: `a` in `a.b.c` or `a::b`.
fn root_ident(node: Node) -> Option<Node> {
    let mut n = node;
    for _ in 0..16 {
        if is_ident(n.kind()) {
            return Some(n);
        }
        let next = ["operand", "object", "path", "value", "argument", "scope", "receiver", "function"]
            .iter()
            .find_map(|f| n.child_by_field_name(f))
            .or_else(|| n.named_child(0));
        n = next?;
    }
    None
}

/// Nearest enclosing named declaration: struct, class, impl, interface,
/// module or namespace. A Go method's receiver type counts. A declaration
/// does not enclose its own name.
fn enclosing_decl(name: Node, source: &[u8]) -> Option<String> {
    let mut cur = name.parent();
    while let Some(n) = cur {
        if let Some(found) = decl_name(n, name, source) {
            return Some(found);
        }
        cur = n.parent();
    }
    None
}

fn decl_name(n: Node, own_name: Node, source: &[u8]) -> Option<String> {
    match n.kind() {
        "method_declaration" => {
            let recv = n.child_by_field_name("receiver")?;
            let mut c = recv.walk();
            let param = recv.named_children(&mut c).find(|p| p.kind() == "parameter_declaration")?;
            Some(base_type(text(param.child_by_field_name("type")?, source)))
        }
        "impl_item" => Some(base_type(text(n.child_by_field_name("type")?, source))),
        "type_spec" | "class_declaration" | "class" | "abstract_class_declaration"
        | "interface_declaration" | "internal_module" | "module" | "class_definition"
        | "struct_item" | "enum_item" | "union_item" | "trait_item" | "mod_item"
        | "struct_specifier" | "class_specifier" | "union_specifier" | "namespace_definition" => {
            let decl_name = n.child_by_field_name("name")?;
            if decl_name.id() == own_name.id() {
                return None;
            }
            Some(text(decl_name, source).to_string())
        }
        _ => None,
    }
}

/// `*Server[T]` and `Server<T>` to `Server`, `pkg.T` to `T`.
fn base_type(t: &str) -> String {
    let t = t.trim().trim_start_matches(['*', '&']).trim();
    let t = t.split(['[', '<']).next().unwrap_or(t);
    let t = t.rsplit(['.', ':']).next().unwrap_or(t);
    t.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query;

    fn attrs_of(q: &str) -> Result<Vec<AttrMatch>, String> {
        last_selector_attrs(&query::parse(q).unwrap())
    }

    #[test]
    fn attrs_on_the_last_selector_are_taken() {
        let a = attrs_of("*function#Go &function#Printf[from=log]").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].key, "from");
        let dot = attrs_of("&function#log.Printf").unwrap();
        assert_eq!((dot[0].key.as_str(), dot[0].value.as_str()), ("qual", "log"));
    }

    #[test]
    fn attrs_elsewhere_are_rejected() {
        let e = attrs_of("*function#Go[in=S] &function#Printf").unwrap_err();
        assert!(e.contains("only supported on the last selector"), "{}", e);
        assert!(attrs_of("(*function @f).body(&function#log.Printf)").is_err());
    }

    #[test]
    fn unknown_keys_and_bad_regex_are_rejected() {
        assert!(attrs_of("function#x[colour=red]").unwrap_err().contains("unknown attribute"));
        assert!(attrs_of("function#x[from=/(/]").unwrap_err().contains("bad regex"));
    }

    #[test]
    fn from_is_unsupported_for_c_cpp_ruby() {
        let a = attrs_of("function#x[from=y]").unwrap();
        assert_eq!(check_lang(&a, "ruby").unwrap_err(), "attribute from is not supported for ruby");
        assert!(check_lang(&a, "c").is_err());
        assert!(check_lang(&a, "cpp").is_err());
        assert!(check_lang(&a, "go").is_ok());
        let in_only = attrs_of("function#x[in=y]").unwrap();
        assert!(check_lang(&in_only, "ruby").is_ok());
    }

    #[test]
    fn ops_match_as_named() {
        assert!(op_matches(&AttrOp::Eq, "log", "log"));
        assert!(!op_matches(&AttrOp::Eq, "log", "log/slog"));
        assert!(op_matches(&AttrOp::Prefix, "lo", "log/slog"));
        assert!(op_matches(&AttrOp::Contains, "g/s", "log/slog"));
        assert!(op_matches(&AttrOp::Suffix, "slog", "log/slog"));
        assert!(op_matches(&AttrOp::Regex, "^log$", "log"));
        assert!(!op_matches(&AttrOp::Regex, "^log$", "log/slog"));
    }

    #[test]
    fn base_type_strips_pointers_generics_and_packages() {
        assert_eq!(base_type("*Server[T]"), "Server");
        assert_eq!(base_type("Server<T>"), "Server");
        assert_eq!(base_type("pkg.T"), "T");
    }
}
