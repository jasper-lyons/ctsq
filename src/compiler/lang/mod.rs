use crate::query::ast::Sigil;

pub mod c;
pub mod cpp;
pub mod go;
pub mod javascript;
pub mod python;
pub mod rust;

/// A single concrete tree-sitter node variant that an abstract type maps to.
#[derive(Debug, Clone)]
pub struct TsVariant {
    pub node: &'static str,
    /// The field containing the callee/name identifier (e.g. "function" for call_expression).
    /// None means the node itself IS the name (e.g. identifier).
    pub name_field: Option<&'static str>,
    /// The TS node type of the name child (e.g. "identifier").
    pub name_node: &'static str,
    /// TS field name for parameters / arguments (for .params() field access).
    pub params_field: Option<&'static str>,
    /// The container node that wraps parameter children (e.g. "argument_list").
    pub params_container: Option<&'static str>,
    /// TS field name for the body (for .body() field access).
    pub body_field: Option<&'static str>,
    /// The container node wrapping body children (e.g. "compound_statement" for C/C++).
    pub body_container: Option<&'static str>,
    /// Chain of (outer_node, inner_field) pairs from name_field down to name_node.
    /// Empty = name_node is a direct child of name_field.
    /// Example: &[("function_declarator", "declarator")] means
    ///   name_field: (function_declarator declarator: (name_node) @cap)
    pub name_path: &'static [(&'static str, &'static str)],
    /// When true, .params() injects via a wildcard bridge node (`_ params_field: ...`)
    /// instead of directly. Needed for C/C++ function_definition where `parameters`
    /// is a field of `function_declarator`, not of `function_definition` itself.
    pub params_needs_bridge: bool,
    /// Structural constraint appended to node_body when no name filter is present.
    /// Used to narrow broad node types (e.g. field_declaration → only method prototypes).
    pub always_constraint: Option<&'static str>,
}

pub trait Lang: Send + Sync {
    /// Resolve an abstract node type + sigil to zero or more TS variants.
    /// - None  → type is unknown; pass through as a concrete TS node name
    /// - Some([]) → type is known but has no mapping in this language (silent empty)
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>>;
    fn ts_language(&self) -> tree_sitter::Language;
}

pub fn for_name(name: &str) -> Option<Box<dyn Lang>> {
    match name {
        "c" => Some(Box::new(c::CLang)),
        "cpp" | "c++" => Some(Box::new(cpp::CppLang)),
        "javascript" | "js" => Some(Box::new(javascript::JsLang)),
        "python" | "py" => Some(Box::new(python::PyLang)),
        "rust" | "rs" => Some(Box::new(rust::RustLang)),
        "go" => Some(Box::new(go::GoLang)),
        _ => None,
    }
}

pub fn lang_for_name_canonical(name: &str) -> Option<&'static str> {
    match name {
        "c" => Some("c"),
        "cpp" | "c++" => Some("cpp"),
        "javascript" | "js" => Some("javascript"),
        "python" | "py" => Some("python"),
        "rust" | "rs" => Some("rust"),
        "go" => Some("go"),
        _ => None,
    }
}

pub fn extensions_for_name(name: &str) -> &'static [&'static str] {
    match name {
        "c" => &["c", "h"],
        "cpp" | "c++" => &["cpp", "cc", "cxx", "h", "hpp", "hxx"],
        "javascript" | "js" => &["js", "mjs", "cjs", "jsx"],
        "python" | "py" => &["py"],
        "rust" | "rs" => &["rs"],
        "go" => &["go"],
        _ => &[],
    }
}

pub fn all_known_extensions() -> &'static [&'static str] {
    &["c", "h", "cpp", "cc", "cxx", "hpp", "hxx", "js", "mjs", "cjs", "jsx", "py", "rs", "go"]
}

pub fn lang_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "py" => Some("python"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        _ => None,
    }
}
