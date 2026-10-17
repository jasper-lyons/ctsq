//! Upward data flow for `<=` and `<==`: from a value back to where it was
//! created. Go only. The walk goes through assignments, from a parameter to
//! the argument of every in-repo caller, from a call result into the
//! callee's return expressions, from a field read to the stores of that
//! field on the same object, and through models in reverse.
//!
//! It stops at a literal (`source literal`), a `source` model (`source`), a
//! parameter of a function with no in-repo callers (`source param`) and an
//! outside call with no model (`source unmodelled`).
//!
//! The node kinds below are tree-sitter-go names. The walk is Go only, so
//! they are named once here instead of read from `Mechanics`, which lets the
//! `from` attribute and value sets use the same helpers without a config.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

use super::Hop;
use crate::config::{LangConfig, Slot};
use crate::resolve::go::{binding_type_qualified, go_imports};
use crate::resolve::graph::CallGraph;
use crate::resolve::{DefIndex, DefKind, TreeStore};
use crate::resolve::scoped::{FlowGraph, Site};

const FUNCTIONS: &[&str] = &["function_declaration", "method_declaration", "func_literal"];
const LITERALS: &[&str] = &[
    "interpreted_string_literal",
    "raw_string_literal",
    "int_literal",
    "float_literal",
    "imaginary_literal",
    "rune_literal",
    "true",
    "false",
    "nil",
    "iota",
    "composite_literal",
    "func_literal",
];
/// Nodes whose children see a declaration made directly inside them.
const SCOPES: &[&str] = &[
    "block",
    "statement_list",
    "if_statement",
    "for_statement",
    "expression_switch_statement",
    "type_switch_statement",
    "select_statement",
    "expression_case",
    "type_case",
    "communication_case",
    "default_case",
    "function_declaration",
    "method_declaration",
    "func_literal",
    "source_file",
];
/// Go's predeclared types: a call to one is a conversion, which keeps the data.
const CONVERSIONS: &[&str] = &[
    "string", "[]byte", "[]rune", "byte", "rune", "bool", "int", "int8", "int16", "int32", "int64", "uint",
    "uint8", "uint16", "uint32", "uint64", "uintptr", "float32", "float64", "complex64", "complex128",
];

/// Internal tag on a hop that matched the end selector. Printers drop it.
pub const END: &str = "@end";
/// Tag of a hop the walk did not follow because of `--max`.
pub const MAX_DEPTH: &str = "stopped: max depth";
/// Hops one walk builds at most, a guard against blow-up.
const BUDGET: usize = 5000;

pub(crate) fn text<'s>(n: Node, src: &'s [u8]) -> &'s str {
    n.utf8_text(src).unwrap_or("")
}

fn line_of(n: Node) -> usize {
    n.start_position().row + 1
}

fn line_text(src: &[u8], line: usize) -> String {
    String::from_utf8_lossy(src).lines().nth(line.saturating_sub(1)).unwrap_or("").trim().to_string()
}

fn named(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

fn by_field<'t>(n: Node<'t>, f: &str) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children_by_field_name(f, &mut c).collect()
}

/// Expression lists flattened to their items.
fn flat(ns: Vec<Node>) -> Vec<Node> {
    ns.into_iter().flat_map(|x| if x.kind() == "expression_list" { named(x) } else { vec![x] }).collect()
}

fn is_function(n: Node) -> bool {
    FUNCTIONS.contains(&n.kind())
}

/// Innermost function holding byte `at`.
pub fn function_at(root: Node, at: usize) -> Option<Node> {
    let mut n = root.descendant_for_byte_range(at, at)?;
    loop {
        if is_function(n) {
            return Some(n);
        }
        n = n.parent()?;
    }
}

/// Name of a function node: `F`, `T.M` for a method, `func` for a literal.
pub fn func_name(f: Node, src: &[u8]) -> String {
    if f.kind() == "func_literal" {
        return "func".into();
    }
    f.child_by_field_name("name").map(|n| text(n, src).to_string()).unwrap_or_default()
}

/// Strip parentheses, `&x` and `*x`.
fn strip(mut e: Node) -> Node {
    loop {
        match e.kind() {
            "parenthesized_expression" => match e.named_child(0) {
                Some(i) => e = i,
                None => return e,
            },
            "unary_expression" => match e.child_by_field_name("operand") {
                Some(i) => e = i,
                None => return e,
            },
            _ => return e,
        }
    }
}

/// A place a name gets its value.
#[derive(Debug, Clone, Copy)]
pub enum Def<'t> {
    /// An assignment, `var` or `const` spec. `value` is the right side paired
    /// with the name, or the single right side when the sides do not pair
    /// (`a, err := f()`), then `pos` is the result position. `compound` is
    /// an operator assignment such as `+=`, which also keeps the old value.
    Assign { node: Node<'t>, value: Option<Node<'t>>, pos: usize, compound: bool },
    /// A range clause, the value is the ranged expression.
    Range { node: Node<'t>, value: Node<'t> },
    /// A parameter of `func` at `pos`, or its receiver.
    Param { func: Node<'t>, pos: usize, receiver: bool },
}

