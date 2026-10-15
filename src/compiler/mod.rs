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
    /// When set, wrap node_body in a parent pattern at emit time to filter by name via parent.
    pending_parent_wrap: Option<(crate::compiler::lang::ParentNameWrap, NameMatch)>,
    // Field routing — set from whichever TsVariant produced this fragment
    params_field: Option<&'static str>,
    params_container: Option<&'static str>,
    params_needs_bridge: bool,
    body_field: Option<&'static str>,
    body_container: Option<&'static str>,
    /// Also capture the emitted node as `@_root`, so a scoped runner can find the
    /// head node even when the user's own captures sit elsewhere in the pattern.
    extra_root: bool,
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

        if let Some((wrap, nm)) = self.pending_parent_wrap {
            // Emit: ((parent name_field: (name_node) @_n self_field: (inner) @_root) predicate)
            let id = fresh_id();
            let name_cap = format!("@_n{}", id);
            let pred = name_predicate(&name_cap, &nm);
            let cap = self.outer_capture.get_or_insert_with(|| format!("_root{}", fresh_id()));
            let cap = if self.extra_root { format!("{} @_root", cap) } else { cap.clone() };
            let inner_name = format!("({}) {}", wrap.name_node, name_cap);
            let wrapped = format!(
                "({} {}: {} {}: {} @{})",
                wrap.parent, wrap.name_field, inner_name,
                wrap.self_field, node, cap,
            );
            return format!("({} {})", wrapped, pred);
        }

        let cap = self.outer_capture.get_or_insert_with(|| format!("_root{}", fresh_id()));
        let with_cap = if self.extra_root {
            format!("{} @{} @_root", node, cap)
        } else {
            format!("{} @{}", node, cap)
        };
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
    if let Some((outer, scope_field, inner)) = try_compile_descendant(query, lang) {
        return CompiledQuery::Scoped { outer, scope_field, inner };
    }
    let frags = compile_query(query, lang);
    CompiledQuery::Simple(emit_alternation(frags))
}

/// Compile a top-level `head tail...` descendant as a two-phase scoped query so
/// the tail may sit at any depth under the head, not just one grammar node down.
/// Only the first top-level descendant gets this treatment. The rest of the query
/// becomes the inner query, where later descendants keep the one-level `(_ ...)`
/// bridge. The scope field is empty, which the runner reads as "the whole node".
fn try_compile_descendant(query: &Query, lang: &dyn Lang) -> Option<(String, String, String)> {
    let (first, rest) = query.tail.split_first()?;
    if first.0 != Combinator::Descendant {
        return None;
    }
    let outer_frags: Vec<Fragment> = compile_selector(&query.head, lang)
        .into_iter()
        .map(|mut f| {
            f.extra_root = true;
            f
        })
        .collect();
    let outer = emit_alternation(outer_frags);
    if outer.is_empty() {
        return None;
    }
    let inner_query = Query { head: first.1.clone(), tail: rest.to_vec() };
    let inner = emit_alternation(compile_query(&inner_query, lang));
    if inner.is_empty() {
        return None;
    }
    Some((outer, String::new(), inner))
}

/// The abstract kinds this language resolves, in vocabulary order. Used for the
/// "valid kinds" list in the unknown-kind error and by nothing else.
pub fn known_kinds(lang: &dyn Lang) -> Vec<&'static str> {
    crate::types::ABSTRACT_TYPES
        .iter()
        .copied()
        .filter(|t| lang.resolve(t, None).is_some())
        .collect()
}

/// Reject a node type that is neither an abstract kind this language knows nor a
/// real tree-sitter node type in it. Without this the query compiles to a
/// nonsense pattern and tree-sitter reports "Invalid node type" from deep inside
/// the runner, which says nothing about what would have worked.
pub fn validate(query: &Query, lang: &dyn Lang, lang_name: &str) -> Result<(), String> {
    if has_semantic(query) {
        return Err(format!(
            "{}call and data-flow combinators and :not() are not available here, use: ctsq path '<query>'",
            SEMANTIC_QUERY_PREFIX
        ));
    }
    let mut kinds = Vec::new();
    collect_node_types(query, &mut kinds);

    let ts_lang = lang.ts_language();
    for kind in kinds {
        if lang.resolve(&kind, None).is_some() {
            continue;
        }
        // Unknown identifiers pass through as concrete tree-sitter node types.
        if ts_lang.id_for_node_kind(&kind, true) != 0 || ts_lang.id_for_node_kind(&kind, false) != 0
        {
            continue;
        }
        return Err(format!(
            "unknown kind '{}' for {}, valid kinds: {}",
            kind,
            lang_name,
            known_kinds(lang).join(", ")
        ));
    }
    Ok(())
}

