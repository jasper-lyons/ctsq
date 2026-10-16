use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct JsLang;

impl Lang for JsLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "function" => Some(match sigil {
                Some(Sigil::Def) | None => {
                    let mut v = def_variants();
                    if sigil.is_none() {
                        v.extend(call_variants());
                    }
                    v
                }
                Some(Sigil::Ref) => call_variants(),
            }),
            "class" => Some(vec![TsVariant {
                node: "class_declaration",
                name_field: Some("name"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: Some("body"),
                body_container: Some("class_body"),
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            // Property names and JSX attribute names are `property_identifier`.
            "id" => Some(vec![identifier(), simple("property_identifier")]),
            "var" => Some(vec![identifier()]),
            // `*variable#X` reaches the whole `const X = …` statement, so --body
            // prints it. `&variable#X` stays on the uses. `const` is an alias.
            "variable" | "const" => Some(match sigil {
                Some(Sigil::Ref) => vec![identifier()],
                Some(Sigil::Def) => var_decl_variants(),
                None => {
                    let mut v = var_decl_variants();
                    v.push(identifier());
                    v
                }
            }),
            "param" => Some(vec![identifier()]),
            "type" => Some(vec![simple("type_identifier")]),
            "import" => Some(import_variants()),
            "literal" => Some(vec![
                simple("number"),
                simple("string"),
                simple("template_string"),
            ]),
            "block" => Some(vec![simple("statement_block")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "while" => Some(vec![simple("while_statement")]),
            "switch" => Some(vec![simple("switch_statement")]),
            "op" => Some(vec![simple("binary_expression")]),
            // Class fields are declarations, `a.b` property reads and writes are accesses.
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![field_at("field_definition", "property", "property_identifier", &[])],
                Some(Sigil::Ref) => vec![field_at("member_expression", "property", "property_identifier", &[])],
                None => {
                    let mut v = vec![field_at("field_definition", "property", "property_identifier", &[])];
                    v.extend(vec![field_at("member_expression", "property", "property_identifier", &[])]);
                    v
                }
            }),
            // `f(x)`, `obj.f(x)` and `it(...)`: the name filter reads the last
            // segment. JSX elements are `function` references, not calls.
            "call" => Some(call_variants().into_iter().take(2).collect()),
            "comment" => Some(vec![simple("comment")]),
            "module" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }
}

fn def_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "function_declaration",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: Some("body"),
            body_container: Some("statement_block"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "function_expression",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: Some("body"),
            body_container: Some("statement_block"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "arrow_function",
            name_field: None,
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: Some("body"),
            body_container: Some("statement_block"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: Some(super::ParentNameWrap {
                parent: "variable_declarator",
                name_field: "name",
                name_node: "identifier",
                self_field: "value",
            }),
            always_callee_eq: None,
        },
        // `let inner = function () {}`: the name lives in the binding, as for arrows.
        TsVariant {
            node: "function_expression",
            name_field: None,
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: Some("body"),
            body_container: Some("statement_block"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: Some(super::ParentNameWrap {
                parent: "variable_declarator",
                name_field: "name",
                name_node: "identifier",
                self_field: "value",
            }),
            always_callee_eq: None,
        },
        TsVariant {
            node: "method_definition",
            name_field: Some("name"),
            name_node: "property_identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: Some("body"),
            body_container: Some("statement_block"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

fn call_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "call_expression",
            name_field: Some("function"),
            name_node: "identifier",
            params_field: Some("arguments"),
            params_container: Some("arguments"),
            body_field: None,
            body_container: None,
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "call_expression",
            name_field: Some("function"),
            name_node: "property_identifier",
            params_field: Some("arguments"),
            params_container: Some("arguments"),
            body_field: None,
            body_container: None,
            name_path: &[("member_expression", "property")],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "jsx_self_closing_element",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "jsx_opening_element",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

/// The statement a `const`, `let` or `var` name is introduced by. The declarator
/// holding the name is a positional child, hence the empty name_field. Shared
/// with TypeScript, whose grammar has the same two nodes.
pub(super) fn var_decl_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "lexical_declaration",
            name_field: Some(""),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("variable_declarator", "name")],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "variable_declaration",
            name_field: Some(""),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("variable_declarator", "name")],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

fn identifier() -> TsVariant {
    TsVariant {
        node: "identifier",
        name_field: None,
        name_node: "identifier",
        params_field: None,
        params_container: None,
        body_field: None,
        body_container: None,
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
    }
}

/// A field node whose name is the `name_node` child reached through
/// `name_field` and `path`. The match lands on the declaration or access, and
/// attribute filters read the field name back off it.
fn field_at(
    node: &'static str,
    name_field: &'static str,
    name_node: &'static str,
    path: &'static [(&'static str, &'static str)],
) -> TsVariant {
    TsVariant { name_field: Some(name_field), name_node, name_path: path, ..simple(node) }
}

fn simple(node: &'static str) -> TsVariant {
    TsVariant {
        node,
        name_field: None,
        name_node: node,
        params_field: None,
        params_container: None,
        body_field: None,
        body_container: None,
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
    }
}

/// All constructs that create a module dependency, so `-q 'import'` matches the
/// same things the import graph does: static `import`, `export … from`
/// re-exports, dynamic `import()`, and CommonJS `require()`. Shared with TS.
pub(super) fn import_variants() -> Vec<TsVariant> {
    vec![
        simple("import_statement"),
        // re-export with a source (`export { x } from "./m"`) — the source field
        // excludes plain `export` declarations.
        TsVariant { always_constraint: Some(" source: (string)"), ..simple("export_statement") },
        // dynamic import: `import("./m")` — the callee is the `import` keyword node.
        TsVariant { always_constraint: Some(" function: (import)"), ..simple("call_expression") },
        // CommonJS: `require("./m")` — force a callee-name predicate.
        TsVariant {
            name_field: Some("function"),
            name_node: "identifier",
            always_callee_eq: Some("require"),
            ..simple("call_expression")
        },
    ]
}