impl<'t> Def<'t> {
    pub fn node(&self) -> Node<'t> {
        match self {
            Def::Assign { node, .. } | Def::Range { node, .. } => *node,
            Def::Param { func, .. } => *func,
        }
    }
}

/// One binder found in a scope.
struct Binder<'t> {
    def: Def<'t>,
    /// Declares the name (`:=`, `var`, `const`, a parameter).
    decl: bool,
    start: usize,
    end: usize,
    /// The node a declaration is visible in.
    scope: Node<'t>,
}

fn scope_of(n: Node) -> Node {
    let mut x = n;
    while let Some(p) = x.parent() {
        if SCOPES.contains(&p.kind()) {
            return p;
        }
        x = p;
    }
    x
}

/// Every binder of `name` directly in `scope`, not inside nested functions.
/// The parameters of `scope` when it is a function.
fn binders<'t>(scope: Node<'t>, src: &[u8], name: &str) -> Vec<Binder<'t>> {
    let mut out = vec![];
    if is_function(scope) {
        let mut pos = 0;
        if let Some(r) = scope.child_by_field_name("receiver") {
            for d in named(r) {
                if by_field(d, "name").iter().any(|n| text(*n, src) == name) {
                    out.push(Binder {
                        def: Def::Param { func: scope, pos: 0, receiver: true },
                        decl: true,
                        start: d.start_byte(),
                        end: d.end_byte(),
                        scope,
                    });
                }
            }
        }
        if let Some(ps) = scope.child_by_field_name("parameters") {
            for d in named(ps) {
                if d.kind() != "parameter_declaration" && d.kind() != "variadic_parameter_declaration" {
                    continue;
                }
                let names = by_field(d, "name");
                if names.is_empty() {
                    pos += 1;
                }
                for n in names {
                    if text(n, src) == name {
                        out.push(Binder {
                            def: Def::Param { func: scope, pos, receiver: false },
                            decl: true,
                            start: d.start_byte(),
                            end: d.end_byte(),
                            scope,
                        });
                    }
                    pos += 1;
                }
            }
        }
    }
    let mut stack: Vec<Node> = match scope.child_by_field_name("body") {
        Some(b) if is_function(scope) => vec![b],
        _ if is_function(scope) => vec![],
        _ => named(scope),
    };
    while let Some(n) = stack.pop() {
        if is_function(n) {
            continue;
        }
        match n.kind() {
            "short_var_declaration" | "assignment_statement" => {
                let op = n.child_by_field_name("operator").map(|o| text(o, src)).unwrap_or(":=");
                let decl = n.kind() == "short_var_declaration";
                let lefts = flat(by_field(n, "left"));
                let rights = flat(by_field(n, "right"));
                for (i, l) in lefts.iter().enumerate() {
                    if l.kind() == "identifier" && text(*l, src) == name {
                        let (value, pos) = if lefts.len() == rights.len() {
                            (Some(rights[i]), 0)
                        } else {
                            (rights.first().copied(), i)
                        };
                        out.push(Binder {
                            def: Def::Assign { node: n, value, pos, compound: !decl && op != "=" },
                            decl,
                            start: n.start_byte(),
                            end: n.end_byte(),
                            scope: scope_of(n),
                        });
                    }
                }
            }
            "var_spec" | "const_spec" => {
                let names = by_field(n, "name");
                let values = flat(by_field(n, "value"));
                for (i, l) in names.iter().enumerate() {
                    if text(*l, src) == name {
                        let (value, pos) = if names.len() == values.len() {
                            (Some(values[i]), 0)
                        } else {
                            (values.first().copied(), i)
                        };
                        out.push(Binder {
                            def: Def::Assign { node: n, value, pos, compound: false },
                            decl: true,
                            start: n.start_byte(),
                            end: n.end_byte(),
                            scope: scope_of(n),
                        });
                    }
                }
            }
            "range_clause" => {
                let lefts = flat(by_field(n, "left"));
                let decl = {
                    let mut c = n.walk();
                    let has = n.children(&mut c).any(|k| k.kind() == ":=");
                    has
                };
                if let Some(r) = n.child_by_field_name("right") {
                    if lefts.iter().any(|l| l.kind() == "identifier" && text(*l, src) == name) {
                        // A range clause's names live in its for statement.
                        let sc = n.parent().unwrap_or(n);
                        out.push(Binder {
                            def: Def::Range { node: n, value: r },
                            decl,
                            start: n.start_byte(),
                            end: n.end_byte(),
                            scope: sc,
                        });
                    }
                }
            }
            _ => {}
        }
        stack.extend(named(n));
    }
    out.sort_by_key(|b| b.start);
    out
}

fn contains(n: Node, at: usize) -> bool {
    n.start_byte() <= at && at < n.end_byte()
}

