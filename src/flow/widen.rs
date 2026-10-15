//! Implementations of a module interface that live outside the module
//! (decision 14). Best effort: a type from another package counts when a
//! literal of it is handed straight to something typed with the interface,
//! as `var s Store = &pkg.T{}` or `use(&pkg.T{})` where `use` takes a `Store`.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::resolve::go::{go_imports, go_signatures, GoSignature};
use crate::resolve::{DefIndex, DefKind};

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

/// Outside implementations by interface name, for every interface of the
/// module. `files` are the Go files as (path, root, source).
pub fn outside_impls(
    files: &[(&str, Node, &[u8])],
    index: &DefIndex,
    module: Option<&str>,
) -> HashMap<String, Vec<OutsideImpl>> {
    let ifaces: HashSet<&str> = index
        .defs
        .iter()
        .filter_map(|d| match &d.kind {
            DefKind::InterfaceMethod { interface } => Some(interface.as_str()),
            _ => None,
        })
        .collect();
    let mut out: HashMap<String, Vec<OutsideImpl>> = HashMap::new();
    if ifaces.is_empty() {
        return out;
    }
    let mut sigs: HashMap<&str, Vec<GoSignature>> = HashMap::new();
    for &(f, root, src) in files {
        sigs.insert(f, go_signatures(root, src));
    }
    for &(file, root, src) in files {
        let imports = go_imports(root, src);
        if imports.is_empty() {
            continue;
        }
        let mut add = |iface: &str, lit: Node, ty: String| {
            let entry = OutsideImpl {
                ty,
                file: file.to_string(),
                line: lit.start_position().row + 1,
                text: text(lit, src).lines().next().unwrap_or("").trim().to_string(),
            };
            out.entry(iface.to_string()).or_default().push(entry);
        };
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "var_spec" => {
                    let iface = n.child_by_field_name("type").map(|t| base(text(t, src)).to_string());
                    if let (Some(iface), Some(values)) = (iface, n.child_by_field_name("value")) {
                        if ifaces.contains(iface.as_str()) {
                            let mut c = values.walk();
                            for v in values.named_children(&mut c) {
                                if let Some(ty) = outside_literal(v, src, &imports, module) {
                                    add(&iface, v, ty);
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
                    if let Some(name) = name {
                        for &d in index.by_name.get(&name).map(|v| v.as_slice()).unwrap_or(&[]) {
                            let def = &index.defs[d];
                            if !matches!(def.kind, DefKind::Function) {
                                continue;
                            }
                            let Some(sig) = sigs.get(def.id.file.as_str()).and_then(|l| l.iter().find(|s| s.line == def.id.line))
                            else {
                                continue;
                            };
                            for (i, a) in args.iter().enumerate() {
                                let Some(ty) = outside_literal(*a, src, &imports, module) else { continue };
                                let Some(p) = sig.params.get(i) else { continue };
                                if ifaces.contains(base(&p.ty)) {
                                    add(base(&p.ty), *a, ty);
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
    for list in out.values_mut() {
        list.sort_by(|a, b| (&a.ty, &a.file, a.line).cmp(&(&b.ty, &b.file, b.line)));
        list.dedup_by(|a, b| a.ty == b.ty);
    }
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
