use crate::query::ast::Sigil;

pub mod c;
pub mod cpp;
pub mod go;
pub mod javascript;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod typescript;

/// A single concrete tree-sitter node variant that an abstract type maps to.
#[derive(Debug, Clone)]
pub struct TsVariant {
    pub node: &'static str,
    /// The field containing the callee/name identifier (e.g. "function" for call_expression).
    /// None means the node itself IS the name (e.g. identifier).
    /// `Some("")` means the name lives in a positional child with no field name
    /// (e.g. the variable_declarator inside a JS/TS lexical_declaration); use it
    /// with `name_path` to say which child.
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
    /// An empty inner_field means the next node down is a positional child with
    /// no field name, e.g. &[("expression_list", "")] for a Go `x := 1`.
    pub name_path: &'static [(&'static str, &'static str)],
    /// When true, .params() injects via a wildcard bridge node (`_ params_field: ...`)
    /// instead of directly. Needed for C/C++ function_definition where `parameters`
    /// is a field of `function_declarator`, not of `function_definition` itself.
    pub params_needs_bridge: bool,
    /// Structural constraint appended to node_body when no name filter is present.
    /// Used to narrow broad node types (e.g. field_declaration → only method prototypes).
    pub always_constraint: Option<&'static str>,
    /// When the node's name lives in a parent (e.g. arrow_function inside variable_declarator),
    /// set this to emit a parent-scoped pattern that filters by name.
    pub parent_name_wrap: Option<ParentNameWrap>,
    /// Force a callee-name predicate even when the query carries no name filter.
    /// Used to match `require(...)` for the `import` abstract type: emits
    /// `<name_field>: (<name_node>) @c (#eq? @c "<value>")`.
    pub always_callee_eq: Option<&'static str>,
}

/// Describes how to find a node's name via its parent when name_field is None.
/// Used for arrow functions whose name lives in the parent variable_declarator.
#[derive(Debug, Clone, Copy)]
pub struct ParentNameWrap {
    /// Parent node type (e.g. "variable_declarator").
    pub parent: &'static str,
    /// Field on parent holding the name (e.g. "name").
    pub name_field: &'static str,
    /// Node type of the name child (e.g. "identifier").
    pub name_node: &'static str,
    /// Field on parent holding self (e.g. "value").
    pub self_field: &'static str,
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
        "typescript" | "ts" => Some(Box::new(typescript::TsLang)),
        "tsx" => Some(Box::new(typescript::TsxLang)),
        "python" | "py" => Some(Box::new(python::PyLang)),
        "ruby" | "rb" => Some(Box::new(ruby::RubyLang)),
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
        "typescript" | "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "python" | "py" => Some("python"),
        "ruby" | "rb" => Some("ruby"),
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
        "typescript" | "ts" => &["ts"],
        "tsx" => &["tsx"],
        "python" | "py" => &["py"],
        "ruby" | "rb" => &["rb", "rake", "gemspec"],
        "rust" | "rs" => &["rs"],
        "go" => &["go"],
        _ => &[],
    }
}

pub fn all_known_extensions() -> &'static [&'static str] {
    &["c", "h", "cpp", "cc", "cxx", "hpp", "hxx", "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rb", "rake", "gemspec", "rs", "go"]
}

pub fn lang_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "py" => Some("python"),
        "rb" | "rake" | "gemspec" => Some("ruby"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        _ => None,
    }
}
