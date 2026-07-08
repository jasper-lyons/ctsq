use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct TsLang;
pub struct TsxLang;

impl Lang for TsLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        ts_resolve(abstract_type, sigil)
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }
}

impl Lang for TsxLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        let mut variants = ts_resolve(abstract_type, sigil)?;
        if matches!(abstract_type, "function") && !matches!(sigil, Some(Sigil::Def)) {
            variants.extend(jsx_call_variants());
        }
        Some(variants)
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    }
}

fn ts_resolve(abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
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
        "class" => Some(match sigil {
            Some(Sigil::Ref) => call_variants(),
            _ => vec![
                TsVariant {
                    node: "class_declaration",
                    name_field: Some("name"),
                    name_node: "type_identifier",
                    params_field: None,
                    params_container: None,
                    body_field: Some("body"),
                    body_container: Some("class_body"),
                    name_path: &[],
                    params_needs_bridge: false,
                    always_constraint: None,
                    parent_name_wrap: None, always_callee_eq: None,
                },
                TsVariant {
                    node: "abstract_class_declaration",
                    name_field: Some("name"),
                    name_node: "type_identifier",
                    params_field: None,
                    params_container: None,
                    body_field: Some("body"),
                    body_container: Some("class_body"),
                    name_path: &[],
                    params_needs_bridge: false,
                    always_constraint: None,
                    parent_name_wrap: None, always_callee_eq: None,
                },
            ],
        }),
        "interface" => Some(vec![TsVariant {
            node: "interface_declaration",
            name_field: Some("name"),
            name_node: "type_identifier",
            params_field: None,
            params_container: None,
            body_field: Some("body"),
            body_container: Some("object_type"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        }]),
        "enum" => Some(vec![TsVariant {
            node: "enum_declaration",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: Some("body"),
            body_container: Some("enum_body"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        }]),
        "type" => Some(vec![
            TsVariant {
                node: "type_alias_declaration",
                name_field: Some("name"),
                name_node: "type_identifier",
                params_field: None,
                params_container: None,
                body_field: None,
                body_container: None,
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
                parent_name_wrap: None, always_callee_eq: None,
            },
            simple("type_identifier"),
        ]),
        "id" => Some(vec![identifier(), simple("type_identifier")]),
        "var" => Some(vec![identifier()]),
        "param" => Some(vec![identifier()]),
        "import" => Some(super::javascript::import_variants()),
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
        "call" | "module" => Some(vec![]),
        _ => None,
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
        // ambient function signature (no body): declare function foo(): void;
        TsVariant {
            node: "function_signature",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("formal_parameters"),
            body_field: None,
            body_container: None,
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
    ]
}

fn jsx_call_variants() -> Vec<TsVariant> {
    vec![
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
