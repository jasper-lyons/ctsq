//! Rust resolver: definition indexing, file scope model, and call resolution.
//!
//! Resolution is heuristic, not full type inference. Call returns are
//! followed one level through the indexed return type (`let x = make();
//! x.m()` resolves when `make`'s `-> Foo` is in the index, `-> Self` maps to
//! the impl type). Trait dispatch is CHA: receivers typed as `dyn Trait`
//! (incl. through `Box`/`Rc`/`Arc`), `T: Trait` bounds, or `impl Trait`
//! expand to one edge per `impl Trait for Type` block — the override when
//! the impl defines the method, else the trait default. Out of scope (calls
//! fall back to ranked name matching): blanket impls (`impl<T: A> B for T`),
//! supertrait methods, macro-generated code, `pub use` re-export chains, and
//! exact block-scope shadowing (the last `let` before the call line wins).

use super::{Callee, Confidence, Def, DefId, DefIndex, DefKind, Receiver, ResolvedCall, Resolver};
use std::collections::HashMap;
use tree_sitter::Node;

#[derive(Default)]
pub struct RustResolver;

impl Resolver for RustResolver {
    fn collect_defs(&self, file: &str, source: &[u8]) -> Vec<Def> {
        let tree = match parse(source) { Some(t) => t, None => return vec![] };
        let mut out = Vec::new();
        let mut mods = file_module_path(file);
        walk_defs(tree.root_node(), source, file, &mut mods, Owner::Free, &mut out);
        out
    }

    fn collect_impls(&self, _file: &str, source: &[u8]) -> Vec<(String, String)> {
        let tree = match parse(source) { Some(t) => t, None => return vec![] };
        let mut out = Vec::new();
        walk_impls(tree.root_node(), source, &mut out);
        out
    }

    fn resolve_calls(&self, file: &str, source: &[u8], index: &DefIndex) -> Vec<ResolvedCall> {
        let tree = match parse(source) { Some(t) => t, None => return vec![] };
        let scope = FileScope::build(tree.root_node(), source);

        // def line -> index, for attributing calls to their enclosing function
        let def_at: HashMap<usize, usize> = index.defs.iter().enumerate()
            .filter(|(_, d)| d.id.file == file)
            .map(|(i, d)| (d.id.line, i))
            .collect();

        let mut out = Vec::new();
        for fun in &scope.fns {
            let Some(&caller) = def_at.get(&fun.line) else { continue };
            let ctx = Ctx { file, scope: &scope, fun, index };
            for (callee, line) in &fun.calls {
                // trait-object dispatch can yield several targets → one edge each
                let targets = resolve_callee(callee, *line, 0, &ctx);
                let display = callee.name().to_string();
                if targets.is_empty() {
                    out.push(ResolvedCall {
                        caller, callee_display: display, line: *line, target: None,
                    });
                } else {
                    for t in targets {
                        out.push(ResolvedCall {
                            caller, callee_display: display.clone(), line: *line, target: Some(t),
                        });
                    }
                }
            }
        }
        out
    }
}

fn parse(source: &[u8]) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).ok()?;
    parser.parse(source, None)
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

// ── Definition indexing ───────────────────────────────────────────────────────

/// Module path implied by the file's location: components after the nearest
/// `src/` ancestor; `lib.rs`/`main.rs`/`mod.rs` contribute their directory
/// only. Heuristic — no Cargo workspace awareness.
fn file_module_path(file: &str) -> Vec<String> {
    let p = std::path::Path::new(file);
    let dirs: Vec<&str> = p.parent()
        .map(|d| d.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect())
        .unwrap_or_default();
    let mut mods: Vec<String> = match dirs.iter().rposition(|c| *c == "src") {
        Some(i) => dirs[i + 1..].iter().map(|s| s.to_string()).collect(),
        None => vec![],
    };
    match p.file_stem().and_then(|s| s.to_str()) {
        Some("lib") | Some("main") | Some("mod") | None => {}
        Some(stem) => mods.push(stem.to_string()),
    }
    mods
}

/// Container a def lives in: an inherent impl, an `impl Trait for Type`, or
/// a trait body (default methods get the trait as receiver, bodyless
/// signatures become `InterfaceMethod`s).
#[derive(Clone, Copy)]
enum Owner<'a> {
    Free,
    Impl { ty: &'a str, of_trait: Option<&'a str> },
    Trait(&'a str),
}

