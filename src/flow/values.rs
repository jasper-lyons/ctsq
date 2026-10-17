//! Value sets for Go string expressions (plan decision 25). A literal adds
//! one value, a constant adds its value, a parameter adds the union over
//! every in-repo caller's argument, a `+` concatenation of known and unknown
//! parts becomes a glob (`admin-*.html.tmpl`) and anything else marks the set
//! open with a reason. Local variables add the values of their reaching
//! assignments, the walk `flow::upward::reaching` gives.

use std::cell::RefCell;
use std::path::Path;

use tree_sitter::Node;

use super::upward::{reaching, Def, Up};
use crate::resolve::scoped::FlowGraph;
use crate::resolve::TreeStore;

/// The strings an expression can hold, each with the file and line it was
/// written at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueSet {
    pub values: Vec<(String, String, usize)>,
    /// Patterns with `*` for each unknown part of a concatenation.
    pub globs: Vec<(String, String, usize)>,
    /// Some value could not be known.
    pub open: bool,
    /// Why the set is open, one entry per unknown part.
    pub reasons: Vec<String>,
}

impl ValueSet {
    fn join(&mut self, o: ValueSet) {
        for v in o.values {
            if !self.values.contains(&v) {
                self.values.push(v);
            }
        }
        for g in o.globs {
            if !self.globs.contains(&g) {
                self.globs.push(g);
            }
        }
        self.open |= o.open;
        for r in o.reasons {
            if !self.reasons.contains(&r) {
                self.reasons.push(r);
            }
        }
    }

    fn open(reason: String) -> ValueSet {
        ValueSet { open: true, reasons: vec![reason], ..Default::default() }
    }

    /// The patterns this set stands for in a concatenation: its values and
    /// globs, or `*` when it is open or larger than `MAX_PRODUCT`.
    fn pieces(&self) -> Vec<String> {
        let n = self.values.len() + self.globs.len();
        if self.open || n == 0 || n > MAX_PRODUCT {
            return vec!["*".into()];
        }
        self.values.iter().chain(self.globs.iter()).map(|(v, _, _)| v.clone()).collect()
    }
}

/// Largest number of alternatives a concatenation multiplies out.
const MAX_PRODUCT: usize = 16;

/// The value set of string expression `node` in `file`.
pub fn value_set(store: &TreeStore, graph: &FlowGraph, file: &str, node: Node) -> ValueSet {
    let up = Up::new(store, graph, None, usize::MAX);
    let w = Walk { store, up, active: RefCell::new(vec![]) };
    w.expr(file, node)
}

struct Walk<'a> {
    store: &'a TreeStore,
    up: Up<'a>,
    /// (def, parameter position) being walked, against recursion.
    active: RefCell<Vec<(usize, usize)>>,
}

fn text<'s>(n: Node, src: &'s [u8]) -> &'s str {
    n.utf8_text(src).unwrap_or("")
}

fn line_of(n: Node) -> usize {
    n.start_position().row + 1
}

/// The text of a Go string literal without its quotes. Escapes in an
/// interpreted literal are kept as written.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && (s.starts_with('"') && s.ends_with('"') || s.starts_with('`') && s.ends_with('`')) {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

impl<'a> Walk<'a> {
    fn src(&self, file: &str) -> Option<(Node<'a>, &'a [u8])> {
        let pf = self.store.get(file)?;
        Some((pf.tree.as_ref()?.root_node(), pf.source.as_slice()))
    }

    fn here(&self, file: &str, n: Node) -> String {
        format!("{} at {}:{}", n.kind().replace('_', " "), file, line_of(n))
    }

    fn expr(&self, file: &str, n: Node) -> ValueSet {
        let Some((root, src)) = self.src(file) else { return ValueSet::open(format!("{} is not parsed", file)) };
        match n.kind() {
            "interpreted_string_literal" | "raw_string_literal" => ValueSet {
                values: vec![(unquote(text(n, src)), file.to_string(), line_of(n))],
                ..Default::default()
            },
            "parenthesized_expression" => match n.named_child(0) {
                Some(i) => self.expr(file, i),
                None => ValueSet::open(self.here(file, n)),
            },
            "identifier" => self.name(file, root, src, n),
            "binary_expression" => {
                let op = n.child_by_field_name("operator").map(|o| text(o, src)).unwrap_or("");
                if op != "+" {
                    return ValueSet::open(self.here(file, n));
                }
                self.concat(file, n)
            }
            _ => ValueSet::open(self.here(file, n)),
        }
    }

