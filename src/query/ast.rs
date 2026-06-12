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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Combinator {
    Descendant, // space
    Child,      // >
    Adjacent,   // +
    Sibling,    // ~
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub head: Selector,
    pub tail: Vec<(Combinator, Selector)>,
}