fn walk_defs(
    node: Node,
    source: &[u8],
    file: &str,
    mods: &mut Vec<String>,
    owner: Owner,
    out: &mut Vec<Def>,
) {
    let receiver = match owner {
        Owner::Free => None,
        Owner::Impl { ty, .. } => Some(ty),
        Owner::Trait(t) => Some(t),
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = text(name_node, source).to_string();
                    let mut qualified = mods.clone();
                    if let Some(r) = receiver { qualified.push(r.to_string()); }
                    qualified.push(name.clone());
                    let kind = match owner {
                        Owner::Impl { ty, of_trait } => DefKind::Method {
                            receiver: ty.to_string(),
                            via_trait: of_trait.map(String::from),
                        },
                        Owner::Trait(t) => DefKind::Method {
                            receiver: t.to_string(),
                            via_trait: None,
                        },
                        Owner::Free => DefKind::Function,
                    };
                    // `-> Self` means the impl type
                    let ret = child.child_by_field_name("return_type")
                        .and_then(|t| type_name(t, source))
                        .map(|t| match (t.as_str(), receiver) {
                            ("Self", Some(r)) => r.to_string(),
                            _ => t,
                        });
                    out.push(Def {
                        name, qualified, kind, ret,
                        id: DefId { file: file.to_string(), line: child.start_position().row + 1 },
                    });
                }
                if let Some(body) = child.child_by_field_name("body") {
                    walk_defs(body, source, file, mods, Owner::Free, out);
                }
            }
            // bodyless trait method signature — a dispatch point
            "function_signature_item" => {
                let (Owner::Trait(tr), Some(name_node)) =
                    (owner, child.child_by_field_name("name"))
                else { continue };
                let name = text(name_node, source).to_string();
                let mut qualified = mods.clone();
                qualified.push(tr.to_string());
                qualified.push(name.clone());
                let ret = child.child_by_field_name("return_type")
                    .and_then(|t| type_name(t, source));
                out.push(Def {
                    name, qualified,
                    kind: DefKind::InterfaceMethod { interface: tr.to_string() },
                    ret,
                    id: DefId { file: file.to_string(), line: child.start_position().row + 1 },
                });
            }
            "mod_item" => {
                if let (Some(name_node), Some(body)) =
                    (child.child_by_field_name("name"), child.child_by_field_name("body"))
                {
                    mods.push(text(name_node, source).to_string());
                    walk_defs(body, source, file, mods, Owner::Free, out);
                    mods.pop();
                }
            }
            "impl_item" => {
                let ty = child.child_by_field_name("type").and_then(|t| type_name(t, source));
                let tr = child.child_by_field_name("trait").and_then(|t| type_name(t, source));
                if let (Some(ty), Some(body)) = (&ty, child.child_by_field_name("body")) {
                    walk_defs(body, source, file, mods,
                        Owner::Impl { ty, of_trait: tr.as_deref() }, out);
                }
            }
            "trait_item" => {
                let name = child.child_by_field_name("name").map(|n| text(n, source).to_string());
                if let (Some(name), Some(body)) = (&name, child.child_by_field_name("body")) {
                    walk_defs(body, source, file, mods, Owner::Trait(name), out);
                }
            }
            _ => {}
        }
    }
}

/// `impl Trait for Type` blocks anywhere in the file (impls can sit inside
/// `mod`s or functions) — recorded even when the body is empty, so types
/// relying entirely on trait defaults still count as implementors.
fn walk_impls(node: Node, source: &[u8], out: &mut Vec<(String, String)>) {
    if node.kind() == "impl_item" {
        let ty = node.child_by_field_name("type").and_then(|t| type_name(t, source));
        let tr = node.child_by_field_name("trait").and_then(|t| type_name(t, source));
        if let (Some(ty), Some(tr)) = (ty, tr) {
            out.push((ty, tr));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_impls(child, source, out);
    }
}

/// Strip references and generics down to the base type name:
/// `&mut Foo<T>` → `Foo`, `mod::Foo` → `Foo`. Dispatch types name their
/// trait (`&dyn W` / `impl W` → `W`), and smart pointers are transparent
/// (`Box<dyn W>` → `W`, `Rc<Foo>` → `Foo`).
fn type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, source).to_string()),
        "generic_type" => {
            let base = node.child_by_field_name("type").and_then(|t| type_name(t, source))?;
            if matches!(base.as_str(), "Box" | "Rc" | "Arc") {
                if let Some(inner) = node.child_by_field_name("type_arguments")
                    .and_then(|args| args.named_child(0))
                    .and_then(|a| type_name(a, source))
                {
                    return Some(inner);
                }
            }
            Some(base)
        }
        "reference_type" =>
            node.child_by_field_name("type").and_then(|t| type_name(t, source)),
        "dynamic_type" | "abstract_type" =>
            node.child_by_field_name("trait").and_then(|t| type_name(t, source)),
        "scoped_type_identifier" =>
            node.child_by_field_name("name").map(|n| text(n, source).to_string()),
        _ => None,
    }
}

// ── File scope model ──────────────────────────────────────────────────────────

#[derive(Debug)]
enum Binding {
    /// `let f = foo;` / `let f = Foo::bar;` — variable holds a function value.
    FnValue(Callee),
    /// `let x: Foo = ...` / `let x = Foo { .. }`.
    Typed(String),
    /// `let x = make();` — type comes from the callee's return type at
    /// resolve time (or the `Type::assoc(..)` constructor heuristic).
    Call(Callee),
}

#[derive(Debug)]
struct FnScope {
    /// Start line of the function_item — matches `Def.id.line`.
    line: usize,
    /// Enclosing impl's type (or trait name for trait default methods).
    impl_type: Option<String>,
    /// Trait name when inside `impl Trait for Type`.
    trait_name: Option<String>,
    /// (var, type) pairs from the parameter list.
    params: Vec<(String, String)>,
    /// type parameter → trait bounds, from `<T: W>` and `where T: W`.
    bounds: HashMap<String, Vec<String>>,
    /// (line, var, binding) in document order.
    bindings: Vec<(usize, String, Binding)>,
    /// (callee, call-site line) in document order.
    calls: Vec<(Callee, usize)>,
}

#[derive(Debug)]
struct FileScope {
    /// local name → full path segments (aliases included).
    imports: HashMap<String, Vec<String>>,
    /// `use path::*` glob paths.
    globs: Vec<Vec<String>>,
    fns: Vec<FnScope>,
}

impl FileScope {
    fn build(root: Node, source: &[u8]) -> FileScope {
        let mut s = FileScope { imports: HashMap::new(), globs: vec![], fns: vec![] };
        let mut fn_stack = Vec::new();
        let mut impl_stack = Vec::new();
        walk_scope(root, source, &mut s, &mut fn_stack, &mut impl_stack);
        s
    }
}

