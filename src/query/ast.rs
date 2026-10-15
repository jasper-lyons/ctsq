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
