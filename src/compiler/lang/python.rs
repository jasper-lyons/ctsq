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
                        v.push(call_variant());
                    }
                    v
                }
                Some(Sigil::Ref) => vec![call_variant()],
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
            }]),
            "id" => Some(vec![identifier()]),
            "var" => Some(vec![identifier()]),
            "param" => Some(vec![identifier()]),
            "import" => Some(vec![simple("import_statement")]),
            "literal" => Some(vec![
                simple("integer"),
                simple("float"),
                simple("string"),
            ]),
            "block" => Some(vec![simple("block")]),
            "if" => Some(vec![simple("if_statement")]),
            "for" => Some(vec![simple("for_statement")]),
            "while" => Some(vec![simple("while_statement")]),
            "call" | "module" | "switch" | "type" | "op" => Some(vec![]),
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
    }
}

fn call_variant() -> TsVariant {
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