fn walk_scope(
    node: Node,
    source: &[u8],
    s: &mut FileScope,
    fn_stack: &mut Vec<usize>,
    impl_stack: &mut Vec<(Option<String>, Option<String>)>,
) {
    match node.kind() {
        "use_declaration" => {
            if let Some(arg) = node.child_by_field_name("argument") {
                collect_use(arg, source, &[], s);
            }
            return;
        }
        "impl_item" => {
            let ty = node.child_by_field_name("type").and_then(|t| type_name(t, source));
            let tr = node.child_by_field_name("trait").and_then(|t| type_name(t, source));
            impl_stack.push((ty, tr));
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, s, fn_stack, impl_stack);
            }
            impl_stack.pop();
            return;
        }
        "trait_item" => {
            let name = node.child_by_field_name("name").map(|n| text(n, source).to_string());
            impl_stack.push((name, None));
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, s, fn_stack, impl_stack);
            }
            impl_stack.pop();
            return;
        }
        "function_item" => {
            let (impl_type, trait_name) = impl_stack.last().cloned().unwrap_or((None, None));
            let idx = s.fns.len();
            s.fns.push(FnScope {
                line: node.start_position().row + 1,
                impl_type,
                trait_name,
                params: collect_params(node, source),
                bounds: collect_bounds(node, source),
                bindings: vec![],
                calls: vec![],
            });
            fn_stack.push(idx);
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, s, fn_stack, impl_stack);
            }
            fn_stack.pop();
            return;
        }
        "let_declaration" => {
            if let Some(&f) = fn_stack.last() {
                if let Some((var, b)) = classify_let(node, source) {
                    s.fns[f].bindings.push((node.start_position().row + 1, var, b));
                }
            }
            // fall through: the value expression may contain calls
        }
        "call_expression" => {
            if let Some(&f) = fn_stack.last() {
                if let Some(c) = classify_call(node, source) {
                    s.fns[f].calls.push((c, node.start_position().row + 1));
                }
            }
            // fall through: arguments and receivers may contain calls
        }
        _ => {}
    }
    walk_children(node, source, s, fn_stack, impl_stack);
}

fn walk_children(
    node: Node,
    source: &[u8],
    s: &mut FileScope,
    fn_stack: &mut Vec<usize>,
    impl_stack: &mut Vec<(Option<String>, Option<String>)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_scope(child, source, s, fn_stack, impl_stack);
    }
}

/// Recursively collect `use` trees into the import map / glob list.
/// `prefix` carries path segments accumulated from enclosing scoped lists.
fn collect_use(node: Node, source: &[u8], prefix: &[String], s: &mut FileScope) {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" => {
            let mut path = prefix.to_vec();
            push_seg(&mut path, text(node, source));
            if let Some(last) = path.last().cloned() {
                s.imports.insert(last, path);
            }
        }
        "scoped_identifier" => {
            let mut path = prefix.to_vec();
            flatten_path(node, source, &mut path);
            if let Some(last) = path.last().cloned() {
                s.imports.insert(last, path);
            }
        }
        "scoped_use_list" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.child_by_field_name("path") {
                flatten_path(p, source, &mut path);
            }
            if let Some(list) = node.child_by_field_name("list") {
                let mut cursor = list.walk();
                for item in list.named_children(&mut cursor) {
                    collect_use(item, source, &path, s);
                }
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for item in node.named_children(&mut cursor) {
                collect_use(item, source, prefix, s);
            }
        }
        "use_as_clause" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.child_by_field_name("path") {
                flatten_path(p, source, &mut path);
            }
            if let Some(alias) = node.child_by_field_name("alias") {
                s.imports.insert(text(alias, source).to_string(), path);
            }
        }
        "use_wildcard" => {
            let mut path = prefix.to_vec();
            if let Some(p) = node.named_child(0) {
                flatten_path(p, source, &mut path);
            }
            s.globs.push(path);
        }
        _ => {}
    }
}

/// Flatten a (possibly scoped/generic) path node into segments, dropping
/// `crate`/`self`/`super` and turbofish type arguments.
fn flatten_path(node: Node, source: &[u8], out: &mut Vec<String>) {
    match node.kind() {
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(p) = node.child_by_field_name("path") {
                flatten_path(p, source, out);
            }
            if let Some(n) = node.child_by_field_name("name") {
                push_seg(out, text(n, source));
            }
        }
        "generic_type" => {
            if let Some(t) = node.child_by_field_name("type") {
                flatten_path(t, source, out);
            }
        }
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            push_seg(out, text(node, source));
        }
        _ => {}
    }
}

fn push_seg(out: &mut Vec<String>, seg: &str) {
    if !matches!(seg, "crate" | "self" | "super" | "") {
        out.push(seg.to_string());
    }
}

fn collect_params(fn_node: Node, source: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(params) = fn_node.child_by_field_name("parameters") else { return out };
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        if p.kind() != "parameter" { continue; }
        let (Some(pat), Some(ty)) = (p.child_by_field_name("pattern"), p.child_by_field_name("type"))
        else { continue };
        if pat.kind() != "identifier" { continue; }
        if let Some(t) = type_name(ty, source) {
            out.push((text(pat, source).to_string(), t));
        }
    }
    out
}

/// Trait bounds on a function's type parameters: `fn f<T: W>(..)` and
/// `where T: W` both map T → [W].
fn collect_bounds(fn_node: Node, source: &[u8]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut add = |name_node: Option<Node>, bounds_node: Option<Node>| {
        let (Some(n), Some(b)) = (name_node, bounds_node) else { return };
        let mut c = b.walk();
        let traits: Vec<String> = b.named_children(&mut c)
            .filter_map(|t| type_name(t, source))
            .collect();
        if !traits.is_empty() {
            out.entry(text(n, source).to_string()).or_default().extend(traits);
        }
    };
    if let Some(tps) = fn_node.child_by_field_name("type_parameters") {
        let mut c = tps.walk();
        for tp in tps.named_children(&mut c) {
            if tp.kind() == "type_parameter" {
                add(tp.child_by_field_name("name"), tp.child_by_field_name("bounds"));
            }
        }
    }
    // where_clause is a plain child, not a field
    let mut c = fn_node.walk();
    for child in fn_node.children(&mut c) {
        if child.kind() != "where_clause" { continue; }
        let mut wc = child.walk();
        for pred in child.named_children(&mut wc) {
            if pred.kind() == "where_predicate" {
                add(pred.child_by_field_name("left"), pred.child_by_field_name("bounds"));
            }
        }
    }
    out
}

