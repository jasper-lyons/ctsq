//! Go resolver: definition indexing, file scope model, and call resolution.
//!
//! Resolution is heuristic, not full type inference. Call returns are
//! followed through indexed return types (`x := makeServer(); x.Run()`, and
//! `x, err := f()` binds the first result), falling back to the `NewType()`
//! naming convention. Interface dispatch is CHA-lite: a call through an
//! interface-typed receiver expands to one edge per project type whose
//! method set covers the interface. Out of scope (calls fall back to ranked
//! name matching): embedded interfaces/structs, struct-field receivers
//! (`s.client.Do()` is an opaque receiver), and plain reassignment
//! (`x = ...` after the declaration).
//!
//! Go-specific shape: `pkg.Fn()` and `x.Method()` are the same syntax
//! (`selector_expression`), so call sites are classified as `Method` with a
//! `Var` receiver and disambiguated at resolve time — local bindings and
//! params first (they shadow package names), then the import map.

use super::{Callee, Confidence, Def, DefId, DefIndex, DefKind, Receiver, ResolvedCall, Resolver};
use std::collections::HashMap;
use tree_sitter::Node;

#[derive(Default)]
pub struct GoResolver;

impl Resolver for GoResolver {
    fn collect_defs(&self, file: &str, source: &[u8]) -> Vec<Def> {
        let tree = match parse(source) { Some(t) => t, None => return vec![] };
        let pkg = file_package_path(file);
        let mut out = Vec::new();
        let mut cursor = tree.root_node().walk();
        for child in tree.root_node().children(&mut cursor) {
            match child.kind() {
                "function_declaration" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let name = text(name_node, source).to_string();
                        let mut qualified = pkg.clone();
                        qualified.push(name.clone());
                        out.push(Def {
                            name, qualified, kind: DefKind::Function,
                            ret: return_type(child, source),
                            id: DefId { file: file.to_string(), line: child.start_position().row + 1 },
                        });
                    }
                }
                "method_declaration" => {
                    let (Some(name_node), Some(recv)) =
                        (child.child_by_field_name("name"), receiver_type(child, source))
                    else { continue };
                    let name = text(name_node, source).to_string();
                    let mut qualified = pkg.clone();
                    qualified.push(recv.clone());
                    qualified.push(name.clone());
                    out.push(Def {
                        name, qualified, kind: DefKind::Method { receiver: recv, via_trait: None },
                        ret: return_type(child, source),
                        id: DefId { file: file.to_string(), line: child.start_position().row + 1 },
                    });
                }
                "type_declaration" => collect_interfaces(child, source, file, &pkg, &mut out),
                _ => {}
            }
        }
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
                // interface dispatch can yield several targets → one edge each
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
    parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
    parser.parse(source, None)
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

// ── Definition indexing ───────────────────────────────────────────────────────

/// Package path implied by the file's location: its directory components.
/// Import paths are matched against this by suffix overlap, so leading
/// components outside the module root are harmless. Heuristic — no go.mod
/// awareness.
fn file_package_path(file: &str) -> Vec<String> {
    std::path::Path::new(file).parent()
        .map(|d| d.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => s.to_str().map(String::from),
                _ => None,
            })
            .collect())
        .unwrap_or_default()
}

/// Receiver type of a method_declaration: `(s *Server)` → "Server",
/// `(s *Server[T])` → "Server".
fn receiver_type(method: Node, source: &[u8]) -> Option<String> {
    let recv = method.child_by_field_name("receiver")?;
    let mut cursor = recv.walk();
    let param = recv.named_children(&mut cursor).find(|c| c.kind() == "parameter_declaration")?;
    type_name(param.child_by_field_name("type")?, source)
}

/// First return type of a function/method: `func f() *Server` → "Server",
/// `func f() (*Server, error)` → "Server" (multi-results are positional and
/// only the first is bound by `x, err := f()`).
fn return_type(decl: Node, source: &[u8]) -> Option<String> {
    let result = decl.child_by_field_name("result")?;
    if result.kind() == "parameter_list" {
        let mut cursor = result.walk();
        let first = result.named_children(&mut cursor)
            .find(|c| c.kind() == "parameter_declaration")?;
        return type_name(first.child_by_field_name("type")?, source);
    }
    type_name(result, source)
}