/// Prefix of the error `validate` returns for a semantic query. Callers that
/// need to treat it differently (exit 2 plus a hint) test with
/// `is_semantic_query_error`.
pub const SEMANTIC_QUERY_PREFIX: &str = "semantic query: ";

#[allow(dead_code)]
pub fn is_semantic_query_error(err: &str) -> bool {
    err.starts_with(SEMANTIC_QUERY_PREFIX)
}

/// True when a semantic combinator or `:not(...)` appears anywhere in the query,
/// including inside groups and field inner queries.
pub fn has_semantic(query: &Query) -> bool {
    selector_has_semantic(&query.head)
        || query
            .tail
            .iter()
            .any(|(c, sel)| c.is_semantic() || selector_has_semantic(sel))
}

fn selector_has_semantic(sel: &Selector) -> bool {
    if !sel.not.is_empty() {
        return true;
    }
    if let SelectorNode::Group { query, .. } = &sel.node {
        if has_semantic(query) {
            return true;
        }
    }
    sel.fields
        .iter()
        .any(|fa| fa.inner.as_ref().is_some_and(|q| has_semantic(q)))
}

fn collect_node_types(query: &Query, out: &mut Vec<String>) {
    collect_selector_node_types(&query.head, out);
    for (_, sel) in &query.tail {
        collect_selector_node_types(sel, out);
    }
}

fn collect_selector_node_types(sel: &Selector, out: &mut Vec<String>) {
    match &sel.node {
        SelectorNode::Bare(atom) => {
            if let Some(t) = &atom.node_type {
                out.push(t.clone());
            }
        }
        SelectorNode::Group { query, .. } => collect_node_types(query, out),
    }
    for fa in &sel.fields {
        if let Some(inner) = &fa.inner {
            collect_node_types(inner, out);
        }
    }
}

