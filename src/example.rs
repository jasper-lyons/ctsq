//! `example` subcommand — query-by-example. Parses a snippet of real code and
//! derives the ctsq queries that would find it, by inverting the same
//! `Lang::resolve` mapping tables the compiler uses to go forwards. Candidates
//! are self-verified: each is compiled and run against the snippet itself, and
//! only queries that actually match it are printed (use --debug to see the rest).

use crate::compiler::lang::{self, Lang, TsVariant};
use crate::compiler::{self, CompiledQuery};
use crate::query::ast::Sigil;
use crate::types::ABSTRACT_TYPES;
use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::ops::Range;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor, Tree};

pub fn run(snippet_arg: &str, lang_flag: Option<&str>, debug: bool) -> i32 {
    let Some(lang_name) = lang_flag else {
        eprintln!("ctsq example: --lang is required (a snippet has no file extension to infer from)");
        return 1;
    };
    let Some(lang) = lang::for_name(lang_name) else {
        eprintln!("unknown language: {} (known: c, cpp, javascript, typescript, tsx, python, rust, go)", lang_name);
        return 1;
    };
    let snippet = match read_snippet(snippet_arg) {
        Ok(s) => s,
        Err(e) => { eprintln!("ctsq example: {}", e); return 1; }
    };
    if snippet.is_empty() {
        eprintln!("ctsq example: empty snippet");
        return 1;
    }

    let groups = match derive(&snippet, lang.as_ref(), lang_name, debug) {
        Ok(g) => g,
        Err(e) => { eprintln!("ctsq example: {}", e); return 1; }
    };

    let mut any_verified = false;
    let multi = groups.len() > 1;
    for (i, group) in groups.iter().enumerate() {
        let shown: Vec<&Candidate> = group.candidates.iter()
            .filter(|c| c.verified || debug)
            .collect();
        if shown.is_empty() { continue; }
        if multi {
            if i > 0 { println!(); }
            println!("# {}", group.heading);
        }
        let width = shown.iter().map(|c| c.query.len()).max().unwrap_or(0);
        for c in &shown {
            let mark = if c.verified { "" } else { "(!) " };
            println!("{:<width$}  # {}{}", c.query, mark, c.label, width = width);
            any_verified |= c.verified;
        }
    }

    if any_verified {
        0
    } else {
        eprintln!("ctsq example: no verified candidates for this snippet{}",
            if debug { "" } else { " (re-run with --debug to see rejected ones)" });
        1
    }
}

struct Candidate {
    query: String,
    label: String,
    verified: bool,
}

struct Group {
    heading: String,
    candidates: Vec<Candidate>,
}

fn derive(snippet: &str, lang: &dyn Lang, lang_name: &str, debug: bool) -> Result<Vec<Group>, String> {
    let canon = lang::lang_for_name_canonical(lang_name).unwrap_or(lang_name);
    let (snippet, placeholders) = substitute_placeholders(snippet);
    let parsed = parse_snippet(lang, canon, &snippet)?;
    if debug && parsed.harnessed {
        eprintln!("debug: snippet wrapped in a {} harness to parse", canon);
    }
    let ts_lang = lang.ts_language();
    let index = build_index(lang);

    let focuses = focus_nodes(&parsed.tree, &parsed.src, &parsed.range);
    let mut groups = Vec::new();
    for focus in focuses {
        if focus.is_error() {
            return Err(format!("could not parse snippet as {}", canon));
        }
        let (cnode, cls) = classify_with_descent(focus, &parsed.src, &index);
        if debug {
            eprintln!("debug: focus node {} (classified via {})", focus.kind(), cnode.kind());
        }

        // Most-literal rung first: the structural query pinning every unmarked
        // named part of the snippet. Leaves are already covered by `id#name`.
        let mut rungs: Vec<(String, String)> = Vec::new();
        if focus.named_child_count() > 0 {
            if let Some(e) = emit_structural(focus, &parsed.src, 0) {
                if e.informative || !placeholders.is_empty() {
                    if e.query.len() <= STRUCTURAL_MAX_LEN {
                        rungs.push((e.query, structural_label(&placeholders)));
                    } else if debug {
                        eprintln!("debug: structural candidate too long ({} chars), skipped", e.query.len());
                    }
                }
            }
        }
        rungs.extend(match cls.first() {
            Some(best) => ladder(best),
            None => fallback_rungs(focus, &parsed.src),
        });

        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        let construct_range = focus.start_byte()..focus.end_byte();
        for (query, label) in rungs {
            if query.contains(SENTINEL_PREFIX) { continue; }
            let Some(compiled) = compile_query_str(&query, lang) else {
                if debug { eprintln!("debug: {} compiled to nothing, dropped", query); }
                continue;
            };
            if !seen.insert(compiled_key(&compiled)) { continue; }
            let verified = matches_snippet(&compiled, &ts_lang, &parsed.tree, &parsed.src, &construct_range);
            candidates.push(Candidate { query, label, verified });
        }
        let mut heading = focus.utf8_text(&parsed.src).unwrap_or("")
            .lines().next().unwrap_or("").to_string();
        for p in &placeholders {
            heading = heading.replace(&format!("{}{}", SENTINEL_PREFIX, p), &format!("${}", p));
        }
        groups.push(Group { heading, candidates });
    }
    Ok(groups)
}