/// Index each method signature of `type I interface { ... }` as an
/// `InterfaceMethod` def. Embedded interfaces (`io.Reader` inside the body)
/// are not expanded.
fn collect_interfaces(decl: Node, source: &[u8], file: &str, pkg: &[String], out: &mut Vec<Def>) {
    let mut cursor = decl.walk();
    for spec in decl.named_children(&mut cursor) {
        if spec.kind() != "type_spec" { continue; }
        let (Some(name_node), Some(body)) =
            (spec.child_by_field_name("name"), spec.child_by_field_name("type"))
        else { continue };
        if body.kind() != "interface_type" { continue; }
        let iface = text(name_node, source).to_string();
        let mut c = body.walk();
        for elem in body.named_children(&mut c) {
            if elem.kind() != "method_elem" { continue; }
            let Some(m) = elem.child_by_field_name("name") else { continue };
            let name = text(m, source).to_string();
            let mut qualified = pkg.to_vec();
            qualified.push(iface.clone());
            qualified.push(name.clone());
            out.push(Def {
                name, qualified,
                kind: DefKind::InterfaceMethod { interface: iface.clone() },
                ret: return_type(elem, source),
                id: DefId { file: file.to_string(), line: elem.start_position().row + 1 },
            });
        }
    }
}

/// Strip pointers and type arguments down to the base type name:
/// `*Server[T]` → `Server`, `pkg.Foo` → `Foo`.
fn type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, source).to_string()),
        "pointer_type" => node.named_child(0).and_then(|t| type_name(t, source)),
        "generic_type" => node.child_by_field_name("type").and_then(|t| type_name(t, source)),
        "qualified_type" => node.child_by_field_name("name").map(|n| text(n, source).to_string()),
        _ => None,
    }
}

// ── File scope model ──────────────────────────────────────────────────────────

#[derive(Debug)]
enum Binding {
    /// `f := helper` / `g := x.Close` — variable holds a function value.
    FnValue(Callee),
    /// `var x Server` / `x := &Server{}`.
    Typed(String),
    /// `x := f()` / `x, err := pkg.F()` — type comes from the callee's
    /// return type at resolve time (or the `NewT` naming convention).
    Call(Callee),
}

#[derive(Debug)]
struct FnScope {
    /// Start line of the declaration — matches `Def.id.line`.
    line: usize,
    /// (var, type) pairs: the method receiver (if any) plus the parameter list.
    params: Vec<(String, String)>,
    /// (line, var, binding) in document order.
    bindings: Vec<(usize, String, Binding)>,
    /// (callee, call-site line) in document order.
    calls: Vec<(Callee, usize)>,
}

#[derive(Debug)]
struct FileScope {
    /// local package name (alias included) → import path segments.
    imports: HashMap<String, Vec<String>>,
    /// `import . "path"` dot-import path segments.
    globs: Vec<Vec<String>>,
    fns: Vec<FnScope>,
}

impl FileScope {
    fn build(root: Node, source: &[u8]) -> FileScope {
        let mut s = FileScope { imports: HashMap::new(), globs: vec![], fns: vec![] };
        let mut fn_stack = Vec::new();
        walk_scope(root, source, &mut s, &mut fn_stack);
        s
    }
}