fn classify_let(node: Node, source: &[u8]) -> Option<(String, Binding)> {
    let pat = node.child_by_field_name("pattern")?;
    if pat.kind() != "identifier" { return None; }
    let var = text(pat, source).to_string();

    if let Some(ty) = node.child_by_field_name("type") {
        if let Some(t) = type_name(ty, source) {
            return Some((var, Binding::Typed(t)));
        }
    }

    let value = node.child_by_field_name("value")?;
    match value.kind() {
        "identifier" => Some((var, Binding::FnValue(Callee::Bare(text(value, source).to_string())))),
        "scoped_identifier" => {
            let mut segs = Vec::new();
            flatten_path(value, source, &mut segs);
            match segs.len() {
                0 => None,
                1 => Some((var, Binding::FnValue(Callee::Bare(segs.pop().unwrap())))),
                _ => Some((var, Binding::FnValue(Callee::Path(segs)))),
            }
        }
        // `let x = make();` — callee return type looked up at resolve time
        "call_expression" => {
            classify_call(value, source).map(|c| (var, Binding::Call(c)))
        }
        "struct_expression" => {
            let name = value.child_by_field_name("name")?;
            let mut segs = Vec::new();
            flatten_path(name, source, &mut segs);
            segs.pop().map(|t| (var, Binding::Typed(t)))
        }
        _ => None,
    }
}

fn classify_call(node: Node, source: &[u8]) -> Option<Callee> {
    classify_callee_expr(node.child_by_field_name("function")?, source)
}

fn classify_callee_expr(f: Node, source: &[u8]) -> Option<Callee> {
    match f.kind() {
        "identifier" => Some(Callee::Bare(text(f, source).to_string())),
        "scoped_identifier" => {
            let mut segs = Vec::new();
            flatten_path(f, source, &mut segs);
            match segs.len() {
                0 => None,
                1 => Some(Callee::Bare(segs.pop().unwrap())),
                _ => Some(Callee::Path(segs)),
            }
        }
        // `foo::<T>()` — turbofish directly on the function
        "generic_function" => {
            f.child_by_field_name("function").and_then(|inner| classify_callee_expr(inner, source))
        }
        "field_expression" => {
            let name = text(f.child_by_field_name("field")?, source).to_string();
            let value = f.child_by_field_name("value")?;
            let receiver = match value.kind() {
                "self" => Receiver::SelfRecv,
                "identifier" => Receiver::Var(text(value, source).to_string()),
                _ => Receiver::Opaque(
                    text(value, source).lines().next().unwrap_or("").to_string(),
                ),
            };
            Some(Callee::Method { receiver, name })
        }
        _ => None,
    }
}

// ── Resolution ────────────────────────────────────────────────────────────────

struct Ctx<'a> {
    file: &'a str,
    scope: &'a FileScope,
    fun: &'a FnScope,
    index: &'a DefIndex,
}

const MAX_BINDING_DEPTH: usize = 4;

/// Resolve a call site to its target definitions. Every path yields zero or
/// one target except trait-object/bound dispatch, which expands to one edge
/// per implementing type.
fn resolve_callee(callee: &Callee, line: usize, depth: usize, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    if depth > MAX_BINDING_DEPTH { return vec![]; }
    match callee {
        Callee::Path(segs) => one(resolve_path(segs, ctx)),
        Callee::Bare(name) => one(resolve_bare(name, line, depth, ctx)),
        Callee::Method { receiver, name } => resolve_method(receiver, name, line, depth, ctx),
    }
}

fn one(hit: Option<(usize, Confidence)>) -> Vec<(usize, Confidence)> {
    hit.into_iter().collect()
}

fn resolve_bare(name: &str, line: usize, depth: usize, ctx: &Ctx) -> Option<(usize, Confidence)> {
    // (a) local binding holding a function value
    if let Some(Binding::FnValue(inner)) = last_binding(ctx.fun, name, line) {
        if let Some(&hit) = resolve_callee(inner, line, depth + 1, ctx).first() {
            return Some(hit);
        }
    }
    // (b) free function in the same file
    let same_file: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| {
            let d = &ctx.index.defs[i];
            d.id.file == ctx.file && matches!(d.kind, DefKind::Function)
        })
        .collect();
    if !same_file.is_empty() {
        return rank(same_file, false, ctx);
    }
    // (c) explicit import — candidates keyed by the path's real target
    // name, which differs from `name` when the import is an alias
    if let Some(path) = ctx.scope.imports.get(name) {
        let target_name = path.last().map(|s| s.as_str()).unwrap_or(name);
        let cands: Vec<usize> = name_candidates(ctx, target_name)
            .filter(|&i| ends_with(&ctx.index.defs[i].qualified, path))
            .collect();
        if !cands.is_empty() {
            return rank(cands, false, ctx);
        }
    }
    // (d) glob imports
    let glob_cands: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| {
            let q = &ctx.index.defs[i].qualified;
            ctx.scope.globs.iter().any(|g| {
                q.len() > g.len()
                    && ends_with(&q[..q.len() - 1], g)
                    && q.last().map(|s| s == name).unwrap_or(false)
            })
        })
        .collect();
    if !glob_cands.is_empty() {
        return rank(glob_cands, false, ctx);
    }
    // (e) ranked fallback over every def with this name
    rank(name_candidates(ctx, name).collect(), false, ctx)
}

