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
            }]),
            "id" => Some(vec![identifier()]),
            "var" => Some(vec![identifier()]),
            "param" => Some(vec![identifier()]),
            "type" => Some(vec![simple("type_identifier")]),
            "import" => Some(vec![simple("import_statement")]),
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
        },
        // arrow_function: name lives in parent variable_declarator — emit without name predicate.
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
