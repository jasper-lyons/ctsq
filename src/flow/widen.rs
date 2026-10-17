//! Implementations of a module interface that live outside the module
//! (decision 14). Best effort: a type from another package counts when a
//! literal of it is handed straight to something typed with the interface,
//! as `var s Store = &pkg.T{}` or `use(&pkg.T{})` where `use` takes a `Store`.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::resolve::go::{go_imports, go_signatures, GoSignature};
use crate::resolve::scoped::FlowGraph;
use crate::resolve::{DefKind, TreeStore};

/// One outside type and the place the module builds it for the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideImpl {
    /// `bytes.Buffer`, as written.
    pub ty: String,
    pub file: String,
    pub line: usize,
    pub text: String,
}

fn text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

/// `&pkg.T{}` or `pkg.T{}` where `pkg` is an import outside `module`: the
/// type as written.
fn outside_literal(n: Node, src: &[u8], imports: &HashMap<String, String>, module: Option<&str>) -> Option<String> {
    let n = if n.kind() == "unary_expression" { n.child_by_field_name("operand")? } else { n };
    if n.kind() != "composite_literal" {
        return None;
    }
    let ty = n.child_by_field_name("type").filter(|t| t.kind() == "qualified_type")?;
    let pkg = text(ty.child_by_field_name("package")?, src);
    let path = imports.get(pkg)?;
    if module.is_some_and(|m| path == m || path.starts_with(&format!("{}/", m))) {
        return None;
    }
    Some(text(ty, src).to_string())
}

/// Base name of a parameter type: `*Store` and `pkg.Store` give `Store`.
fn base(ty: &str) -> &str {
    let t = ty.trim_start_matches('*').trim_start_matches("...");
    t.rsplit('.').next().unwrap_or(t)
}

/// Outside implementations of the module interface `iface`, sorted by type,
/// one per type. Only files that can add one are read: a `var` typed with
/// the interface holds its name, and so does the file declaring a function
/// that takes it, whose name the calling file holds.
pub fn outside_impls_for(iface: &str, store: &TreeStore, graph: &FlowGraph, module: Option<&str>) -> Vec<OutsideImpl> {
    let mut out: Vec<OutsideImpl> = vec![];
    if !graph.is_interface(iface) {
        return out;
    }
    let sigs_of = |f: &str| -> Vec<GoSignature> {
        match store.get(f) {
            Some(pf) if pf.lang == "go" => pf.tree.as_ref().map(|t| go_signatures(t.root_node(), &pf.source)).unwrap_or_default(),
            _ => vec![],
        }
    };
    // Functions with a parameter of the interface's type.
    let mut takers: Vec<String> = vec![];
    for (f, pf) in store.holding(&[iface]) {
        if pf.lang != "go" {
            continue;
        }
        for sig in sigs_of(f) {
            if sig.params.iter().any(|p| base(&p.ty) == iface) {
                if let Some(d) = graph.def_at(f, sig.line) {
                    let d = graph.def(d);
                    if matches!(d.kind, DefKind::Function) && !takers.contains(&d.name) {
                        takers.push(d.name.clone());
                    }
                }
            }
        }
    }
    let mut sigs: HashMap<String, Vec<GoSignature>> = HashMap::new();
    let mut names: Vec<&str> = vec![iface];
    names.extend(takers.iter().map(|t| t.as_str()));
    for (file, pf) in store.holding(&names) {
        if pf.lang != "go" {
            continue;
        }
        let Some(tree) = &pf.tree else { continue };
        let (root, src) = (tree.root_node(), pf.source.as_slice());
        let imports = go_imports(root, src);
        if imports.is_empty() {
            continue;
        }
        let mut add = |lit: Node, ty: String| {
            out.push(OutsideImpl {
                ty,
                file: file.to_string(),
                line: lit.start_position().row + 1,
                text: text(lit, src).lines().next().unwrap_or("").trim().to_string(),
            });
        };
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "var_spec" => {
                    let typed = n.child_by_field_name("type").map(|t| base(text(t, src)).to_string());
                    if let (Some(typed), Some(values)) = (typed, n.child_by_field_name("value")) {
                        if typed == iface {
                            let mut c = values.walk();
                            for v in values.named_children(&mut c) {
                                if let Some(ty) = outside_literal(v, src, &imports, module) {
                                    add(v, ty);
                                }
                            }
                        }
                    }
                }
                "call_expression" => {
                    let name = n.child_by_field_name("function").map(|f| {
                        let f = f.child_by_field_name("field").unwrap_or(f);
                        text(f, src).to_string()
                    });
                    let args: Vec<Node> = n
                        .child_by_field_name("arguments")
                        .map(|a| {
                            let mut c = a.walk();
                            a.named_children(&mut c).filter(|x| x.kind() != "comment").collect()
                        })
                        .unwrap_or_default();
                    // Only a function that takes the interface adds to its list.
                    if let Some(name) = name.filter(|n| takers.contains(n)) {
                        for d in graph.by_name(&name) {
                            let def = graph.def(d);
                            if !matches!(def.kind, DefKind::Function) {
                                continue;
                            }
                            let list = sigs.entry(def.id.file.clone()).or_insert_with(|| sigs_of(&def.id.file));
                            let Some(sig) = list.iter().find(|s| s.line == def.id.line) else { continue };
                            for (i, a) in args.iter().enumerate() {
                                let Some(ty) = outside_literal(*a, src, &imports, module) else { continue };
                                let Some(p) = sig.params.get(i) else { continue };
                                if base(&p.ty) == iface {
                                    add(*a, ty);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            let mut c = n.walk();
            stack.extend(n.children(&mut c));
        }
    }
    out.sort_by(|a, b| (&a.ty, &a.file, a.line).cmp(&(&b.ty, &b.file, b.line)));
    out.dedup_by(|a, b| a.ty == b.ty);
    out
}

/// The tag a widened call row carries.
pub fn widened_tag(in_repo: usize, outside: usize) -> String {
    if outside == 0 {
        "widened".to_string()
    } else {
        format!("widened: {} in repo, {} outside", in_repo, outside)
    }
}
