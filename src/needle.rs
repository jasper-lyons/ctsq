//! Needles: the literal texts a query cannot match without.
//!
//! A file whose bytes lack the needles of a query cannot hold a match for it,
//! so it is skipped before it is parsed (perf plan decision 02). A needle must
//! be provably required, so only name filters count: an exact name, or a regex
//! that is a literal or an alternation of literals. A bare kind, a regex with
//! metacharacters, a capture and a `:not(...)` exclusion give none, and a query
//! with no needle parses every file as before.

use crate::query::ast::{NameMatch, Query, Selector, SelectorNode};

/// Groups of alternative texts. A file can match only when, for every group,
/// it holds at least one of the group's texts. An exact name is a group of one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Needles {
    groups: Vec<Vec<String>>,
}

impl Needles {
    /// No needle: every file may match.
    pub fn none() -> Needles {
        Needles::default()
    }

    /// The needle of one exact name. An empty name gives none.
    pub fn exact(name: &str) -> Needles {
        Needles::from_name(&NameMatch::Exact(name.to_string()))
    }

    /// The needle of one name filter, if it has one.
    pub fn from_name(nm: &NameMatch) -> Needles {
        let mut n = Needles::none();
        n.add_name(nm);
        n
    }

    /// The needles every match of `q` must hold: the name filters of its
    /// selectors, of the queries inside groups and of the inner queries of
    /// field accesses. `:not(...)` selectors are left out, they exclude.
    pub fn from_query(q: &Query) -> Needles {
        let mut n = Needles::none();
        n.add_query(q);
        n
    }

    /// The needles of several queries that must all match in the same file.
    pub fn from_queries(qs: &[Query]) -> Needles {
        let mut n = Needles::none();
        for q in qs {
            n.add_query(q);
        }
        n
    }

    pub fn is_none(&self) -> bool {
        self.groups.is_empty()
    }

    /// The groups, as the tests read them.
    #[cfg(test)]
    pub fn groups(&self) -> &[Vec<String>] {
        &self.groups
    }

    /// Whether `source` holds one text of every group.
    pub fn holds(&self, source: &[u8]) -> bool {
        self.groups.iter().all(|alts| alts.iter().any(|n| holds(source, n)))
    }

    fn add_name(&mut self, nm: &NameMatch) {
        match nm {
            NameMatch::Exact(n) if !n.is_empty() => self.groups.push(vec![n.clone()]),
            NameMatch::Regex(r) => {
                if let Some(alts) = regex_literals(r) {
                    self.groups.push(alts);
                }
            }
            _ => {}
        }
    }

    fn add_query(&mut self, q: &Query) {
        self.add_selector(&q.head);
        for (_, s) in &q.tail {
            self.add_selector(s);
        }
    }

    fn add_selector(&mut self, s: &Selector) {
        match &s.node {
            SelectorNode::Bare(a) => {
                if let Some(nm) = &a.name_match {
                    self.add_name(nm);
                }
            }
            SelectorNode::Group { query, .. } => self.add_query(query),
        }
        for f in &s.fields {
            if let Some(inner) = &f.inner {
                self.add_query(inner);
            }
        }
    }
}

/// The literal texts a regex name filter cannot match without: one of them
/// has to occur in the node's text. Only the plain shapes give any, a literal
/// or an alternation of literals, optionally anchored and grouped
/// (`Error$`, `^(Get|Post)$`). Anything else (classes, repeats, flags) gives
/// None and the filter is not used to skip files.
pub fn regex_literals(src: &str) -> Option<Vec<String>> {
    let s = src.strip_prefix('^').unwrap_or(src);
    let s = s.strip_suffix('$').unwrap_or(s);
    let s = match s.strip_prefix('(') {
        Some(inner) => inner.strip_prefix("?:").unwrap_or(inner).strip_suffix(')')?,
        None => s,
    };
    let alts: Vec<String> = s.split('|').map(str::to_string).collect();
    let plain = |a: &String| !a.is_empty() && a.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' ' | ':'));
    alts.iter().all(plain).then_some(alts)
}

