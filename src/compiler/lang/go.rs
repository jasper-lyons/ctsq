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
            "class" => Some(type_decl_variants()),
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
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            // `*variable#X` reaches the whole declaration statement, so --body
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
            // Declarations are `Name string` in a struct, accesses are `x.Name`.
            // Method calls are selectors too, so `&field` also lists them.
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![field_at("field_declaration", "name", "field_identifier", &[])],
                Some(Sigil::Ref) => vec![field_at("selector_expression", "field", "field_identifier", &[])],
                None => vec![
                    field_at("field_declaration", "name", "field_identifier", &[]),
                    field_at("selector_expression", "field", "field_identifier", &[]),
                ],
            }),
            // `Run()` and `pkg.Func()`: the name filter reads the last segment.
            "call" => Some(vec![call_ident(), call_selector()]),
            "comment" => Some(vec![simple("comment")]),
            "module" | "while" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }
}

/// The declaration forms a Go type name is introduced by: `type X struct{…}`
/// and `type X = Y`. Both hang the name off a spec node, so --body prints the
/// whole declaration rather than the bare identifier.
fn type_decl_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
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
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "type_alias",
            name_field: Some("name"),
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

/// The statement a Go variable or constant name is introduced by: `var x = …`,
/// `const x = …` and `x := …`. The spec node holding the name is a positional
/// child, hence the empty name_field.
fn var_decl_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "var_declaration",
            name_field: Some(""),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("var_spec", "name")],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "const_declaration",
            name_field: Some(""),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("const_spec", "name")],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "short_var_declaration",
            name_field: Some("left"),
            name_node: "identifier",
            params_field: None,
            params_container: None,
            body_field: None,
            body_container: None,
            name_path: &[("expression_list", "")],
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
        parent_name_wrap: None, always_callee_eq: None,
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
        parent_name_wrap: None, always_callee_eq: None,
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
        parent_name_wrap: None, always_callee_eq: None,
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