fn resolve_method(
    receiver: &Receiver,
    name: &str,
    line: usize,
    depth: usize,
    ctx: &Ctx,
) -> Vec<(usize, Confidence)> {
    match receiver {
        Receiver::SelfRecv => {
            // self is statically the impl type — no dispatch expansion
            if let Some(ty) = &ctx.fun.impl_type {
                if let Some(hit) = method_on(ty, name, ctx) { return one(Some(hit)); }
            }
            if let Some(tr) = &ctx.fun.trait_name {
                if let Some(hit) = method_on(tr, name, ctx) { return one(Some(hit)); }
            }
        }
        Receiver::Var(v) => {
            if let Some(ty) = var_type(v, line, depth, ctx) {
                let hits = dispatch_or_method(&ty, name, ctx);
                if !hits.is_empty() { return hits; }
                // generic type parameter: follow its trait bounds
                for b in ctx.fun.bounds.get(&ty).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let hits = dispatch_or_method(b, name, ctx);
                    if !hits.is_empty() { return hits; }
                }
            }
        }
        Receiver::Opaque(_) => {}
    }
    // fallback: any def with this name, methods preferred
    one(rank(name_candidates(ctx, name).collect(), true, ctx))
}

/// Methods on `ty`: trait dispatch expansion when `ty` is a trait with
/// implementations in the project, else the directly defined method.
fn dispatch_or_method(ty: &str, name: &str, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    let hits = trait_dispatch(ty, name, ctx);
    if !hits.is_empty() { return hits; }
    one(method_on(ty, name, ctx))
}

/// CHA for `dyn Trait` / `T: Trait` / `impl Trait` receivers: one edge per
/// type with an `impl Trait for Type` block — the overriding method when the
/// impl defines it, else the trait's default. A single target is exact;
/// several are each a guess. With no impl blocks in the project the edge
/// points at the signature (defaults are found by `method_on` instead).
fn trait_dispatch(tr: &str, name: &str, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    // `name` must belong to the trait: a signature or a default method
    let is_trait_method = name_candidates(ctx, name).any(|i| match &ctx.index.defs[i].kind {
        DefKind::InterfaceMethod { interface } => interface == tr,
        DefKind::Method { receiver, via_trait: None } => receiver == tr,
        _ => false,
    });
    if !is_trait_method { return vec![]; }

    let mut implementors: Vec<&str> = ctx.index.impls.iter()
        .filter(|(_, t)| t == tr)
        .map(|(ty, _)| ty.as_str())
        .collect();
    implementors.sort();
    implementors.dedup();

    if implementors.is_empty() {
        // no impls in the project: point at the signature if that's all there is
        return one(name_candidates(ctx, name)
            .find(|&i| matches!(
                &ctx.index.defs[i].kind,
                DefKind::InterfaceMethod { interface } if interface == tr
            ))
            .map(|i| (i, Confidence::Exact)));
    }

    let default = name_candidates(ctx, name).find(|&i| matches!(
        &ctx.index.defs[i].kind,
        DefKind::Method { receiver, via_trait: None } if receiver == tr
    ));
    let mut targets: Vec<usize> = implementors.iter()
        .filter_map(|ty| {
            name_candidates(ctx, name)
                .find(|&i| matches!(
                    &ctx.index.defs[i].kind,
                    DefKind::Method { receiver, .. } if receiver == ty
                ))
                .or(default)
        })
        .collect();
    targets.sort();
    targets.dedup();
    targets.sort_by(|&a, &b| {
        let (da, db) = (&ctx.index.defs[a].id, &ctx.index.defs[b].id);
        da.file.cmp(&db.file).then(da.line.cmp(&db.line))
    });
    match targets.len() {
        1 => vec![(targets[0], Confidence::Exact)],
        _ => targets.into_iter().map(|i| (i, Confidence::Guess)).collect(),
    }
}

/// Qualified-path resolution: expand the leading segment through the import
/// map, then suffix-match against definition paths. A path that matches
/// nothing is treated as external — no name-only fallback, so e.g.
/// `HashMap::new()` doesn't resolve to an unrelated local `new`.
fn resolve_path(segs: &[String], ctx: &Ctx) -> Option<(usize, Confidence)> {
    let expanded: Vec<String> = match segs.first().and_then(|s| ctx.scope.imports.get(s)) {
        Some(prefix) => prefix.iter().chain(&segs[1..]).cloned().collect(),
        None => segs.to_vec(),
    };
    let name = expanded.last()?;
    let cands: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| ends_with(&ctx.index.defs[i].qualified, &expanded))
        .collect();
    rank(cands, false, ctx)
}

fn method_on(ty: &str, name: &str, ctx: &Ctx) -> Option<(usize, Confidence)> {
    let cands: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| matches!(&ctx.index.defs[i].kind, DefKind::Method { receiver, .. } if receiver == ty))
        .collect();
    rank(cands, true, ctx)
}

fn name_candidates<'a>(ctx: &'a Ctx, name: &str) -> impl Iterator<Item = usize> + 'a {
    ctx.index.by_name.get(name).map(|v| v.as_slice()).unwrap_or(&[]).iter().copied()
}

/// Most recent binding of `var` at or before `line` in this function.
fn last_binding<'a>(fun: &'a FnScope, var: &str, line: usize) -> Option<&'a Binding> {
    fun.bindings.iter().rev()
        .find(|(l, v, _)| *l <= line && v == var)
        .map(|(_, _, b)| b)
}

