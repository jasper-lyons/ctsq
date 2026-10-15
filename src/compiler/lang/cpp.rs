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
                    parent_name_wrap: None, always_callee_eq: None,
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
                    parent_name_wrap: None, always_callee_eq: None,
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
                parent_name_wrap: None, always_callee_eq: None,
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
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            // Bare `type` is the identifier node, so `*type#X` used to match the
            // name and nothing else. The def sigil now reaches the declaration
            // the name belongs to, which is what --body needs.
            "type" => Some(match sigil {
                Some(Sigil::Ref) => vec![simple("type_identifier")],
                Some(Sigil::Def) => cpp_type_decl_variants(),
                None => {
                    let mut v = cpp_type_decl_variants();
                    v.push(simple("type_identifier"));
                    v
                }
            }),
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
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![
                field_at("field_declaration", "declarator", "field_identifier", &[]),
                field_at("field_declaration", "declarator", "field_identifier", &[("pointer_declarator", "declarator")]),
                field_at("field_declaration", "declarator", "field_identifier", &[("array_declarator", "declarator")]),
            ],
                Some(Sigil::Ref) => vec![field_at("field_expression", "field", "field_identifier", &[])],
                None => {
                    let mut v = vec![
                field_at("field_declaration", "declarator", "field_identifier", &[]),
                field_at("field_declaration", "declarator", "field_identifier", &[("pointer_declarator", "declarator")]),
                field_at("field_declaration", "declarator", "field_identifier", &[("array_declarator", "declarator")]),
            ];
                    v.extend(vec![field_at("field_expression", "field", "field_identifier", &[])]);
                    v
                }
            }),
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
            parent_name_wrap: None, always_callee_eq: None,
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
            parent_name_wrap: None, always_callee_eq: None,
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
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

/// The declaration forms a C++ type name is introduced by: everything C has,
/// plus the class specifier.
fn cpp_type_decl_variants() -> Vec<TsVariant> {
    let mut v = super::c::type_decl_variants();
    v.push(TsVariant {
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
        parent_name_wrap: None, always_callee_eq: None,
    });
    v
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
            parent_name_wrap: None, always_callee_eq: None,
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