/// If the query has a head-selector with exactly one field access that has an inner
/// query and no tail combinators, compile it as a two-phase scoped query.
/// The scope_field is the field name used to navigate from each outer match to the
/// subtree that the inner query searches within.
fn try_compile_scoped(query: &Query, lang: &dyn Lang) -> Option<(String, String, String)> {
    if !query.tail.is_empty() {
        return None;
    }

    // With two or more constrained fields, scoping on one would silently drop
    // the others — let the Simple path inline them all conjunctively.
    if query.head.fields.iter().filter(|fa| fa.inner.is_some() && fa.field != "params").count() > 1 {
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
        not: query.head.not.clone(),
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
            // Semantic combinators never reach here: `validate` rejects them. A caller
            // that skips validation gets each side emitted independently.
            _ => {
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
                // A named group overrides the fragment's capture; a bare group
                // must not clobber the internal capture resolved above.
                if capture.is_some() {
                    frag.outer_capture = capture.clone();
                }
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
        pending_parent_wrap: None,
        params_field: None,
        params_container: None,
        params_needs_bridge: false,
        body_field: None,
        body_container: None,
        extra_root: false,
    }
}

fn compile_passthrough(node_type: &str, atom: &Atom) -> Fragment {
    Fragment {
        node_body: node_type.to_string(),
        predicates: vec![],
        outer_capture: None,
        // Unknown / pass-through nodes: their text IS their name, same logic as self-name.
        pending_name_match: atom.name_match.clone(),
        pending_parent_wrap: None,
        params_field: None,
        params_container: None,
        params_needs_bridge: false,
        body_field: None,
        body_container: None,
        extra_root: false,
    }
}

fn compile_variant(variant: &TsVariant, atom: &Atom) -> Fragment {
    let id = fresh_id();
    let mut node_body = variant.node.to_string();
    let mut predicates = Vec::new();
    let mut pending_name_match = None;
    let mut pending_parent_wrap = None;

    if let Some(nm) = &atom.name_match {
        match variant.name_field {
            Some(name_field) => {
                // Name lives in a child field — use an internal capture on that child.
                // An empty field name means a positional child instead, which is how
                // a JS/TS declaration holds its declarator.
                let cap = format!("@_n{}", id);
                let pred = name_predicate(&cap, nm);
                let mut inner = format!("({}) {}", variant.name_node, cap);
                for &(outer_node, inner_field) in variant.name_path.iter().rev() {
                    inner = if inner_field.is_empty() {
                        format!("({} {})", outer_node, inner)
                    } else {
                        format!("({} {}: {})", outer_node, inner_field, inner)
                    };
                }
                if name_field.is_empty() {
                    node_body.push_str(&format!(" {}", inner));
                } else {
                    node_body.push_str(&format!(" {}: {}", name_field, inner));
                }
                predicates.push(pred);
            }
            None if variant.name_node == variant.node => {
                // The node itself IS the name (e.g. identifier). Defer predicate to resolve()
                // so we can use the outer_capture provided by an enclosing Group.
                pending_name_match = Some(nm.clone());
            }
            None => {
                // Name lives in parent (e.g. arrow_function inside variable_declarator).
                if let Some(wrap) = variant.parent_name_wrap {
                    pending_parent_wrap = Some((wrap, nm.clone()));
                }
                // else: no way to filter by name — variant matches unfiltered.
            }
        }
    } else if let Some(value) = variant.always_callee_eq {
        // Force a callee-name predicate with no user name filter (e.g. require(...)).
        let cap = format!("@_c{}", id);
        node_body.push_str(&format!(" {}: ({}) {}",
            variant.name_field.unwrap_or("function"), variant.name_node, cap));
        predicates.push(format!("(#eq? {} \"{}\")", cap, value));
    } else if let Some(constraint) = variant.always_constraint {
        node_body.push_str(constraint);
    }

    Fragment {
        node_body,
        predicates,
        outer_capture: None,
        pending_name_match,
        pending_parent_wrap,
        params_field: variant.params_field,
        params_container: variant.params_container,
        params_needs_bridge: variant.params_needs_bridge,
        body_field: variant.body_field,
        body_container: variant.body_container,
        extra_root: false,
    }
}

fn apply_field_access(frags: Vec<Fragment>, fa: &FieldAccess, lang: &dyn Lang) -> Vec<Fragment> {
    let inner_raw = match &fa.inner {
        None => vec![Fragment {
            node_body: "_".into(),
            predicates: vec![],
            outer_capture: None,
            pending_name_match: None,
            pending_parent_wrap: None,
            params_field: None,
            params_container: None,
            params_needs_bridge: false,
            body_field: None,
            body_container: None,
            extra_root: false,
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
    fn test_captureless_group_keeps_name_capture() {
        // (identifier#size) without @cap must still emit the internal capture
        // its #eq? predicate references.
        let out = compile_str("(identifier#size)", "javascript");
        assert!(out.contains("(identifier) @_nm"), "got: {}", out);
    }

    #[test]
    fn test_multiple_inner_fields_all_constrained() {
        // Two constrained fields must both survive (Simple path, inlined),
        // not be silently dropped by the scoped two-phase path.
        let out = compile_str("binary_expression.left(identifier#x).right(identifier#y)", "javascript");
        assert!(!out.starts_with("SCOPED"), "got: {}", out);
        assert!(out.contains("\"x\""), "got: {}", out);
        assert!(out.contains("\"y\""), "got: {}", out);
    }

    #[test]
    fn test_python_method_call_matches_attribute() {
        // &function#send_email must also match `u.send_email()` (attribute callee)
        let out = compile_str("(&function#send_email @c)", "python");
        assert!(out.contains("attribute"), "got: {}", out);
        assert!(out.contains("\"send_email\""), "got: {}", out);
    }

    #[test]
    fn test_main_def_python() {
        let out = compile_str("(*function#main @f)", "python");
        assert!(out.contains("function_definition"), "got: {}", out);
        assert!(out.contains("\"main\""), "got: {}", out);
    }

    #[test]
    fn test_main_def_ruby() {
        let out = compile_str("(*function#main @f)", "ruby");
        assert!(out.contains("method"), "got: {}", out);
        assert!(out.contains("singleton_method"), "def self.main counts too: {}", out);
        assert!(out.contains("\"main\""), "got: {}", out);
    }

    #[test]
    fn test_ruby_call_matches_receiver_and_receiverless() {
        // One `call` variant covers `send_email(x)` and `u.send_email(x)` alike,
        // because Ruby hangs both names off the same `method` field.
        let out = compile_str("(&function#send_email @c)", "ruby");
        assert!(out.contains("(call method:"), "got: {}", out);
        assert!(out.contains("\"send_email\""), "got: {}", out);
    }

    #[test]
    fn test_ruby_import_matches_both_require_forms() {
        let out = compile_str("import", "ruby");
        assert!(out.contains("\"require\""), "got: {}", out);
        assert!(out.contains("\"require_relative\""), "got: {}", out);
    }

    #[test]
    fn test_ruby_class_and_module() {
        let cls = compile_str("class#Foo", "ruby");
        assert!(cls.contains("(class name: (constant)"), "got: {}", cls);
        assert!(cls.contains("\"Foo\""), "got: {}", cls);
        let module = compile_str("module#Foo", "ruby");
        assert!(module.contains("(module name: (constant)"), "got: {}", module);
        assert!(module.contains("\"Foo\""), "got: {}", module);
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
    fn def_type_reaches_the_declaration_not_the_identifier() {
        // `*type#X` used to compile to (type_identifier), so --body printed the
        // name and nothing else.
        let go = compile_str("(*type#Meta @t)", "go");
        assert!(go.contains("type_spec"), "go: {}", go);
        assert!(!go.contains("(type_identifier) @t"), "go still matches the bare name: {}", go);

        let ts = compile_str("(*type#Meta @t)", "typescript");
        assert!(ts.contains("type_alias_declaration"), "ts: {}", ts);
        assert!(ts.contains("interface_declaration"), "ts interfaces: {}", ts);

        let rs = compile_str("(*type#Meta @t)", "rust");
        assert!(rs.contains("struct_item"), "rust: {}", rs);
        assert!(rs.contains("type_item"), "rust alias: {}", rs);

        let c = compile_str("(*type#Meta @t)", "c");
        assert!(c.contains("struct_specifier"), "c: {}", c);
        assert!(c.contains("type_definition"), "c typedef: {}", c);

        let cpp = compile_str("(*type#Meta @t)", "cpp");
        assert!(cpp.contains("class_specifier"), "cpp: {}", cpp);
    }

    #[test]
    fn ref_type_still_matches_the_identifier() {
        for lang in ["go", "typescript", "rust", "c", "cpp"] {
            let out = compile_str("&type#Meta", lang);
            assert!(out.contains("type_identifier"), "{}: {}", lang, out);
            assert!(!out.contains("type_spec"), "{}: {}", lang, out);
        }
    }

    #[test]
    fn bare_type_matches_declaration_and_identifier_both() {
        let out = compile_str("type#Meta", "go");
        assert!(out.contains("type_spec"), "{}", out);
        assert!(out.contains("type_identifier"), "{}", out);
    }

    /// Capture names carry a global counter, so two compilations of the same
    /// query differ only in their digits. Strip them to compare shapes.
    fn shape(s: &str) -> String {
        s.chars().filter(|c| !c.is_ascii_digit()).collect()
    }

    #[test]
    fn def_variable_reaches_the_declaration_not_the_identifier() {
        // `*variable#X` used to hit no mapping at all and pass through as a
        // node type called "variable", which tree-sitter rejects.
        for lang in ["typescript", "tsx", "javascript"] {
            let out = compile_str("(*variable#config @v)", lang);
            assert!(out.contains("lexical_declaration"), "{}: {}", lang, out);
            assert!(out.contains("variable_declaration"), "{} var form: {}", lang, out);
            assert!(out.contains("variable_declarator name: (identifier)"), "{}: {}", lang, out);
            assert!(!out.contains("(identifier) @v"), "{} still matches the bare name: {}", lang, out);
        }
    }

    #[test]
    fn ref_variable_stays_on_the_identifier() {
        for lang in ["typescript", "javascript"] {
            let out = compile_str("&variable#config", lang);
            assert!(out.contains("identifier"), "{}: {}", lang, out);
            assert!(!out.contains("lexical_declaration"), "{}: {}", lang, out);
        }
    }

    #[test]
    fn bare_variable_matches_declaration_and_uses_both() {
        let out = compile_str("variable#config", "typescript");
        assert!(out.contains("lexical_declaration"), "{}", out);
        assert!(out.contains("(identifier)"), "{}", out);
    }

    #[test]
    fn const_is_an_alias_of_variable() {
        for lang in ["typescript", "javascript", "go", "rust"] {
            for sigil in ["*", "&", ""] {
                let v = compile_str(&format!("{}variable#config", sigil), lang);
                let c = compile_str(&format!("{}const#config", sigil), lang);
                assert_eq!(shape(&v), shape(&c), "{} {}variable: {} vs {}", lang, sigil, v, c);
            }
        }
    }

    #[test]
    fn go_and_rust_variables_reach_their_declaration_statements() {
        let go = compile_str("*variable#Timeout", "go");
        assert!(go.contains("var_declaration"), "{}", go);
        assert!(go.contains("const_declaration"), "{}", go);
        assert!(go.contains("short_var_declaration left: (expression_list (identifier)"), "{}", go);

        let rs = compile_str("*variable#MAX", "rust");
        assert!(rs.contains("let_declaration"), "{}", rs);
        assert!(rs.contains("const_item"), "{}", rs);
        assert!(rs.contains("static_item"), "{}", rs);
    }

    #[test]
    fn an_unknown_kind_is_rejected_with_the_valid_ones() {
        let q = parse("*bogus#X").unwrap();
        let lang = for_name("typescript").unwrap();
        let err = validate(&q, lang.as_ref(), "typescript").unwrap_err();
        assert!(err.starts_with("unknown kind 'bogus' for typescript, valid kinds: "), "{}", err);
        assert!(err.contains("variable"), "the new kinds are listed: {}", err);
        assert!(err.contains("id, function"), "vocabulary order: {}", err);
    }

    #[test]
    fn a_real_tree_sitter_node_type_still_passes_validation() {
        let lang = for_name("typescript").unwrap();
        for q in ["call_expression", "arrow_function", "id#foo", "*variable#foo"] {
            let parsed = parse(q).unwrap();
            assert!(validate(&parsed, lang.as_ref(), "typescript").is_ok(), "{}", q);
        }
    }

    #[test]
    fn validation_reaches_inside_groups_and_field_access() {
        let lang = for_name("cpp").unwrap();
        let q = parse("(*function @f).body(bogus#X)").unwrap();
        assert!(validate(&q, lang.as_ref(), "cpp").is_err(), "inner query is checked too");
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
        assert!(compile_str("switch", "ruby").contains("case"), "Ruby has case");
        assert!(compile_str("while", "ruby").contains("until"), "Ruby has until");
        assert!(compile_str("switch", "go").contains("expression_switch_statement"));
        assert!(compile_str("switch", "go").contains("type_switch_statement"));
        // call is now a known-absent no-op in all languages
        assert!(compile_str("call", "c").is_empty());
        assert!(compile_str("call", "javascript").is_empty());
    }

    #[test]
    fn a_top_level_descendant_compiles_to_an_empty_field_scope() {
        let q = parse("(*function#handle @f) variable#name").unwrap();
        let lang = for_name("go").unwrap();
        match compile(&q, lang.as_ref()) {
            CompiledQuery::Scoped { outer, scope_field, inner } => {
                assert_eq!(scope_field, "");
                assert!(outer.contains("@f @_root"), "explicit @_root beside the user capture: {}", outer);
                assert!(inner.contains("\"name\""), "{}", inner);
            }
            other => panic!("expected Scoped, got {:?}", other),
        }
    }

    #[test]
    fn nested_and_later_descendants_keep_the_one_level_bridge() {
        let lang = for_name("go").unwrap();
        // Inside a group: stays Simple with the (_ ...) bridge.
        let q = parse("(function#a variable#b)").unwrap();
        match compile(&q, lang.as_ref()) {
            CompiledQuery::Simple(s) => assert!(s.contains("(_ "), "{}", s),
            other => panic!("expected Simple, got {:?}", other),
        }
        // A second descendant stays in the inner query as a bridge.
        let q = parse("function#a block variable#b").unwrap();
        match compile(&q, lang.as_ref()) {
            CompiledQuery::Scoped { inner, .. } => assert!(inner.contains("(_ "), "{}", inner),
            other => panic!("expected Scoped, got {:?}", other),
        }
        // Child combinator is untouched.
        let q = parse("function#a > variable#b").unwrap();
        assert!(matches!(compile(&q, lang.as_ref()), CompiledQuery::Simple(_)));
    }

    #[test]
    fn validate_rejects_semantic_queries_anywhere() {
        let lang = for_name("go").unwrap();
        for q in [
            "function#a --> function#b",
            "(function#a --> function#b)",
            "function#a -> function#b",
            "variable#a ==> function#b",
            "(*function @f).body(function#a <- function#b)",
            "function:not(function#a)",
            "(function:not(variable#a) @f)",
        ] {
            let parsed = parse(q).unwrap_or_else(|e| panic!("{}: {}", q, e));
            let err = validate(&parsed, lang.as_ref(), "go").unwrap_err();
            assert!(is_semantic_query_error(&err), "{}: {}", q, err);
        }
    }

    #[test]
    fn validate_accepts_attributes_and_dot_shorthand() {
        let lang = for_name("go").unwrap();
        for q in ["function#log.Printf", "function[from=\"log/slog\"]", "*function#x[in=Server]"] {
            let parsed = parse(q).unwrap();
            assert!(validate(&parsed, lang.as_ref(), "go").is_ok(), "{}", q);
        }
    }

    #[test]
    fn attrs_do_not_change_compiled_output() {
        let with = compile_str("&function#Printf[from=fmt]", "go");
        let without = compile_str("&function#Printf", "go");
        assert_eq!(shape(&with), shape(&without));
    }
}
