use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct CLang;

impl Lang for CLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "function" => Some(match sigil {
                Some(Sigil::Def) | None => {
                    let mut v = vec![def_variant()];
                    if sigil.is_none() {
                        v.push(call_variant());
                    }
                    v
                }
                Some(Sigil::Ref) => vec![call_variant()],
            }),
            "id" => Some(vec![identifier(), field_identifier()]),
            "var" => Some(vec![identifier(), field_identifier()]),
            "param" => Some(vec![TsVariant {
                node: "parameter_declaration",
                name_field: Some("declarator"),
                name_node: "identifier",
                params_field: None,
                params_container: None,
                body_field: None,
                body_container: None,
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            // Bare `type` is the identifier node, so `*type#X` used to match the
            // name and nothing else. The def sigil now reaches the declaration
            // the name belongs to, which is what --body needs.
            "type" => Some(match sigil {
                Some(Sigil::Ref) => vec![simple("type_identifier")],
                Some(Sigil::Def) => type_decl_variants(),
                None => {
                    let mut v = type_decl_variants();
                    v.push(simple("type_identifier"));
                    v
                }
            }),
            "import" => Some(vec![simple("preproc_include")]),
            "literal" => Some(vec![
                simple("number_literal"),
                simple("string_literal"),
                simple("char_literal"),
            ]),
            "block" => Some(vec![simple("compound_statement")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "while" => Some(vec![simple("while_statement")]),
            "switch" => Some(vec![simple("switch_statement")]),
            // Known abstract types with no C equivalent
            "call" | "class" | "module" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_c::LANGUAGE.into()
    }
}

/// The declaration forms a C type name is introduced by: the tagged struct,
/// union and enum specifiers plus a typedef, whose name sits in `declarator`.
pub(crate) fn type_decl_variants() -> Vec<TsVariant> {
    let tagged = |node: &'static str, body_container: &'static str| TsVariant {
        node,
        name_field: Some("name"),
        name_node: "type_identifier",
        params_field: None,
        params_container: None,
        body_field: Some("body"),
        body_container: Some(body_container),
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
    };
    vec![
        tagged("struct_specifier", "field_declaration_list"),
        tagged("union_specifier", "field_declaration_list"),
        tagged("enum_specifier", "enumerator_list"),
        TsVariant {
            node: "type_definition",
            name_field: Some("declarator"),
            name_node: "type_identifier",
            params_field: None,
            params_container: None,
            body_field: Some("type"),
            body_container: None,
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

fn call_variant() -> TsVariant {
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
        parent_name_wrap: None, always_callee_eq: None,
    }
}

fn def_variant() -> TsVariant {
    TsVariant {
        node: "function_definition",
        name_field: Some("declarator"),
        name_node: "identifier",
        params_field: Some("parameters"),
        params_container: Some("parameter_list"),
        body_field: Some("body"),
        body_container: Some("compound_statement"),
        name_path: &[("function_declarator", "declarator")],
        params_needs_bridge: true,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
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
        parent_name_wrap: None, always_callee_eq: None,
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
