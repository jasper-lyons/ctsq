//! Go `text/template` and `html/template` files (`.tmpl`, `.gotmpl`).
//! The grammar is vendored, see build.rs and DESIGN.md.

use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

unsafe extern "C" {
    fn tree_sitter_gotmpl() -> *const ();
}

/// The grammar as a tree-sitter language function, like the grammar crates export.
pub const LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_gotmpl) };

pub struct GotmplLang;

/// Every node a printing action can hold. A `pipeline_action` holding a
/// `variable_definition` or an `assignment` prints nothing, so it is left out.
const PRINTING: &str = " [(dot) (field) (variable) (selector_expression) (function_call) \
(method_call) (chained_pipeline) (parenthesized_pipeline) (nil) (true) (false) (int_literal) \
(float_literal) (imaginary_literal) (rune_literal) (interpreted_string_literal) (raw_string_literal)]";

impl Lang for GotmplLang {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "id" => Some(vec![simple("identifier"), simple("field_identifier")]),
            // `{{define "x"}}` and `{{block "x" .}}` define a template, a call
            // such as `printf` refers to a template function. The define name
            // is a string literal, so `*function#x` must be written
            // `*function#/"x"/` or `*function#"x"`.
            "function" => Some(match sigil {
                Some(Sigil::Def) => define_variants(),
                Some(Sigil::Ref) => vec![function_call()],
                None => {
                    let mut v = define_variants();
                    v.push(function_call());
                    v
                }
            }),
            // Template function calls, method calls and `{{template "x" .}}`.
            "call" => Some(vec![
                function_call(),
                at("method_call", "method", "identifier", &[("field", "name")]),
                at("template_action", "name", "interpreted_string_literal", &[]),
            ]),
            // `{{$x := …}}` declares, `$x` uses.
            "variable" | "var" | "const" => Some(match sigil {
                Some(Sigil::Def) => vec![var_def()],
                Some(Sigil::Ref) => vec![var_ref()],
                None => vec![var_def(), var_ref()],
            }),
            // `.Name` is a field, `.A.B` is a selector whose last part is a
            // field_identifier. Templates declare no fields.
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![],
                _ => vec![
                    at("field", "name", "identifier", &[]),
                    at("selector_expression", "field", "field_identifier", &[]),
                ],
            }),
            "literal" => Some(
                [
                    "interpreted_string_literal", "raw_string_literal", "int_literal",
                    "float_literal", "imaginary_literal", "rune_literal", "true", "false", "nil",
                ]
                .into_iter()
                .map(simple)
                .collect(),
            ),
            "if" => Some(vec![simple("if_action")]),
            "for" => Some(vec![simple("range_action")]),
            // An action that prints a value. `template_action#Phone` narrows it
            // to actions printing the field `.Phone` directly.
            "template_action" => Some(vec![TsVariant {
                always_constraint: Some(PRINTING),
                ..at("pipeline_action", "", "identifier", &[("field", "name")])
            }]),
            "class" | "module" | "type" | "param" | "import" | "block" | "while"
            | "switch" | "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        LANGUAGE.into()
    }
}

fn define_variants() -> Vec<TsVariant> {
    vec![
        TsVariant { body_field: Some("body"), ..at("define_action", "name", "interpreted_string_literal", &[]) },
        TsVariant { body_field: Some("body"), ..at("block_action", "name", "interpreted_string_literal", &[]) },
    ]
}

fn function_call() -> TsVariant {
    TsVariant {
        params_field: Some("arguments"),
        params_container: Some("argument_list"),
        ..at("function_call", "function", "identifier", &[])
    }
}

fn var_def() -> TsVariant {
    at("variable_definition", "variable", "identifier", &[("variable", "name")])
}

fn var_ref() -> TsVariant {
    at("variable", "name", "identifier", &[])
}

/// A node whose name is the `name_node` child reached through `name_field`
/// and `path`.
fn at(
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
        parent_name_wrap: None,
        always_callee_eq: None,
    }
}