/// Type of `var` at `line`: last typed binding — for `let x = make()` the
/// resolved callee's return type, else the `Type::assoc(..)` constructor
/// heuristic — else parameter type.
fn var_type(var: &str, line: usize, depth: usize, ctx: &Ctx) -> Option<String> {
    match last_binding(ctx.fun, var, line) {
        Some(Binding::Typed(t)) => return Some(t.clone()),
        Some(Binding::Call(callee)) => {
            if let Some(&(t, _)) = resolve_callee(callee, line, depth + 1, ctx).first() {
                if let Some(ret) = &ctx.index.defs[t].ret {
                    return Some(ret.clone());
                }
            }
            // `Foo::new(..)` with no resolvable def still implies Foo
            if let Callee::Path(segs) = callee {
                let ty = &segs[segs.len().saturating_sub(2)];
                if segs.len() >= 2
                    && ty.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                {
                    return Some(ty.clone());
                }
            }
            return None;
        }
        Some(Binding::FnValue(_)) | None => {}
    }
    ctx.fun.params.iter().find(|(v, _)| v == var).map(|(_, t)| t.clone())
}

fn ends_with(qualified: &[String], suffix: &[String]) -> bool {
    suffix.len() <= qualified.len() && qualified[qualified.len() - suffix.len()..] == *suffix
}

/// Pick the best candidate. A single candidate is Exact; several rank by
/// locality (same file > same dir > imported > anywhere) and yield a Guess.
fn rank(mut cands: Vec<usize>, prefer_methods: bool, ctx: &Ctx) -> Option<(usize, Confidence)> {
    match cands.len() {
        0 => None,
        1 => Some((cands[0], Confidence::Exact)),
        _ => {
            cands.sort_by(|&a, &b| {
                let (da, db) = (&ctx.index.defs[a], &ctx.index.defs[b]);
                let kind_pen = |d: &Def| {
                    (prefer_methods && !matches!(d.kind, DefKind::Method { .. })) as u8
                };
                (kind_pen(da), locality(da, ctx), &da.id.file, da.id.line)
                    .cmp(&(kind_pen(db), locality(db, ctx), &db.id.file, db.id.line))
            });
            Some((cands[0], Confidence::Guess))
        }
    }
}

fn locality(d: &Def, ctx: &Ctx) -> u8 {
    if d.id.file == ctx.file {
        0
    } else if std::path::Path::new(&d.id.file).parent()
        == std::path::Path::new(ctx.file).parent()
    {
        1
    } else if is_imported(d, ctx) {
        2
    } else {
        3
    }
}

