use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct PyLang;

impl Lang for PyLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "function" => Some(match sigil {
                Some(Sigil::Def) | None => {
                    let mut v = vec![def_variant()];
                    if sigil.is_none() {
                        v.extend(call_variants());
                    }
                    v
                }
                Some(Sigil::Ref) => call_variants(),
            }),
            "class" => Some(vec![TsVariant {
                node: "class_definition",
                name_field: Some("name"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: Some("body"),
                body_container: Some("block"),
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            "id" => Some(vec![identifier()]),
            "var" => Some(vec![identifier()]),
            "param" => Some(vec![identifier()]),
            "import" => Some(vec![
                simple("import_statement"),
                simple("import_from_statement"),
            ]),
            "literal" => Some(vec![
                simple("integer"),
                simple("float"),
                simple("string"),
            ]),
            "block" => Some(vec![simple("block")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "while" => Some(vec![simple("while_statement")]),
            "call" => Some(call_variants()),
            "comment" => Some(vec![simple("comment")]),
            // `self.x = …` declares an instance field. Every `a.b` is an access.
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![field_at("assignment", "left", "identifier", &[("attribute", "attribute")])],
                Some(Sigil::Ref) => vec![field_at("attribute", "attribute", "identifier", &[])],
                None => {
                    let mut v = vec![field_at("assignment", "left", "identifier", &[("attribute", "attribute")])];
                    v.extend(vec![field_at("attribute", "attribute", "identifier", &[])]);
                    v
                }
            }),
            "module" | "switch" | "type" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }
}

fn def_variant() -> TsVariant {
    TsVariant {
        node: "function_definition",
        name_field: Some("name"),
        name_node: "identifier",
        params_field: Some("parameters"),
        params_container: Some("parameters"),
        body_field: Some("body"),
        body_container: Some("block"),
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
    }
}

fn call_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "call",
            name_field: Some("function"),
            name_node: "identifier",
            params_field: Some("arguments"),
            params_container: Some("argument_list"),
            body_field: None,
            body_container: None,
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        // Method call: u.send_email() — callee is an `attribute` node whose
        // `attribute` field holds the method-name identifier.
        TsVariant {
            node: "call",
            name_field: Some("function"),
            name_node: "identifier",
            params_field: Some("arguments"),
            params_container: Some("argument_list"),
            body_field: None,
            body_container: None,
            name_path: &[("attribute", "attribute")],
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
