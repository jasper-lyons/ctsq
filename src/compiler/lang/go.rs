use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct GoLang;

impl Lang for GoLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "function" => Some(match sigil {
                Some(Sigil::Def) | None => {
                    let mut v = vec![fn_decl(), method_decl()];
                    if sigil.is_none() {
                        v.push(call_ident());
                        v.push(call_selector());
                    }
                    v
                }
                Some(Sigil::Ref) => vec![call_ident(), call_selector()],
            }),
            "class" => Some(vec![TsVariant {
                node: "type_spec",
                name_field: Some("name"),
                name_node: "type_identifier",
                params_field: None,
                params_container: None,
                body_field: Some("type"),
                body_container: None,
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
            }]),
            "id" => Some(vec![identifier(), field_identifier()]),
            "var" => Some(vec![TsVariant {
                node: "var_spec",
                name_field: Some("name"),
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
                node: "parameter_declaration",
                name_field: Some("name"),
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
            "import" => Some(vec![simple("import_declaration")]),
            "literal" => Some(vec![
                simple("interpreted_string_literal"),
                simple("raw_string_literal"),
                simple("int_literal"),
                simple("float_literal"),
                simple("rune_literal"),
            ]),
            "block" => Some(vec![simple("block")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "switch" => Some(vec![
                simple("expression_switch_statement"),
                simple("type_switch_statement"),
            ]),
            "call" | "module" | "while" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
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

fn field_identifier() -> TsVariant {
    TsVariant {
        node: "field_identifier",
        name_field: None,
        name_node: "field_identifier",
        params_field: None,
        params_container: None,
        body_field: None,
        body_container: None,
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
    }
}

fn fn_decl() -> TsVariant {
    TsVariant {
        node: "function_declaration",
        name_field: Some("name"),
        name_node: "identifier",
        params_field: Some("parameters"),
        params_container: Some("parameter_list"),
        body_field: Some("body"),
        body_container: Some("block"),
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
    }
}

fn method_decl() -> TsVariant {
    TsVariant {
        node: "method_declaration",
        name_field: Some("name"),
        name_node: "field_identifier",
        params_field: Some("parameters"),
        params_container: Some("parameter_list"),
        body_field: Some("body"),
        body_container: Some("block"),
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
    }
}

// foo() — plain identifier call
fn call_ident() -> TsVariant {
    TsVariant {
        node: "call_expression",
        name_field: Some("function"),
        name_node: "identifier",
        params_field: Some("arguments"),
        params_container: Some("argument_list"),
        body_field: None,
        body_container: None,
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
    }
}

// obj.Method() — selector call; navigate selector_expression.field to reach field_identifier
fn call_selector() -> TsVariant {
    TsVariant {
        node: "call_expression",
        name_field: Some("function"),
        name_node: "field_identifier",
        params_field: Some("arguments"),
        params_container: Some("argument_list"),
        body_field: None,
        body_container: None,
        name_path: &[("selector_expression", "field")],
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