/// Whether `hay` holds the bytes of `needle`. Valid UTF-8, which source nearly
/// always is, goes through the standard library's substring search, which is
/// far faster than comparing a window at every offset.
pub fn holds(hay: &[u8], needle: &str) -> bool {
    match std::str::from_utf8(hay) {
        Ok(s) => s.contains(needle),
        Err(_) => {
            let n = needle.as_bytes();
            n.is_empty() || hay.windows(n.len()).any(|w| w == n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(q: &str) -> Vec<Vec<String>> {
        Needles::from_query(&crate::query::parse(q).unwrap()).groups().to_vec()
    }

    fn g(alts: &[&str]) -> Vec<String> {
        alts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_lone_exact_name_is_a_needle() {
        assert_eq!(of("id#X"), vec![g(&["X"])]);
        assert_eq!(of("*function#X"), vec![g(&["X"])]);
        assert_eq!(of("&function#X"), vec![g(&["X"])]);
        assert_eq!(of("id#\"two words\""), vec![g(&["two words"])]);
    }

    #[test]
    fn a_regex_of_plain_literals_gives_alternatives_and_others_give_none() {
        assert_eq!(regex_literals("Error$"), Some(g(&["Error"])));
        assert_eq!(regex_literals("^(Get|Post)$"), Some(g(&["Get", "Post"])));
        assert_eq!(regex_literals("^(?:aria-label|title)$"), Some(g(&["aria-label", "title"])));
        for none in ["^[A-Z]", "\\S", "a.b", "(?i)foo", "a*", "", "(a)(b)", "a|b.", "a+"] {
            assert_eq!(regex_literals(none), None, "{}", none);
        }
        assert_eq!(of("id#/^write/"), vec![g(&["write"])]);
        assert_eq!(of("id#/^(Get|Post)$/"), vec![g(&["Get", "Post"])]);
    }

    #[test]
    fn a_regex_with_metacharacters_gives_no_needle() {
        assert!(of("id#/x.y/").is_empty());
        assert!(of("id#/^[A-Z]/").is_empty());
        assert!(Needles::from_query(&crate::query::parse("id#/(?i)foo/").unwrap()).is_none());
    }

    #[test]
    fn a_sibling_pair_needs_both_names() {
        assert_eq!(of("(*function#register @a) ~ (*function#login @b)"), vec![g(&["register"]), g(&["login"])]);
        // One side without an exact name adds none, the other still counts.
        assert_eq!(of("(*function#register @a) ~ (id#/x.y/ @b)"), vec![g(&["register"])]);
        assert_eq!(of("function#a id#b"), vec![g(&["a"]), g(&["b"])]);
    }

    #[test]
    fn a_body_filter_inside_a_field_is_required() {
        assert_eq!(of("*function#register.body(&function#writeError)"), vec![g(&["register"]), g(&["writeError"])]);
        assert_eq!(of("*function.body(id#X)"), vec![g(&["X"])]);
        assert!(of("*function.body()").is_empty());
    }

    #[test]
    fn bare_kinds_and_captures_only_give_none() {
        assert!(of("*function").is_empty());
        assert!(of("id").is_empty());
        assert!(of("(*function @f)").is_empty());
        assert!(of("(function_declaration @f)").is_empty());
        assert!(Needles::from_query(&crate::query::parse("*function").unwrap()).is_none());
    }

    #[test]
    fn a_group_keeps_the_names_inside_it() {
        assert_eq!(of("(&function#malloc @call)"), vec![g(&["malloc"])]);
    }

    #[test]
    fn several_queries_need_all_their_names() {
        let a = crate::query::parse("id#A").unwrap();
        let b = crate::query::parse("*function").unwrap();
        let c = crate::query::parse("id#/B|C/").unwrap();
        let n = Needles::from_queries(&[a, b, c]);
        assert_eq!(n.groups().to_vec(), vec![g(&["A"]), g(&["B", "C"])]);
        assert!(n.holds(b"A and C"));
        assert!(!n.holds(b"A only"));
        assert!(!n.holds(b"B only"));
        assert!(Needles::none().holds(b""));
    }

    #[test]
    fn holds_searches_non_utf8_bytes_too() {
        assert!(holds(b"ab\xffcd", "cd"));
        assert!(!holds(b"ab\xffcd", "xy"));
        assert!(holds(b"abc", ""));
    }
}
