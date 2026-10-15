//! Data flow inside one function (plan decisions 09, 10 and 11).
//!
//! The engine walks the statements of one function in order with a set of
//! tracked variables. It reads every node kind and child field name from
//! `config::Mechanics`, so no language node name is written here.
#![allow(dead_code)]

mod engine;
mod models;
mod objects;
mod report;
#[cfg(test)]
mod tests;

// Used by the path and models packages.
#[allow(unused_imports)]
pub use engine::{analyse_function, analyse_function_with, enclosing_function, function_name, start_at, start_in};
#[allow(unused_imports)]
pub use models::{candidate_keys, model_key, qualify_type, ModelOutside};
#[allow(unused_imports)]
pub use report::report;
#[allow(unused_imports)]
pub use objects::{base_type, join_paths, FieldInfo, Module, Paths, Reader};

use crate::config::Mechanics;
use tree_sitter::Node;

/// One parsed file and the mechanics of its language.
pub struct FlowCtx<'a, 't> {
    /// Path printed on every hop in this file.
    pub file: &'a str,
    pub src: &'a [u8],
    /// Root node of the parsed file.
    pub root: Node<'t>,
    pub mech: &'a Mechanics,
    /// Structs and package-level variables of every scanned file.
    pub module: &'a Module,
}

/// What the walk tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartKind {
    /// A variable or parameter, tracked by name.
    Var(String),
    /// Every read of this field inside the function. `owner` is the type that
    /// declares the field, when known. Reads whose object has another known
    /// type are skipped.
    Field { name: String, owner: Option<String> },
}

/// A start point, built by `start_at` from a byte range.
#[derive(Debug, Clone)]
pub struct Start {
    pub kind: StartKind,
    pub start_byte: usize,
    pub end_byte: usize,
    /// Location of the start node, printed as the root row.
    pub file: String,
    pub line: usize,
    pub func: String,
    pub text: String,
    /// Where a variable start holds the data: itself, or fields of an object.
    pub paths: Paths,
}

/// A call that receives a tracked value, handed to the `Outside` hook.
pub struct CallSite<'t> {
    pub node: Node<'t>,
    /// Source text of the callee, such as `strings.ToUpper` or `b.Write`.
    pub callee: String,
    /// Full import path when the callee is `pkg.Name` and `pkg` is an import.
    pub package: Option<String>,
    /// Name after the last dot of a selector callee.
    pub method: Option<String>,
    /// Type of the receiver variable, from `resolve::go::binding_type`.
    pub recv_type: Option<String>,
    /// `recv_type` with its package name replaced by the import path, as
    /// `html/template.Template`. None when the type has no package part.
    pub recv_path: Option<String>,
    /// Zero based positions of the arguments that carry tracked data.
    pub tracked_args: Vec<usize>,
    /// Where each tracked argument holds the data, in `tracked_args` order.
    pub arg_paths: Vec<Paths>,
    /// Whether the receiver object carries tracked data.
    pub recv_tracked: bool,
    /// Where the receiver holds the data, when tracked.
    pub recv_paths: Paths,
    /// Untracked arguments that are objects of the start field's owner type.
    /// A `whole` model carries the field through them (decision 13).
    pub whole_args: Vec<usize>,
    /// The callee is a plain name declared as a function in this file.
    pub local: bool,
}

/// What a call does with tracked data, answered by models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEffect {
    /// No model. The result carries the data and the hop is tagged `unmodelled`.
    Unknown,
    /// The result carries the data.
    Pass,
    /// The result is about the data but is not the data (decision 11).
    Clear,
    /// The receiver object and the result both carry the data, as in a buffer write.
    Whole,
    /// A model's `pass` entries and `whole` flag. Each entry whose `from` slot
    /// holds tracked data moves it to its `to` slot: the result, or the object
    /// an argument or the receiver is rooted at (`&form` tracks `form`).
    /// `whole` also counts `CallSite::whole_args` as tracked.
    Model { pass: Vec<crate::config::Pass>, whole: bool },
    /// A call into a function in the repo. The result carries the data and the
    /// hop is tagged `via <name>` instead of `unmodelled`.
    Via(String),
    /// A call into a function in the repo whose body was walked. `trees` are
    /// the walks from the parameters that received tracked arguments, hung
    /// under the call's hop. The result carries the data only when the
    /// callee `returns` it. The hop is tagged `via <name>`.
    Into { name: String, returns: bool, trees: Vec<Hop> },
}

/// The hook for calls whose effect the engine cannot see.
pub trait Outside {
    fn effect(&self, call: &CallSite) -> CallEffect;
}

impl<F: Fn(&CallSite) -> CallEffect> Outside for F {
    fn effect(&self, call: &CallSite) -> CallEffect {
        self(call)
    }
}

/// The hook used until models exist: every call is `Unknown`.
pub fn no_models(_: &CallSite) -> CallEffect {
    CallEffect::Unknown
}

/// The end selector: true when a call node is a sink.
pub type EndFn<'e> = dyn Fn(Node, &[u8]) -> bool + 'e;

/// One step of a route. Children are the hops that read what this hop
/// produced, after the walks into any called in-repo function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub file: String,
    /// 1 based.
    pub line: usize,
    pub func: String,
    /// Trimmed source line.
    pub text: String,
    /// Tags of decision 09, in the order primary, field, conditional, unmodelled, sink.
    pub tags: Vec<String>,
    /// Callee text of the first unmodelled call on this hop.
    pub unmodelled: Option<String>,
    /// A return statement that hands tracked data back to the caller.
    pub returns: bool,
    /// The root of a walk inside a called function, hung under the call.
    pub into: bool,
    pub children: Vec<Hop>,
}

impl Hop {
    pub fn has(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// This hop and every descendant, depth first.
    pub fn walk(&self) -> Vec<&Hop> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

/// The routes from one start point through one function.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowResult {
    /// The start point, tagged `source`.
    pub root: Hop,
    /// Whether a return statement of the walked function carries tracked data.
    pub returns: bool,
}

impl FlowResult {
    /// Number of hops tagged `sink`.
    pub fn sinks(&self) -> usize {
        self.root.walk().iter().filter(|h| h.has("sink")).count()
    }

    /// Leaf hops that end at an unmodelled call without reaching a sink.
    pub fn unmodelled_ends(&self) -> Vec<&Hop> {
        self.root
            .walk()
            .into_iter()
            .filter(|h| h.children.is_empty() && h.unmodelled.is_some() && !h.has("sink"))
            .collect()
    }
}
