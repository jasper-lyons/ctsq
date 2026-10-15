#![allow(dead_code)]
//! Stages that run after tree-sitter matching: attribute filters, and the
//! call combinators (`calls`). A `path` query is cut at its top-level
//! semantic combinators into AST segments, each compiled and run like a `-q`
//! query, and the last selector of each segment supplies the definitions the
//! walk starts at, passes through or ends at.

pub mod attrs;
pub mod calls;

use crate::compiler;
use crate::query::ast::{Combinator, Query, Selector};
use crate::resolve::{DefIndex, TreeStore};
use tree_sitter::{Node, Point};

/// A query cut at its top-level semantic combinators.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// AST segments in reading order. None holds a semantic combinator or a
    /// `:not()`, so each compiles like a `-q` query.
    pub segments: Vec<Query>,
    /// `joins[i]` sits between `segments[i]` and `segments[i + 1]`.
    pub joins: Vec<Combinator>,
    /// The `:not()` selectors taken off the last selector of each segment,
    /// one list per segment.
    pub nots: Vec<Vec<Selector>>,
}

const MISPLACED: &str = "call and data-flow combinators are only allowed at the top level of a path query, and :not() only on the last selector of each step";

/// Cut `q` at every top-level semantic combinator. A semantic combinator
/// inside a group or a field inner query, or a `:not()` anywhere but on the
/// last selector of a segment, is an error.
pub fn split_query(q: &Query) -> Result<Split, String> {
    let mut split = Split { segments: vec![], joins: vec![], nots: vec![] };
    let mut cur = Query { head: q.head.clone(), tail: vec![] };
    for (comb, sel) in &q.tail {
        if comb.is_semantic() {
            let done = std::mem::replace(&mut cur, Query { head: sel.clone(), tail: vec![] });
            finish_segment(done, &mut split)?;
            split.joins.push(comb.clone());
        } else {
            cur.tail.push((comb.clone(), sel.clone()));
        }
    }
    finish_segment(cur, &mut split)?;
    Ok(split)
}

fn finish_segment(mut seg: Query, split: &mut Split) -> Result<(), String> {
    let last = match seg.tail.last_mut() {
        Some((_, sel)) => sel,
        None => &mut seg.head,
    };
    let nots = std::mem::take(&mut last.not);
    for n in &nots {
        if compiler::has_semantic(&Query { head: n.clone(), tail: vec![] }) {
            return Err(MISPLACED.into());
        }
    }
    if compiler::has_semantic(&seg) {
        return Err(MISPLACED.into());
    }
    split.segments.push(seg);
    split.nots.push(nots);
    Ok(())
}