/// The definitions of `name` that reach byte `at` in this file: the nearest
/// visible declaration (block scopes respected) and every plain assignment
/// after it and before `at`. Closures see the declarations of the functions
/// around them. Package-level `var` and `const` of this file come last.
/// Empty when the name is declared in another file or not at all.
pub fn reaching<'t>(root: Node<'t>, src: &[u8], name: &str, at: usize) -> Vec<Def<'t>> {
    let mut extra: Vec<Def<'t>> = vec![];
    let mut scope = function_at(root, at);
    while let Some(f) = scope {
        let bs = binders(f, src, name);
        let decl = bs
            .iter()
            .filter(|b| b.decl && contains(b.scope, at))
            .filter(|b| matches!(b.def, Def::Param { .. }) || b.end <= at)
            .max_by_key(|b| b.start);
        let after = decl.map(|d| d.start).unwrap_or(0);
        let mut plain: Vec<Def<'t>> =
            bs.iter().filter(|b| !b.decl && b.start >= after && b.end <= at).map(|b| b.def).collect();
        plain.append(&mut extra);
        extra = plain;
        if let Some(d) = decl {
            let mut out = vec![d.def];
            out.extend(extra);
            return out;
        }
        scope = f.parent().and_then(|p| function_at(root, p.start_byte()).filter(|g| g.id() != f.id()));
    }
    // Package level, in this file.
    let mut out: Vec<Def<'t>> = vec![];
    for top in named(root) {
        if top.kind() == "var_declaration" || top.kind() == "const_declaration" {
            for b in binders(top, src, name) {
                out.push(b.def);
            }
        }
    }
    out.extend(extra);
    out
}

/// Root identifier of a callee or qualifier chain: `zap` in `zap.New().Sugar`.
fn chain_root(mut n: Node) -> Option<Node> {
    for _ in 0..16 {
        match n.kind() {
            "identifier" => return Some(n),
            "call_expression" => n = n.child_by_field_name("function")?,
            "selector_expression" => n = n.child_by_field_name("operand")?,
            "parenthesized_expression" | "unary_expression" => n = strip(n),
            "generic_type" | "index_expression" => n = n.named_child(0)?,
            _ => return None,
        }
    }
    None
}

/// The import a Go variable was created from: walks `name` at `at` back
/// through local assignments to the call that made it (`l := zap.NewExample()`)
/// and gives that call's import path. File local, None when unknown.
pub fn creating_import(root: Node, src: &[u8], name: &str, at: usize, imports: &HashMap<String, String>) -> Option<String> {
    let mut seen: HashSet<(String, usize)> = HashSet::new();
    creating_import_in(root, src, name, at, imports, &mut seen)
}

fn creating_import_in(
    root: Node,
    src: &[u8],
    name: &str,
    at: usize,
    imports: &HashMap<String, String>,
    seen: &mut HashSet<(String, usize)>,
) -> Option<String> {
    if !seen.insert((name.to_string(), at)) || seen.len() > 32 {
        return None;
    }
    for d in reaching(root, src, name, at) {
        let Def::Assign { value: Some(v), .. } = d else { continue };
        let v = strip(v);
        let Some(r) = chain_root(v) else { continue };
        let rn = text(r, src);
        if v.kind() != "identifier" {
            if let Some(p) = imports.get(rn) {
                return Some(p.clone());
            }
        }
        if let Some(p) = creating_import_in(root, src, rn, r.start_byte(), imports, seen) {
            return Some(p);
        }
    }
    None
}

/// What the expression on one row came from: tags for the row and the hops
/// (on other rows) it reads.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub tags: Vec<String>,
    pub children: Vec<Hop>,
    pub unmodelled: Option<String>,
}

/// `primary` plus the stop and via tags of `parts`, their children joined.
fn fold(primary: &str, parts: Vec<Origin>) -> Origin {
    let mut o = Origin { tags: vec![primary.to_string()], ..Default::default() };
    for p in parts {
        for t in p.tags {
            if !["exact", "derived", "moved"].contains(&t.as_str()) && !o.tags.contains(&t) {
                o.tags.push(t);
            }
        }
        o.children.extend(p.children);
        if o.unmodelled.is_none() {
            o.unmodelled = p.unmodelled;
        }
    }
    o
}

fn leaf(tag: &str) -> Origin {
    Origin { tags: vec![tag.to_string()], ..Default::default() }
}

/// The flow graph over a whole-folder call graph (`CTSQ_WHOLE_GRAPH=1`).
pub fn whole(g: &CallGraph) -> FlowGraph {
    FlowGraph::whole(g)
}

/// End predicate over (file, call node).
pub type UpEnd<'e> = dyn Fn(&str, Node) -> bool + 'e;

/// The upward walk over every Go file in a store.
pub struct Up<'a> {
    pub store: &'a TreeStore,
    pub graph: &'a FlowGraph,
    /// Models, when loaded. Without them every outside call is unmodelled.
    pub cfg: Option<&'a LangConfig>,
    /// Deepest chain of function crossings followed.
    pub max: usize,
    pub end: Option<&'a UpEnd<'a>>,
    /// (def, slot) being walked, against recursion.
    active: RefCell<Vec<(usize, String)>>,
    depth: Cell<usize>,
    budget: Cell<usize>,
    imports: RefCell<HashMap<String, HashMap<String, String>>>,
}

