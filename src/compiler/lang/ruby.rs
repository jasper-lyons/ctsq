use crate::query::ast::Sigil;
use super::{Lang, TsVariant};

pub struct RubyLang;

impl Lang for RubyLang {
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
            "call" => Some(call_variants()),
            "class" => Some(vec![body_holder("class")]),
            "module" => Some(vec![body_holder("module")]),
            "id" => Some(vec![identifier()]),
            "var" => Some(vec![identifier()]),
            "param" => Some(vec![identifier()]),
            // `*variable#x` reaches the whole `x = …` assignment, so --body prints
            // it. `&variable#x` stays on the uses.
            "variable" => Some(match sigil {
                Some(Sigil::Ref) => vec![identifier()],
                Some(Sigil::Def) => vec![assignment_to("identifier")],
                None => vec![assignment_to("identifier"), identifier()],
            }),
            // A Ruby constant is a capitalised name, not a keyword, so `const`
            // is its own mapping rather than an alias of `variable`.
            "const" => Some(match sigil {
                Some(Sigil::Ref) => vec![simple("constant")],
                Some(Sigil::Def) => vec![assignment_to("constant")],
                None => vec![assignment_to("constant"), simple("constant")],
            }),
            // Ruby has no type declarations. A constant standing in for a class
            // or module name is the nearest thing, so `type` matches those uses.
            "type" => Some(vec![simple("constant")]),
            "import" => Some(import_variants()),
            "literal" => Some(vec![
                simple("integer"),
                simple("float"),
                simple("string"),
                simple("simple_symbol"),
                simple("true"),
                simple("false"),
                simple("nil"),
            ]),
            "block" => Some(vec![
                simple("body_statement"),
                simple("block"),
                simple("do_block"),
            ]),
            "if" => Some(vec![simple("if"), simple("unless")]),
            "while" => Some(vec![simple("while"), simple("until")]),
            "for" => Some(vec![simple("for")]),
            "switch" => Some(vec![simple("case")]),
            // A Ruby field is an instance variable. `@x = …` declares it and every
            // `@x` is an access. Receiver calls like `u.x` are method calls, not fields.
            "field" => Some(match sigil {
                Some(Sigil::Def) => vec![field_at("assignment", "left", "instance_variable", &[])],
                _ => vec![simple("instance_variable")],
            }),
            "comment" => Some(vec![simple("comment")]),
            "op" => Some(vec![]),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_ruby::LANGUAGE.into()
    }
}

/// `def foo` and `def self.foo`. Both hang the name off a `name` field that can
/// hold an identifier, a constant, a setter or an operator. Only the identifier
/// form is filtered by name, which covers ordinary method names.
fn def_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            node: "method",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("method_parameters"),
            body_field: Some("body"),
            body_container: Some("body_statement"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
        TsVariant {
            node: "singleton_method",
            name_field: Some("name"),
            name_node: "identifier",
            params_field: Some("parameters"),
            params_container: Some("method_parameters"),
            body_field: Some("body"),
            body_container: Some("body_statement"),
            name_path: &[],
            params_needs_bridge: false,
            always_constraint: None,
            parent_name_wrap: None, always_callee_eq: None,
        },
    ]
}

/// `foo(1)` and `u.send_email(x)` are both a `call` whose `method` field holds
/// the name, so one variant covers the receiverless and the receiver forms.
/// A bare `foo` with neither parentheses nor receiver parses as a plain
/// identifier, not a call, and is out of reach here the way it is in Python.
fn call_variants() -> Vec<TsVariant> {
    vec![TsVariant {
        node: "call",
        name_field: Some("method"),
        name_node: "identifier",
        params_field: Some("arguments"),
        params_container: Some("argument_list"),
        body_field: None,
        body_container: None,
        name_path: &[],
        params_needs_bridge: false,
        always_constraint: None,
        parent_name_wrap: None, always_callee_eq: None,
    }]
}

/// `class Foo … end` and `module Foo … end` share one shape: a constant name
/// and a `body_statement` body.
fn body_holder(node: &'static str) -> TsVariant {
    TsVariant {
        name_field: Some("name"),
        name_node: "constant",
        body_field: Some("body"),
        body_container: Some("body_statement"),
        ..simple(node)
    }
}

/// `X = …`, filtered on whichever left-hand node kind the caller wants: a
/// `constant` for `const`, an `identifier` for `variable`.
fn assignment_to(name_node: &'static str) -> TsVariant {
    TsVariant {
        name_field: Some("left"),
        name_node,
        ..simple("assignment")
    }
}

/// Every construct that pulls in another file. `require` and `require_relative`
/// are ordinary method calls, so each needs a forced callee-name predicate the
/// way CommonJS `require(...)` does in JavaScript.
fn import_variants() -> Vec<TsVariant> {
    vec![
        TsVariant {
            name_field: Some("method"),
            name_node: "identifier",
            always_callee_eq: Some("require"),
            ..simple("call")
        },
        TsVariant {
            name_field: Some("method"),
            name_node: "identifier",
            always_callee_eq: Some("require_relative"),
            ..simple("call")
        },
    ]
}

fn identifier() -> TsVariant {
    simple("identifier")
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
        parent_name_wrap: None,
        always_callee_eq: None,
    }
}