    /// `a + b`: every combination of the parts' pieces, an unknown part as
    /// `*`. A result with no `*` is a value, otherwise a glob. All unknown is
    /// open.
    fn concat(&self, file: &str, n: Node) -> ValueSet {
        let mut parts = vec![];
        let mut stack = vec![n];
        // Flatten the left-leaning `+` chain in order.
        while let Some(x) = stack.pop() {
            let is_plus = x.kind() == "binary_expression"
                && x.child_by_field_name("operator").is_some_and(|o| o.kind() == "+");
            if is_plus {
                if let (Some(l), Some(r)) = (x.child_by_field_name("left"), x.child_by_field_name("right")) {
                    stack.push(r);
                    stack.push(l);
                    continue;
                }
            }
            parts.push(x);
        }
        let mut reasons = vec![];
        let mut acc: Vec<String> = vec![String::new()];
        for p in parts {
            let vs = self.expr(file, p);
            if vs.open {
                reasons.extend(vs.reasons.clone());
            }
            let pieces = vs.pieces();
            let mut next = vec![];
            for a in &acc {
                for b in &pieces {
                    let mut s = format!("{}{}", a, b);
                    while s.contains("**") {
                        s = s.replace("**", "*");
                    }
                    if !next.contains(&s) {
                        next.push(s);
                    }
                }
            }
            acc = next;
        }
        let line = line_of(n);
        let mut out = ValueSet::default();
        for s in acc {
            if s == "*" {
                out.open = true;
                out.reasons.extend(reasons.clone());
                out.reasons.dedup();
            } else if s.contains('*') {
                out.globs.push((s, file.to_string(), line));
            } else {
                out.values.push((s, file.to_string(), line));
            }
        }
        out
    }