/// Definitions (indices into `index.defs`) whose declaration the last
/// selector of `seg` matches, sorted. A row matches a definition when it
/// overlaps the definition's header: from the start of the declaration to
/// the start of its body. So `function#main` matches the declaration of
/// `main` and not a call to `main` inside another body.
///
/// A file whose language rejects the segment (an unknown kind, an attribute
/// the language does not support) is skipped. When every file rejects it,
/// the first error is returned.
pub fn segment_defs(seg: &Query, store: &TreeStore, index: &DefIndex) -> Result<Vec<usize>, String> {
    let attrs = attrs::last_selector_attrs(seg)?;
    let mut first_err: Option<String> = None;
    let mut accepted = false;
    let mut out = Vec::new();
    for (file, pf) in store.iter() {
        let Some(tree) = &pf.tree else { continue };
        let Some(lang) = compiler::lang::for_name(pf.lang) else { continue };
        let checked = compiler::validate(seg, lang.as_ref(), pf.lang)
            .and_then(|_| attrs::check_lang(&attrs, pf.lang));
        if let Err(e) = checked {
            first_err.get_or_insert(e);
            continue;
        }
        accepted = true;
        let compiled = compiler::compile(seg, lang.as_ref());
        let rows = crate::runner::run_rows(&compiled, lang.as_ref(), &pf.source, false)?;
        let rows = attrs::filter_rows(rows, &attrs, pf.lang, &lang.ts_language(), &pf.source);
        if rows.is_empty() {
            continue;
        }
        let headers = def_headers(file, tree.root_node(), &pf.source, index);
        for r in &rows {
            for &(d, start, end) in &headers {
                if r.start_byte < end && r.end_byte > start {
                    out.push(d);
                }
            }
        }
    }
    if !accepted {
        if let Some(e) = first_err {
            return Err(e);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// (def index, declaration start byte, header end byte) for every def in
/// `file`. The declaration is the outermost node that starts where the def's
/// line starts its code. The header ends where its body starts, or with the
/// declaration when it has none.
pub fn def_headers(file: &str, root: Node, source: &[u8], index: &DefIndex) -> Vec<(usize, usize, usize)> {
    let mut line_starts = vec![0usize];
    for (i, b) in source.iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let mut out = Vec::new();
    for (i, d) in index.defs.iter().enumerate() {
        if d.id.file != file {
            continue;
        }
        let Some(&ls) = line_starts.get(d.id.line.saturating_sub(1)) else { continue };
        let col = source[ls..]
            .iter()
            .take_while(|b| **b == b' ' || **b == b'\t')
            .count();
        let p = Point { row: d.id.line - 1, column: col };
        let Some(mut node) = root.descendant_for_point_range(p, p) else { continue };
        while let Some(parent) = node.parent() {
            if parent.parent().is_none() || parent.start_byte() != node.start_byte() {
                break;
            }
            node = parent;
        }
        let body = node.child_by_field_name("body").or_else(|| {
            let mut c = node.walk();
            let kids: Vec<Node> = node.named_children(&mut c).collect();
            kids.into_iter().find_map(|k| k.child_by_field_name("body"))
        });
        let end = body.map(|b| b.start_byte()).unwrap_or(node.end_byte());
        out.push((i, node.start_byte(), end));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::parse;

    #[test]
    fn a_query_splits_at_top_level_semantic_combinators_only() {
        let q = parse("function#main > call#x --> function#load <- function#b").unwrap();
        let s = split_query(&q).unwrap();
        assert_eq!(s.segments.len(), 3);
        assert_eq!(s.segments[0].tail.len(), 1, "the AST combinator stays in its segment");
        assert_eq!(s.joins, vec![Combinator::CallsEventually, Combinator::CalledBy]);
        assert!(s.segments.iter().all(|q| !compiler::has_semantic(q)));
    }

    #[test]
    fn not_on_the_last_selector_is_taken_off() {
        let q = parse("function#main --> function#c:not(function#a)").unwrap();
        let s = split_query(&q).unwrap();
        assert!(s.nots[0].is_empty());
        assert_eq!(s.nots[1].len(), 1);
        assert!(s.segments[1].head.not.is_empty());
    }

    #[test]
    fn misplaced_semantic_parts_are_rejected() {
        for q in [
            "function#main:not(call#a) > call#b --> function#c",
            "function#main:not(function#a:not(function#b))",
            "(function#a --> function#b)",
        ] {
            let parsed = parse(q).unwrap();
            assert!(split_query(&parsed).is_err(), "{} should be rejected", q);
        }
    }

    #[test]
    fn a_declaration_matches_its_def_and_a_call_in_a_body_does_not() {
        let src = "package a\nfunc helper() {}\nfunc Go() { helper() }\n";
        let mut store = TreeStore::default();
        store.insert("a/a.go", "go", src.as_bytes().to_vec());
        let g = crate::resolve::graph::build_from_store(&store);
        let q = parse("function#helper").unwrap();
        let defs = segment_defs(&q, &store, &g.index).unwrap();
        let names: Vec<&str> = defs.iter().map(|&d| g.index.defs[d].name.as_str()).collect();
        assert_eq!(names, vec!["helper"]);
    }
}
