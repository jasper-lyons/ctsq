//! Per-language flow mechanics: which node kinds play which role.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Mechanics {
    pub outside: Outside,
    pub nodes: Nodes,
    /// Child field names the flow engine reads from the node kinds above.
    pub fields: Fields,
    /// Token texts the flow engine compares against.
    pub tokens: Tokens,
    /// What counts as a test, hidden by default from search output.
    pub tests: Tests,
}

/// Test detection for one language.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Tests {
    /// Globs on the file name, such as `*_test.go`. A glob holding a `/` is
    /// matched against the path relative to the search root instead.
    pub files: Vec<String>,
    /// Directory names: a file under any path segment named so is a test.
    pub dirs: Vec<String>,
    /// Selectors whose matches are test scopes inside non-test files.
    pub scopes: Vec<String>,
}

impl Mechanics {
    /// Whether `kind` is listed in `list`.
    pub fn is(list: &[String], kind: &str) -> bool {
        list.iter().any(|k| k == kind)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Outside {
    /// File holding the module path, such as go.mod.
    pub module_file: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Nodes {
    pub assign: Vec<String>,
    pub call: Vec<String>,
    #[serde(rename = "return")]
    pub return_: Vec<String>,
    pub field: Vec<String>,
    pub param: Vec<String>,
    pub import: Vec<String>,
    pub branch: Vec<String>,
    #[serde(rename = "loop")]
    pub loop_: Vec<String>,
    pub assert: Vec<String>,
    pub composite: Vec<String>,
    /// Function bodies: declarations, methods and literals.
    pub function: Vec<String>,
    /// Statement containers walked in order.
    pub block: Vec<String>,
    /// Arms of a branch node (switch cases).
    pub case: Vec<String>,
    /// The arm taken when no other matches. Without one a switch may run no arm.
    pub default_case: Vec<String>,
    /// Plain variable names.
    pub name: Vec<String>,
    /// The field name inside a field read, and in a struct declaration.
    pub field_name: Vec<String>,
    /// A field declared in a struct type.
    pub field_decl: Vec<String>,
    /// A named type declaration, its name gives the owner of a field.
    pub type_decl: Vec<String>,
    /// Loop header that binds each element of a collection.
    pub range: Vec<String>,
    /// Comma separated lists flattened into their items.
    pub list: Vec<String>,
    /// Header of a counted loop (initializer, condition, update).
    pub loop_clause: Vec<String>,
    /// Element store such as `m[k]`, assigning through it follows the collection.
    pub index: Vec<String>,
    /// Channel send statement. Data sent stops the walk (decision 12).
    pub send: Vec<String>,
    /// Declaration that, at the top of a file, makes package-level variables.
    pub global: Vec<String>,
    /// Map type node, so an element store through it is a shared stop.
    pub map_type: Vec<String>,
    /// `key: value` element of a composite literal.
    pub keyed: Vec<String>,
    /// Wrapper around one element, key or value of a composite literal.
    pub element: Vec<String>,
    /// Conversion `T(x)`, which is a call when `T` is a generic instantiation.
    pub conversion: Vec<String>,
    /// Generic instantiation such as `F[T]` in type position.
    pub generic: Vec<String>,
    /// Assignments that declare their names in the enclosing block, shadowing
    /// an outer name until the block ends.
    pub declare: Vec<String>,
    /// The current data value of a template, `.` in Go templates.
    pub dot: Vec<String>,
    /// Actions that run their body with the data rebound to another value.
    pub rebind: Vec<String>,
    /// Actions that run a named template with an argument as its data.
    pub template_call: Vec<String>,
    /// Actions that define a named template body.
    pub define: Vec<String>,
    /// A chain of commands where each result is the last argument of the next.
    pub pipeline: Vec<String>,
    /// Actions that print the value of their pipeline.
    pub print: Vec<String>,
    // The keys below are TypeScript and JavaScript roles. Each is left out of
    // `config dump` when empty, so a language without them dumps as before.
    /// Object literal `{ a: x }`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub object: Vec<String>,
    /// `key: value` entry of an object literal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pair: Vec<String>,
    /// Array literal `[a, b]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub array: Vec<String>,
    /// JSX element with children, `<A>...</A>`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsx_element: Vec<String>,
    /// JSX element without children, `<A />`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsx_self_closing: Vec<String>,
    /// `{...}` expression container inside JSX.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsx_expression: Vec<String>,
    /// `name={value}` attribute of a JSX element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsx_attribute: Vec<String>,
    /// Literal text between JSX tags.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jsx_text: Vec<String>,
    /// Destructuring patterns and their parts, `{a, b: c}`, `[x, ...y]`, `a = 1`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pattern: Vec<String>,
    /// `await x`.
    #[serde(rename = "await", skip_serializing_if = "Vec::is_empty")]
    pub await_: Vec<String>,
    /// `...x` in a call, array or object.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spread: Vec<String>,
    /// Literal values: strings, template strings, numbers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub literal: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Fields {
    /// Left side of an assignment.
    pub left: Vec<String>,
    /// Right side of an assignment.
    pub right: Vec<String>,
    /// Operator of an assignment.
    pub operator: Vec<String>,
    /// Condition of a branch or loop, which carries no data.
    pub condition: Vec<String>,
    /// Statement run before a branch or loop.
    pub initializer: Vec<String>,
    /// Statement run after each loop iteration.
    pub update: Vec<String>,
    /// Then arm of a branch.
    pub consequence: Vec<String>,
    /// Else arm of a branch.
    pub alternative: Vec<String>,
    /// Body of a loop or function.
    pub body: Vec<String>,
    /// Name bound to the switched value in a type switch.
    pub alias: Vec<String>,
    /// Case labels, skipped when walking a case body.
    pub case_label: Vec<String>,
    /// Callee of a call.
    pub function: Vec<String>,
    /// Argument list of a call.
    pub arguments: Vec<String>,
    /// Object part of a field read.
    pub operand: Vec<String>,
    /// Field part of a field read.
    pub field: Vec<String>,
    /// Name of a function or type declaration.
    pub name: Vec<String>,
    /// Type of a field declaration, composite literal, conversion or generic.
    #[serde(rename = "type")]
    pub ty: Vec<String>,
    /// Struct tag of a field declaration.
    pub tag: Vec<String>,
    /// Key of a keyed composite element.
    pub key: Vec<String>,
    /// Value of a keyed composite element or a channel send.
    pub value: Vec<String>,
    /// Channel of a send statement.
    pub channel: Vec<String>,
    /// Parameter list of a function literal.
    pub params: Vec<String>,
    /// Data argument of a template call.
    pub argument: Vec<String>,
    /// Collection a loop header ranges over.
    pub range: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Tokens {
    /// Assignment operators that replace the left side. Any other operator
    /// (such as `+=`) keeps what the left side already held.
    pub plain_assign: Vec<String>,
    /// Names that discard a value.
    pub blank: Vec<String>,
    /// Token that makes a loop header declare its names, as `:=` in a range.
    pub declare: Vec<String>,
    /// Operators whose result is about the data but is not the data, such as
    /// comparisons (decision 11). Their result carries nothing.
    pub clear_ops: Vec<String>,
}