    fn name(&self, file: &str, root: Node<'a>, src: &'a [u8], n: Node<'a>) -> ValueSet {
        let name = text(n, src);
        let mut defs: Vec<(String, Def<'a>)> =
            reaching(root, src, name, n.start_byte()).into_iter().map(|d| (file.to_string(), d)).collect();
        if defs.is_empty() {
            // Package-level declarations in the other files of the package.
            let dir = Path::new(file).parent().map(|p| p.to_path_buf());
            // A declaration of `name` is in a file holding it.
            for (f, pf) in self.store.holding(&[name]) {
                if f == file || pf.lang != "go" || Path::new(f).parent().map(|p| p.to_path_buf()) != dir {
                    continue;
                }
                let Some((r, s)) = self.src(f) else { continue };
                // Byte 0 is outside every function, so only package-level
                // declarations reach it.
                defs.extend(reaching(r, s, name, 0).into_iter().map(|d| (f.to_string(), d)));
            }
        }
        if defs.is_empty() {
            return ValueSet::open(format!("no definition of {} at {}:{}", name, file, line_of(n)));
        }
        let mut out = ValueSet::default();
        for (f, d) in defs {
            match d {
                Def::Assign { value: Some(v), pos: 0, compound: false, .. } => out.join(self.expr(&f, v)),
                Def::Assign { node, .. } | Def::Range { node, .. } => out.join(ValueSet::open(self.here(&f, node))),
                Def::Param { func, pos, receiver } => out.join(self.param(&f, func, pos, receiver, name)),
            }
        }
        out
    }

    /// The union over every in-repo caller's argument at `pos`.
    fn param(&self, file: &str, func: Node<'a>, pos: usize, receiver: bool, name: &str) -> ValueSet {
        let fname = super::upward::func_name(func, self.src(file).map(|(_, s)| s).unwrap_or(b""));
        let Some(def) = self.up.def_of(file, func) else {
            return ValueSet::open(format!("parameter {} of {} is not a named function", name, fname));
        };
        let sites = self.up.callers_of(def);
        if sites.is_empty() {
            return ValueSet::open(format!("parameter {} of {} has no callers", name, fname));
        }
        if receiver {
            return ValueSet::open(format!("receiver {} of {}", name, fname));
        }
        if self.active.borrow().contains(&(def, pos)) {
            return ValueSet::default();
        }
        self.active.borrow_mut().push((def, pos));
        let mut out = ValueSet::default();
        for s in sites {
            match self.up.arg_at(&s, pos, false) {
                Some((f, a)) => out.join(self.expr(&f, a)),
                None => out.join(ValueSet::open(format!("a call of {} without argument {}", fname, pos + 1))),
            }
        }
        self.active.borrow_mut().pop();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Value set of the expression `needle` (its `nth` occurrence) in a.go.
    fn vs(files: &[(&str, &str)], needle: &str, nth: usize) -> ValueSet {
        let mut store = TreeStore::default();
        for (f, src) in files {
            store.insert(f, "go", src.as_bytes().to_vec());
        }
        let g = crate::resolve::scoped::FlowGraph::whole(&crate::resolve::graph::build_from_store(&store));
        let src = files[0].1;
        // A whole-word occurrence, so `n` does not match inside `func`.
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let at = src
            .match_indices(needle)
            .map(|(i, _)| i)
            .filter(|&i| !word(src[..i].chars().last()) && !word(src[i + needle.len()..].chars().next()))
            .nth(nth)
            .unwrap();
        let pf = store.get(files[0].0).unwrap();
        let root = pf.tree.as_ref().unwrap().root_node();
        let mut n = root.descendant_for_byte_range(at, at + needle.len()).unwrap();
        while let Some(p) = n.parent() {
            if p.start_byte() == at && p.end_byte() == at + needle.len() {
                n = p;
            } else {
                break;
            }
        }
        value_set(&store, &g, files[0].0, n)
    }

    fn values(v: &ValueSet) -> Vec<&str> {
        v.values.iter().map(|(s, _, _)| s.as_str()).collect()
    }

    #[test]
    fn a_literal_adds_one_value_with_its_line() {
        let v = vs(&[("t/a.go", "package t\nfunc F() {\n\tuse(\"a.tmpl\")\n}\n")], "\"a.tmpl\"", 0);
        assert_eq!(v.values, vec![("a.tmpl".to_string(), "t/a.go".to_string(), 3)]);
        assert!(!v.open);
        let raw = vs(&[("t/a.go", "package t\nfunc F() {\n\tuse(`b.tmpl`)\n}\n")], "`b.tmpl`", 0);
        assert_eq!(values(&raw), vec!["b.tmpl"]);
    }

    #[test]
    fn a_constant_adds_its_value_from_another_file_of_the_package() {
        let a = "package t\nfunc F() {\n\tuse(page)\n}\n";
        let b = "package t\nconst page = \"p.tmpl\"\n";
        let v = vs(&[("t/a.go", a), ("t/b.go", b)], "page", 0);
        assert_eq!(v.values, vec![("p.tmpl".to_string(), "t/b.go".to_string(), 2)]);
        assert!(!v.open);
    }

    #[test]
    fn a_local_variable_adds_each_reaching_assignment() {
        let a = "package t\nfunc F(c bool) {\n\tn := \"a\"\n\tif c {\n\t\tn = \"b\"\n\t}\n\tuse(n)\n}\n";
        let v = vs(&[("t/a.go", a)], "n", 2);
        assert_eq!(values(&v), vec!["a", "b"]);
    }

    #[test]
    fn a_parameter_unions_over_every_caller_recursively() {
        let a = "package t\nfunc render(name string) {\n\tuse(name)\n}\nfunc a() { render(\"a\") }\nfunc b() { render(\"b\") }\nfunc c(x string) { render(x) }\nfunc d() { c(\"d\") }\n";
        let v = vs(&[("t/a.go", a)], "name", 1);
        assert_eq!(values(&v), vec!["a", "b", "d"]);
        assert!(!v.open, "{:?}", v);
    }

    #[test]
    fn a_concatenation_of_known_and_unknown_parts_is_a_glob() {
        let a = "package t\nfunc admin(page string) {\n\tuse(\"admin-\" + page + \".html.tmpl\")\n}\n";
        let v = vs(&[("t/a.go", a)], "\"admin-\" + page + \".html.tmpl\"", 0);
        assert_eq!(v.globs, vec![("admin-*.html.tmpl".to_string(), "t/a.go".to_string(), 3)]);
        assert!(v.values.is_empty());
        assert!(!v.open, "a glob is a bounded set");
    }

    #[test]
    fn a_concatenation_of_known_parts_multiplies_out() {
        let a = "package t\nconst dir = \"views/\"\nfunc F(c bool) {\n\tn := \"a\"\n\tif c {\n\t\tn = \"b\"\n\t}\n\tuse(dir + n)\n}\n";
        let v = vs(&[("t/a.go", a)], "dir + n", 0);
        assert_eq!(values(&v), vec!["views/a", "views/b"]);
    }

    #[test]
    fn anything_else_is_open_with_a_reason() {
        let a = "package t\nimport \"os\"\nfunc F() {\n\tuse(os.Getenv(\"X\"))\n}\nfunc G(p string) {\n\tuse(p)\n}\n";
        let call = vs(&[("t/a.go", a)], "os.Getenv(\"X\")", 0);
        assert!(call.open);
        assert_eq!(call.reasons, vec!["call expression at t/a.go:4"]);
        let param = vs(&[("t/a.go", a)], "p", 1);
        assert!(param.open);
        assert_eq!(param.reasons, vec!["parameter p of G has no callers"]);
        let all_unknown = vs(&[("t/a.go", "package t\nfunc F(a, b string) {\n\tuse(a + b)\n}\n")], "a + b", 0);
        assert!(all_unknown.open);
        assert!(all_unknown.globs.is_empty());
    }
}
