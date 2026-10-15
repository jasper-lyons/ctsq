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
}
