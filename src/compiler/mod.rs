pub mod lang;

use crate::query::ast::*;
use lang::{Lang, TsVariant};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The compiled output of a ctsq query.
#[derive(Debug, Clone)]
pub enum CompiledQuery {
    /// A single tree-sitter pattern — executed directly.
    Simple(String),
    /// Two-phase scoped search: run `outer` to find container nodes, navigate each to
    /// `scope_field`, then search that subtree with `inner`.
    Scoped { outer: String, scope_field: String, inner: String },
}

impl CompiledQuery {
    pub fn is_empty(&self) -> bool {
        match self {
            CompiledQuery::Simple(s) => s.is_empty(),
            CompiledQuery::Scoped { outer, inner, .. } => outer.is_empty() || inner.is_empty(),
        }
    }
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fresh_id() -> usize {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Fragment — structured intermediate representation of a compiled TS pattern.
//
// Serialised form:
//   no predicates:   (node_body) @outer_cap
//   with predicates: ((node_body) @outer_cap pred1 pred2 ...)
//
// `pending_name_match` is set for "self-name" nodes (e.g. identifier) where
// the node's own text IS the name. It is resolved against the outer_capture
// (provided by a Group) before emitting, producing a concrete predicate.
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
struct Fragment {
    node_body: String,
    predicates: Vec<String>,
    outer_capture: Option<String>,
    /// Deferred name predicate for nodes where the node itself is the name.
    /// Resolved when a Group provides outer_capture, or at emit time.
    pending_name_match: Option<NameMatch>,
    // Field routing — set from whichever TsVariant produced this fragment
    params_field: Option<&'static str>,
    params_container: Option<&'static str>,
    params_needs_bridge: bool,
    body_field: Option<&'static str>,
    body_container: Option<&'static str>,
}

impl Fragment {
    /// Resolve `pending_name_match` against the current `outer_capture` (or a fresh internal cap).
    fn resolve(&mut self) {
        if let Some(nm) = self.pending_name_match.take() {
            let cap = self
                .outer_capture
                .clone()
                .unwrap_or_else(|| {
                    let id = fresh_id();
                    let c = format!("_nm{}", id);
                    self.outer_capture = Some(c.clone());
                    c
                });
            self.predicates.push(name_predicate(&format!("@{}", cap), &nm));
        }
    }

    fn emit(mut self) -> String {
        self.resolve();
        let node = format!("({})", self.node_body);
        let cap = self.outer_capture.get_or_insert_with(|| format!("_root{}", fresh_id()));
        let with_cap = format!("{} @{}", node, cap);
        if self.predicates.is_empty() {
            with_cap
        } else {
            format!("({} {})", with_cap, self.predicates.join(" "))
        }
    }

    /// Produce the inline node string for use inside a field position (no outer predicates).
    fn inline(&self) -> String {
        let node = format!("({})", self.node_body);
        if let Some(cap) = &self.outer_capture {
            format!("{} @{}", node, cap)
        } else {
            node
        }
    }

    /// Inject a field access and lift inner predicates to this fragment.
    fn inject_field(&mut self, ts_field: &str, container: Option<&str>, inner: &Fragment) {
        let inner_inline = inner.inline();
        let field_value = if let Some(c) = container {
            format!("({} {})", c, inner_inline)
        } else {
            inner_inline
        };
        self.node_body
            .push_str(&format!(" {}: {}", ts_field, field_value));
        self.predicates.extend(inner.predicates.clone());
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------
pub fn compile(query: &Query, lang: &dyn Lang) -> CompiledQuery {
    if let Some((outer, scope_field, inner)) = try_compile_scoped(query, lang) {
        return CompiledQuery::Scoped { outer, scope_field, inner };
    }
    let frags = compile_query(query, lang);
    CompiledQuery::Simple(emit_alternation(frags))
}

/// If the query has a head-selector with exactly one field access that has an inner
/// query and no tail combinators, compile it as a two-phase scoped query.
/// The scope_field is the field name used to navigate from each outer match to the
/// subtree that the inner query searches within.
fn try_compile_scoped(query: &Query, lang: &dyn Lang) -> Option<(String, String, String)> {
    if !query.tail.is_empty() {
        return None;
    }

    // Find the first field access that carries an inner query — that's the scope.
    // "params" uses structural embedding (not containment search) so it stays in the Simple path.
    let scoped_fa = query.head.fields.iter()
        .find(|fa| fa.inner.is_some() && fa.field != "params")?;
    let scope_field = scoped_fa.field.clone();
    let inner_q = scoped_fa.inner.as_ref()?;

    // Outer: head node with any non-scoped field accesses retained (e.g. .params() filters)
    // but the scoped field access removed.
    let outer_sel = Selector {
        node: query.head.node.clone(),
        fields: query.head.fields.iter()
            .filter(|fa| fa.inner.is_none())
            .cloned()
            .collect(),
    };
    let outer_str = emit_alternation(compile_selector(&outer_sel, lang));
    if outer_str.is_empty() {
        return None;
    }

    let inner_str = emit_alternation(compile_query(inner_q, lang));
    if inner_str.is_empty() {
        return None;
    }

    Some((outer_str, scope_field, inner_str))
}

fn emit_alternation(frags: Vec<Fragment>) -> String {
    let mut unique: Vec<Fragment> = Vec::new();
    for frag in frags {
        let dup = unique.iter().any(|f| {
            f.node_body == frag.node_body
                && f.predicates == frag.predicates
                && f.pending_name_match == frag.pending_name_match
                && f.outer_capture == frag.outer_capture
        });
        if !dup {
            unique.push(frag);
        }
    }
    match unique.len() {
        0 => String::new(),
        1 => unique.into_iter().next().unwrap().emit(),
        _ => {
            let parts: Vec<String> = unique.into_iter().map(|f| f.emit()).collect();
            format!("[{}]", parts.join("\n "))
        }
    }
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------
fn compile_query(query: &Query, lang: &dyn Lang) -> Vec<Fragment> {
    let mut frags = compile_selector(&query.head, lang);
    for (combinator, sel) in &query.tail {
        match combinator {
            Combinator::Child => {
                // Direct child: nest the tail pattern inside the head.
                let tail_frags: Vec<Fragment> = compile_selector(sel, lang)
                    .into_iter()
                    .map(|mut f| { f.resolve(); f })
                    .collect();
                if tail_frags.is_empty() {
                    continue;
                }
                frags = frags
                    .into_iter()
                    .flat_map(|head| {
                        tail_frags.iter().map(move |tail| {
                            let mut out = head.clone();
                            out.node_body.push_str(&format!(" {}", tail.inline()));
                            out.predicates.extend(tail.predicates.clone());
                            out
                        })
                    })
                    .collect();
            }
            Combinator::Descendant => {
                // Tree-sitter nesting is direct-child only, so wrap the tail in a
                // single `_` wildcard to bridge one intermediate grammar node
                // (e.g. class_specifier → field_declaration_list → function_definition).
                let tail_frags: Vec<Fragment> = compile_selector(sel, lang)
                    .into_iter()
                    .map(|mut f| { f.resolve(); f })
                    .collect();
                if tail_frags.is_empty() {
                    continue;
                }
                frags = frags
                    .into_iter()
                    .flat_map(|head| {
                        tail_frags.iter().map(move |tail| {
                            let mut out = head.clone();
                            // (_ (child)) lets the tail appear one grammar node deeper.
                            out.node_body.push_str(&format!(" (_ {})", tail.inline()));
                            out.predicates.extend(tail.predicates.clone());
                            out
                        })
                    })
                    .collect();
            }
            Combinator::Adjacent | Combinator::Sibling => {
                // Sibling combinators have no direct TS query equivalent — emit independently.
                frags.extend(compile_selector(sel, lang));
            }
        }
    }
    frags
}

fn compile_selector(sel: &Selector, lang: &dyn Lang) -> Vec<Fragment> {
    let mut frags = compile_selector_node(&sel.node, lang);
    for fa in &sel.fields {
        frags = apply_field_access(frags, fa, lang);
    }
    frags
}

fn compile_selector_node(node: &SelectorNode, lang: &dyn Lang) -> Vec<Fragment> {
    match node {
        SelectorNode::Bare(atom) => compile_atom(atom, lang),
        SelectorNode::Group { query, capture } => {
            let mut inner = compile_query(query, lang);
            for frag in &mut inner {
                // Resolve pending name predicates using the Group's capture first.
                if let Some(nm) = frag.pending_name_match.take() {
                    if let Some(cap) = capture {
                        frag.predicates
                            .push(name_predicate(&format!("@{}", cap), &nm));
                    } else {
                        // No user-visible capture — resolve with an internal one.
                        let id = fresh_id();
                        let cap = format!("_nm{}", id);
                        frag.outer_capture = Some(cap.clone());
                        frag.predicates
                            .push(name_predicate(&format!("@{}", cap), &nm));
                    }
                }
                frag.outer_capture = capture.clone();
            }
            inner
        }
    }
}

fn compile_atom(atom: &Atom, lang: &dyn Lang) -> Vec<Fragment> {
    let node_type = match &atom.node_type {
        Some(t) => t.as_str(),
        None => return vec![compile_wildcard(atom)],
    };

    match lang.resolve(node_type, atom.sigil.as_ref()) {
        None => vec![compile_passthrough(node_type, atom)],
        Some(variants) if variants.is_empty() => vec![],
        Some(variants) => variants.iter().map(|v| compile_variant(v, atom)).collect(),
    }
}

fn compile_wildcard(atom: &Atom) -> Fragment {
    Fragment {
        node_body: "_".into(),
        predicates: vec![],
        outer_capture: None,
        pending_name_match: atom.name_match.clone(),
        params_field: None,
        params_container: None,
        params_needs_bridge: false,
        body_field: None,
        body_container: None,
    }
}

fn compile_passthrough(node_type: &str, atom: &Atom) -> Fragment {
    Fragment {
        node_body: node_type.to_string(),
        predicates: vec![],
        outer_capture: None,
        // Unknown / pass-through nodes: their text IS their name, same logic as self-name.
        pending_name_match: atom.name_match.clone(),
        params_field: None,
        params_container: None,
        params_needs_bridge: false,
        body_field: None,
        body_container: None,
    }
}

fn compile_variant(variant: &TsVariant, atom: &Atom) -> Fragment {
    let id = fresh_id();
    let mut node_body = variant.node.to_string();
    let mut predicates = Vec::new();
    let mut pending_name_match = None;

    if let Some(nm) = &atom.name_match {
        match variant.name_field {
            Some(name_field) => {
                // Name lives in a child field — use an internal capture on that child.
                let cap = format!("@_n{}", id);
                let pred = name_predicate(&cap, nm);
                let mut inner = format!("({}) {}", variant.name_node, cap);
                for &(outer_node, inner_field) in variant.name_path.iter().rev() {
                    inner = format!("({} {}: {})", outer_node, inner_field, inner);
                }
                node_body.push_str(&format!(" {}: {}", name_field, inner));
                predicates.push(pred);
            }
            None if variant.name_node == variant.node => {
                // The node itself IS the name (e.g. identifier). Defer predicate to resolve()
                // so we can use the outer_capture provided by an enclosing Group.
                pending_name_match = Some(nm.clone());
            }
            None => {
                // Node has no accessible name (e.g. arrow_function with name in parent).
                // Skip the name predicate — the variant still matches, just unfiltered.
            }
        }
    } else if let Some(constraint) = variant.always_constraint {
        node_body.push_str(constraint);
    }

    Fragment {
        node_body,
        predicates,
        outer_capture: None,
        pending_name_match,
        params_field: variant.params_field,
        params_container: variant.params_container,
        params_needs_bridge: variant.params_needs_bridge,
        body_field: variant.body_field,
        body_container: variant.body_container,
    }
}

fn apply_field_access(frags: Vec<Fragment>, fa: &FieldAccess, lang: &dyn Lang) -> Vec<Fragment> {
    let inner_raw = match &fa.inner {
        None => vec![Fragment {
            node_body: "_".into(),
            predicates: vec![],
            outer_capture: None,
            pending_name_match: None,
            params_field: None,
            params_container: None,
            params_needs_bridge: false,
            body_field: None,
            body_container: None,
        }],
        Some(inner_query) => compile_query(inner_query, lang),
    };

    if inner_raw.is_empty() {
        return frags;
    }

    // Resolve pending name matches in inner fragments before injecting.
    let inner_frags: Vec<Fragment> = inner_raw
        .into_iter()
        .map(|mut f| {
            f.resolve();
            f
        })
        .collect();

    frags
        .into_iter()
        .flat_map(|outer| {
            inner_frags.iter().filter_map(move |inner| {
                let mut out = outer.clone();
                match fa.field.as_str() {
                    "params" => {
                        if out.params_field.is_none() {
                            return None;
                        }
                        let ts_field = out.params_field.unwrap();
                        let container = out.params_container;
                        let needs_bridge = out.params_needs_bridge;
                        let inner_inline = inner.inline();
                        let field_value = if let Some(c) = container {
                            format!("({} {})", c, inner_inline)
                        } else {
                            inner_inline
                        };
                        if needs_bridge {
                            out.node_body.push_str(&format!(" (_ {}: {})", ts_field, field_value));
                        } else {
                            out.node_body.push_str(&format!(" {}: {}", ts_field, field_value));
                        }
                        out.predicates.extend(inner.predicates.clone());
                    }
                    "body" => {
                        if out.body_field.is_none() {
                            return None;
                        }
                        let ts_field = out.body_field.unwrap();
                        let container = out.body_container;
                        let inner_inline = inner.inline();
                        // Wrap with `_` bridge so the inner pattern matches at any
                        // depth inside the container (e.g. call_expression inside
                        // expression_statement inside compound_statement).
                        let bridged = format!("(_ {})", inner_inline);
                        let field_value = if let Some(c) = container {
                            format!("({} {})", c, bridged)
                        } else {
                            bridged
                        };
                        out.node_body.push_str(&format!(" {}: {}", ts_field, field_value));
                        out.predicates.extend(inner.predicates.clone());
                    }
                    other => {
                        out.inject_field(other, None, inner);
                    }
                }
                Some(out)
            })
        })
        .collect()
}

fn name_predicate(cap: &str, nm: &NameMatch) -> String {
    match nm {
        NameMatch::Exact(s) => format!("(#eq? {} \"{}\")", cap, s),
        NameMatch::Regex(s) => format!("(#match? {} \"{}\")", cap, s),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse;
    use lang::for_name;

    fn compile_str(query: &str, lang_name: &str) -> String {
        let q = parse(query).expect("parse failed");
        let lang = for_name(lang_name).expect("unknown lang");
        match compile(&q, lang.as_ref()) {
            CompiledQuery::Simple(s) => s,
            CompiledQuery::Scoped { outer, inner, .. } => format!("SCOPED outer={} inner={}", outer, inner),
        }
    }

    #[test]
    fn test_passthrough_arrow_function() {
        // arrow_function is an unknown abstract type → pass-through
        let out = compile_str("arrow_function", "javascript");
        assert!(out.starts_with("(arrow_function)"), "got: {}", out);
    }

    #[test]
    fn test_sizeof_call_c() {
        let out = compile_str("(&function#sizeof @f)", "c");
        assert!(out.contains("call_expression"), "got: {}", out);
        assert!(out.contains("#eq?"), "got: {}", out);
        assert!(out.contains("\"sizeof\""), "got: {}", out);
        assert!(out.contains("@f"), "got: {}", out);
    }

    #[test]
    fn test_main_def_js_has_multiple_variants() {
        let out = compile_str("(*function#main @f)", "javascript");
        assert!(
            out.starts_with('[') || out.contains("function_declaration"),
            "got: {}", out
        );
        assert!(out.contains("\"main\""), "got: {}", out);
        // arrow_function should be in the alternation but without a name predicate
        assert!(out.contains("arrow_function"), "got: {}", out);
    }

    #[test]
    fn test_malloc_params_c() {
        let out = compile_str("(&function#malloc @f).params((var#ARRAY_SIZE @v))", "c");
        assert!(out.contains("call_expression"), "got: {}", out);
        assert!(out.contains("\"malloc\""), "got: {}", out);
        assert!(out.contains("arguments"), "got: {}", out);
        assert!(out.contains("ARRAY_SIZE"), "got: {}", out);
        assert!(out.contains("@v"), "got: {}", out);
    }

    #[test]
    fn test_python_does_not_panic() {
        let out = compile_str("(&function#malloc @f).params((var#ARRAY_SIZE @v))", "python");
        let _ = out;
    }

    #[test]
    fn test_empty_on_known_missing() {
        // "class" has no mapping in C
        let out = compile_str("class", "c");
        assert!(out.is_empty(), "expected empty, got: {}", out);
    }

    #[test]
    fn test_regex_name_match() {
        let out = compile_str("(&function#/malloc|calloc/ @f)", "c");
        assert!(out.contains("#match?"), "got: {}", out);
        assert!(out.contains("malloc|calloc"), "got: {}", out);
    }

    #[test]
    fn test_main_def_python() {
        let out = compile_str("(*function#main @f)", "python");
        assert!(out.contains("function_definition"), "got: {}", out);
        assert!(out.contains("\"main\""), "got: {}", out);
    }

    #[test]
    fn test_body_combinator_cpp() {
        let q = parse("(*function @f).body((&function#Loading @c))").expect("parse failed");
        let lang = for_name("cpp").expect("unknown lang");
        let compiled = compile(&q, lang.as_ref());
        match compiled {
            CompiledQuery::Scoped { outer, scope_field, inner } => {
                assert_eq!(scope_field, "body");
                assert!(outer.contains("function_definition"), "outer: {}", outer);
                assert!(inner.contains("call_expression"), "inner: {}", inner);
                assert!(inner.contains("Loading"), "inner: {}", inner);
            }
            CompiledQuery::Simple(s) => panic!("expected Scoped, got Simple: {}", s),
        }
    }

    #[test]
    fn test_control_flow_types() {
        assert!(compile_str("if", "c").contains("if_statement"));
        assert!(compile_str("if", "rust").contains("if_expression"));
        assert!(compile_str("for", "cpp").contains("for_statement"));
        assert!(compile_str("for", "rust").contains("for_expression"));
        assert!(compile_str("while", "cpp").contains("while_statement"));
        assert!(compile_str("while", "go").is_empty(), "Go has no while");
        assert!(compile_str("switch", "javascript").contains("switch_statement"));
        assert!(compile_str("switch", "rust").contains("match_expression"));
        assert!(compile_str("switch", "python").is_empty(), "Python has no switch");
        assert!(compile_str("switch", "go").contains("expression_switch_statement"));
        assert!(compile_str("switch", "go").contains("type_switch_statement"));
        // call is now a known-absent no-op in all languages
        assert!(compile_str("call", "c").is_empty());
        assert!(compile_str("call", "javascript").is_empty());
    }
}
