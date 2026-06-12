use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct RustLang;

impl Lang for RustLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "function" => Some(match sigil {
                Some(Sigil::Def) | None => {
                    let mut v = vec![fn_item()];
                    if sigil.is_none() {
                        v.push(call_variant());
                    }
                    v
                }
                Some(Sigil::Ref) => vec![call_variant()],
            }),
            "class" => Some(vec![
                TsVariant {
                    node: "struct_item",
                    name_field: Some("name"),
                    name_node: "type_identifier",
                    params_field: None,
                    params_container: None,
                    body_field: Some("body"),
                    body_container: Some("field_declaration_list"),
                    name_path: &[],
                    params_needs_bridge: false,
                    always_constraint: None,
                },
                TsVariant {
                    node: "enum_item",
                    name_field: Some("name"),
                    name_node: "type_identifier",
                    params_field: None,
                    params_container: None,
                    body_field: Some("body"),
                    body_container: Some("enum_variant_list"),
                    name_path: &[],
                    params_needs_bridge: false,
                    always_constraint: None,
                },
                TsVariant {
                    node: "trait_item",
                    name_field: Some("name"),
                    name_node: "type_identifier",
                    params_field: None,
                    params_container: None,
                    body_field: Some("body"),
                    body_container: Some("declaration_list"),
                    name_path: &[],
                    params_needs_bridge: false,
                    always_constraint: None,
                },
            ]),
            "module" => Some(vec![TsVariant {
                node: "mod_item",
                name_field: Some("name"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: Some("body"),
                body_container: Some("declaration_list"),
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
            }]),
            "id" => Some(vec![identifier()]),
            "var" => Some(vec![TsVariant {
                node: "let_declaration",
                name_field: Some("pattern"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: None,
                body_container: None,
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
            }]),
            "param" => Some(vec![TsVariant {
                node: "parameter",
                name_field: Some("pattern"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: None,
                body_container: None,
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
            }]),
            "type" => Some(vec![simple("type_identifier")]),
            "import" => Some(vec![simple("use_declaration")]),
            "literal" => Some(vec![
                simple("integer_literal"),
                simple("float_literal"),
                simple("string_literal"),
                simple("boolean_literal"),
                simple("char_literal"),
            ]),
            "block" => Some(vec![simple("block")]),
            "if" => Some(vec![simple("if_expression")]),
            "for" => Some(vec![simple("for_expression")]),
            "while" => Some(vec![simple("while_expression")]),
            "switch" => Some(vec![simple("match_expression")]),
            "call" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }
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
    }
}

fn fn_item() -> TsVariant {
    TsVariant {
        node: "function_item",
        name_field: Some("name"),
        name_node: "identifier",
        params_field: Some("parameters"),
        params_container: Some("parameters"),
        body_field: Some("body"),
        body_container: Some("block"),
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
    }
}

fn call_variant() -> TsVariant {
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
    }
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
    }
}