fn walk_scope(node: Node, source: &[u8], s: &mut FileScope, fn_stack: &mut Vec<usize>) {
    match node.kind() {
        "import_declaration" => {
            collect_imports(node, source, s);
            return;
        }
        "function_declaration" | "method_declaration" => {
            let mut params = Vec::new();
            if node.kind() == "method_declaration" {
                if let Some(recv) = node.child_by_field_name("receiver") {
                    collect_param_list(recv, source, &mut params);
                }
            }
            if let Some(list) = node.child_by_field_name("parameters") {
                collect_param_list(list, source, &mut params);
            }
            let idx = s.fns.len();
            s.fns.push(FnScope {
                line: node.start_position().row + 1,
                params,
                bindings: vec![],
                calls: vec![],
            });
            fn_stack.push(idx);
            if let Some(body) = node.child_by_field_name("body") {
                walk_children(body, source, s, fn_stack);
            }
            fn_stack.pop();
            return;
        }
        "short_var_declaration" => {
            if let Some(&f) = fn_stack.last() {
                if let Some((var, b)) = classify_short_var(node, source) {
                    s.fns[f].bindings.push((node.start_position().row + 1, var, b));
                }
            }
            // fall through: the value expression may contain calls
        }
        "var_spec" => {
            if let Some(&f) = fn_stack.last() {
                for (var, b) in classify_var_spec(node, source) {
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
    walk_children(node, source, s, fn_stack);
}

fn walk_children(node: Node, source: &[u8], s: &mut FileScope, fn_stack: &mut Vec<usize>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_scope(child, source, s, fn_stack);
    }
}

/// Collect `import_spec`s: plain imports key the path's last segment, aliases
/// key the alias, `.` imports go to `globs`, `_` imports are dropped.
fn collect_imports(node: Node, source: &[u8], s: &mut FileScope) {
    let mut cursor = node.walk();
    let mut stack: Vec<Node> = node.named_children(&mut cursor).collect();
    while let Some(n) = stack.pop() {
        match n.kind() {
            "import_spec_list" => {
                let mut c = n.walk();
                stack.extend(n.named_children(&mut c));
            }
            "import_spec" => {
                let Some(path_node) = n.child_by_field_name("path") else { continue };
                let path: Vec<String> = text(path_node, source)
                    .trim_matches('"')
                    .split('/')
                    .filter(|seg| !seg.is_empty())
                    .map(String::from)
                    .collect();
                if path.is_empty() { continue; }
                match n.child_by_field_name("name") {
                    Some(name) if name.kind() == "dot" => s.globs.push(path),
                    Some(name) if name.kind() == "blank_identifier" => {}
                    Some(name) => { s.imports.insert(text(name, source).to_string(), path); }
                    None => { s.imports.insert(path.last().unwrap().clone(), path); }
                }
            }
            _ => {}
        }
    }
}

/// Append (name, type) pairs from a parameter_list; `a, b int` repeats the
/// name field, variadic/unnamed parameters are skipped.
fn collect_param_list(list: Node, source: &[u8], out: &mut Vec<(String, String)>) {
    let mut cursor = list.walk();
    for p in list.named_children(&mut cursor) {
        if p.kind() != "parameter_declaration" { continue; }
        let Some(ty) = p.child_by_field_name("type").and_then(|t| type_name(t, source))
        else { continue };
        let mut names = p.walk();
        for name in p.children_by_field_name("name", &mut names) {
            if name.kind() == "identifier" {
                out.push((text(name, source).to_string(), ty.clone()));
            }
        }
    }
}

/// `x := expr`, plus the first variable of `x, err := f()` — only the first
/// result type is tracked, so the remaining variables stay untyped.
fn classify_short_var(node: Node, source: &[u8]) -> Option<(String, Binding)> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    if right.named_child_count() != 1 { return None; }
    let value = right.named_child(0)?;
    if left.named_child_count() > 1 && value.kind() != "call_expression" { return None; }
    let var = left.named_child(0)?;
    if var.kind() != "identifier" { return None; }
    classify_value(value, source)
        .map(|b| (text(var, source).to_string(), b))
}

/// `var x Server` / `var a, b Server` / `var f = helper`.
fn classify_var_spec(node: Node, source: &[u8]) -> Vec<(String, Binding)> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for n in node.children_by_field_name("name", &mut cursor) {
        if n.kind() == "identifier" {
            names.push(text(n, source).to_string());
        }
    }
    if let Some(ty) = node.child_by_field_name("type").and_then(|t| type_name(t, source)) {
        return names.into_iter().map(|v| (v, Binding::Typed(ty.clone()))).collect();
    }
    // value bindings: only the single-name single-value form
    if names.len() == 1 {
        if let Some(value) = node.child_by_field_name("value")
            .filter(|v| v.named_child_count() == 1)
            .and_then(|v| v.named_child(0))
        {
            if let Some(b) = classify_value(value, source) {
                return vec![(names.pop().unwrap(), b)];
            }
        }
    }
    vec![]
}

fn classify_value(value: Node, source: &[u8]) -> Option<Binding> {
    match value.kind() {
        // `f := helper` / `g := x.Close` — function value, resolved like a call
        "identifier" | "selector_expression" =>
            classify_callee_expr(value, source).map(Binding::FnValue),
        "composite_literal" => {
            type_name(value.child_by_field_name("type")?, source).map(Binding::Typed)
        }
        // `&Server{...}`
        "unary_expression" => {
            classify_value(value.child_by_field_name("operand")?, source)
        }
        // `x := f()` — the callee's return type is looked up at resolve time
        "call_expression" => {
            classify_call(value, source).map(Binding::Call)
        }
        _ => None,
    }
}

/// Constructor naming convention: `NewServer` (or `pkg.NewServer`) returns a
/// `Server`. Fallback when the callee's definition isn't in the index.
fn new_convention_type(callee: &Callee) -> Option<String> {
    let ty = callee.name().strip_prefix("New")?;
    ty.chars().next()
        .filter(|c| c.is_uppercase())
        .map(|_| ty.to_string())
}

fn classify_call(node: Node, source: &[u8]) -> Option<Callee> {
    classify_callee_expr(node.child_by_field_name("function")?, source)
}

fn classify_callee_expr(f: Node, source: &[u8]) -> Option<Callee> {
    match f.kind() {
        "identifier" => Some(Callee::Bare(text(f, source).to_string())),
        // `pkg.Fn()` and `x.Method()` are syntactically identical; classify as
        // a method with a Var receiver and let resolution disambiguate.
        "selector_expression" => {
            let name = text(f.child_by_field_name("field")?, source).to_string();
            let operand = f.child_by_field_name("operand")?;
            let receiver = match operand.kind() {
                "identifier" => Receiver::Var(text(operand, source).to_string()),
                _ => Receiver::Opaque(
                    text(operand, source).lines().next().unwrap_or("").to_string(),
                ),
            };
            Some(Callee::Method { receiver, name })
        }
        // `f[int]()` — generic instantiation
        "index_expression" | "generic_function" =>
            f.child_by_field_name("operand")
                .or_else(|| f.child_by_field_name("function"))
                .and_then(|inner| classify_callee_expr(inner, source)),
        "parenthesized_expression" =>
            f.named_child(0).and_then(|inner| classify_callee_expr(inner, source)),
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
/// one target except interface dispatch, which expands to one edge per
/// implementing method.
fn resolve_callee(callee: &Callee, line: usize, depth: usize, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    if depth > MAX_BINDING_DEPTH { return vec![]; }
    match callee {
        Callee::Path(segs) => {
            let Some((name, pkg)) = segs.split_last() else { return vec![] };
            resolve_pkg_fn(pkg, name, ctx).into_iter().collect()
        }
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
    // (b) function in the same file
    let same_file: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| {
            let d = &ctx.index.defs[i];
            d.id.file == ctx.file && matches!(d.kind, DefKind::Function)
        })
        .collect();
    if !same_file.is_empty() {
        return rank(same_file, false, ctx);
    }
    // (c) function in the same package (same directory)
    let dir = std::path::Path::new(ctx.file).parent();
    let same_pkg: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| {
            let d = &ctx.index.defs[i];
            matches!(d.kind, DefKind::Function)
                && std::path::Path::new(&d.id.file).parent() == dir
        })
        .collect();
    if !same_pkg.is_empty() {
        return rank(same_pkg, false, ctx);
    }
    // (d) dot imports
    let glob_cands: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| {
            let d = &ctx.index.defs[i];
            matches!(d.kind, DefKind::Function)
                && ctx.scope.globs.iter().any(|g| pkg_match(def_pkg(d), g))
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
        Receiver::Var(v) => {
            // local bindings and params shadow package names
            if let Some(ty) = var_type(v, line, depth, ctx) {
                if let Some(hit) = method_on(&ty, name, ctx) { return one(Some(hit)); }
                let impls = interface_impls(&ty, name, ctx);
                if !impls.is_empty() { return impls; }
            } else if let Some(path) = ctx.scope.imports.get(v) {
                // imported package: a function it doesn't define is
                // external — no name-only fallback, so fmt.Println()
                // never grabs an unrelated local Println
                return one(resolve_pkg_fn(path, name, ctx));
            } else if v.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                // method expression: Server.Run(s)
                if let Some(hit) = method_on(v, name, ctx) { return one(Some(hit)); }
                let impls = interface_impls(v, name, ctx);
                if !impls.is_empty() { return impls; }
            }
        }
        Receiver::Opaque(_) | Receiver::SelfRecv => {}
    }
    // fallback: any def with this name, methods preferred
    one(rank(name_candidates(ctx, name).collect(), true, ctx))
}

/// CHA-lite: a call through interface `iface` expands to `name` on every
/// project type whose method set covers all of the interface's methods. A
/// single implementor is exact; several are each a guess. With no
/// implementors in the project the edge points at the signature itself.
fn interface_impls(iface: &str, name: &str, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    let sig_methods: Vec<&str> = ctx.index.defs.iter()
        .filter(|d| matches!(&d.kind, DefKind::InterfaceMethod { interface } if interface == iface))
        .map(|d| d.name.as_str())
        .collect();
    if !sig_methods.contains(&name) { return vec![]; }

    let implements = |ty: &str| {
        sig_methods.iter().all(|m| {
            name_candidates(ctx, m).any(|i| matches!(
                &ctx.index.defs[i].kind,
                DefKind::Method { receiver, .. } if receiver == ty
            ))
        })
    };
    let mut impls: Vec<usize> = name_candidates(ctx, name)
        .filter(|&i| matches!(
            &ctx.index.defs[i].kind,
            DefKind::Method { receiver, .. } if implements(receiver)
        ))
        .collect();
    impls.sort_by(|&a, &b| {
        let (da, db) = (&ctx.index.defs[a].id, &ctx.index.defs[b].id);
        da.file.cmp(&db.file).then(da.line.cmp(&db.line))
    });
    match impls.len() {
        0 => one(name_candidates(ctx, name)
            .find(|&i| matches!(
                &ctx.index.defs[i].kind,
                DefKind::InterfaceMethod { interface } if interface == iface
            ))
            .map(|i| (i, Confidence::Exact))),
        1 => vec![(impls[0], Confidence::Exact)],
        _ => impls.into_iter().map(|i| (i, Confidence::Guess)).collect(),
    }
}

/// Resolve `pkg.Fn` where `pkg` maps to import path segments: functions named
/// `Fn` whose package directory overlaps the import path, keeping only the
/// deepest overlap (so `a/util` beats an unrelated `b/util`).
fn resolve_pkg_fn(path: &[String], name: &str, ctx: &Ctx) -> Option<(usize, Confidence)> {
    let scored: Vec<(usize, usize)> = name_candidates(ctx, name)
        .filter(|&i| matches!(ctx.index.defs[i].kind, DefKind::Function))
        .map(|i| (i, common_suffix_len(def_pkg(&ctx.index.defs[i]), path)))
        .filter(|&(_, s)| s > 0)
        .collect();
    let best = scored.iter().map(|&(_, s)| s).max()?;
    let cands: Vec<usize> = scored.into_iter()
        .filter(|&(_, s)| s == best)
        .map(|(i, _)| i)
        .collect();
    rank(cands, false, ctx)
}

/// Package (directory) part of a def's qualified path: everything before the
/// receiver + name.
fn def_pkg(d: &Def) -> &[String] {
    let tail = match d.kind {
        DefKind::Method { .. } | DefKind::InterfaceMethod { .. } => 2,
        DefKind::Function => 1,
    };
    &d.qualified[..d.qualified.len().saturating_sub(tail)]
}

/// Directory components vs import path segments: defs know their on-disk path
/// ("/tmp/demo/server") while imports carry the module path
/// ("example.com/demo/server") — only the tails agree, so match on any shared
/// suffix. `resolve_pkg_fn` prefers deeper overlaps when several packages
/// share a directory name.
fn pkg_match(dirs: &[String], import_path: &[String]) -> bool {
    common_suffix_len(dirs, import_path) > 0
}

fn common_suffix_len(a: &[String], b: &[String]) -> usize {
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
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

/// Type of `var` at `line`: last typed binding — for `x := f()` the resolved
/// callee's return type, else the `NewT` convention — else receiver or
/// parameter type.
fn var_type(var: &str, line: usize, depth: usize, ctx: &Ctx) -> Option<String> {
    match last_binding(ctx.fun, var, line) {
        Some(Binding::Typed(t)) => return Some(t.clone()),
        Some(Binding::Call(callee)) => {
            if let Some(&(t, _)) = resolve_callee(callee, line, depth + 1, ctx).first() {
                if let Some(ret) = &ctx.index.defs[t].ret {
                    return Some(ret.clone());
                }
            }
            return new_convention_type(callee);
        }
        Some(Binding::FnValue(_)) | None => {}
    }
    ctx.fun.params.iter().find(|(v, _)| v == var).map(|(_, t)| t.clone())
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

/// Whether the def's package is reachable through this file's imports or dot
/// imports.
fn is_imported(d: &Def, ctx: &Ctx) -> bool {
    let pkg = def_pkg(d);
    ctx.scope.imports.values().chain(&ctx.scope.globs).any(|p| pkg_match(pkg, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Confidence, DefIndex, Resolver};

    fn build_index(files: &[(&str, &str)]) -> DefIndex {
        let r = GoResolver;
        let mut idx = DefIndex::default();
        for (f, src) in files {
            for d in r.collect_defs(f, src.as_bytes()) {
                idx.push(d);
            }
        }
        idx
    }

    // Resolve everything and return (caller_name, callee_display, target file:line, confidence).
    fn resolve_all(files: &[(&str, &str)]) -> Vec<(String, String, Option<(String, usize, Confidence)>)> {
        let r = GoResolver;
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
    fn defs_carry_package_path_and_receiver() {
        let idx = build_index(&[(
            "proj/util/geo.go",
            "package util\nfunc Free() {}\nfunc (f *Foo) Dist() {}\n",
        )]);
        let free = idx.defs.iter().find(|d| d.name == "Free").unwrap();
        assert_eq!(free.qualified, vec!["proj", "util", "Free"]);
        assert!(matches!(free.kind, DefKind::Function));
        let dist = idx.defs.iter().find(|d| d.name == "Dist").unwrap();
        assert_eq!(dist.qualified, vec!["proj", "util", "Foo", "Dist"]);
        assert!(matches!(&dist.kind, DefKind::Method { receiver, .. } if receiver == "Foo"));
    }

    #[test]
    fn imports_collect_aliases_dot_and_blank() {
        let src = "package main\nimport (\n  \"fmt\"\n  u \"proj/util\"\n  . \"proj/dot\"\n  _ \"side\"\n)\nimport \"single/pkg\"\n";
        let tree = parse(src.as_bytes()).unwrap();
        let scope = FileScope::build(tree.root_node(), src.as_bytes());
        assert_eq!(scope.imports.get("fmt").unwrap(), &vec!["fmt"]);
        assert_eq!(scope.imports.get("u").unwrap(), &vec!["proj", "util"]);
        assert_eq!(scope.imports.get("pkg").unwrap(), &vec!["single", "pkg"]);
        assert!(!scope.imports.contains_key("side"), "blank import is dropped");
        assert_eq!(scope.globs, vec![vec!["proj", "dot"]]);
    }

    #[test]
    fn same_file_bare_call_is_exact() {
        let results = resolve_all(&[
            ("a/a.go", "package a\nfunc helper() {}\nfunc Go() { helper() }\n"),
            ("b/b.go", "package b\nfunc helper() {}\n"),
        ]);
        let (file, line, conf) = target_of(&results, "Go", "helper").unwrap();
        assert_eq!((file.as_str(), line, conf), ("a/a.go", 2, Confidence::Exact));
    }

    #[test]
    fn same_package_cross_file_bare_call_is_exact() {
        let results = resolve_all(&[
            ("a/one.go", "package a\nfunc helper() {}\n"),
            ("a/two.go", "package a\nfunc Go() { helper() }\n"),
            ("b/b.go", "package b\nfunc helper() {}\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "helper").unwrap();
        assert_eq!((file.as_str(), conf), ("a/one.go", Confidence::Exact));
    }

    #[test]
    fn import_resolves_cross_package() {
        let results = resolve_all(&[
            ("proj/util/u.go", "package util\nfunc Helper() {}\n"),
            ("proj/other/o.go", "package other\nfunc Helper() {}\n"),
            ("proj/main.go", "package main\nimport \"example.com/proj/util\"\nfunc Go() { util.Helper() }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), conf), ("proj/util/u.go", Confidence::Exact));
    }

    #[test]
    fn import_matches_when_only_path_tails_agree() {
        // on-disk path "/tmp/demo/util" vs module path "example.com/demo/util"
        let results = resolve_all(&[
            ("/tmp/demo/util/u.go", "package util\nfunc Helper() {}\n"),
            ("/tmp/demo/main.go", "package main\nimport \"example.com/demo/util\"\nfunc Go() { util.Helper() }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), conf), ("/tmp/demo/util/u.go", Confidence::Exact));
    }

    #[test]
    fn deeper_path_overlap_beats_shared_directory_name() {
        let results = resolve_all(&[
            ("proj/a/util/u.go", "package util\nfunc Helper() {}\n"),
            ("proj/b/util/u.go", "package util\nfunc Helper() {}\n"),
            ("proj/main.go", "package main\nimport \"example.com/proj/a/util\"\nfunc Go() { util.Helper() }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), conf), ("proj/a/util/u.go", Confidence::Exact));
    }

    #[test]
    fn alias_import_resolves() {
        let results = resolve_all(&[
            ("proj/util/u.go", "package util\nfunc Helper() {}\n"),
            ("proj/main.go", "package main\nimport u \"example.com/proj/util\"\nfunc Go() { u.Helper() }\n"),
        ]);
        let (file, line, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), line, conf), ("proj/util/u.go", 2, Confidence::Exact));
    }

    #[test]
    fn dot_import_resolves_bare_call() {
        let results = resolve_all(&[
            ("proj/util/u.go", "package util\nfunc Helper() {}\n"),
            ("proj/other/o.go", "package other\nfunc Helper() {}\n"),
            ("proj/main.go", "package main\nimport . \"example.com/proj/util\"\nfunc Go() { Helper() }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), conf), ("proj/util/u.go", Confidence::Exact));
    }

    #[test]
    fn external_package_call_stays_unresolved() {
        let results = resolve_all(&[(
            "proj/main.go",
            "package main\nimport \"fmt\"\nfunc Println() {}\nfunc Go() { fmt.Println() }\n",
        )]);
        assert_eq!(target_of(&results, "Go", "Println"), None,
            "fmt.Println must not resolve to the local Println");
    }

    #[test]
    fn value_binding_resolves_through_variable() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\nfunc helper() {}\nfunc Go() {\n  f := helper\n  f()\n}\n",
        )]);
        let (file, line, conf) = target_of(&results, "Go", "f").unwrap();
        assert_eq!((file.as_str(), line, conf), ("a/a.go", 2, Confidence::Exact));
    }

    #[test]
    fn value_binding_shadowing_last_wins() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\nfunc one() {}\nfunc two() {}\nfunc Go() {\n  f := one\n  f = two\n  g := two\n  g()\n  f()\n}\n",
        )]);
        // `f = two` is a plain assignment (out of scope) — f still resolves to one
        let (_, line, _) = target_of(&results, "Go", "g").unwrap();
        assert_eq!(line, 3);
        let (_, line, _) = target_of(&results, "Go", "f").unwrap();
        assert_eq!(line, 2, "declaration binding wins; reassignment is out of scope");
    }

    #[test]
    fn method_value_binding_resolves() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\nfunc (s *S) Close() {}\nfunc Go(s *S) {\n  f := s.Close\n  f()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "f").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact));
    }

    #[test]
    fn receiver_param_resolves_method() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\nfunc (s *S) A() { s.B() }\nfunc (s *S) B() {}\nfunc B() {}\n",
        )]);
        let (_, line, conf) = target_of(&results, "A", "B").unwrap();
        assert_eq!((line, conf), (4, Confidence::Exact), "should pick (*S).B, not free B");
    }

    #[test]
    fn var_declaration_type_resolves_method() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc (s S) Run() {}\nfunc (t T) Run() {}\nfunc Go() {\n  var x S\n  x.Run()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (4, Confidence::Exact));
    }

    #[test]
    fn composite_literal_resolves_method() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc (s *S) Run() {}\nfunc (t *T) Run() {}\nfunc Go() {\n  x := &S{}\n  x.Run()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (4, Confidence::Exact));
    }

    #[test]
    fn new_constructor_convention_gives_type_evidence() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc NewS() *S { return &S{} }\nfunc (s *S) Run() {}\nfunc (t *T) Run() {}\nfunc Go() {\n  x := NewS()\n  x.Run()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (5, Confidence::Exact));
    }

    #[test]
    fn return_type_resolves_through_call_binding() {
        // no New prefix — only the indexed return type can explain this
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc makeIt() *S { return &S{} }\nfunc (s *S) Run() {}\nfunc (t *T) Run() {}\nfunc Go() {\n  x := makeIt()\n  x.Run()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (5, Confidence::Exact));
    }

    #[test]
    fn multi_assign_binds_first_result_type() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc open() (*S, error) { return nil, nil }\nfunc (s *S) Run() {}\nfunc (t *T) Run() {}\nfunc Go() {\n  x, err := open()\n  _ = err\n  x.Run()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (5, Confidence::Exact));
    }

    #[test]
    fn return_type_resolves_cross_package() {
        let results = resolve_all(&[
            ("proj/server/s.go",
             "package server\ntype Server struct{}\nfunc Open() *Server { return nil }\nfunc (s *Server) Run() {}\n"),
            ("proj/other/o.go", "package other\ntype Other struct{}\nfunc (o *Other) Run() {}\n"),
            ("proj/main.go",
             "package main\nimport \"example.com/proj/server\"\nfunc Go() {\n  s := server.Open()\n  s.Run()\n}\n"),
        ]);
        let (file, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((file.as_str(), line, conf), ("proj/server/s.go", 4, Confidence::Exact));
    }

    #[test]
    fn interfaces_index_as_interface_methods() {
        let idx = build_index(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write(p []byte) (int, error)\n  Close() error\n}\n",
        )]);
        let write = idx.defs.iter().find(|d| d.name == "Write").unwrap();
        assert!(matches!(&write.kind, DefKind::InterfaceMethod { interface } if interface == "Writer"));
        assert_eq!(write.qualified, vec!["a", "Writer", "Write"]);
        assert_eq!(write.id.line, 3);
        assert!(idx.defs.iter().any(|d| d.name == "Close"));
    }

    #[test]
    fn interface_dispatch_expands_to_implementations() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write()\n}\ntype File struct{}\nfunc (f *File) Write() {}\ntype Buf struct{}\nfunc (b *Buf) Write() {}\ntype NotWriter struct{}\nfunc (n *NotWriter) Other() {}\nfunc Save(w Writer) {\n  w.Write()\n}\n",
        )]);
        let targets: Vec<_> = results.iter()
            .filter(|(cr, ce, _)| cr == "Save" && ce == "Write")
            .filter_map(|(_, _, t)| t.clone())
            .collect();
        assert_eq!(targets.len(), 2, "one edge per implementing type: {:?}", targets);
        assert!(targets.iter().all(|(_, _, c)| *c == Confidence::Guess));
        let lines: Vec<usize> = targets.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(lines, vec![6, 8], "File.Write and Buf.Write, not the signature");
    }

    #[test]
    fn interface_dispatch_single_impl_is_exact() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write()\n}\ntype File struct{}\nfunc (f *File) Write() {}\nfunc Save(w Writer) {\n  w.Write()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn interface_dispatch_requires_full_method_set() {
        // Half implements only one of Writer's two methods — not an implementor.
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write()\n  Close()\n}\ntype Full struct{}\nfunc (f *Full) Write() {}\nfunc (f *Full) Close() {}\ntype Half struct{}\nfunc (h *Half) Write() {}\nfunc Save(w Writer) {\n  w.Write()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (7, Confidence::Exact), "only Full implements Writer");
    }

    #[test]
    fn interface_with_no_impls_resolves_to_signature() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write()\n}\nfunc Save(w Writer) {\n  w.Write()\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (3, Confidence::Exact), "edge points at the signature");
    }

    #[test]
    fn param_type_resolves_method() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype S struct{}\ntype T struct{}\nfunc (s *S) Run() {}\nfunc (t *T) Run() {}\nfunc Go(x *S, n int) { x.Run() }\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((line, conf), (4, Confidence::Exact));
    }

    #[test]
    fn local_binding_shadows_package_name() {
        let results = resolve_all(&[
            ("proj/util/u.go", "package util\nfunc Run() {}\n"),
            ("proj/main.go",
             "package main\nimport \"example.com/proj/util\"\ntype S struct{}\nfunc (s *S) Run() {}\nfunc Go() {\n  util := &S{}\n  util.Run()\n}\n"),
        ]);
        let (file, line, conf) = target_of(&results, "Go", "Run").unwrap();
        assert_eq!((file.as_str(), line, conf), ("proj/main.go", 4, Confidence::Exact),
            "local variable shadows the imported package");
    }

    #[test]
    fn calls_in_go_defer_and_closures_are_attributed() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\nfunc run() {}\nfunc cleanup() {}\nfunc inner() {}\nfunc Go() {\n  go run()\n  defer cleanup()\n  f := func() { inner() }\n  f()\n}\n",
        )]);
        assert!(target_of(&results, "Go", "run").is_some());
        assert!(target_of(&results, "Go", "cleanup").is_some());
        assert!(target_of(&results, "Go", "inner").is_some(),
            "calls inside func literals attribute to the enclosing function");
    }

    #[test]
    fn generic_call_is_extracted() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\nfunc Map() {}\nfunc Go() { Map[int]() }\n",
        )]);
        let (_, line, conf) = target_of(&results, "Go", "Map").unwrap();
        assert_eq!((line, conf), (2, Confidence::Exact));
    }

    #[test]
    fn ambiguous_name_ranks_same_dir_and_marks_guess() {
        let results = resolve_all(&[
            ("a/near.go", "package a\nfunc Helper() {}\n"),
            ("far/far.go", "package far\nfunc Helper() {}\n"),
            ("a/main.go", "package a\nfunc Go() { x.Helper() }\n"),
        ]);
        let (file, _, conf) = target_of(&results, "Go", "Helper").unwrap();
        assert_eq!((file.as_str(), conf), ("a/near.go", Confidence::Guess));
    }

    #[test]
    fn zero_candidates_stay_unresolved() {
        let results = resolve_all(&[("a/a.go", "package a\nfunc Go() { println() }\n")]);
        assert_eq!(target_of(&results, "Go", "println"), None);
    }
}

#[cfg(test)]
mod spike {
    // Spike: dump the grammar's actual node kinds for the constructs the
    // resolver depends on, so the walker code above is built on verified names.
    #[test]
    fn dump_node_kinds() {
        let snippets = [
            "package main\nimport (\n  \"fmt\"\n  u \"myproj/util\"\n  . \"myproj/dot\"\n  _ \"side\"\n)",
            "package main\nfunc (s *Server) Handle(w http.ResponseWriter, n int) { s.log(); helper(); u.Helper() }",
            "package main\nfunc main() {\n  x := NewServer()\n  var y Server\n  z := &Server{}\n  f := helper\n  f()\n  go run()\n  defer cleanup()\n}",
            "package main\ntype Writer interface {\n  Write(p []byte) (int, error)\n  Close() error\n  io.Reader\n}",
            "package main\nfunc One() *Server { return nil }\nfunc Two() (int, error) { return 0, nil }\nfunc Three() (n int, err error) { return }",
            "package main\nfunc main() {\n  x, err := NewServer()\n  _ = err\n  x.Run()\n}",
        ];
        for src in snippets {
            let tree = super::parse(src.as_bytes()).unwrap();
            println!("--- {}\n{}\n", src, tree.root_node().to_sexp());
        }
    }
}
