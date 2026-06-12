use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct CppLang;

impl Lang for CppLang {
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
            "class" => Some(vec![
                TsVariant {
                    node: "class_specifier",
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
                    node: "struct_specifier",
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
            ]),
            "module" => Some(vec![TsVariant {
                node: "namespace_definition",
                name_field: Some("name"),
                name_node: "namespace_identifier",
                params_field: None,
                params_container: None,
                body_field: Some("body"),
                body_container: Some("declaration_list"),
                name_path: &[],
                params_needs_bridge: false,
                always_constraint: None,
            }]),
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
            }]),
            "type" => Some(vec![simple("type_identifier")]),
            "import" => Some(vec![simple("preproc_include")]),
            "literal" => Some(vec![
                simple("number_literal"),
                simple("string_literal"),
                simple("char_literal"),
                simple("true"),
                simple("false"),
            ]),
            "block" => Some(vec![simple("compound_statement")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "while" => Some(vec![simple("while_statement")]),
            "switch" => Some(vec![simple("switch_statement")]),
            "op" => Some(vec![simple("binary_expression")]),
            "call" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_cpp::LANGUAGE.into()
    }
}

fn def_variants() -> Vec<TsVariant> {
    vec![
        // Plain function: `void foo()`
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
        },
        // Scoped out-of-line definition: `Foo::bar()` — navigate to qualified_identifier.name
        TsVariant {
            node: "function_definition",
            name_field: Some("declarator"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("parameter_list"),
            body_field: Some("body"),
            body_container: Some("compound_statement"),
            name_path: &[("function_declarator", "declarator"), ("qualified_identifier", "name")],
            params_needs_bridge: true,
            always_constraint: None,
        },
        // Method prototype inside a class body: `int Find(K key);`
        TsVariant {
            node: "field_declaration",
            name_field: Some("declarator"),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("function_declarator", "declarator")],
            params_needs_bridge: false,
            always_constraint: Some(" declarator: (function_declarator)"),
        },
    ]
}

fn call_variants() -> Vec<TsVariant> {
    vec![
        // free function call: foo(...)
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
        },
        // method call: obj.method(...) or obj->method(...)
        TsVariant {
            node: "call_expression",
            name_field: Some("function"),
            name_node: "field_identifier",
            params_field: Some("arguments"),
            params_container: Some("argument_list"),
            body_field: None,
            body_container: None,
            name_path: &[("field_expression", "field")],
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