impl<'a> Up<'a> {
    pub fn new(store: &'a TreeStore, graph: &'a FlowGraph, cfg: Option<&'a LangConfig>, max: usize) -> Up<'a> {
        Up {
            store,
            graph,
            cfg,
            max,
            end: None,
            active: RefCell::new(vec![]),
            depth: Cell::new(0),
            budget: Cell::new(BUDGET),
            imports: RefCell::new(HashMap::new()),
        }
    }

    fn tree(&self, file: &str) -> Option<(Node<'a>, &'a [u8])> {
        let pf = self.store.get(file)?;
        let t = pf.tree.as_ref()?;
        Some((t.root_node(), pf.source.as_slice()))
    }

    fn imports(&self, file: &str, root: Node, src: &[u8]) -> HashMap<String, String> {
        self.imports.borrow_mut().entry(file.to_string()).or_insert_with(|| go_imports(root, src)).clone()
    }

    /// The def index of a function node, matched by file and line.
    pub fn def_of(&self, file: &str, func: Node) -> Option<usize> {
        self.graph.def_at(file, line_of(func), |d| !matches!(d.kind, DefKind::InterfaceMethod { .. }))
    }

    /// Sites calling `def`, in file then document order.
    pub fn callers_of(&self, def: usize) -> Vec<std::rc::Rc<Site>> {
        self.graph.callers_of(def)
    }

    /// In-repo targets of a call node.
    pub fn targets(&self, file: &str, call: Node) -> Option<Vec<usize>> {
        let t = self.graph.site_targets(file, call.start_byte(), call.end_byte())?;
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// The node of argument `pos` at caller site `site`, with its file, or
    /// the receiver when `receiver`.
    pub fn arg_at(&self, s: &Site, pos: usize, receiver: bool) -> Option<(String, Node<'a>)> {
        let file = s.file.clone();
        let (root, _) = self.tree(&file)?;
        if receiver {
            let call = root.descendant_for_byte_range(s.start_byte, s.end_byte)?;
            let call = if call.kind() == "call_expression" { call } else { call.parent()? };
            let f = call.child_by_field_name("function")?;
            return Some((file, f.child_by_field_name("operand")?));
        }
        let &(a, b) = s.args.get(pos)?;
        let mut n = root.descendant_for_byte_range(a, b)?;
        while let Some(p) = n.parent() {
            if p.start_byte() == a && p.end_byte() == b && p.kind() != "argument_list" {
                n = p;
            } else {
                break;
            }
        }
        Some((file, n))
    }

    fn spend(&self) -> bool {
        let b = self.budget.get();
        if b == 0 {
            return false;
        }
        self.budget.set(b - 1);
        true
    }

    fn hop(&self, file: &str, src: &[u8], at: Node, func: Option<Node>, o: Origin) -> Hop {
        let line = line_of(at);
        Hop {
            file: file.to_string(),
            line,
            func: func.map(|f| func_name(f, src)).unwrap_or_else(|| "var".into()),
            text: line_text(src, line),
            tags: o.tags,
            unmodelled: o.unmodelled,
            returns: false,
            into: false,
            children: o.children,
        }
    }

    /// The tree for a start name node: when the node is where the name is
    /// assigned or declared, that definition is the root, otherwise the row
    /// of the use with the reaching definitions under it.
    pub fn start(&self, file: &str, ident: Node<'a>) -> Option<Hop> {
        let (root, src) = self.tree(file)?;
        let name = text(ident, src);
        let at = ident.start_byte();
        // A definition site: the binder whose left side holds `ident`.
        let scope = function_at(root, at);
        let mut defs: Vec<Def> = vec![];
        let mut scopes = vec![];
        if let Some(f) = scope {
            scopes.push(f);
        }
        let mut found = false;
        for sc in scopes.into_iter().chain(named(root).into_iter().filter(|t| {
            t.kind() == "var_declaration" || t.kind() == "const_declaration"
        })) {
            for b in binders(sc, src, name) {
                let left_holds = match b.def {
                    Def::Param { .. } => b.start <= at && at < b.end,
                    Def::Assign { value, node, .. } => {
                        contains(node, at) && value.is_none_or(|v| at < v.start_byte())
                    }
                    Def::Range { value, node } => contains(node, at) && at < value.start_byte(),
                };
                if left_holds {
                    defs.push(b.def);
                    found = true;
                }
            }
            if found {
                break;
            }
        }
        if let Some(d) = defs.first() {
            let mut h = self.def_hop(file, root, src, *d, name);
            // A compound assignment also keeps the earlier value.
            if let Def::Assign { compound: true, node, .. } = d {
                h.children.extend(self.name_hops(file, root, src, name, node.start_byte()));
            }
            return Some(h);
        }
        let children = self.name_hops(file, root, src, name, at);
        let o = Origin { tags: vec![], children, unmodelled: None };
        Some(self.hop(file, src, ident, function_at(root, at), o))
    }

    /// Hops for each definition of `name` reaching `at`, searching the other
    /// files of the package for package-level declarations.
    fn name_hops(&self, file: &str, root: Node<'a>, src: &'a [u8], name: &str, at: usize) -> Vec<Hop> {
        let defs = reaching(root, src, name, at);
        if !defs.is_empty() {
            return defs.into_iter().map(|d| self.def_hop(file, root, src, d, name)).collect();
        }
        let dir = Path::new(file).parent().map(|p| p.to_path_buf());
        let mut out = vec![];
        // A declaration of `name` is in a file holding it.
        for (f, pf) in self.store.holding(&[name]) {
            if f == file || pf.lang != "go" || Path::new(f).parent().map(|p| p.to_path_buf()) != dir {
                continue;
            }
            let Some((r, s)) = self.tree(f) else { continue };
            for top in named(r) {
                if top.kind() == "var_declaration" || top.kind() == "const_declaration" {
                    for b in binders(top, s, name) {
                        out.push(self.def_hop(f, r, s, b.def, name));
                    }
                }
            }
        }
        out
    }

    /// One definition as a hop: its row, tags and the origins under it.
    pub fn def_hop(&self, file: &str, root: Node<'a>, src: &'a [u8], d: Def<'a>, name: &str) -> Hop {
        let node = d.node();
        let func = function_at(root, node.start_byte());
        if !self.spend() {
            return self.hop(file, src, node, func, leaf("stopped: budget"));
        }
        let o = match d {
            Def::Assign { value: None, .. } => leaf("source literal"),
            Def::Assign { value: Some(v), pos, compound, node } => {
                let mut o = self.expr(file, root, src, v, pos);
                if compound {
                    o = fold("derived", vec![o]);
                    o.children.extend(self.name_hops(file, root, src, name, node.start_byte()));
                }
                o
            }
            Def::Range { value, .. } => fold("derived", vec![self.expr(file, root, src, value, 0)]),
            Def::Param { func, pos, receiver } => return self.param_hop(file, src, func, pos, receiver),
        };
        self.hop(file, src, node, func, o)
    }

    /// A parameter: the signature row with every in-repo caller's argument
    /// under it, or `source param` when nothing in the repo calls it.
    fn param_hop(&self, file: &str, src: &'a [u8], func: Node<'a>, pos: usize, receiver: bool) -> Hop {
        let mk = |o: Origin| self.hop(file, src, func, Some(func), o);
        let Some(def) = self.def_of(file, func) else { return mk(leaf("source param")) };
        let sites = self.callers_of(def);
        if sites.is_empty() {
            return mk(leaf("source param"));
        }
        if self.depth.get() >= self.max {
            return mk(leaf(MAX_DEPTH));
        }
        let mark = (def, format!("p{}{}", pos, receiver));
        if self.active.borrow().contains(&mark) {
            return mk(Origin::default());
        }
        self.active.borrow_mut().push(mark);
        self.depth.set(self.depth.get() + 1);
        let mut children = vec![];
        for s in sites {
            let Some((cfile, arg)) = self.arg_at(&s, pos, receiver) else { continue };
            let Some((croot, csrc)) = self.tree(&cfile) else { continue };
            let o = fold("moved", vec![self.expr(&cfile, croot, csrc, arg, 0)]);
            children.push(self.hop(&cfile, csrc, arg, function_at(croot, arg.start_byte()), o));
        }
        self.depth.set(self.depth.get() - 1);
        self.active.borrow_mut().pop();
        mk(Origin { tags: vec![], children, unmodelled: None })
    }

    /// Where the value of expression `e` (result position `pos` for a call)
    /// came from.
    pub fn expr(&self, file: &str, root: Node<'a>, src: &'a [u8], e: Node<'a>, pos: usize) -> Origin {
        let e = match e.kind() {
            "parenthesized_expression" | "unary_expression" => strip(e),
            _ => e,
        };
        if LITERALS.contains(&e.kind()) {
            return leaf("source literal");
        }
        match e.kind() {
            "identifier" => {
                let children = self.name_hops(file, root, src, text(e, src), e.start_byte());
                if children.is_empty() {
                    return leaf("stopped: no definition");
                }
                Origin { tags: vec!["exact".into()], children, unmodelled: None }
            }
            "binary_expression" => {
                let parts: Vec<Origin> = ["left", "right"]
                    .iter()
                    .filter_map(|f| e.child_by_field_name(f))
                    .map(|x| self.expr(file, root, src, x, 0))
                    .collect();
                if parts.iter().all(|p| p.tags == ["source literal"]) {
                    return leaf("source literal");
                }
                let parts = parts.into_iter().filter(|p| p.tags != ["source literal"]).collect();
                fold("derived", parts)
            }
            "call_expression" | "type_conversion_expression" => self.call(file, root, src, e, pos),
            "selector_expression" => self.selector(file, root, src, e),
            "index_expression" | "slice_expression" | "type_assertion_expression" => {
                let op = e.child_by_field_name("operand").or_else(|| e.named_child(0));
                match op {
                    Some(o) => fold("derived", vec![self.expr(file, root, src, o, 0)]),
                    None => leaf("stopped: expression"),
                }
            }
            k => leaf(&format!("stopped: {}", k.replace('_', " "))),
        }
    }

    fn call(&self, file: &str, root: Node<'a>, src: &'a [u8], call: Node<'a>, pos: usize) -> Origin {
        if self.end.is_some_and(|f| f(file, call)) {
            return Origin { tags: vec!["source".into(), END.into()], ..Default::default() };
        }
        let args: Vec<Node> = call.child_by_field_name("arguments").map(named).unwrap_or_default();
        let callee = call.child_by_field_name("function").or_else(|| call.child_by_field_name("type"));
        let callee_text = callee.map(|c| text(c, src)).unwrap_or("");
        if call.kind() == "type_conversion_expression" || CONVERSIONS.contains(&callee_text) {
            let a = call.child_by_field_name("operand").or_else(|| args.first().copied());
            return match a {
                Some(a) => fold("derived", vec![self.expr(file, root, src, a, 0)]),
                None => leaf("source literal"),
            };
        }
        if let Some(targets) = self.targets(file, call) {
            return self.into_returns(&targets, pos, callee_text);
        }
        // Outside the repo: a model in reverse.
        let key = self.model_keys(file, root, src, call);
        let model = self.cfg.and_then(|c| key.iter().find_map(|k| c.model(k)));
        let Some(m) = model else {
            return Origin {
                tags: vec!["source unmodelled".into()],
                children: vec![],
                unmodelled: Some(callee_text.to_string()),
            };
        };
        if m.source.is_some() {
            return leaf("source");
        }
        let froms: Vec<&Slot> = m.pass.iter().filter(|p| p.to == Slot::Return).map(|p| &p.from).collect();
        if froms.is_empty() {
            return leaf("source");
        }
        let mut parts = vec![];
        for f in froms {
            match f {
                Slot::Arg(i) => parts.extend(args.get(*i).map(|a| self.expr(file, root, src, *a, 0))),
                Slot::AllArgs => parts.extend(args.iter().map(|a| self.expr(file, root, src, *a, 0))),
                Slot::Receiver => {
                    let op = callee.and_then(|c| c.child_by_field_name("operand"));
                    parts.extend(op.map(|o| self.expr(file, root, src, o, 0)));
                }
                Slot::Return => {}
            }
        }
        let parts: Vec<Origin> = parts.into_iter().filter(|p| p.tags != ["source literal"]).collect();
        if parts.is_empty() {
            return leaf("source literal");
        }
        fold("derived", parts)
    }

    /// Model keys of an outside call, as `flow::candidate_keys` builds them.
    fn model_keys(&self, file: &str, root: Node<'a>, src: &'a [u8], call: Node<'a>) -> Vec<String> {
        let Some(f) = call.child_by_field_name("function") else { return vec![] };
        let imports = self.imports(file, root, src);
        match f.kind() {
            "identifier" => vec![format!("builtin.{}", text(f, src))],
            "selector_expression" => {
                let (Some(op), Some(m)) = (f.child_by_field_name("operand"), f.child_by_field_name("field")) else {
                    return vec![];
                };
                let m = text(m, src);
                let on = text(op, src);
                if op.kind() == "identifier" {
                    if let Some(p) = imports.get(on) {
                        return vec![format!("{}.{}", p, m)];
                    }
                    let ty = binding_type_qualified(root, src, file, on, line_of(call), &DefIndex::default());
                    if let Some(q) = ty.and_then(|t| super::qualify_type(&t, &imports)) {
                        return vec![format!("(*{}).{}", q, m), format!("({}).{}", q, m)];
                    }
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// A call into the repo: the return rows of every target at result `pos`.
    fn into_returns(&self, targets: &[usize], pos: usize, callee: &str) -> Origin {
        let name = callee.rsplit('.').next().unwrap_or(callee);
        let mut o = Origin { tags: vec!["derived".into(), format!("via {}", name)], ..Default::default() };
        if self.depth.get() >= self.max {
            o.tags.insert(0, MAX_DEPTH.into());
            return o;
        }
        self.depth.set(self.depth.get() + 1);
        for &t in targets {
            let mark = (t, format!("r{}", pos));
            if self.active.borrow().contains(&mark) {
                continue;
            }
            let d = self.graph.def(t);
            let Some((root, src)) = self.tree(&d.id.file) else { continue };
            let file = d.id.file.clone();
            let Some(func) = func_at_line(root, d.id.line) else { continue };
            self.active.borrow_mut().push(mark);
            for (ret, e, p) in returns_of(func, pos) {
                let inner = self.expr(&file, root, src, e, p);
                let mut r = fold("moved", vec![inner]);
                if targets.len() > 1 {
                    r.tags.push("widened".into());
                }
                o.children.push(self.hop(&file, src, ret, Some(func), r));
            }
            self.active.borrow_mut().pop();
        }
        self.depth.set(self.depth.get() - 1);
        o
    }

    /// `o.F`: a package variable of an import is outside. A field read goes
    /// back to the stores of `F` on the same object in this function, or to
    /// the composite literal the object was built from.
    fn selector(&self, file: &str, root: Node<'a>, src: &'a [u8], e: Node<'a>) -> Origin {
        let (Some(op), Some(field)) = (e.child_by_field_name("operand"), e.child_by_field_name("field")) else {
            return leaf("stopped: selector");
        };
        let fname = text(field, src);
        if op.kind() == "identifier" && self.imports(file, root, src).contains_key(text(op, src)) {
            return Origin {
                tags: vec!["source unmodelled".into()],
                children: vec![],
                unmodelled: Some(text(e, src).to_string()),
            };
        }
        if op.kind() != "identifier" {
            return fold("derived", vec![self.expr(file, root, src, op, 0)]);
        }
        let obj = text(op, src);
        let at = e.start_byte();
        let func = function_at(root, at);
        let mut children = vec![];
        // Stores `obj.F = x` before the read.
        if let Some(f) = func {
            let mut stack = vec![f];
            let mut found = vec![];
            while let Some(n) = stack.pop() {
                if n.kind() == "assignment_statement" && n.end_byte() <= at {
                    let lefts = flat(by_field(n, "left"));
                    let rights = flat(by_field(n, "right"));
                    for (i, l) in lefts.iter().enumerate() {
                        let is_store = l.kind() == "selector_expression"
                            && l.child_by_field_name("operand").is_some_and(|o| text(o, src) == obj)
                            && l.child_by_field_name("field").is_some_and(|x| text(x, src) == fname);
                        if is_store && rights.len() == lefts.len() {
                            found.push((n, rights[i]));
                        }
                    }
                }
                stack.extend(named(n));
            }
            found.sort_by_key(|(n, _)| n.start_byte());
            for (n, v) in found {
                let o = fold(&format!("field {}", fname), vec![self.expr(file, root, src, v, 0)]);
                children.push(self.hop(file, src, n, Some(f), o));
            }
        }
        // The literal the object was built from.
        if children.is_empty() {
            for d in reaching(root, src, obj, at) {
                let Def::Assign { value: Some(v), node, .. } = d else { continue };
                let v = strip(v);
                if v.kind() != "composite_literal" {
                    continue;
                }
                let Some(body) = v.child_by_field_name("body") else { continue };
                for el in named(body) {
                    if el.kind() != "keyed_element" {
                        continue;
                    }
                    let kv = named(el);
                    let key = kv.first().map(|k| text(*k, src).trim().to_string());
                    if key.as_deref() == Some(fname) {
                        if let Some(val) = kv.get(1) {
                            let val = if val.kind() == "literal_element" { val.named_child(0).unwrap_or(*val) } else { *val };
                            let o = fold(&format!("field {}", fname), vec![self.expr(file, root, src, val, 0)]);
                            children.push(self.hop(file, src, node, function_at(root, node.start_byte()), o));
                        }
                    }
                }
            }
        }
        if children.is_empty() {
            return leaf(&format!("stopped: field {}", fname));
        }
        Origin { tags: vec!["exact".into()], children, unmodelled: None }
    }
}

/// The function declared on `line`.
pub fn func_at_line(root: Node, line: usize) -> Option<Node> {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if is_function(n) && line_of(n) == line && n.kind() != "func_literal" {
            return Some(n);
        }
        if n.start_position().row + 1 <= line && line <= n.end_position().row + 1 {
            stack.extend(named(n));
        }
    }
    None
}

/// (return statement, expression, result position inside it) for result
/// `pos` of every return in `func`, nested literals left out. A single call
/// returned for several results (`return f()`) passes `pos` through.
pub fn returns_of(func: Node, pos: usize) -> Vec<(Node, Node, usize)> {
    let mut out = vec![];
    let Some(body) = func.child_by_field_name("body") else { return out };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if is_function(n) {
            continue;
        }
        if n.kind() == "return_statement" {
            let es: Vec<Node> = n.named_child(0).map(|l| flat(vec![l])).unwrap_or_default();
            if es.len() == 1 && pos > 0 {
                out.push((n, es[0], pos));
            } else if let Some(e) = es.get(pos) {
                out.push((n, *e, 0));
            }
            continue;
        }
        stack.extend(named(n));
    }
    out.sort_by_key(|(n, _, _)| n.start_byte());
    out
}

/// Drop branches that do not reach a hop tagged `END`.
pub fn prune_to_end(h: &Hop) -> Option<Hop> {
    let children: Vec<Hop> = h.children.iter().filter_map(prune_to_end).collect();
    if children.is_empty() && !h.has(END) {
        return None;
    }
    Some(Hop { children, ..h.clone() })
}

/// Keep `depth` levels below `h`.
pub fn truncate(h: &Hop, depth: usize) -> Hop {
    let children = if depth == 0 { vec![] } else { h.children.iter().map(|c| truncate(c, depth - 1)).collect() };
    Hop { children, ..h.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(files: &[(&str, &str)]) -> TreeStore {
        let mut s = TreeStore::default();
        for (f, src) in files {
            s.insert(f, "go", src.as_bytes().to_vec());
        }
        s
    }

    fn ident<'t>(root: Node<'t>, src: &str, needle: &str, nth: usize) -> Node<'t> {
        // A whole-word occurrence, so `n` does not match inside `func`.
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let at = src
            .match_indices(needle)
            .map(|(i, _)| i)
            .filter(|&i| !word(src[..i].chars().last()) && !word(src[i + needle.len()..].chars().next()))
            .nth(nth)
            .unwrap();
        root.descendant_for_byte_range(at, at + needle.len()).unwrap()
    }

    fn all_tags(h: &Hop) -> Vec<(usize, String)> {
        h.walk().iter().flat_map(|x| x.tags.iter().map(|t| (x.line, t.clone()))).collect()
    }

    #[test]
    fn reaching_respects_block_scopes_and_later_assignments() {
        let src = "package t\nfunc F() string {\n\tx := \"a\"\n\tif true {\n\t\tx := \"b\"\n\t\t_ = x\n\t}\n\tx = \"c\"\n\treturn x\n}\n";
        let s = store(&[("t/a.go", src)]);
        let pf = s.get("t/a.go").unwrap();
        let root = pf.tree.as_ref().unwrap().root_node();
        let at = src.find("return x").unwrap() + 7;
        let defs = reaching(root, src.as_bytes(), "x", at);
        let lines: Vec<usize> = defs.iter().map(|d| line_of(d.node())).collect();
        assert_eq!(lines, vec![3, 8], "the inner block's x does not reach");
    }

    #[test]
    fn a_param_walks_to_each_callers_argument() {
        let src = "package t\nfunc r(n string) {}\nfunc a() { r(\"x\") }\nfunc b() { v := \"y\"; r(v) }\n";
        let s = store(&[("t/a.go", src)]);
        let g = whole(&crate::resolve::graph::build_from_store(&s));
        let up = Up::new(&s, &g, None, 12);
        let root = s.get("t/a.go").unwrap().tree.as_ref().unwrap().root_node();
        let h = up.start("t/a.go", ident(root, src, "n", 0)).unwrap();
        assert_eq!(h.line, 2);
        assert_eq!(h.children.len(), 2);
        assert_eq!(h.children[0].tags, vec!["moved", "source literal"]);
        assert_eq!(h.children[1].tags, vec!["moved"]);
        assert_eq!(h.children[1].children[0].tags, vec!["source literal"]);
    }

    #[test]
    fn a_call_result_goes_back_into_the_callee_return() {
        let src = "package t\nfunc mk() string {\n\treturn \"lit\"\n}\nfunc F() {\n\tq := mk()\n\t_ = q\n}\n";
        let s = store(&[("t/a.go", src)]);
        let g = whole(&crate::resolve::graph::build_from_store(&s));
        let up = Up::new(&s, &g, None, 12);
        let root = s.get("t/a.go").unwrap().tree.as_ref().unwrap().root_node();
        let h = up.start("t/a.go", ident(root, src, "q", 0)).unwrap();
        assert_eq!(h.tags, vec!["derived", "via mk"]);
        assert_eq!(h.children[0].line, 3);
        assert_eq!(h.children[0].tags, vec!["moved", "source literal"]);
    }

    #[test]
    fn a_field_read_goes_back_to_its_store_and_an_outside_call_is_unmodelled() {
        let src = "package t\nimport \"x/y\"\ntype B struct{ P string }\nfunc F() {\n\tb := B{}\n\tb.P = y.Get()\n\tq := b.P\n\t_ = q\n}\n";
        let s = store(&[("t/a.go", src)]);
        let g = whole(&crate::resolve::graph::build_from_store(&s));
        let up = Up::new(&s, &g, None, 12);
        let root = s.get("t/a.go").unwrap().tree.as_ref().unwrap().root_node();
        let h = up.start("t/a.go", ident(root, src, "q", 0)).unwrap();
        let tags = all_tags(&h);
        assert!(tags.contains(&(6, "field P".into())), "{:?}", tags);
        assert!(tags.contains(&(6, "source unmodelled".into())), "{:?}", tags);
    }

    #[test]
    fn recursion_and_max_depth_stop() {
        let src = "package t\nfunc r(n int) { r(n - 1) }\nfunc a() { r(3) }\n";
        let s = store(&[("t/a.go", src)]);
        let g = whole(&crate::resolve::graph::build_from_store(&s));
        let root = s.get("t/a.go").unwrap().tree.as_ref().unwrap().root_node();
        let up = Up::new(&s, &g, None, 12);
        let h = up.start("t/a.go", ident(root, src, "n", 0)).unwrap();
        assert!(all_tags(&h).iter().any(|(_, t)| t == "source literal"));
        let up0 = Up::new(&s, &g, None, 0);
        let h0 = up0.start("t/a.go", ident(root, src, "n", 0)).unwrap();
        assert_eq!(h0.tags, vec![MAX_DEPTH]);
    }

    #[test]
    fn creating_import_follows_local_assignments() {
        let src = "package t\nimport \"go.uber.org/zap\"\nfunc F() {\n\tbase := zap.NewExample()\n\tl := base.Sugar()\n\tl.Info(\"x\")\n}\n";
        let s = store(&[("t/a.go", src)]);
        let root = s.get("t/a.go").unwrap().tree.as_ref().unwrap().root_node();
        let imports = go_imports(root, src.as_bytes());
        let at = src.find("l.Info").unwrap();
        assert_eq!(creating_import(root, src.as_bytes(), "l", at, &imports).as_deref(), Some("go.uber.org/zap"));
        assert_eq!(creating_import(root, src.as_bytes(), "nope", at, &imports), None);
    }
}
