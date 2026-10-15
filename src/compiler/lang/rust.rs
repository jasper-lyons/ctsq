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
            "class" => Some(class_variants()),
            // Bare `type` is the identifier node, so `*type#X` used to match the
            // name and nothing else. The def sigil now reaches the item the name
            // belongs to, which is what --body needs.
            "type" => Some(match sigil {
                Some(Sigil::Ref) => vec![simple("type_identifier")],
                Some(Sigil::Def) => type_decl_variants(),
                None => {
                    let mut v = type_decl_variants();
                    v.push(simple("type_identifier"));
                    v
                }
            }),
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
                parent_name_wrap: None, always_callee_eq: None,
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
                parent_name_wrap: None, always_callee_eq: None,
            }]),
            // `*variable#X` reaches the whole `let`, `const` or `static` item, so
            // --body prints it. `&variable#X` stays on the uses. `const` is an alias.
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
                parent_name_wrap: None, always_callee_eq: None,
            }]),
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
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![field_at("field_declaration", "name", "field_identifier", &[])],
                Some(Sigil::Ref) => vec![field_at("field_expression", "field", "field_identifier", &[])],
                None => {
                    let mut v = vec![field_at("field_declaration", "name", "field_identifier", &[])];
                    v.extend(vec![field_at("field_expression", "field", "field_identifier", &[])]);
                    v
                }
            }),
            "call" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }
}

fn named_item(node: &'static str, body_container: &'static str) -> TsVariant {
    TsVariant {
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
    }
}

fn class_variants() -> Vec<TsVariant> {
    vec![
        named_item("struct_item", "field_declaration_list"),
        named_item("enum_item", "enum_variant_list"),
        named_item("trait_item", "declaration_list"),
    ]
}

/// The item forms a Rust type name is introduced by. Adds the alias and the
/// union to the struct/enum/trait set, so `*type#X --body` prints whichever
/// declaration the name belongs to.
fn type_decl_variants() -> Vec<TsVariant> {
    let mut v = class_variants();
    v.push(named_item("union_item", "field_declaration_list"));
    v.push(TsVariant {
        node: "type_item",
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
    });
    v
}

/// The item a Rust value name is introduced by: `let x = …`, `const X: T = …`
/// and `static X: T = …`.
fn var_decl_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
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
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "const_item",
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
            node: "static_item",
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
        parent_name_wrap: None, always_callee_eq: None,
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