// ---------------------------------------------------------------------------
// Snippet input & parsing
// ---------------------------------------------------------------------------

fn read_snippet(arg: &str) -> Result<String, String> {
    let raw = if arg == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| e.to_string())?;
        buf
    } else {
        arg.to_string()
    };
    Ok(dedent(&raw).trim().to_string())
}

/// Placeholders (`$X`, `$RECV`, …) are substituted with sentinel identifiers so
/// the snippet parses in languages where `$` is illegal; any sentinel found
/// during query generation means "leave this position unconstrained".
const SENTINEL_PREFIX: &str = "__ctsq_";

fn is_sentinel(name: &str) -> bool {
    name.starts_with(SENTINEL_PREFIX)
}

/// Replace `$UPPERCASE` placeholders outside string literals with sentinel
/// identifiers. Returns the substituted snippet and the placeholder names.
fn substitute_placeholders(snippet: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(snippet.len());
    let mut names: Vec<String> = Vec::new();
    let mut quote: Option<char> = None;
    let mut chars = snippet.char_indices();
    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some((_, escaped)) = chars.next() { out.push(escaped); }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => { quote = Some(c); out.push(c); }
            '$' => {
                let rest = &snippet[i + 1..];
                let end = rest
                    .find(|ch: char| !(ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_'))
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                if name.chars().next().map_or(false, |ch| ch.is_ascii_uppercase()) {
                    out.push_str(SENTINEL_PREFIX);
                    out.push_str(name);
                    if !names.iter().any(|n| n == name) { names.push(name.to_string()); }
                    for _ in 0..name.len() { chars.next(); }
                } else {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    (out, names)
}

/// Strip the common leading indentation, so snippets copied from inside a
/// function body (indentation-sensitive in Python) parse at top level.
fn dedent(s: &str) -> String {
    let indent = s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    s.lines()
        .map(|l| if l.len() >= indent { &l[indent..] } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

/// (prefix, suffix) wrappers to retry with when a bare snippet doesn't parse —
/// C-family, Rust, and Go reject bare expressions/statements at top level.
fn harnesses(canon: &str) -> &'static [(&'static str, &'static str)] {
    match canon {
        "c" | "cpp" => &[("", ""), ("void __ctsq(void) {\n", "\n;\n}\n")],
        "rust" => &[("", ""), ("fn __ctsq() {\n", "\n;\n}\n")],
        "go" => &[
            ("", ""),
            ("package __ctsq\n", ""),
            ("package __ctsq\nfunc __ctsq() {\n", "\n}\n"),
        ],
        _ => &[("", "")],
    }
}

struct Parsed {
    tree: Tree,
    src: Vec<u8>,
    /// Byte range of the snippet within `src` (identical when no harness used).
    range: Range<usize>,
    harnessed: bool,
}

fn parse_snippet(lang: &dyn Lang, canon: &str, snippet: &str) -> Result<Parsed, String> {
    let ts_lang = lang.ts_language();
    let mut parser = Parser::new();
    parser.set_language(&ts_lang).map_err(|e| e.to_string())?;

    let mut first: Option<Parsed> = None;
    for (prefix, suffix) in harnesses(canon) {
        let src = format!("{}{}{}", prefix, snippet, suffix).into_bytes();
        let Some(tree) = parser.parse(&src, None) else { continue };
        let parsed = Parsed {
            tree,
            src,
            range: prefix.len()..prefix.len() + snippet.len(),
            harnessed: !prefix.is_empty(),
        };
        if !parsed.tree.root_node().has_error() {
            return Ok(parsed);
        }
        if first.is_none() { first = Some(parsed); }
    }
    first.ok_or_else(|| "failed to parse snippet".to_string())
}

// ---------------------------------------------------------------------------
// Focus node
// ---------------------------------------------------------------------------

/// Transparent containers between the parse root and the construct the user
/// actually wrote: file roots, statement wrappers, and harness function bodies.
const WRAPPERS: &[&str] = &[
    "program", "module", "source_file", "translation_unit",
    "expression_statement", "parenthesized_expression",
    "block", "statement_block", "compound_statement",
];

fn focus_nodes<'t>(tree: &'t Tree, src: &[u8], range: &Range<usize>) -> Vec<Node<'t>> {
    let mut start = range.start;
    let mut end = range.end.min(src.len());
    while start < end && (src[start] as char).is_whitespace() { start += 1; }
    while end > start && (src[end - 1] as char).is_whitespace() { end -= 1; }
    let node = tree.root_node()
        .named_descendant_for_byte_range(start, end)
        .unwrap_or_else(|| tree.root_node());
    unwrap_construct(node)
}

fn unwrap_construct(mut node: Node) -> Vec<Node> {
    loop {
        if !WRAPPERS.contains(&node.kind()) {
            return vec![node];
        }
        let mut cursor = node.walk();
        let named: Vec<Node> = node.named_children(&mut cursor)
            .filter(|c| c.kind() != "comment")
            .collect();
        match named.len() {
            0 => return vec![node],
            1 => node = named[0],
            _ => return named.into_iter().flat_map(unwrap_construct).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Inversion: concrete node kind → abstract type + sigil + name
// ---------------------------------------------------------------------------

struct InvEntry {
    atype: &'static str,
    /// None = the variant appears under both sigils (sigil doesn't discriminate).
    sigil: Option<Sigil>,
    variant: TsVariant,
    priority: usize,
}

type ShapeKey = (
    &'static str, Option<&'static str>, &'static str,
    &'static [(&'static str, &'static str)],
    Option<&'static str>, Option<&'static str>,
);

fn shape_key(v: &TsVariant) -> ShapeKey {
    (v.node, v.name_field, v.name_node, v.name_path, v.always_callee_eq, v.always_constraint)
}

fn build_index(lang: &dyn Lang) -> Vec<InvEntry> {
    let mut entries = Vec::new();
    for (priority, &atype) in ABSTRACT_TYPES.iter().enumerate() {
        let defs = lang.resolve(atype, Some(&Sigil::Def)).unwrap_or_default();
        let refs = lang.resolve(atype, Some(&Sigil::Ref)).unwrap_or_default();
        let ref_keys: Vec<ShapeKey> = refs.iter().map(shape_key).collect();
        let def_keys: Vec<ShapeKey> = defs.iter().map(shape_key).collect();
        for v in &defs {
            let sigil = if ref_keys.contains(&shape_key(v)) { None } else { Some(Sigil::Def) };
            entries.push(InvEntry { atype, sigil, variant: v.clone(), priority });
        }
        for v in refs {
            if def_keys.contains(&shape_key(&v)) { continue; }
            entries.push(InvEntry { atype, sigil: Some(Sigil::Ref), variant: v, priority });
        }
    }
    entries
}

#[derive(Clone)]
struct Classification {
    atype: &'static str,
    sigil: Option<Sigil>,
    name: Option<String>,
    priority: usize,
}

fn classify(node: Node, src: &[u8], index: &[InvEntry]) -> Vec<Classification> {
    let mut out: Vec<Classification> = Vec::new();
    for entry in index {
        let v = &entry.variant;
        if v.node != node.kind() { continue; }
        // Fixed-callee variants (e.g. `require(...)` under `import`) only apply
        // when the callee text matches — otherwise every call would classify
        // as an import.
        if let Some(value) = v.always_callee_eq {
            match extract_name(node, v, src) {
                Some(n) if n == value => out.push(Classification {
                    atype: entry.atype, sigil: entry.sigil.clone(),
                    name: None, priority: entry.priority,
                }),
                _ => {}
            }
            continue;
        }
        if let Some(constraint) = v.always_constraint {
            if !constraint_holds(node, constraint) { continue; }
        }
        let name = extract_name(node, v, src).filter(|n| !is_sentinel(n));
        out.push(Classification {
            atype: entry.atype, sigil: entry.sigil.clone(), name, priority: entry.priority,
        });
    }
    out.sort_by_key(|c| (c.name.is_none(), c.priority));
    out.dedup_by(|a, b| a.atype == b.atype && a.sigil == b.sigil && a.name == b.name);
    out
}

/// Walk the TsVariant's name shape against a real node to pull out its name.
fn extract_name(node: Node, v: &TsVariant, src: &[u8]) -> Option<String> {
    if let Some(field) = v.name_field {
        // An empty field name means the name hangs off a positional child (the
        // declarator in a JS `const x = 1`); name_path says which child.
        let mut n = if field.is_empty() { node } else { node.child_by_field_name(field)? };
        for (i, &(outer, inner)) in v.name_path.iter().enumerate() {
            if i == 0 && field.is_empty() {
                n = named_child_of_kind(n, outer)?;
            }
            if n.kind() != outer { return None; }
            n = if inner.is_empty() { n.named_child(0)? } else { n.child_by_field_name(inner)? };
        }
        if n.kind() != v.name_node { return None; }
        return Some(n.utf8_text(src).ok()?.to_string());
    }
    if let Some(w) = v.parent_name_wrap {
        let parent = node.parent()?;
        if parent.kind() != w.parent { return None; }
        if parent.child_by_field_name(w.self_field)?.id() != node.id() { return None; }
        let name = parent.child_by_field_name(w.name_field)?;
        if name.kind() != w.name_node { return None; }
        return Some(name.utf8_text(src).ok()?.to_string());
    }
    // Self-named leaves (identifier, literals): the node's text is its name.
    if v.name_node == v.node && node.named_child_count() == 0 {
        return Some(node.utf8_text(src).ok()?.to_string());
    }
    None
}

fn named_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// always_constraint strings all have the form " field: (kind)".
fn constraint_holds(node: Node, constraint: &str) -> bool {
    let Some((field, rest)) = constraint.trim().split_once(':') else { return true };
    let kind = rest.trim().trim_start_matches('(').trim_end_matches(')');
    node.child_by_field_name(field.trim())
        .map(|ch| ch.kind() == kind)
        .unwrap_or(false)
}

/// Classify the focus node; when it maps to nothing, look for the most
/// meaningful construct among its descendants (e.g. the arrow_function inside
/// a `const f = () => {}` declaration). Leaves are skipped so identifiers and
/// literals can't hijack a structural snippet.
fn classify_with_descent<'t>(focus: Node<'t>, src: &[u8], index: &[InvEntry])
    -> (Node<'t>, Vec<Classification>)
{
    let cls = classify(focus, src, index);
    if let Some(top) = cls.first() {
        // A binding that holds a named construct — `const f = () => {}` — is
        // better described by the construct than by the binding. Only when the
        // two carry the same name, so `const x = foo()` stays a variable.
        if let Some((node, inner)) = best_descendant(focus, src, index) {
            if inner.priority < top.priority && inner.name.is_some() && inner.name == top.name {
                return (node, vec![inner]);
            }
        }
        return (focus, cls);
    }
    match best_descendant(focus, src, index) {
        Some((node, c)) => (node, vec![c]),
        None => (focus, vec![]),
    }
}

/// The most meaningful classified construct below `focus`, lowest priority
/// first. Leaves are skipped so identifiers and literals can't hijack a
/// structural snippet.
fn best_descendant<'t>(focus: Node<'t>, src: &[u8], index: &[InvEntry])
    -> Option<(Node<'t>, Classification)>
{
    let mut best: Option<(Node, Classification)> = None;
    let mut queue: VecDeque<(Node, usize)> = VecDeque::new();
    let mut cursor = focus.walk();
    queue.extend(focus.named_children(&mut cursor).map(|c| (c, 1)));
    while let Some((node, depth)) = queue.pop_front() {
        if node.named_child_count() > 0 {
            if let Some(c) = classify(node, src, index).into_iter().next() {
                let better = match &best {
                    None => true,
                    Some((_, b)) => (c.name.is_none(), c.priority) < (b.name.is_none(), b.priority),
                };
                if better { best = Some((node, c)); }
            }
        }
        if depth < 3 {
            let mut cursor = node.walk();
            queue.extend(node.named_children(&mut cursor).map(|c| (c, depth + 1)));
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Ladder: classification → candidate queries, specific → general
// ---------------------------------------------------------------------------

/// Types whose name-dropped rung matches essentially everything.
const NAME_ONLY_TYPES: &[&str] = &["id", "var", "param", "literal", "type"];

fn ladder(c: &Classification) -> Vec<(String, String)> {
    let mut rungs = Vec::new();
    let mut push = |sigil: &Option<Sigil>, name: Option<&str>| {
        if let Some(q) = render(sigil, c.atype, name) {
            rungs.push((q, label(sigil, c.atype, name)));
        }
    };
    match &c.name {
        Some(name) => {
            push(&c.sigil, Some(name));
            if c.sigil.is_some() {
                push(&None, Some(name));
            }
            if !NAME_ONLY_TYPES.contains(&c.atype) {
                push(&c.sigil, None);
            }
        }
        None => push(&c.sigil, None),
    }
    rungs
}

fn fallback_rungs(focus: Node, src: &[u8]) -> Vec<(String, String)> {
    let kind = focus.kind();
    let mut rungs = vec![(
        kind.to_string(),
        format!("raw {} nodes (tree-sitter passthrough)", kind),
    )];
    if let Some(name) = first_identifier_descendant(focus, src) {
        if let Some(nq) = render_name(&name) {
            rungs.push((
                format!("{} id{}", kind, nq),
                format!("{} containing the identifier {}", kind, name),
            ));
        }
    }
    rungs
}

fn first_identifier_descendant(node: Node, src: &[u8]) -> Option<String> {
    let mut queue: VecDeque<(Node, usize)> = VecDeque::new();
    let mut cursor = node.walk();
    queue.extend(node.named_children(&mut cursor).map(|c| (c, 1)));
    while let Some((n, depth)) = queue.pop_front() {
        if n.kind() == "identifier" {
            let text = n.utf8_text(src).ok()?;
            if !is_sentinel(text) {
                return Some(text.to_string());
            }
        }
        if depth < 3 {
            let mut cursor = n.walk();
            queue.extend(n.named_children(&mut cursor).map(|c| (c, depth + 1)));
        }
    }
    None
}

fn render(sigil: &Option<Sigil>, atype: &str, name: Option<&str>) -> Option<String> {
    let s = match sigil {
        Some(Sigil::Def) => "*",
        Some(Sigil::Ref) => "&",
        None => "",
    };
    match name {
        None => Some(format!("{}{}", s, atype)),
        Some(n) => Some(format!("{}{}{}", s, atype, render_name(n)?)),
    }
}

fn render_name(n: &str) -> Option<String> {
    if !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some(format!("#{}", n))
    } else if !n.contains('"') && !n.contains('\n') {
        Some(format!("#\"{}\"", n))
    } else {
        None
    }
}

fn label(sigil: &Option<Sigil>, atype: &str, name: Option<&str>) -> String {
    match (atype, sigil, name) {
        ("function", Some(Sigil::Ref), Some(n)) => format!("calls to {}", n),
        ("function", Some(Sigil::Ref), None) => "any function call".into(),
        ("function", Some(Sigil::Def), Some(n)) => format!("definitions of {}", n),
        ("function", Some(Sigil::Def), None) => "any function definition".into(),
        ("function", None, Some(n)) => format!("definitions of, or calls to, {}", n),
        ("function", None, None) => "any function definition or call".into(),
        ("id", _, Some(n)) => format!("any occurrence of the identifier {}", n),
        ("import", _, _) => "imports (all forms)".into(),
        (t, Some(Sigil::Def), Some(n)) => format!("{} definitions named {}", t, n),
        (t, Some(Sigil::Def), None) => format!("any {} definition", t),
        (t, Some(Sigil::Ref), Some(n)) => format!("references to {} ({})", n, t),
        (t, Some(Sigil::Ref), None) => format!("any {} reference", t),
        (t, None, Some(n)) => format!("{} named {}", t, n),
        (t, None, None) => format!("any {}", t),
    }
}

// ---------------------------------------------------------------------------
// Structural emitter: concrete tree → literal query
// ---------------------------------------------------------------------------

const STRUCTURAL_MAX_DEPTH: usize = 4;
const STRUCTURAL_MAX_LEN: usize = 200;
const STRUCTURAL_MAX_LEAF_TEXT: usize = 40;

/// `body` and `params` are the query language's own field-access keywords with
/// language-specific routing that drops passthrough fragments — never emit them.
const RESERVED_FIELDS: &[&str] = &["body", "params"];

struct StructuralEmit {
    query: String,
    /// True when the subtree pins at least one literal name.
    informative: bool,
}

/// Emit a literal structural selector for `node`: raw node kinds, a field
/// access per named field-bearing child, `#name` filters on leaves. Placeholder
/// subtrees return None so their position stays unconstrained. Non-field
/// children can't be conjoined by the query language, so the first informative
/// one is pinned with a child combinator.
fn emit_structural(node: Node, src: &[u8], depth: usize) -> Option<StructuralEmit> {
    // True leaves only (no children at all): empty containers like `arguments`
    // have anonymous punctuation children and must not be text-pinned.
    if node.child_count() == 0 {
        let text = node.utf8_text(src).ok()?;
        if is_sentinel(text) {
            return None;
        }
        let name = if text.len() <= STRUCTURAL_MAX_LEAF_TEXT { render_name(text) } else { None };
        return Some(match name {
            Some(nq) => StructuralEmit { query: format!("{}{}", node.kind(), nq), informative: true },
            None => StructuralEmit { query: node.kind().to_string(), informative: false },
        });
    }
    if depth >= STRUCTURAL_MAX_DEPTH {
        return Some(StructuralEmit { query: node.kind().to_string(), informative: false });
    }

    let mut query = node.kind().to_string();
    let mut informative = false;
    let mut seen_fields: Vec<&str> = Vec::new();
    let mut first_nonfield: Option<String> = None;
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() && child.kind() != "comment" {
                match cursor.field_name() {
                    Some(field) if !RESERVED_FIELDS.contains(&field) && !seen_fields.contains(&field) => {
                        seen_fields.push(field);
                        if let Some(sub) = emit_structural(child, src, depth + 1) {
                            if sub.informative {
                                query.push_str(&format!(".{}({})", field, sub.query));
                                informative = true;
                            }
                        }
                        // None (placeholder): field stays unconstrained.
                    }
                    Some(_) => {}
                    None => {
                        if first_nonfield.is_none() {
                            if let Some(sub) = emit_structural(child, src, depth + 1) {
                                if sub.informative { first_nonfield = Some(sub.query); }
                            }
                        }
                    }
                }
            }
            if !cursor.goto_next_sibling() { break; }
        }
    }
    if let Some(sub) = first_nonfield {
        query.push_str(&format!(" > ({})", sub));
        informative = true;
    }
    Some(StructuralEmit { query, informative })
}

fn structural_label(placeholders: &[String]) -> String {
    if placeholders.is_empty() {
        "this exact code shape".to_string()
    } else {
        let names: Vec<String> = placeholders.iter().map(|p| format!("${}", p)).collect();
        format!("this code shape, for any {}", names.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Self-verification
// ---------------------------------------------------------------------------

fn compile_query_str(query_str: &str, lang: &dyn Lang) -> Option<CompiledQuery> {
    let ast = crate::query::parse(query_str).ok()?;
    let compiled = compiler::compile(&ast, lang);
    if compiled.is_empty() { None } else { Some(compiled) }
}

fn compiled_key(compiled: &CompiledQuery) -> String {
    match compiled {
        CompiledQuery::Simple(s) => s.clone(),
        CompiledQuery::Scoped { outer, scope_field, inner } => {
            format!("{}\u{1}{}\u{1}{}", outer, scope_field, inner)
        }
    }
}

/// A candidate verifies only if some capture lands fully inside the construct's
/// byte range — containment, not overlap, so a harness function that *contains*
/// the snippet can't confirm a candidate the snippet itself doesn't match.
fn matches_snippet(
    compiled: &CompiledQuery,
    ts_lang: &tree_sitter::Language,
    tree: &Tree,
    src: &[u8],
    range: &Range<usize>,
) -> bool {
    match compiled {
        CompiledQuery::Simple(sexp) => {
            let Ok(query) = Query::new(ts_lang, sexp) else { return false };
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(&query, tree.root_node(), src);
            while let Some(m) = matches.next() {
                if m.captures.iter().any(|c| {
                    c.node.start_byte() >= range.start && c.node.end_byte() <= range.end
                }) {
                    return true;
                }
            }
            false
        }
        CompiledQuery::Scoped { outer, scope_field, inner } => {
            let Ok(outer_q) = Query::new(ts_lang, outer) else { return false };
            let Ok(inner_q) = Query::new(ts_lang, inner) else { return false };
            let mut scopes: Vec<Node> = Vec::new();
            {
                let mut cursor = QueryCursor::new();
                let mut matches = cursor.matches(&outer_q, tree.root_node(), src);
                while let Some(m) = matches.next() {
                    if let Some(cap) = m.captures.first() {
                        if cap.node.start_byte() >= range.start && cap.node.end_byte() <= range.end {
                            scopes.push(
                                cap.node.child_by_field_name(scope_field.as_str()).unwrap_or(cap.node),
                            );
                        }
                    }
                }
            }
            scopes.into_iter().any(|scope| {
                let mut cursor = QueryCursor::new();
                cursor.matches(&inner_q, scope, src).next().is_some()
            })
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verified candidate query strings for a snippet, flattened across constructs.
    fn candidates(snippet: &str, lang_name: &str) -> Vec<String> {
        let lang = lang::for_name(lang_name).unwrap();
        let snippet = dedent(snippet).trim().to_string();
        derive(&snippet, lang.as_ref(), lang_name, false)
            .unwrap()
            .into_iter()
            .flat_map(|g| g.candidates)
            .filter(|c| c.verified)
            .map(|c| c.query)
            .collect()
    }

    #[test]
    fn js_method_call() {
        let c = candidates("u.sendEmail()", "javascript");
        // Most-literal first: the structural rung pins receiver and method.
        assert!(c[0].starts_with("call_expression"), "got: {:?}", c);
        assert!(c[0].contains("identifier#u"), "got: {:?}", c);
        assert!(c[0].contains("property_identifier#sendEmail"), "got: {:?}", c);
        assert_eq!(c[1], "&function#sendEmail", "got: {:?}", c);
        assert!(c.contains(&"function#sendEmail".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_plain_call() {
        let c = candidates("sendEmail()", "javascript");
        assert!(c[0].starts_with("call_expression"), "got: {:?}", c);
        assert!(c.contains(&"&function#sendEmail".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_function_def() {
        let c = candidates("function foo() {}", "javascript");
        assert!(c[0].contains("function_declaration"), "got: {:?}", c);
        assert!(c[0].contains("identifier#foo"), "got: {:?}", c);
        assert!(c.contains(&"*function#foo".to_string()), "got: {:?}", c);
        assert!(c.contains(&"function#foo".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_arrow_function() {
        let c = candidates("const f = () => {}", "javascript");
        assert!(c.contains(&"*function#f".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_class() {
        // JS `class` resolves identically under both sigils, so the honest
        // rendering is the bare form (compiles the same as *class#User).
        let c = candidates("class User {}", "javascript");
        assert!(c.contains(&"class#User".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_import() {
        let c = candidates("import x from 'y'", "javascript");
        assert!(c.contains(&"import".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_if() {
        let c = candidates("if (x) {}", "javascript");
        assert!(c.contains(&"if".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_unmapped_passthrough() {
        let c = candidates("try {} catch (e) {}", "javascript");
        assert!(c.contains(&"try_statement".to_string()), "got: {:?}", c);
    }

    #[test]
    fn js_bare_identifier() {
        let c = candidates("sendEmail", "javascript");
        assert_eq!(c, vec!["id#sendEmail".to_string()], "got: {:?}", c);
    }

    #[test]
    fn python_def() {
        let c = candidates("def foo(): pass", "python");
        assert!(c.contains(&"*function#foo".to_string()), "got: {:?}", c);
    }

    #[test]
    fn python_method_call() {
        let c = candidates("u.send_email()", "python");
        assert!(c[0].starts_with("call"), "got: {:?}", c);
        assert!(c[0].contains("identifier#send_email"), "got: {:?}", c);
        assert!(c.contains(&"&function#send_email".to_string()), "got: {:?}", c);
    }

    #[test]
    fn python_indented_snippet_dedents() {
        let c = candidates("    u.send_email()", "python");
        assert!(c.contains(&"&function#send_email".to_string()), "got: {:?}", c);
    }

    #[test]
    fn c_call_needs_harness() {
        let c = candidates("foo(1);", "c");
        assert!(c.contains(&"&function#foo".to_string()), "got: {:?}", c);
    }

    #[test]
    fn rust_call_needs_harness() {
        let c = candidates("foo();", "rust");
        assert!(c.contains(&"&function#foo".to_string()), "got: {:?}", c);
    }

    #[test]
    fn go_method_call_needs_harness() {
        let c = candidates("u.Send()", "go");
        assert!(c.contains(&"&function#Send".to_string()), "got: {:?}", c);
    }

    #[test]
    fn structural_pins_literal_argument() {
        let c = candidates("u.sendEmail(size)", "javascript");
        assert!(c[0].contains("identifier#size"), "got: {:?}", c);
        assert!(c[0].contains("property_identifier#sendEmail"), "got: {:?}", c);
    }

    #[test]
    fn placeholder_receiver_generalizes() {
        let c = candidates("$X.sendEmail()", "javascript");
        assert!(c[0].contains("property_identifier#sendEmail"), "got: {:?}", c);
        assert!(!c[0].contains("object"), "receiver must be unconstrained: {:?}", c);
        assert!(c.contains(&"&function#sendEmail".to_string()), "got: {:?}", c);
        assert!(!c.iter().any(|q| q.contains(SENTINEL_PREFIX)), "sentinel leaked: {:?}", c);
    }

    #[test]
    fn placeholder_name_suppresses_name_rungs() {
        let c = candidates("function $F() {}", "javascript");
        assert!(c.contains(&"*function".to_string()), "got: {:?}", c);
        assert!(!c.iter().any(|q| q.contains(SENTINEL_PREFIX)), "sentinel leaked: {:?}", c);
    }

    #[test]
    fn placeholder_inside_string_stays_literal() {
        let c = candidates("foo(\"$X\")", "javascript");
        assert!(c.iter().any(|q| q.contains("$X")), "string content must stay literal: {:?}", c);
    }

    #[test]
    fn python_placeholder_receiver() {
        let c = candidates("$X.send_email()", "python");
        assert!(c[0].contains("identifier#send_email"), "got: {:?}", c);
        assert!(!c[0].contains("object"), "receiver must be unconstrained: {:?}", c);
        assert!(c.contains(&"&function#send_email".to_string()), "got: {:?}", c);
    }

    #[test]
    fn verification_containment_rejects_harness_match() {
        // `*function` matches the __ctsq harness wrapper but nothing inside the
        // snippet itself — containment (not overlap) must reject it.
        let lang = lang::for_name("rust").unwrap();
        let prefix = "fn __ctsq() {\n";
        let src = format!("{}foo();\n}}\n", prefix).into_bytes();
        let range = prefix.len()..prefix.len() + "foo();".len();
        let ts_lang = lang.ts_language();
        let mut parser = Parser::new();
        parser.set_language(&ts_lang).unwrap();
        let tree = parser.parse(&src, None).unwrap();
        assert!(!tree.root_node().has_error());

        let wrong = compile_query_str("*function", lang.as_ref()).unwrap();
        assert!(!matches_snippet(&wrong, &ts_lang, &tree, &src, &range));
        let right = compile_query_str("&function#foo", lang.as_ref()).unwrap();
        assert!(matches_snippet(&right, &ts_lang, &tree, &src, &range));
    }
}