/// Whether the def (or its containing module) is reachable through this
/// file's imports or globs.
fn is_imported(d: &Def, ctx: &Ctx) -> bool {
    let q = &d.qualified;
    let module = &q[..q.len().saturating_sub(1)];
    ctx.scope.imports.values().any(|p| ends_with(q, p) || ends_with(module, p))
        || ctx.scope.globs.iter().any(|g| ends_with(module, g))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Confidence, DefIndex, Resolver};

    fn build_index(files: &[(&str, &str)]) -> DefIndex {
        let r = RustResolver;
        let mut idx = DefIndex::default();
        for (f, src) in files {
            for d in r.collect_defs(f, src.as_bytes()) {
                idx.push(d);
            }
            idx.impls.extend(r.collect_impls(f, src.as_bytes()));
        }
        idx
    }

    // Resolve everything and return (caller_name, callee_display, target file:line, confidence).
    fn resolve_all(files: &[(&str, &str)]) -> Vec<(String, String, Option<(String, usize, Confidence)>)> {
        let r = RustResolver;
        let idx = build_index(files);
        let mut out = Vec::new();
        for (f, src) in files {
            for rc in r.resolve_calls(f, src.as_bytes(), &idx) {
                let caller = idx.defs[rc.caller].name.clone();
                let target = rc.target.map(|(t, c)| {
                    let d = &idx.defs[t];
                    (d.id.file.clone(), d.id.line, c)
                });
                out.push((caller, rc.callee_display, target));
            }
        }
        out
    }

    fn target_of(
        results: &[(String, String, Option<(String, usize, Confidence)>)],
        caller: &str,
        callee: &str,
    ) -> Option<(String, usize, Confidence)> {
        results.iter()
            .find(|(cr, ce, _)| cr == caller && ce == callee)
            .and_then(|(_, _, t)| t.clone())
    }

    #[test]
    fn defs_carry_module_path_and_receiver() {
        let idx = build_index(&[(
            "src/util/geo.rs",
            "mod inner {\n  pub struct Foo;\n  impl Foo {\n    pub fn dist(&self) {}\n  }\n  pub fn free() {}\n}\n",
        )]);
        let dist = idx.defs.iter().find(|d| d.name == "dist").unwrap();
        assert_eq!(dist.qualified, vec!["util", "geo", "inner", "Foo", "dist"]);
        assert!(matches!(&dist.kind, DefKind::Method { receiver, .. } if receiver == "Foo"));
        let free = idx.defs.iter().find(|d| d.name == "free").unwrap();
        assert_eq!(free.qualified, vec!["util", "geo", "inner", "free"]);
        assert!(matches!(free.kind, DefKind::Function));
    }

    #[test]
    fn main_and_lib_contribute_directory_only() {
        assert_eq!(file_module_path("proj/src/main.rs"), Vec::<String>::new());
        assert_eq!(file_module_path("proj/src/util/mod.rs"), vec!["util"]);
        assert_eq!(file_module_path("proj/src/util/geo.rs"), vec!["util", "geo"]);
    }

    #[test]
    fn imports_collect_aliases_nested_lists_and_globs() {
        let src = "use a::{b as c, d::*, e::f};\nuse g;\n";
        let tree = parse(src.as_bytes()).unwrap();
        let scope = FileScope::build(tree.root_node(), src.as_bytes());
        assert_eq!(scope.imports.get("c").unwrap(), &vec!["a", "b"]);
        assert_eq!(scope.imports.get("f").unwrap(), &vec!["a", "e", "f"]);
        assert_eq!(scope.imports.get("g").unwrap(), &vec!["g"]);
        assert_eq!(scope.globs, vec![vec!["a", "d"]]);
    }

    #[test]
    fn same_file_bare_call_is_exact() {
        let results = resolve_all(&[
            ("src/a.rs", "fn helper() {}\nfn go() { helper(); }\n"),
            ("src/b.rs", "fn helper() {}\n"),
        ]);
        let (file, line, conf) = target_of(&results, "go", "helper").unwrap();
        assert_eq!((file.as_str(), line, conf), ("src/a.rs", 1, Confidence::Exact));
    }

    #[test]
    fn import_resolves_cross_file() {
        let results = resolve_all(&[
            ("src/util.rs", "pub fn helper() {}\n"),
            ("src/other.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "use util::helper;\nfn go() { helper(); }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "go", "helper").unwrap();
        assert_eq!((file.as_str(), conf), ("src/util.rs", Confidence::Exact));
    }

    #[test]
    fn alias_resolves_to_original() {
        let results = resolve_all(&[
            ("src/util.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "use util::helper as h;\nfn go() { h(); }\n"),
        ]);
        // alias display name is the alias itself; target is the original def
        let (file, line, conf) = target_of(&results, "go", "h").unwrap();
        assert_eq!((file.as_str(), line, conf), ("src/util.rs", 1, Confidence::Exact));
    }

    #[test]
    fn glob_import_resolves() {
        let results = resolve_all(&[
            ("src/util.rs", "pub fn helper() {}\n"),
            ("src/other.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "use util::*;\nfn go() { helper(); }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "go", "helper").unwrap();
        assert_eq!((file.as_str(), conf), ("src/util.rs", Confidence::Exact));
    }

    #[test]
    fn value_binding_resolves_through_variable() {
        let results = resolve_all(&[(
            "src/a.rs",
            "fn helper() {}\nfn go() {\n  let f = helper;\n  f();\n}\n",
        )]);
        let (file, line, conf) = target_of(&results, "go", "f").unwrap();
        assert_eq!((file.as_str(), line, conf), ("src/a.rs", 1, Confidence::Exact));
    }

    #[test]
    fn value_binding_shadowing_last_wins() {
        let results = resolve_all(&[(
            "src/a.rs",
            "fn one() {}\nfn two() {}\nfn go() {\n  let f = one;\n  let f = two;\n  f();\n}\n",
        )]);
        let (_, line, _) = target_of(&results, "go", "f").unwrap();
        assert_eq!(line, 2, "last binding before the call should win");
    }

    #[test]
    fn value_binding_path_resolves_to_method() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nimpl Foo {\n  fn new() -> Foo { Foo }\n}\nfn go() {\n  let f = Foo::new;\n  f();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "f").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact));
    }

    #[test]
    fn self_method_resolves_within_impl() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nimpl Foo {\n  fn a(&self) { self.b(); }\n  fn b(&self) {}\n}\nfn b() {}\n",
        )]);
        let (_, line, conf) = target_of(&results, "a", "b").unwrap();
        assert_eq!((line, conf), (4, Confidence::Exact), "should pick Foo::b, not free b");
    }

    #[test]
    fn self_method_in_trait_impl_falls_back_to_trait_default() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait Tr {\n  fn dflt(&self) {}\n}\nstruct Foo;\nimpl Tr for Foo {\n  fn go(&self) { self.dflt(); }\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "dflt").unwrap();
        assert_eq!((line, conf), (2, Confidence::Exact));
    }

    #[test]
    fn typed_receiver_resolves_method() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo { fn run(&self) {} }\nimpl Bar { fn run(&self) {} }\nfn go() {\n  let x: Foo = make();\n  x.run();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact), "should pick Foo::run via let type");
    }

    #[test]
    fn constructor_call_gives_type_evidence() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo {\n  fn new() -> Foo { Foo }\n  fn run(&self) {}\n}\nimpl Bar { fn run(&self) {} }\nfn go() {\n  let x = Foo::new();\n  x.run();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (5, Confidence::Exact));
    }

    #[test]
    fn return_type_resolves_through_call_binding() {
        // free function, no Type::assoc heuristic available
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo { fn run(&self) {} }\nimpl Bar { fn run(&self) {} }\nfn make_it() -> Foo { Foo }\nfn go() {\n  let x = make_it();\n  x.run();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact));
    }

    #[test]
    fn self_return_type_means_impl_type() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo {\n  fn create() -> Self { Foo }\n  fn run(&self) {}\n}\nimpl Bar { fn run(&self) {} }\nfn go() {\n  let x = Foo::create();\n  x.run();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (5, Confidence::Exact));
    }

    #[test]
    fn return_type_beats_constructor_name_heuristic() {
        // Factory::build returns a Widget, not a Factory.
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Factory;\nstruct Widget;\nimpl Factory {\n  fn build() -> Widget { Widget }\n  fn run(&self) {}\n}\nimpl Widget { fn run(&self) {} }\nfn go() {\n  let x = Factory::build();\n  x.run();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (7, Confidence::Exact), "should pick Widget::run via return type");
    }

    #[test]
    fn param_type_resolves_method() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo { fn run(&self) {} }\nimpl Bar { fn run(&self) {} }\nfn go(x: &Foo) { x.run(); }\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact));
    }

    #[test]
    fn trait_signatures_index_as_interface_methods_with_impl_links() {
        let idx = build_index(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n  fn flush(&self) {}\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nimpl File {\n  fn open() {}\n}\n",
        )]);
        let sig = idx.defs.iter().find(|d| d.name == "write" && d.id.line == 2).unwrap();
        assert!(matches!(&sig.kind, DefKind::InterfaceMethod { interface } if interface == "W"));
        let dflt = idx.defs.iter().find(|d| d.name == "flush").unwrap();
        assert!(matches!(&dflt.kind,
            DefKind::Method { receiver, via_trait: None } if receiver == "W"));
        let imp = idx.defs.iter().find(|d| d.name == "write" && d.id.line == 7).unwrap();
        assert!(matches!(&imp.kind,
            DefKind::Method { receiver, via_trait: Some(v) } if receiver == "File" && v == "W"));
        let inherent = idx.defs.iter().find(|d| d.name == "open").unwrap();
        assert!(matches!(&inherent.kind,
            DefKind::Method { via_trait: None, .. }));
    }

    #[test]
    fn dyn_param_expands_to_implementations() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nstruct Buf;\nimpl W for Buf {\n  fn write(&self) {}\n}\nfn save(w: &dyn W) {\n  w.write();\n}\n",
        )]);
        let targets: Vec<_> = results.iter()
            .filter(|(cr, ce, _)| cr == "save" && ce == "write")
            .filter_map(|(_, _, t)| t.clone())
            .collect();
        assert_eq!(targets.len(), 2, "one edge per impl: {:?}", targets);
        assert!(targets.iter().all(|(_, _, c)| *c == Confidence::Guess));
        let lines: Vec<usize> = targets.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(lines, vec![6, 10], "File::write and Buf::write, not the signature");
    }

    #[test]
    fn box_dyn_single_impl_is_exact() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nfn save(w: Box<dyn W>) {\n  w.write();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "save", "write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn generic_bound_dispatches_through_trait() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nfn save<T: W>(t: T) {\n  t.write();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "save", "write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn where_clause_bound_dispatches_through_trait() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nfn save<T>(t: T) where T: W {\n  t.write();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "save", "write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn impl_trait_param_dispatches_through_trait() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nstruct File;\nimpl W for File {\n  fn write(&self) {}\n}\nfn save(t: impl W) {\n  t.write();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "save", "write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn non_overriding_impl_falls_back_to_trait_default() {
        // Buf overrides log; File relies on the default. Expansion yields the
        // override plus one shared default edge.
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn log(&self) {}\n}\nstruct File;\nimpl W for File {}\nstruct Buf;\nimpl W for Buf {\n  fn log(&self) {}\n}\nfn save(w: &dyn W) {\n  w.log();\n}\n",
        )]);
        let targets: Vec<_> = results.iter()
            .filter(|(cr, ce, _)| cr == "save" && ce == "log")
            .filter_map(|(_, _, t)| t.clone())
            .collect();
        let lines: Vec<usize> = targets.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(lines, vec![2, 8], "trait default (for File) plus Buf's override");
    }

    #[test]
    fn dyn_with_no_impls_resolves_to_signature() {
        let results = resolve_all(&[(
            "src/a.rs",
            "trait W {\n  fn write(&self);\n}\nfn save(w: &dyn W) {\n  w.write();\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "save", "write").unwrap();
        assert_eq!((line, conf), (2, Confidence::Exact), "edge points at the signature");
    }

    #[test]
    fn smart_pointer_param_sees_through_to_concrete_type() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nstruct Bar;\nimpl Foo { fn run(&self) {} }\nimpl Bar { fn run(&self) {} }\nfn go(x: Rc<Foo>) { x.run(); }\n",
        )]);
        let (_, line, conf) = target_of(&results, "go", "run").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact));
    }

    #[test]
    fn qualified_call_suffix_matches_module_path() {
        let results = resolve_all(&[
            ("src/util.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "fn go() { util::helper(); }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "go", "helper").unwrap();
        assert_eq!((file.as_str(), conf), ("src/util.rs", Confidence::Exact));
    }

    #[test]
    fn unmatched_qualified_path_is_external() {
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nimpl Foo { fn new() -> Foo { Foo } }\nfn go() { std::collections::HashMap::new(); }\n",
        )]);
        assert_eq!(target_of(&results, "go", "new"), None,
            "HashMap::new must not resolve to the local Foo::new");
    }

    #[test]
    fn ambiguous_name_ranks_same_dir_and_marks_guess() {
        let results = resolve_all(&[
            ("src/near.rs", "pub fn helper() {}\n"),
            ("other/far.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "fn go() { helper(); }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "go", "helper").unwrap();
        assert_eq!((file.as_str(), conf), ("src/near.rs", Confidence::Guess));
    }

    #[test]
    fn zero_candidates_stay_unresolved() {
        let results = resolve_all(&[("src/a.rs", "fn go() { println(); }\n")]);
        assert_eq!(target_of(&results, "go", "println"), None);
    }

    #[test]
    fn method_and_qualified_calls_are_extracted() {
        // regression for the old query-based extraction, which only saw
        // `function: (identifier)` call sites in Rust
        let results = resolve_all(&[(
            "src/a.rs",
            "struct Foo;\nimpl Foo {\n  fn m(&self) {}\n  fn s() {}\n}\nfn go() {\n  let x: Foo = make();\n  x.m();\n  Foo::s();\n}\n",
        )]);
        assert!(target_of(&results, "go", "m").is_some(), "method call extracted+resolved");
        assert!(target_of(&results, "go", "s").is_some(), "qualified call extracted+resolved");
    }
}

#[cfg(test)]
mod spike {
    // Spike: dump the grammar's actual node kinds for the constructs the
    // resolver depends on, so the walker code above is built on verified names.
    #[test]
    fn dump_node_kinds() {
        let snippets = [
            "fn f() { x.foo(); }",
            "fn f() { Foo::bar(); }",
            "use a::{b as c, d::*};",
            "trait W { fn write(&self); fn flush(&self) {} }",
            "impl W for File { fn write(&self) {} }",
            "fn a(w: &dyn W) {}",
            "fn b(w: Box<dyn W>) {}",
            "fn c<T: W>(t: T) {}",
            "fn d(t: impl W) {}",
            "fn e<T>(t: T) where T: W {}",
        ];
        for src in snippets {
            let tree = super::parse(src.as_bytes()).unwrap();
            println!("--- {}\n{}\n", src, tree.root_node().to_sexp());
        }
    }
}
