#[derive(Debug, Clone, PartialEq)]
pub enum Sigil {
    Def, // *
    Ref, // &
}

#[derive(Debug, Clone, PartialEq)]
pub enum NameMatch {
    Exact(String),  // #word or #"text"
    Regex(String),  // #/regex/
}

#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub sigil: Option<Sigil>,
    pub node_type: Option<String>,
    pub name_match: Option<NameMatch>,
    /// `[key op value]` filters. Carried in the AST, ignored by `compile_atom`.
    pub attrs: Vec<AttrMatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttrOp {
    Eq,       // =
    Prefix,   // ^=
    Contains, // *=
    Suffix,   // $=
    Regex,    // =/regex/
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrMatch {
    pub key: String,
    pub op: AttrOp,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAccess {
    pub field: String,
    pub inner: Option<Box<Query>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectorNode {
    Bare(Atom),
    Group {
        query: Box<Query>,
        capture: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub node: SelectorNode,
    pub fields: Vec<FieldAccess>,
    /// `:not(selector)` exclusions, in source order.
    pub not: Vec<Selector>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Combinator {
    Descendant, // space
    Child,      // >
    Adjacent,   // +
    Sibling,    // ~
    // Semantic combinators. Rejected by `compiler::validate`, evaluated by `path`.
    Calls,                // ->
    CallsEventually,      // -->
    CalledBy,             // <-
    CalledByEventually,   // <--
    Flows,                // =>
    FlowsEventually,      // ==>
    FlowsFrom,            // <=
    FlowsFromEventually,  // <==
}

impl Combinator {
    pub fn is_semantic(&self) -> bool {
        !matches!(
            self,
            Combinator::Descendant | Combinator::Child | Combinator::Adjacent | Combinator::Sibling
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub head: Selector,
    pub tail: Vec<(Combinator, Selector)>,
}

/// A query that does not parse or does not compile. `pos` is a byte offset into
/// the query string the user typed, so the report can put a caret under the fault.
/// `hint` is a suggested fix when one is knowable.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryError {
    pub msg: String,
    pub pos: usize,
    pub hint: Option<String>,
}

impl QueryError {
    pub fn new(msg: impl Into<String>, pos: usize) -> Self {
        Self { msg: msg.into(), pos, hint: None }
    }

    /// Sets the hint unless one is already there, so the most specific one wins.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        if self.hint.is_none() {
            self.hint = Some(hint.into());
        }
        self
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at position {}", self.msg, self.pos)
    }
}
