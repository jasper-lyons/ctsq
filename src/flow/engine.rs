//! The statement walk of decision 10.

use std::collections::{BTreeMap, HashMap, HashSet};

use tree_sitter::Node;

use super::models::{qualify_type, result_type};
use super::objects::{is_map_expr, join_paths, under, Paths};
use super::{CallEffect, CallSite, EndFn, FlowCtx, FlowResult, Hop, Outside, Start, StartKind};
use crate::config::{Mechanics, Slot};
use crate::resolve::go::{binding_type, binding_type_qualified, go_imports, go_result_names, go_result_types};
use crate::resolve::DefIndex;

/// Loop passes always allowed. A loop runs to its fixed point, the set of
/// tracked names only grows inside it, so this floor plus a bound from the
/// number of names in the function is a safety net for field paths that
/// keep nesting (`o.Next = o`). A loop that hits it is tagged `stopped: loop cap`.
const MIN_LOOP_PASSES: usize = 32;

pub(crate) fn text<'s>(n: Node, src: &'s [u8]) -> &'s str {
    n.utf8_text(src).unwrap_or("")
}

fn line_text(src: &[u8], line: usize) -> String {
    let s = String::from_utf8_lossy(src);
    s.lines().nth(line.saturating_sub(1)).unwrap_or("").trim().to_string()
}

fn line_of(n: Node) -> usize {
    n.start_position().row + 1
}

/// Children of `n` under any of the field `names`, in source order.
pub(crate) fn kids<'t>(n: Node<'t>, names: &[String]) -> Vec<Node<'t>> {
    let mut out = vec![];
    for name in names {
        let mut c = n.walk();
        out.extend(n.children_by_field_name(name, &mut c));
    }
    out.sort_by_key(|k| k.start_byte());
    out.dedup_by_key(|k| k.id());
    out
}

pub(crate) fn named<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

/// The innermost function node containing `byte`.
pub fn enclosing_function<'t>(root: Node<'t>, mech: &Mechanics, byte: usize) -> Option<Node<'t>> {
    let mut n = root.descendant_for_byte_range(byte, byte)?;
    loop {
        if Mechanics::is(&mech.nodes.function, n.kind()) {
            return Some(n);
        }
        n = n.parent()?;
    }
}

/// Name of a function node, or `func` for a literal.
pub fn function_name(func: Node, src: &[u8], mech: &Mechanics) -> String {
    kids(func, &mech.fields.name).first().map(|n| text(*n, src).to_string()).unwrap_or_else(|| "func".into())
}

/// Classify the node at a byte range as a start point. A field name (in a
/// struct declaration or a field read) starts a field walk, a plain name
/// starts a variable walk. Anything else gives `None`.
pub fn start_at(cx: &FlowCtx, start_byte: usize, end_byte: usize) -> Option<Start> {
    let m = cx.mech;
    let n = cx.root.descendant_for_byte_range(start_byte, end_byte)?;
    let line = line_of(n);
    let name = text(n, cx.src).to_string();
    let func = enclosing_function(cx.root, m, start_byte)
        .map(|f| function_name(f, cx.src, m))
        .unwrap_or_default();
    let base = |kind, func, text| Start {
        kind,
        start_byte,
        end_byte,
        file: cx.file.to_string(),
        line,
        func,
        text,
        paths: None,
    };
    if Mechanics::is(&m.nodes.field_name, n.kind()) {
        let p = n.parent()?;
        let owner = if Mechanics::is(&m.nodes.field_decl, p.kind()) {
            let mut a = p;
            loop {
                if Mechanics::is(&m.nodes.type_decl, a.kind()) {
                    break kids(a, &m.fields.name).first().map(|t| text(*t, cx.src).to_string());
                }
                match a.parent() {
                    Some(up) => a = up,
                    None => break None,
                }
            }
        } else if Mechanics::is(&m.nodes.field, p.kind()) {
            kids(p, &m.fields.operand).first().and_then(|o| operand_type(cx, *o, line))
        } else {
            None
        };
        let label = match &owner {
            Some(o) => format!("{}.{}", o, name),
            None => name.clone(),
        };
        return Some(base(StartKind::Field { name, owner }, "field".into(), label));
    }
    if Mechanics::is(&m.nodes.name, n.kind()) {
        return Some(base(StartKind::Var(name), func, line_text(cx.src, line)));
    }
    None
}

/// The struct type an object expression has: a variable's type, and for a
/// chain `o.A` the declared type of field `A` on the type of `o`, so the
/// owner of `F` in `o.A.F` is the type of `A`, not of `o`. None when a step
/// is unknown.
pub fn operand_type(cx: &FlowCtx, op: Node, line: usize) -> Option<String> {
    let m = cx.mech;
    if Mechanics::is(&m.nodes.name, op.kind()) {
        return binding_type(cx.root, cx.src, cx.file, text(op, cx.src), line, &DefIndex::default());
    }
    if Mechanics::is(&m.nodes.field, op.kind()) {
        let inner = operand_type(cx, *kids(op, &m.fields.operand).first()?, line)?;
        let f = kids(op, &m.fields.field).first().copied()?;
        return cx.module.field(&inner, text(f, cx.src)).map(|(_, fi)| fi.ty.clone());
    }
    None
}

/// The type of field selector `sel` (`d.db`), from the struct declaring the
/// field: its base name and, when the declared type has a package part, the
/// type as `path.T` (`database/sql.DB` for `*sql.DB`). None when the owner
/// type or the field is unknown.
pub fn field_type_path(cx: &FlowCtx, sel: Node, line: usize) -> Option<(String, Option<String>)> {
    let m = cx.mech;
    if !Mechanics::is(&m.nodes.field, sel.kind()) {
        return None;
    }
    let owner = operand_type(cx, *kids(sel, &m.fields.operand).first()?, line)?;
    let f = kids(sel, &m.fields.field).first().copied()?;
    cx.module.field(&owner, text(f, cx.src)).map(|(_, fi)| (fi.ty.clone(), fi.path.clone()))
}

/// The call variable `name` was last bound from before this use, as its
/// first result or paired with it. None when that value is not a call.
pub fn bound_from<'t>(m: &Mechanics, src: &[u8], name: Node<'t>) -> Option<Node<'t>> {
    let want = text(name, src);
    let mut outer = None;
    let mut x = name;
    while let Some(p) = x.parent() {
        if Mechanics::is(&m.nodes.function, p.kind()) {
            outer = Some(p);
        }
        x = p;
    }
    let flat = |ns: Vec<Node<'t>>| -> Vec<Node<'t>> {
        ns.into_iter().flat_map(|n| if Mechanics::is(&m.nodes.list, n.kind()) { named(n) } else { vec![n] }).collect()
    };
    let mut best: Option<Node<'t>> = None;
    let mut stack = vec![outer?];
    while let Some(a) = stack.pop() {
        stack.extend(named(a));
        if !Mechanics::is(&m.nodes.assign, a.kind()) || a.start_byte() >= name.start_byte() {
            continue;
        }
        let lefts = flat(kids(a, &m.fields.left));
        let rights = flat(kids(a, &m.fields.right));
        let Some(i) = lefts.iter().position(|l| text(*l, src) == want) else { continue };
        let v = if rights.len() == lefts.len() {
            rights.get(i).copied()
        } else if i == 0 && rights.len() == 1 {
            rights.first().copied()
        } else {
            None
        };
        if let Some(v) = v {
            if best.is_none_or(|b| b.start_byte() < v.start_byte()) {
                best = Some(v);
            }
        }
    }
    best.filter(|v| Mechanics::is(&m.nodes.call, v.kind()))
}

/// Pointer variables of a function and the names each one aliases, both
/// ways. A parameter or variable of pointer type, `p := &y` and `new(T)`
/// make pointers. `p := &y` aliases p and y, `c := b` aliases c and b when
/// b is a pointer, so a write through one reaches the other.
fn pointer_aliases(func: Node, src: &[u8], m: &Mechanics) -> (HashSet<String>, HashMap<String, Vec<String>>) {
    let mut ptrs: HashSet<String> = HashSet::new();
    let mut alias: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending: Vec<(String, String)> = vec![];
    let link = |a: &str, b: &str, alias: &mut HashMap<String, Vec<String>>| {
        for (x, y) in [(a, b), (b, a)] {
            let e = alias.entry(x.to_string()).or_default();
            if !e.iter().any(|v| v == y) {
                e.push(y.to_string());
            }
        }
    };
    fn flat<'t>(ns: Vec<Node<'t>>, m: &Mechanics) -> Vec<Node<'t>> {
        let mut out = vec![];
        for n in ns {
            if Mechanics::is(&m.nodes.list, n.kind()) {
                out.extend(named(n));
            } else {
                out.push(n);
            }
        }
        out
    }
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        stack.extend(named(n));
        let is_param = Mechanics::is(&m.nodes.param, n.kind());
        let is_assign = Mechanics::is(&m.nodes.assign, n.kind());
        if !is_param && !is_assign {
            continue;
        }
        let lefts: Vec<Node> = flat(kids(n, &m.fields.left), m).into_iter().filter(|l| Mechanics::is(&m.nodes.name, l.kind())).collect();
        if kids(n, &m.fields.ty).first().is_some_and(|t| text(*t, src).starts_with('*')) {
            for l in &lefts {
                ptrs.insert(text(*l, src).to_string());
            }
        }
        if !is_assign {
            continue;
        }
        let rights = flat(kids(n, &m.fields.right), m);
        if rights.len() != lefts.len() {
            continue;
        }
        for (l, r) in lefts.iter().zip(rights) {
            let (lt, rt) = (text(*l, src), text(r, src));
            if rt.starts_with('&') || rt.starts_with("new(") {
                ptrs.insert(lt.to_string());
                if let Some(o) = kids(r, &m.fields.operand).first().filter(|o| Mechanics::is(&m.nodes.name, o.kind())) {
                    link(lt, text(*o, src), &mut alias);
                }
            } else if Mechanics::is(&m.nodes.name, r.kind()) {
                pending.push((lt.to_string(), rt.to_string()));
            }
        }
    }
    loop {
        let mut changed = false;
        for (l, r) in &pending {
            if ptrs.contains(r) && !alias.get(l).is_some_and(|v| v.contains(r)) {
                ptrs.insert(l.clone());
                link(l, r, &mut alias);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (ptrs, alias)
}

/// Names of the parameters and receiver of `func` declared with a pointer
/// type. Nested function literals are not entered.
fn pointer_params(func: Node, src: &[u8], m: &Mechanics) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = named(func);
    while let Some(n) = stack.pop() {
        if Mechanics::is(&m.nodes.function, n.kind()) {
            continue;
        }
        if Mechanics::is(&m.nodes.param, n.kind()) {
            if kids(n, &m.fields.ty).first().is_some_and(|t| text(*t, src).starts_with('*')) {
                for l in kids(n, &m.fields.name) {
                    out.insert(text(l, src).to_string());
                }
            }
            continue;
        }
        stack.extend(named(n));
    }
    out
}

/// `start_at` for a query row, whose node may be a whole field declaration,
/// field access or parameter declaration: the name inside it is the start.
pub fn start_in(cx: &FlowCtx, start_byte: usize, end_byte: usize) -> Option<Start> {
    let m = cx.mech;
    let n = cx.root.descendant_for_byte_range(start_byte, end_byte)?;
    let inner = if Mechanics::is(&m.nodes.field_decl, n.kind()) || Mechanics::is(&m.nodes.param, n.kind()) {
        kids(n, &m.fields.name).first().copied()
    } else if Mechanics::is(&m.nodes.field, n.kind()) {
        kids(n, &m.fields.field).first().copied()
    } else {
        None
    };
    match inner {
        Some(x) => start_at(cx, x.start_byte(), x.end_byte()),
        None => start_at(cx, start_byte, end_byte),
    }
}

/// Names a function declares (parameters, `:=` and `var`), and those of
/// them declared as maps.
fn declared(func: Node, src: &[u8], m: &Mechanics) -> (HashSet<String>, HashSet<String>) {
    let (locals, maps, _) = declared_kinds(func, src, m);
    (locals, maps)
}

/// `declared`, plus the names declared as slices, arrays or strings, whose
/// range key is a position.
fn declared_kinds(func: Node, src: &[u8], m: &Mechanics) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let (mut locals, mut maps, mut seqs) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        let decl = Mechanics::is(&m.nodes.param, n.kind())
            || Mechanics::is(&m.nodes.range, n.kind())
            || (Mechanics::is(&m.nodes.assign, n.kind()) && kids(n, &m.fields.operator).is_empty());
        if decl {
            let is_map = kids(n, &m.fields.ty).first().is_some_and(|t| is_map_expr(*t, m))
                || kids(n, &m.fields.right)
                    .first()
                    .map(|r| named(*r).into_iter().next().unwrap_or(*r))
                    .is_some_and(|v| is_map_expr(v, m));
            let seq_text = |t: &str| t.starts_with('[') || t.starts_with("...") || t == "string" || t.starts_with("make([");
            let is_seq = kids(n, &m.fields.ty).first().is_some_and(|t| seq_text(text(*t, src)))
                || kids(n, &m.fields.right)
                    .first()
                    .map(|r| named(*r).into_iter().next().unwrap_or(*r))
                    .is_some_and(|v| seq_text(text(v, src)));
            let mut names = vec![];
            for l in kids(n, &m.fields.left) {
                if Mechanics::is(&m.nodes.list, l.kind()) {
                    names.extend(named(l));
                } else {
                    names.push(l);
                }
            }
            for x in names.into_iter().filter(|x| Mechanics::is(&m.nodes.name, x.kind())) {
                let t = text(x, src).to_string();
                if is_map {
                    maps.insert(t.clone());
                }
                if is_seq && !Mechanics::is(&m.nodes.range, n.kind()) {
                    seqs.insert(t.clone());
                }
                locals.insert(t);
            }
        }
        stack.extend(named(n));
    }
    (locals, maps, seqs)
}

#[derive(Debug, Clone, PartialEq)]
struct Var {
    origin: usize,
    cond: Option<usize>,
    /// Where the variable holds the data: itself, or fields of an object.
    paths: Paths,
}

impl Var {
    fn new(origin: usize, cond: Option<usize>) -> Var {
        Var { origin, cond, paths: None }
    }
}

type State = BTreeMap<String, Var>;

/// What an expression yields.
#[derive(Debug, Default, Clone)]
struct Val {
    /// Hop whose value the result carries.
    carry: Option<usize>,
    /// First hop read anywhere in the expression, even when the result is cleared.
    used: Option<usize>,
    cond: Option<usize>,
    exact: bool,
    unmodelled: Option<String>,
    /// In-repo function the value passed through.
    via: Option<String>,
    sink: bool,
    calls: usize,
    /// Receiver variables that now carry the data.
    whole: Vec<String>,
    /// Walks inside called in-repo functions, hung under this hop.
    trees: Vec<Hop>,
    /// Where the result holds the data, see `Var::paths`.
    paths: Paths,
    /// Tags after the primary one: field writes and struct tags.
    extra: Vec<String>,
    /// For a call with several results, which result positions carry the data.
    positions: Option<Vec<bool>>,
    /// Variables a called function stored the data in through a pointer,
    /// with the field paths that now hold it.
    writes: Vec<(String, Paths)>,
    /// For the value of an outside call: Some(true) unmodelled, Some(false)
    /// modelled. Its error results take no tracked data (see `error_left`).
    outside: Option<bool>,
}

impl Val {
    fn join(&mut self, o: Val) {
        if self.carry.is_none() {
            self.carry = o.carry;
            self.paths = o.paths;
            self.positions = o.positions;
        } else if o.carry.is_some() {
            self.paths = join_paths(&self.paths, &o.paths);
            self.positions = None;
        }
        self.writes.extend(o.writes);
        for t in o.extra {
            if !self.extra.contains(&t) {
                self.extra.push(t);
            }
        }
        if self.used.is_none() {
            self.used = o.used;
        }
        if self.cond.is_none() {
            self.cond = o.cond;
        }
        if self.unmodelled.is_none() {
            self.unmodelled = o.unmodelled;
        }
        if self.via.is_none() {
            self.via = o.via;
        }
        self.exact = false;
        self.sink |= o.sink;
        self.calls += o.calls;
        self.whole.extend(o.whole);
        self.trees.extend(o.trees);
    }
}

struct HopData {
    parent: usize,
    line: usize,
    tags: Vec<String>,
    unmodelled: Option<String>,
    /// Callee walks, printed before the hops that read this one.
    inner: Vec<Hop>,
    returns: bool,
}

struct Walker<'a, 't> {
    cx: &'a FlowCtx<'a, 't>,
    m: &'a Mechanics,
    start: &'a Start,
    is_end: &'a EndFn<'a>,
    outside: &'a dyn Outside,
    imports: HashMap<String, String>,
    func: String,
    hops: Vec<HopData>,
    by_key: HashMap<usize, usize>,
    /// Names of the functions declared at the top of this file.
    local_fns: Vec<String>,
    /// Byte of a variable start inside the body, not yet reached.
    pending: Option<usize>,
    /// Whether the start node is the bound side of an assignment.
    start_is_decl: bool,
    /// Per result position, whether a return statement carries tracked data.
    ret: Vec<bool>,
    /// Result names, one per position, empty when unnamed.
    results: Vec<String>,
    /// Result positions declared `error`: they never carry tracked data.
    errors: Vec<bool>,
    /// The union of the states at each return and at the end of the body.
    exit: Option<State>,
    /// Depth of function literal bodies being walked: their returns are not
    /// the function's.
    in_closure: usize,
    /// Pointer variables, see `pointer_aliases`.
    pointers: HashSet<String>,
    /// Names each pointer variable aliases.
    aliases: HashMap<String, Vec<String>>,
    /// Names the function declares (parameters and local variables).
    locals: HashSet<String>,
    /// Local names declared as maps.
    local_maps: HashSet<String>,
    /// Local names declared as slices, arrays or strings.
    local_seqs: HashSet<String>,
    /// One frame per open block: each name the block declared, with the outer
    /// binding it shadows (None when the outer name was not tracked).
    scopes: Vec<HashMap<String, Option<Var>>>,
}

/// Walk `func` from `start`. `is_end` marks sink calls, `outside` answers
/// for every other call that receives tracked data.
pub fn analyse_function(
    cx: &FlowCtx,
    func: Node,
    start: &Start,
    is_end: &EndFn,
    outside: &dyn Outside,
) -> FlowResult {
    analyse_function_with(cx, func, start, &[], is_end, outside)
}

/// `analyse_function` that also tracks the variables `also` from the top of
/// the body, as when several parameters receive tracked arguments.
pub fn analyse_function_with(
    cx: &FlowCtx,
    func: Node,
    start: &Start,
    also: &[(String, Paths)],
    is_end: &EndFn,
    outside: &dyn Outside,
) -> FlowResult {
    let m = cx.mech;
    let (locals, local_maps, local_seqs) = declared_kinds(func, cx.src, m);
    let (pointers, aliases) = pointer_aliases(func, cx.src, m);
    let results = go_result_names(func, cx.src);
    let mut w = Walker {
        cx,
        m,
        start,
        is_end,
        outside,
        imports: go_imports(cx.root, cx.src),
        func: function_name(func, cx.src, m),
        hops: vec![HopData { parent: 0, line: start.line, tags: vec!["source".into()], unmodelled: None, inner: vec![], returns: false }],
        by_key: HashMap::new(),
        local_fns: named(cx.root)
            .into_iter()
            .filter(|n| Mechanics::is(&m.nodes.function, n.kind()))
            .filter_map(|n| kids(n, &m.fields.name).first().map(|x| text(*x, cx.src).to_string()))
            .collect(),
        pending: None,
        start_is_decl: false,
        ret: vec![false; results.len()],
        errors: go_result_types(func, cx.src).iter().map(|t| t == "error").collect(),
        results,
        exit: None,
        in_closure: 0,
        pointers,
        aliases,
        locals,
        local_maps,
        local_seqs,
        scopes: vec![],
    };
    let mut st = State::new();
    for (name, paths) in also {
        st.insert(name.clone(), Var { origin: 0, cond: None, paths: paths.clone() });
    }
    let body = kids(func, &m.fields.body);
    if let StartKind::Var(name) = &start.kind {
        let in_body = start.file == cx.file
            && body.iter().any(|b| b.start_byte() <= start.start_byte && start.start_byte < b.end_byte());
        if in_body {
            w.pending = Some(start.start_byte);
            w.start_is_decl = w.is_decl();
        } else {
            st.insert(name.clone(), Var { origin: 0, cond: None, paths: start.paths.clone() });
        }
    }
    for b in body {
        w.walk_stmt(b, &mut st);
    }
    w.record_exit(&st);
    let exit = w.exit.take().unwrap_or_default();
    // A write through a pointer parameter or receiver reaches the caller, so
    // its hop stays in the callee tree the way a return does.
    let ptr_params = pointer_params(func, cx.src, m);
    for (k, v) in &exit {
        if v.origin != 0 && ptr_params.contains(k) {
            w.hops[v.origin].returns = true;
        }
    }
    let exits = exit.into_iter().filter(|(_, v)| v.origin != 0).map(|(k, v)| (k, v.paths)).collect();
    FlowResult { root: w.tree(), returns: w.ret.clone(), exits }
}

impl<'a, 't> Walker<'a, 't> {
    fn is(&self, list: &[String], n: Node) -> bool {
        Mechanics::is(list, n.kind())
    }

    fn t(&self, n: Node) -> &'a str {
        text(n, self.cx.src)
    }

    /// Whether the start node is bound by an assignment, a range header or a
    /// switch alias, so tracking begins after that statement.
    fn is_decl(&self) -> bool {
        let Some(mut x) = self.cx.root.descendant_for_byte_range(self.start.start_byte, self.start.end_byte) else {
            return false;
        };
        while let Some(p) = x.parent() {
            if self.is(&self.m.nodes.function, p) {
                return false;
            }
            let binders = if self.is(&self.m.nodes.assign, p) || self.is(&self.m.nodes.range, p) {
                kids(p, &self.m.fields.left)
            } else {
                kids(p, &self.m.fields.alias)
            };
            if binders.iter().any(|b| b.id() == x.id()) {
                return true;
            }
            x = p;
        }
        false
    }

    /// Some(after) when this leaf holds the pending start.
    fn pending_in(&self, n: Node) -> Option<bool> {
        let b = self.pending?;
        (n.start_byte() <= b && b < n.end_byte()).then_some(self.start_is_decl)
    }

    fn begin(&mut self, st: &mut State) {
        if self.pending.take().is_some() {
            if let StartKind::Var(name) = &self.start.kind {
                st.insert(name.clone(), Var { origin: 0, cond: None, paths: self.start.paths.clone() });
            }
        }
    }

    /// Join `st` into the exit state.
    fn record_exit(&mut self, st: &State) {
        let e = self.exit.get_or_insert_with(State::new);
        for (k, v) in st {
            match e.get_mut(k) {
                Some(old) => {
                    old.paths = join_paths(&old.paths, &v.paths);
                    if old.origin == 0 {
                        old.origin = v.origin;
                    }
                }
                None => {
                    e.insert(k.clone(), v.clone());
                }
            }
        }
    }

    fn mark_ret(&mut self, i: usize) {
        if self.errors.get(i).copied().unwrap_or(false) {
            return;
        }
        if self.ret.len() <= i {
            self.ret.resize(i + 1, false);
        }
        self.ret[i] = true;
    }

    /// Every name aliased with `name`, through chains of aliases.
    fn alias_set(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let mut stack = vec![name.to_string()];
        while let Some(x) = stack.pop() {
            for a in self.aliases.get(&x).into_iter().flatten() {
                if a != name && !out.contains(a) {
                    out.push(a.clone());
                    stack.push(a.clone());
                }
            }
        }
        out
    }

    /// Track `name` and its aliases from hop `id`, holding the data at
    /// `paths` on top of what they already held.
    fn store_through(&self, st: &mut State, name: &str, paths: &Paths, id: usize, cond: Option<usize>) {
        let mut names = vec![name.to_string()];
        names.extend(self.alias_set(name));
        for n in names {
            let p = match st.get(&n) {
                Some(old) => join_paths(&old.paths, paths),
                None => paths.clone(),
            };
            st.insert(n, Var { origin: id, cond, paths: p });
        }
    }

    // ── Scopes ───────────────────────────────────────────────────────────────

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Close the innermost block: every name it declared gets back the outer
    /// binding it shadowed.
    fn pop_scope(&mut self, st: &mut State) {
        let Some(frame) = self.scopes.pop() else { return };
        for (name, outer) in frame {
            match outer {
                Some(v) => {
                    st.insert(name, v);
                }
                None => {
                    st.remove(&name);
                }
            }
        }
    }

    /// `name` is declared in the innermost block. The first declaration there
    /// saves the outer binding, a redeclaration in the same block reuses it.
    fn declare(&mut self, name: &str, st: &State) {
        if let Some(frame) = self.scopes.last_mut() {
            frame.entry(name.to_string()).or_insert_with(|| st.get(name).cloned());
        }
    }

    /// Whether assignment `n` declares its names (`:=`, `var`, a `:=` range).
    fn is_declaration(&self, n: Node) -> bool {
        if self.is(&self.m.nodes.declare, n) {
            return true;
        }
        let mut c = n.walk();
        let found = n.children(&mut c).any(|x| !x.is_named() && self.m.tokens.declare.iter().any(|t| t == x.kind()));
        found
    }

    // ── Hops ───────────────────────────────────────────────────────────────

    /// Record a hop for statement `n`, merging tags when a loop pass sees it again.
    fn hop(&mut self, n: Node, v: &Val, primary: &[String]) -> usize {
        let mut tags: Vec<String> = primary.to_vec();
        for t in &v.extra {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        if let Some(f) = &v.via {
            tags.push(format!("via {}", f));
        }
        if let Some(c) = v.cond {
            tags.push(format!("conditional@{}", c));
        }
        if v.unmodelled.is_some() {
            tags.push("unmodelled".into());
        }
        if v.sink {
            tags.push("sink".into());
        }
        // A call `--max` did not enter leads its row, so the rest of the row
        // reads as it would without the cut.
        if let Some(i) = tags.iter().position(|t| t == "stopped: max depth") {
            let t = tags.remove(i);
            tags.insert(0, t);
        }
        if let Some(&id) = self.by_key.get(&n.start_byte()) {
            for t in tags {
                if !self.hops[id].tags.contains(&t) {
                    self.hops[id].tags.push(t);
                }
            }
            for t in &v.trees {
                if !self.hops[id].inner.contains(t) {
                    self.hops[id].inner.push(t.clone());
                }
            }
            return id;
        }
        let id = self.hops.len();
        let mut inner: Vec<Hop> = vec![];
        for t in &v.trees {
            if !inner.contains(t) {
                inner.push(t.clone());
            }
        }
        self.hops.push(HopData {
            parent: v.carry.or(v.used).unwrap_or(0),
            line: line_of(n),
            tags,
            unmodelled: v.unmodelled.clone(),
            inner,
            returns: false,
        });
        self.by_key.insert(n.start_byte(), id);
        id
    }

    fn tree(&self) -> Hop {
        let mut kids: Vec<Vec<usize>> = vec![vec![]; self.hops.len()];
        for (i, h) in self.hops.iter().enumerate().skip(1) {
            kids[h.parent].push(i);
        }
        self.build(0, &kids)
    }

    fn build(&self, i: usize, kids: &[Vec<usize>]) -> Hop {
        let h = &self.hops[i];
        let (file, func, text) = if i == 0 {
            (self.start.file.clone(), self.start.func.clone(), self.start.text.clone())
        } else {
            (self.cx.file.to_string(), self.func.clone(), line_text(self.cx.src, h.line))
        };
        Hop {
            file,
            line: h.line,
            func,
            text,
            tags: h.tags.clone(),
            unmodelled: h.unmodelled.clone(),
            returns: h.returns,
            into: false,
            children: h.inner.iter().cloned().chain(kids[i].iter().map(|&k| self.build(k, kids))).collect(),
        }
    }

    // ── Statements ─────────────────────────────────────────────────────────

    fn walk_stmt(&mut self, n: Node<'t>, st: &mut State) {
        let m = self.m;
        if self.is(&m.nodes.block, n) {
            self.push_scope();
            for c in named(n) {
                self.walk_stmt(c, st);
            }
            self.pop_scope(st);
        } else if self.is(&m.nodes.branch, n) {
            self.branch(n, st);
        } else if self.is(&m.nodes.loop_, n) {
            self.lp(n, st);
        } else if self.is(&m.nodes.function, n) {
            // A literal is walked by `closures` from the statement holding it.
        } else {
            self.leaf(n, st);
        }
    }

    fn leaf(&mut self, n: Node<'t>, st: &mut State) {
        let p = self.pending_in(n);
        if p == Some(false) {
            self.begin(st);
        }
        // A declaration list is walked spec by spec, each spec finds its own literals.
        let wrapper = !self.is(&self.m.nodes.assign, n) && self.holds_assign(n);
        let returned = if wrapper { vec![] } else { self.closures(n, st) };
        if !returned.is_empty() {
            // A returned closure hands its captured data to the caller.
            if self.in_closure == 0 {
                for i in 0..self.ret.len().max(1) {
                    self.mark_ret(i);
                }
            }
            for id in returned {
                self.hops[id].returns = true;
            }
        }
        if self.is(&self.m.nodes.assign, n) {
            self.assign(n, st, false);
        } else if self.is(&self.m.nodes.send, n) {
            self.send(n, st);
        } else if self.is(&self.m.nodes.return_, n) {
            let exprs = self.flat(named(n));
            let mut v = Val::default();
            let mut pos: Vec<bool> = vec![];
            if exprs.is_empty() {
                // A bare return hands back the named results.
                for r in self.results.clone() {
                    let x = match st.get(&r) {
                        Some(var) => Val {
                            carry: Some(var.origin),
                            used: Some(var.origin),
                            cond: var.cond,
                            paths: var.paths.clone(),
                            ..Val::default()
                        },
                        None => Val::default(),
                    };
                    pos.push(x.carry.is_some());
                    v.join(x);
                }
            } else {
                let one = exprs.len() == 1;
                for c in exprs {
                    let x = self.eval(c, st);
                    match (&x.positions, one) {
                        // `return two(x)` hands back the callee's positions.
                        (Some(p), true) => pos = p.clone(),
                        _ => pos.push(x.carry.is_some()),
                    }
                    v.join(x);
                }
            }
            if self.in_closure == 0 {
                for (i, p) in pos.iter().enumerate() {
                    if *p {
                        self.mark_ret(i);
                    }
                }
                self.record_exit(st);
            }
            if v.carry.is_some() || v.sink || !v.trees.is_empty() {
                let prim: Vec<String> =
                    if v.carry.is_some() || !v.trees.is_empty() { vec!["moved".into()] } else { vec![] };
                let id = self.hop(n, &v, &prim);
                // A value handed back only in `error` positions does not
                // reach the caller.
                let errs = &self.errors;
                let reaches = if self.in_closure == 0 && !pos.is_empty() {
                    pos.iter().enumerate().any(|(i, p)| *p && !errs.get(i).copied().unwrap_or(false))
                } else {
                    v.carry.is_some()
                };
                self.hops[id].returns |= reaches;
            }
        } else if self.holds_assign(n) {
            for c in named(n) {
                self.walk_stmt(c, st);
            }
        } else {
            let v = self.eval(n, st);
            if v.sink || (v.calls > 0 && v.used.is_some()) {
                let prim: Vec<String> = if v.sink { vec![] } else { vec!["moved".into()] };
                let id = self.hop(n, &v, &prim);
                for name in &v.whole {
                    st.insert(name.clone(), Var::new(id, v.cond));
                }
                for (name, paths) in &v.writes {
                    self.store_through(st, name, paths, id, v.cond);
                }
            }
        }
        if p == Some(true) {
            self.begin(st);
        }
    }

    /// Whether `n` wraps assignments, as a declaration list does.
    fn holds_assign(&self, n: Node) -> bool {
        named(n).into_iter().any(|c| {
            self.is(&self.m.nodes.assign, c)
                || (!self.is(&self.m.nodes.function, c) && !self.is(&self.m.nodes.call, c) && self.holds_assign(c))
        })
    }

    /// Function literals in statement `n` that the walk enters: the callee of
    /// a call (Some(call)), an argument of a call, or a returned value (true).
    /// Literals inside other literals are found when their parent is walked.
    fn literals(&self, n: Node<'t>) -> Vec<(Node<'t>, Option<Node<'t>>, bool)> {
        let m = self.m;
        let mut out = vec![];
        let mut stack = named(n);
        stack.reverse();
        while let Some(x) = stack.pop() {
            if !self.is(&m.nodes.function, x) {
                let mut c = named(x);
                c.reverse();
                stack.extend(c);
                continue;
            }
            let Some(p) = x.parent() else { continue };
            let is_call = |c: Node| self.is(&m.nodes.call, c);
            if is_call(p) && kids(p, &m.fields.function).iter().any(|f| f.id() == x.id()) {
                out.push((x, Some(p), false));
            } else if p.parent().is_some_and(|c| is_call(c) && kids(c, &m.fields.arguments).iter().any(|a| a.id() == p.id()))
            {
                out.push((x, None, false));
            } else if self.is(&m.nodes.return_, p)
                || (self.is(&m.nodes.list, p) && p.parent().is_some_and(|r| self.is(&m.nodes.return_, r)))
            {
                out.push((x, None, true));
            }
        }
        out
    }

    /// Parameter names of a function literal, one per position.
    fn literal_params(&self, lit: Node<'t>) -> Vec<String> {
        let mut out = vec![];
        for list in kids(lit, &self.m.fields.params) {
            for d in named(list) {
                let names = kids(d, &self.m.fields.name);
                if names.is_empty() {
                    out.push(String::new());
                }
                for nm in names {
                    out.push(self.t(nm).to_string());
                }
            }
        }
        out
    }

    /// Walk the body of each literal `literals` finds in `n` that captures a
    /// tracked variable or, called in place, receives a tracked argument. The
    /// literal's line becomes a hop tagged `moved, closure` and the captured
    /// variables are tracked from it inside the body. Returns the hops of
    /// literals in return position.
    fn closures(&mut self, n: Node<'t>, st: &mut State) -> Vec<usize> {
        let m = self.m;
        let mut returned = vec![];
        for (lit, call, ret) in self.literals(n) {
            let Some(body) = kids(lit, &m.fields.body).first().copied() else { continue };
            let params = self.literal_params(lit);
            let mut inner = st.clone();
            for p in &params {
                inner.remove(p);
            }
            let mut captured: Vec<String> = vec![];
            let mut stack = vec![body];
            while let Some(x) = stack.pop() {
                if self.is(&m.nodes.name, x) {
                    let t = self.t(x);
                    if inner.contains_key(t) && !captured.iter().any(|c| c == t) {
                        captured.push(t.to_string());
                    }
                }
                stack.extend(named(x));
            }
            let mut bound: Vec<(String, Val)> = vec![];
            if let Some(c) = call {
                let (_, args) = self.call_parts(c);
                for (i, a) in args.iter().enumerate() {
                    let v = self.eval(*a, st);
                    match params.get(i) {
                        Some(p) if v.carry.is_some() && !p.is_empty() && p != "_" => bound.push((p.clone(), v)),
                        _ => {}
                    }
                }
            }
            let from = captured
                .iter()
                .filter_map(|c| inner.get(c))
                .map(|v| Val { carry: Some(v.origin), used: Some(v.origin), cond: v.cond, ..Val::default() })
                .chain(bound.iter().map(|(_, v)| Val { carry: v.carry, used: v.used, cond: v.cond, ..Val::default() }))
                .next();
            let Some(from) = from else { continue };
            let id = self.hop(body, &from, &["moved".into(), "closure".into()]);
            for c in &captured {
                if let Some(v) = inner.get_mut(c) {
                    v.origin = id;
                }
            }
            for (p, v) in bound {
                inner.insert(p, Var { origin: id, cond: v.cond, paths: v.paths });
            }
            let before = inner.clone();
            self.in_closure += 1;
            self.walk_stmt(body, &mut inner);
            self.in_closure -= 1;
            if ret {
                returned.push(id);
            } else {
                // Outer variables the body stored the data in hold it after the call.
                let (own, _) = declared(lit, self.cx.src, m);
                for (name, v) in inner {
                    if own.contains(&name) || before.get(&name) == Some(&v) {
                        continue;
                    }
                    let paths = match st.get(&name) {
                        Some(old) => join_paths(&old.paths, &v.paths),
                        None => v.paths.clone(),
                    };
                    st.insert(name, Var { paths, ..v });
                }
            }
        }
        returned
    }

    fn flat(&self, ns: Vec<Node<'t>>) -> Vec<Node<'t>> {
        let mut out = vec![];
        for n in ns {
            if self.is(&self.m.nodes.list, n) {
                out.extend(named(n));
            } else {
                out.push(n);
            }
        }
        out
    }

    /// The variable an object expression is rooted at: `o` in `o.a.b` or `m[k]`.
    fn root_name(&self, mut n: Node<'t>) -> Option<String> {
        loop {
            if self.is(&self.m.nodes.name, n) {
                return Some(self.t(n).to_string());
            }
            if self.is(&self.m.nodes.call, n) {
                return None;
            }
            // `o.f`, `m[k]`, `&o` and `*p` are all rooted at their operand.
            n = *kids(n, &self.m.fields.operand).first()?;
        }
    }

    /// `left = right`. `element` marks a range header, whose names take one
    /// element of the right side, so they are never `exact`. Storing into a
    /// field of `o` tracks `o` with that field path. Storing into a map
    /// element, a package-level variable or a channel stops (decision 12).
    fn assign(&mut self, n: Node<'t>, st: &mut State, element: bool) {
        let m = self.m;
        let lefts = self.flat(kids(n, &m.fields.left));
        let rights = self.flat(kids(n, &m.fields.right));
        let compound = kids(n, &m.fields.operator)
            .first()
            .map(|o| !m.tokens.plain_assign.iter().any(|p| p == self.t(*o)))
            .unwrap_or(false);
        let rvals: Vec<Val> = rights.iter().map(|r| self.eval(*r, st)).collect();
        let mut all = Val::default();
        for v in &rvals {
            all.join(v.clone());
        }
        let pairwise = lefts.len() == rvals.len() && !element;

        enum Act {
            Set(String, Option<usize>, Paths),
            Drop(String),
            /// An untracked value overwrites this field path of the object.
            Cut(String, Vec<String>),
        }
        let mut acts = vec![];
        let (mut any_carry, mut any_derived) = (false, false);
        let mut field_tags: Vec<String> = vec![];
        let mut stops: Vec<(&'static str, String)> = vec![];
        let mut ptr_stop = false;
        let mut summary = all.clone();
        // `_, err := two(x)`: only the result positions the callee returns
        // the data in carry it.
        let positions = (!pairwise && !element && rvals.len() == 1 && lefts.len() > 1)
            .then(|| rvals[0].positions.clone())
            .flatten();
        for (i, l) in lefts.iter().enumerate() {
            let lt = self.t(*l);
            if m.tokens.blank.iter().any(|b| b == lt) {
                continue;
            }
            let mut v = if pairwise { rvals[i].clone() } else { all.clone() };
            if !pairwise {
                v.exact = false;
            }
            if positions.as_ref().is_some_and(|p| !p.get(i).copied().unwrap_or(false)) {
                v.carry = None;
            }
            if rvals.len() == 1 && self.error_left(*l, n, &rvals[0], lefts.len()) {
                v.carry = None;
            }
            if element && i == 0 && rights.len() == 1 && self.indexed(rights[0]) {
                // The key of a range over a slice, array or string is a position.
                v.carry = None;
            }
            if self.is(&m.nodes.field, *l) && self.field_read(*l) {
                // A write of the start field: the object now holds it.
                v = Val { carry: Some(0), used: Some(0), exact: true, ..Val::default() };
                summary.join(v.clone());
            }
            if compound {
                if let Some(old) = st.get(lt) {
                    let o = Val {
                        carry: Some(old.origin),
                        used: Some(old.origin),
                        cond: old.cond,
                        paths: old.paths.clone(),
                        ..Val::default()
                    };
                    v.join(o.clone());
                    summary.join(o);
                }
            }
            if v.carry.is_none() {
                if self.is(&m.nodes.name, *l) {
                    acts.push(Act::Drop(lt.to_string()));
                } else if !compound && self.is(&m.nodes.field, *l) {
                    if let Some(obj) = self.root_name(*l).filter(|o| self.stop_kind(*l, o).is_none()) {
                        let chain = self.field_chain(*l);
                        if !chain.is_empty() {
                            for a in self.alias_set(&obj) {
                                acts.push(Act::Cut(a, chain.clone()));
                            }
                            acts.push(Act::Cut(obj, chain));
                        }
                    }
                }
                continue;
            }
            let tag = if v.exact && !compound { "exact" } else { "derived" };
            if self.is(&m.nodes.name, *l) {
                if self.is_global(lt) {
                    stops.push(("global", lt.to_string()));
                } else {
                    acts.push(Act::Set(lt.to_string(), v.cond, v.paths.clone()));
                }
            } else if let Some(obj) = self.root_name(*l) {
                if self.is(&m.nodes.field, *l) {
                    if let Some(f) = kids(*l, &m.fields.field).first() {
                        let ft = format!("field {}", self.t(*f));
                        if !field_tags.contains(&ft) {
                            field_tags.push(ft);
                        }
                        if let Some(t) = self.struct_tag(*l) {
                            if !field_tags.contains(&t) {
                                field_tags.push(t);
                            }
                        }
                    }
                }
                match self.stop_kind(*l, &obj) {
                    Some(kind) => stops.push((kind, obj)),
                    None => {
                        let chain = self.field_chain(*l);
                        let p = if chain.is_empty() { v.paths.clone() } else { under(&chain, &v.paths) };
                        // A write through a pointer reaches what it aliases.
                        for a in self.alias_set(&obj) {
                            let pa = match st.get(&a) {
                                Some(old) => join_paths(&old.paths, &p),
                                None => p.clone(),
                            };
                            acts.push(Act::Set(a, v.cond, pa));
                        }
                        // `*p = x` through a pointer of unknown origin.
                        if lt.starts_with('*') && !self.pointers.contains(&obj) {
                            ptr_stop = true;
                        }
                        let p = match st.get(&obj) {
                            Some(old) => join_paths(&old.paths, &p),
                            None => p,
                        };
                        acts.push(Act::Set(obj, v.cond, p));
                    }
                }
            } else {
                // `*f() = x`: the walk cannot tell what is written.
                ptr_stop = true;
            }
            any_carry = true;
            any_derived |= tag == "derived";
        }
        let carried = acts.iter().any(|a| matches!(a, Act::Set(..))) || !stops.is_empty() || ptr_stop;
        let mut id = None;
        if carried || summary.sink || !summary.whole.is_empty() || !summary.trees.is_empty() {
            let mut prim: Vec<String> = match (any_carry, any_derived) {
                (false, _) if (!summary.whole.is_empty() || !summary.trees.is_empty()) && !summary.sink => {
                    vec!["moved".into()]
                }
                (false, _) => vec![],
                (true, true) => vec!["derived".into()],
                (true, false) => vec!["exact".into()],
            };
            prim.extend(field_tags);
            if ptr_stop {
                prim.push("stopped: pointer write".into());
            }
            let readers = self.stop_tags(&stops, &mut prim);
            let hid = self.hop(n, &summary, &prim);
            self.add_readers(hid, readers);
            // An unmodelled result nobody reads is not where a route ends.
            if summary.unmodelled.is_some() && stops.is_empty() && !ptr_stop {
                let set: Vec<String> =
                    acts.iter().filter_map(|a| if let Act::Set(x, ..) = a { Some(x.clone()) } else { None }).collect();
                if !set.is_empty() && !self.read_after(n, &set, n.end_byte()) {
                    self.hops[hid].unmodelled = None;
                }
            }
            id = Some(hid);
        }
        if self.is_declaration(n) {
            for l in &lefts {
                let lt = self.t(*l);
                if self.is(&m.nodes.name, *l) && !m.tokens.blank.iter().any(|b| b == lt) {
                    self.declare(lt, st);
                }
            }
        }
        for a in acts {
            match a {
                Act::Set(name, cond, paths) => {
                    st.insert(name, Var { origin: id.unwrap_or(0), cond, paths });
                }
                Act::Drop(name) => {
                    st.remove(&name);
                }
                Act::Cut(name, chain) => {
                    let empty = match st.get_mut(&name).and_then(|v| v.paths.as_mut()) {
                        Some(ps) => {
                            ps.retain(|p| !p.starts_with(&chain));
                            ps.is_empty()
                        }
                        None => false,
                    };
                    if empty {
                        st.remove(&name);
                    }
                }
            }
        }
        if let Some(id) = id {
            for name in &summary.whole {
                st.insert(name.clone(), Var::new(id, summary.cond));
            }
            for (name, paths) in &summary.writes {
                self.store_through(st, name, paths, id, summary.cond);
            }
        }
    }

    /// Whether left side `l` of assignment `n` takes an error result of an
    /// outside call: a name `err` or a variable declared `error`, bound from
    /// an unmodelled call, or from a modelled one with several results. A
    /// model cannot name result positions, so its `return` is the value.
    fn error_left(&self, l: Node<'t>, n: Node<'t>, v: &Val, lefts: usize) -> bool {
        let applies = match v.outside {
            Some(true) => true,
            Some(false) => lefts > 1,
            None => false,
        };
        if !applies || !self.is(&self.m.nodes.name, l) {
            return false;
        }
        let name = self.t(l);
        name == "err"
            || binding_type(self.cx.root, self.cx.src, self.cx.file, name, line_of(n), &DefIndex::default())
                .is_some_and(|t| t == "error")
    }

    /// Whether range operand `r` is a name declared as a slice, array or
    /// string, so the first range variable is an index, not data.
    fn indexed(&self, r: Node<'t>) -> bool {
        self.is(&self.m.nodes.name, r) && {
            let name = self.t(r);
            !self.local_maps.contains(name) && self.local_seqs.contains(name)
        }
    }

    /// `ch <- v`: a tracked value sent on a channel stops there.
    fn send(&mut self, n: Node<'t>, st: &mut State) {
        let m = self.m;
        let mut v = Val::default();
        for c in kids(n, &m.fields.value) {
            v.join(self.eval(c, st));
        }
        if v.carry.is_some() {
            let ch = kids(n, &m.fields.channel).first().and_then(|c| self.root_name(*c)).unwrap_or_default();
            let mut prim = vec!["moved".to_string()];
            let readers = self.stop_tags(&[("channel", ch)], &mut prim);
            let id = self.hop(n, &v, &prim);
            self.add_readers(id, readers);
        } else if v.sink || !v.trees.is_empty() {
            let prim: Vec<String> = if v.sink { vec![] } else { vec!["moved".into()] };
            self.hop(n, &v, &prim);
        }
    }

    /// Push `stopped: shared <kind>` and a reader count for each stop, and
    /// return the reader rows.
    fn stop_tags(&self, stops: &[(&'static str, String)], prim: &mut Vec<String>) -> Vec<Hop> {
        let mut rows: Vec<Hop> = vec![];
        for (kind, name) in stops {
            let t = format!("stopped: shared {}", kind);
            if !prim.contains(&t) {
                prim.push(t);
            }
            for r in self.readers(name) {
                if !rows.contains(&r) {
                    rows.push(r);
                }
            }
        }
        if !rows.is_empty() {
            let word = if rows.len() == 1 { "reader" } else { "readers" };
            prim.push(format!("{} {}", rows.len(), word));
        }
        rows
    }

    fn add_readers(&mut self, id: usize, rows: Vec<Hop>) {
        for r in rows {
            if !self.hops[id].inner.contains(&r) {
                self.hops[id].inner.push(r);
            }
        }
    }

    /// The other functions that use package-level variable `name`, as rows.
    fn readers(&self, name: &str) -> Vec<Hop> {
        if !self.is_global(name) {
            return vec![];
        }
        self.cx
            .module
            .readers
            .get(name)
            .into_iter()
            .flatten()
            .filter(|r| !(r.file == self.cx.file && r.func == self.func))
            .map(|r| Hop {
                file: r.file.clone(),
                line: r.line,
                func: r.func.clone(),
                text: String::new(),
                tags: vec![],
                unmodelled: None,
                returns: false,
                into: false,
                children: vec![],
            })
            .collect()
    }

    /// Whether `name` is a package-level variable not shadowed in this function.
    fn is_global(&self, name: &str) -> bool {
        self.cx.module.globals.contains_key(name) && !self.locals.contains(name)
    }

    /// The shared state a store through `l` (rooted at `obj`) reaches: a map
    /// element, or a package-level variable.
    fn stop_kind(&self, l: Node<'t>, obj: &str) -> Option<&'static str> {
        let mut x = l;
        let mut through_index = false;
        while !self.is(&self.m.nodes.name, x) {
            through_index |= self.is(&self.m.nodes.index, x);
            match kids(x, &self.m.fields.operand).first() {
                Some(o) => x = *o,
                None => break,
            }
        }
        let global = self.is_global(obj);
        let is_map = if global {
            self.cx.module.globals.get(obj).copied().unwrap_or(false)
        } else {
            self.local_maps.contains(obj)
        };
        if through_index && is_map {
            Some("map")
        } else if global {
            Some("global")
        } else {
            None
        }
    }

    /// Field names from the root object to `l`: `[A, B]` for `o.A.B`.
    /// Element reads and pointer steps add nothing.
    fn field_chain(&self, l: Node<'t>) -> Vec<String> {
        let mut out = vec![];
        let mut x = l;
        while !self.is(&self.m.nodes.name, x) {
            if self.is(&self.m.nodes.field, x) {
                if let Some(f) = kids(x, &self.m.fields.field).first() {
                    out.push(self.t(*f).to_string());
                }
            }
            match kids(x, &self.m.fields.operand).first() {
                Some(o) => x = *o,
                None => break,
            }
        }
        out.reverse();
        out
    }

    /// `tag json:"phone"` for a field access whose field has a struct tag.
    fn struct_tag(&self, sel: Node<'t>) -> Option<String> {
        let f = kids(sel, &self.m.fields.field).first().copied()?;
        let ty = kids(sel, &self.m.fields.operand)
            .first()
            .filter(|o| self.is(&self.m.nodes.name, **o))
            .and_then(|o| binding_type(self.cx.root, self.cx.src, self.cx.file, self.t(*o), line_of(sel), &DefIndex::default()));
        self.cx.module.tag_of(ty.as_deref(), self.t(f))
    }

    /// Conditions carry no data, only their sinks are recorded.
    fn condition(&mut self, n: Node<'t>, st: &State) {
        let v = self.eval(n, st);
        if v.sink || !v.trees.is_empty() {
            let prim: Vec<String> = if v.sink { vec![] } else { vec!["moved".into()] };
            self.hop(n, &v, &prim);
        }
    }

    fn branch(&mut self, n: Node<'t>, st: &mut State) {
        // The header's declarations live until the end of the statement.
        self.push_scope();
        self.branch_inner(n, st);
        self.pop_scope(st);
    }

    fn branch_inner(&mut self, n: Node<'t>, st: &mut State) {
        let m = self.m;
        for init in kids(n, &m.fields.initializer) {
            self.walk_stmt(init, st);
        }
        let conds = kids(n, &m.fields.condition);
        let line = conds.first().map(|c| line_of(*c)).unwrap_or_else(|| line_of(n));
        let aliases = self.flat(kids(n, &m.fields.alias));
        if aliases.is_empty() {
            for c in &conds {
                self.condition(*c, st);
            }
        } else {
            // `switch v := x.(type)` binds v to the switched value.
            let p = self.pending_in(n).filter(|_| self.start_is_decl);
            let mut v = Val::default();
            for c in &conds {
                v.join(self.eval(*c, st));
            }
            v.exact = conds.len() == 1;
            let id = if v.carry.is_some() || v.sink {
                let tag = if v.exact { "exact" } else { "derived" };
                Some(self.hop(n, &v, &[tag.into()]))
            } else {
                None
            };
            for a in &aliases {
                let name = self.t(*a).to_string();
                self.declare(&name, st);
                match id.filter(|_| v.carry.is_some()) {
                    Some(id) => {
                        st.insert(name, Var { origin: id, cond: v.cond, paths: v.paths.clone() });
                    }
                    None => {
                        st.remove(&name);
                    }
                }
            }
            if p.is_some() {
                self.begin(st);
            }
        }

        let mut outs = vec![];
        let cons = kids(n, &m.fields.consequence);
        if let Some(c) = cons.first() {
            let mut s1 = st.clone();
            self.walk_stmt(*c, &mut s1);
            outs.push(s1);
            let mut s2 = st.clone();
            if let Some(a) = kids(n, &m.fields.alternative).first() {
                self.walk_stmt(*a, &mut s2);
            }
            outs.push(s2);
        } else {
            let mut has_default = false;
            for case in named(n).into_iter().filter(|c| Mechanics::is(&m.nodes.case, c.kind())).collect::<Vec<_>>() {
                has_default |= self.is(&m.nodes.default_case, case);
                let labels: Vec<usize> = kids(case, &m.fields.case_label).iter().map(|l| l.id()).collect();
                let mut s = st.clone();
                // Each case body is its own block.
                self.push_scope();
                for c in named(case).into_iter().filter(|c| !labels.contains(&c.id())) {
                    self.walk_stmt(c, &mut s);
                }
                self.pop_scope(&mut s);
                outs.push(s);
            }
            if !has_default {
                outs.push(st.clone());
            }
        }
        *st = merge(&outs, line);
    }

    fn lp(&mut self, n: Node<'t>, st: &mut State) {
        // The header's declarations (`i := 0`, `for _, x := range`) live
        // until the end of the loop and shadow outer names.
        self.push_scope();
        self.lp_inner(n, st);
        self.pop_scope(st);
    }

    fn lp_inner(&mut self, n: Node<'t>, st: &mut State) {
        let m = self.m;
        let line = line_of(n);
        let header = named(n);
        let clause = header.iter().find(|c| self.is(&m.nodes.loop_clause, **c)).copied();
        let range = header.iter().find(|c| self.is(&m.nodes.range, **c)).copied();
        if let Some(c) = clause {
            for init in kids(c, &m.fields.initializer) {
                self.walk_stmt(init, st);
            }
        }
        let body = kids(n, &m.fields.body);
        let mut acc = st.clone();
        let cap = MIN_LOOP_PASSES + 4 * (self.locals.len() + 1);
        let mut converged = false;
        for _ in 0..cap {
            let mut s = acc.clone();
            if let Some(r) = range {
                let p = self.pending_in(r);
                self.assign(r, &mut s, true);
                if p.is_some() {
                    self.begin(&mut s);
                }
            }
            if let Some(c) = clause {
                for cond in kids(c, &m.fields.condition) {
                    self.condition(cond, &s);
                }
            }
            for b in &body {
                self.walk_stmt(*b, &mut s);
            }
            if let Some(c) = clause {
                for u in kids(c, &m.fields.update) {
                    self.walk_stmt(u, &mut s);
                }
            }
            let next = merge(&[acc.clone(), s], line);
            let done = same(&next, &acc);
            acc = next;
            if done {
                converged = true;
                break;
            }
        }
        if !converged {
            let from = acc.values().map(|v| v.origin).max();
            let v = Val { carry: from, used: from, ..Val::default() };
            self.hop(n, &v, &["stopped: loop cap".into()]);
        }
        *st = acc;
    }

    // ── Expressions ────────────────────────────────────────────────────────

    fn eval(&self, n: Node<'t>, st: &State) -> Val {
        let m = self.m;
        if self.is(&m.nodes.function, n) {
            return Val::default();
        }
        if self.is(&m.nodes.name, n) {
            return match st.get(self.t(n)) {
                Some(v) => Val {
                    carry: Some(v.origin),
                    used: Some(v.origin),
                    cond: v.cond,
                    exact: true,
                    paths: v.paths.clone(),
                    ..Val::default()
                },
                None => Val::default(),
            };
        }
        if self.is(&m.nodes.field, n) {
            return self.field(n, st);
        }
        if self.is(&m.nodes.call, n) || self.generic_conversion(n) {
            return self.call(n, st);
        }
        if self.is(&m.nodes.composite, n) {
            return self.composite(n, st);
        }
        let parts = named(n);
        if parts.len() == 1 && (self.is(&m.nodes.list, n) || self.is(&m.nodes.composite, parts[0])) {
            return self.eval(parts[0], st);
        }
        let mut v = Val::default();
        for c in parts {
            v.join(self.eval(c, st));
        }
        v
    }

    /// `o.F`: the start field itself, the part of a tracked object stored
    /// under `F`, or the whole of a tracked value.
    fn field(&self, n: Node<'t>, st: &State) -> Val {
        let m = self.m;
        if self.field_read(n) {
            let mut v = Val { carry: Some(0), used: Some(0), exact: true, ..Val::default() };
            v.extra.extend(self.struct_tag(n));
            return v;
        }
        let ops = kids(n, &m.fields.operand);
        let mut v = Val::default();
        let mut op_exact = false;
        for o in &ops {
            let ov = self.eval(*o, st);
            // The operand's exactness, kept for a read of a stored field.
            op_exact = ops.len() == 1 && ov.exact;
            v.join(ov);
        }
        let Some(f) = kids(n, &m.fields.field).first().copied() else { return v };
        if v.carry.is_none() {
            return v;
        }
        let Some(ps) = v.paths.clone() else {
            v.extra.extend(self.struct_tag(n));
            return v;
        };
        let name = self.t(f);
        let (mut whole, mut rest) = (false, vec![]);
        for p in &ps {
            match self.strip(p, name) {
                Some(r) if r.is_empty() => whole = true,
                Some(r) => {
                    if !rest.contains(&r) {
                        rest.push(r)
                    }
                }
                None => {}
            }
        }
        if !whole && rest.is_empty() {
            // Another field of the object: not tracked.
            return Val { carry: None, used: None, paths: None, extra: vec![], ..v };
        }
        v.exact = op_exact;
        v.paths = if whole { None } else { Some(rest) };
        v.extra.extend(self.struct_tag(n));
        v
    }

    /// What is left of path `p` after reading field `name`, through embedded
    /// structs. None when the read is of another field.
    fn strip(&self, p: &[String], name: &str) -> Option<Vec<String>> {
        let first = p.first()?;
        if first == name {
            return Some(p[1..].to_vec());
        }
        let md = self.cx.module;
        if md.is_embedded(first) {
            // `ab.CustomerPhone` reads `ab.Booking.CustomerPhone`.
            return self.strip(&p[1..], name);
        }
        if md.is_embedded(name) && md.field(name, first).is_some() {
            // `ab.Booking` holds a field stored as `ab.CustomerPhone`.
            return Some(p.to_vec());
        }
        None
    }

    /// `T{F: x}` and `T{x}`: an object holding `x` under field `F`. In a
    /// field start, a write of the start field makes the object hold it.
    fn composite(&self, n: Node<'t>, st: &State) -> Val {
        let m = self.m;
        let md = self.cx.module;
        let ty_node = kids(n, &m.fields.ty).first().copied();
        let ty = ty_node.map(|t| self.t(t).to_string()).unwrap_or_default();
        let is_struct = ty_node.is_some_and(|t| !is_map_expr(t, m)) && !ty.starts_with('[');
        let elems = kids(n, &m.fields.body).first().map(|b| named(*b)).unwrap_or_default();
        let mut out = Val::default();
        let (mut carrying, mut exact) = (0, false);
        for (i, el) in elems.into_iter().enumerate() {
            let (key, value) = if self.is(&m.nodes.keyed, el) {
                let k = kids(el, &m.fields.key).first().map(|k| self.t(*k).to_string());
                (k, kids(el, &m.fields.value).first().copied())
            } else {
                (md.positional(&ty, i), Some(el))
            };
            let Some(mut value) = value else { continue };
            while self.is(&m.nodes.element, value) && named(value).len() == 1 {
                value = named(value)[0];
            }
            let key = key.filter(|_| is_struct);
            let mut v = self.eval(value, st);
            if let (Some(k), StartKind::Field { name, owner }) = (&key, &self.start.kind) {
                if k == name && owner.as_ref().is_none_or(|o| md.promotes(&ty, o)) {
                    v = Val { carry: Some(0), used: Some(0), exact: true, ..Val::default() };
                }
            }
            if v.carry.is_none() {
                out.join(v);
                continue;
            }
            carrying += 1;
            exact = v.exact;
            if let Some(k) = &key {
                v.paths = under(std::slice::from_ref(k), &v.paths);
                v.extra.insert(0, format!("field {}", k));
                v.extra.extend(md.tag_of(Some(&ty), k));
            }
            out.join(v);
        }
        out.exact = carrying == 1 && exact;
        out
    }

    /// Whether `n` is `F[T](x)`, which parses as a conversion to a generic
    /// type but calls the generic function `F`.
    fn generic_conversion(&self, n: Node<'t>) -> bool {
        self.is(&self.m.nodes.conversion, n)
            && kids(n, &self.m.fields.ty).first().is_some_and(|t| self.is(&self.m.nodes.generic, *t))
    }

    /// The callee and arguments of a call, with type arguments dropped:
    /// `JSON[T](w, v)` calls `JSON`, `ExtractForm[T](r)` calls `ExtractForm`.
    fn call_parts(&self, n: Node<'t>) -> (Option<Node<'t>>, Vec<Node<'t>>) {
        let m = self.m;
        if self.generic_conversion(n) {
            let f = kids(n, &m.fields.ty).first().and_then(|g| kids(*g, &m.fields.ty).first().copied());
            return (f, kids(n, &m.fields.operand));
        }
        let mut f = kids(n, &m.fields.function).first().copied();
        while let Some(x) = f {
            if self.is(&m.nodes.index, x) {
                f = kids(x, &m.fields.operand).first().copied();
            } else if self.is(&m.nodes.generic, x) {
                f = kids(x, &m.fields.ty).first().copied();
            } else {
                break;
            }
        }
        let args = kids(n, &m.fields.arguments).first().map(|a| named(*a)).unwrap_or_default();
        (f, args)
    }

    /// `path.T` for a receiver built by a constructor call `pkg.NewT(...)`,
    /// so `json.NewDecoder(r).Decode` finds the `Decoder` model.
    /// The type of expression `n` as (base name, `path.T` when it has a
    /// package part): a variable's declared type, or the result type of the
    /// call it is bound from, a field selector's declared type, or the
    /// result type of a call (`NewT` constructors and `RESULT_TYPES`).
    fn expr_type(&self, n: Node<'t>, st: &State, depth: usize) -> Option<(String, Option<String>)> {
        let m = self.m;
        if depth > 6 {
            return None;
        }
        let from_path = |p: String| Some((p.rsplit('.').next().unwrap_or("").to_string(), Some(p)));
        if self.is(&m.nodes.name, n) {
            let on = self.t(n);
            if self.imports.contains_key(on) && !st.contains_key(on) {
                return None;
            }
            let q = binding_type_qualified(self.cx.root, self.cx.src, self.cx.file, on, line_of(n), &DefIndex::default());
            if let Some(q) = q {
                let path = qualify_type(&q, &self.imports);
                return Some((q.rsplit('.').next().unwrap_or("").to_string(), path));
            }
            let call = self.bound_from(n)?;
            return self.expr_type(call, st, depth + 1);
        }
        if self.is(&m.nodes.field, n) {
            return field_type_path(self.cx, n, line_of(n));
        }
        if !self.is(&m.nodes.call, n) {
            return None;
        }
        if let Some(t) = self.ctor_type(n, st) {
            return from_path(t);
        }
        let f = kids(n, &m.fields.function).first().copied()?;
        if self.is(&m.nodes.name, f) {
            // A function in the module.
            return self.cx.module.results.get(self.t(f)).cloned();
        }
        if !self.is(&m.nodes.field, f) {
            return None;
        }
        let o = kids(f, &m.fields.operand).first().copied()?;
        let meth = self.t(*kids(f, &m.fields.field).first()?);
        let on = self.t(o);
        if self.is(&m.nodes.name, o) && !st.contains_key(on) {
            if let Some(path) = self.imports.get(on) {
                return result_type(Some(&format!("{}.{}", path, meth)), None, None).and_then(from_path);
            }
        }
        match self.expr_type(o, st, depth + 1)? {
            (_, Some(recv)) => result_type(None, Some(&recv), Some(meth)).and_then(from_path),
            // A method on a type the module declares.
            (base, None) => self.cx.module.results.get(&format!("{}.{}", base, meth)).cloned(),
        }
    }

    fn bound_from(&self, name: Node<'t>) -> Option<Node<'t>> {
        bound_from(self.m, self.cx.src, name)
    }

    /// Whether any of `names` is read after byte `after` in the function
    /// holding `n`. Being assigned to, and `_ = name`, are not reads.
    fn read_after(&self, n: Node<'t>, names: &[String], after: usize) -> bool {
        let m = self.m;
        let Some(func) = enclosing_function(self.cx.root, m, n.start_byte()) else { return true };
        let mut stack = vec![func];
        while let Some(x) = stack.pop() {
            stack.extend(named(x));
            if x.start_byte() < after || !self.is(&m.nodes.name, x) || !names.iter().any(|v| v == self.t(x)) {
                continue;
            }
            let Some(p) = x.parent() else { return true };
            let p = if self.is(&m.nodes.list, p) { p.parent().unwrap_or(p) } else { p };
            if self.is(&m.nodes.assign, p) {
                let lefts = self.flat(kids(p, &m.fields.left));
                if lefts.iter().any(|l| l.id() == x.id()) {
                    continue;
                }
                let blank = |l: &Node| m.tokens.blank.iter().any(|b| b == self.t(*l));
                let rights = self.flat(kids(p, &m.fields.right));
                if lefts.iter().all(blank) && rights.iter().any(|r| r.id() == x.id()) {
                    continue;
                }
            }
            return true;
        }
        false
    }

    fn ctor_type(&self, recv: Node<'t>, st: &State) -> Option<String> {
        if !self.is(&self.m.nodes.call, recv) {
            return None;
        }
        let f = kids(recv, &self.m.fields.function).first().copied()?;
        let pkg = kids(f, &self.m.fields.operand).first().copied()?;
        let name = self.t(*kids(f, &self.m.fields.field).first()?);
        let ty = name.strip_prefix("New").filter(|t| t.starts_with(|c: char| c.is_uppercase()))?;
        let on = self.t(pkg);
        if st.contains_key(on) {
            return None;
        }
        self.imports.get(on).map(|p| format!("{}.{}", p, ty))
    }

    /// Whether `n` reads the start field.
    fn field_read(&self, n: Node<'t>) -> bool {
        let StartKind::Field { name, owner } = &self.start.kind else {
            return false;
        };
        let Some(f) = kids(n, &self.m.fields.field).first().copied() else {
            return false;
        };
        if self.t(f) != name {
            return false;
        }
        let Some(owner) = owner else {
            return true;
        };
        // The type of the operand just before the field: `o.A` in `o.A.F`.
        match kids(n, &self.m.fields.operand).first().and_then(|o| operand_type(self.cx, *o, line_of(n))) {
            Some(t) => &t == owner || self.cx.module.promotes(&t, owner),
            None => true,
        }
    }

    /// The variable an argument names, through `&` and `*`: `u` in `&u`.
    fn object_name(&self, mut n: Node<'t>) -> Option<String> {
        loop {
            if self.is(&self.m.nodes.name, n) {
                return Some(self.t(n).to_string());
            }
            if self.is(&self.m.nodes.field, n) || self.is(&self.m.nodes.index, n) || self.is(&self.m.nodes.call, n) {
                return None;
            }
            n = *kids(n, &self.m.fields.operand).first()?;
        }
    }

    /// Untracked arguments whose type declares the start field.
    fn whole_args(&self, n: Node<'t>, args: &[Node<'t>], argvals: &[Val]) -> Vec<usize> {
        let StartKind::Field { owner: Some(owner), .. } = &self.start.kind else {
            return vec![];
        };
        let line = line_of(n);
        (0..args.len())
            .filter(|&i| argvals[i].carry.is_none())
            .filter(|&i| {
                self.object_name(args[i]).is_some_and(|v| {
                    binding_type(self.cx.root, self.cx.src, self.cx.file, &v, line, &DefIndex::default())
                        .is_some_and(|t| &t == owner || self.cx.module.promotes(&t, owner))
                })
            })
            .collect()
    }

    fn call(&self, n: Node<'t>, st: &State) -> Val {
        let m = self.m;
        let (func, args) = self.call_parts(n);
        let mut argvals: Vec<Val> = args.iter().map(|a| self.eval(*a, st)).collect();
        let mut recv_node = None;
        let recv = match func {
            Some(f) if self.is(&m.nodes.field, f) => {
                recv_node = kids(f, &m.fields.operand).first().copied();
                if self.field_read(f) {
                    // A read of the start field as a callee is a receiver use.
                    Some(self.eval(f, st))
                } else {
                    // The method name is not a field read of the receiver.
                    let mut r = Val::default();
                    for o in kids(f, &m.fields.operand) {
                        r.join(self.eval(o, st));
                    }
                    Some(r)
                }
            }
            Some(f) if !self.is(&m.nodes.name, f) => Some(self.eval(f, st)),
            _ => None,
        };
        let tracked = |vals: &[Val]| -> Vec<usize> {
            vals.iter().enumerate().filter(|(_, v)| v.carry.is_some()).map(|(i, _)| i).collect()
        };
        let mut tracked_args = tracked(&argvals);
        let recv_tracked = recv.as_ref().is_some_and(|r| r.carry.is_some());
        let whole_args = self.whole_args(n, &args, &argvals);
        let mut out = Val::default();
        let fold = |argvals: &[Val], out: &mut Val| -> Val {
            let mut inner = Val::default();
            for v in argvals.iter().cloned().chain(recv.clone()) {
                inner.join(v);
            }
            *out = Val {
                used: inner.used,
                cond: inner.cond,
                unmodelled: inner.unmodelled.clone(),
                via: inner.via.clone(),
                sink: inner.sink,
                calls: inner.calls,
                whole: inner.whole.clone(),
                extra: inner.extra.clone(),
                writes: inner.writes.clone(),
                // Walks of calls nested in the arguments, innermost first.
                trees: inner.trees.clone(),
                ..Val::default()
            };
            inner
        };
        let mut inner = fold(&argvals, &mut out);
        if tracked_args.is_empty() && !recv_tracked && whole_args.is_empty() {
            return out;
        }
        if func.is_some_and(|f| self.is(&m.nodes.function, f)) {
            // A literal called in place: its body was walked by `closures`.
            return out;
        }
        let is_end = (self.is_end)(n, self.cx.src);
        // A sink that takes `&x` may still write the data into x (a template
        // rendering into a buffer), so its effect is asked for those writes.
        let sink_writes = !tracked_args.is_empty() && is_end && args.iter().any(|a| self.t(*a).starts_with('&'));
        if !tracked_args.is_empty() && is_end && !sink_writes {
            out.calls += 1;
            out.sink = true;
            out.carry = inner.carry;
            out.paths = inner.paths;
            return out;
        }
        let callee = func.map(|f| self.t(f).to_string()).unwrap_or_default();
        let mut site = CallSite {
            node: n,
            callee: callee.clone(),
            package: None,
            method: None,
            recv_type: None,
            recv_path: None,
            tracked_args: tracked_args.clone(),
            arg_paths: tracked_args.iter().map(|&i| argvals[i].paths.clone()).collect(),
            recv_tracked,
            recv_paths: recv.as_ref().filter(|r| r.carry.is_some()).and_then(|r| r.paths.clone()),
            whole_args: whole_args.clone(),
            local: !callee.contains(['.', '(', '[']) && self.local_fns.iter().any(|x| *x == callee),
        };
        if let (Some(f), Some(o)) = (func, recv_node) {
            site.method = kids(f, &m.fields.field).first().map(|x| self.t(*x).to_string());
            let on = self.t(o);
            match self.imports.get(on) {
                Some(path) if self.is(&m.nodes.name, o) && !st.contains_key(on) => site.package = Some(path.clone()),
                _ => {
                    if let Some((t, p)) = self.expr_type(o, st, 0) {
                        site.recv_type = Some(t);
                        site.recv_path = p;
                    }
                }
            }
        }
        let effect = self.outside.effect(&site);
        if sink_writes {
            match &effect {
                CallEffect::Into { writes, .. } => {
                    for (slot, paths) in writes {
                        let Some(t) = (match slot {
                            Slot::Arg(i) => args.get(*i).copied(),
                            Slot::Receiver => recv_node,
                            _ => None,
                        }) else {
                            continue;
                        };
                        if let Some(name) = self.root_name(t) {
                            let chain = self.field_chain(t);
                            let p = if chain.is_empty() { paths.clone() } else { under(&chain, paths) };
                            out.writes.push((name, p));
                        }
                    }
                }
                CallEffect::Model { pass, .. } => {
                    for p in pass {
                        let from_tracked = match p.from {
                            Slot::Arg(i) => argvals.get(i).is_some_and(|v| v.carry.is_some()),
                            Slot::AllArgs => !tracked_args.is_empty(),
                            Slot::Receiver => recv_tracked,
                            Slot::Return => false,
                        };
                        let target = match p.to {
                            Slot::Arg(j) => args.get(j).copied(),
                            Slot::Receiver => recv_node,
                            _ => None,
                        };
                        if let Some(name) = target.filter(|_| from_tracked).and_then(|t| self.root_name(t)) {
                            if !out.whole.contains(&name) {
                                out.whole.push(name);
                            }
                        }
                    }
                }
                _ => {}
            }
            out.calls += 1;
            out.sink = true;
            out.carry = inner.carry;
            out.paths = inner.paths;
            return out;
        }
        if matches!(effect, CallEffect::Model { whole: true, .. }) && !whole_args.is_empty() {
            // The object carries every field, the start field among them.
            for &i in &whole_args {
                argvals[i] = Val { carry: Some(0), used: Some(0), ..Val::default() };
            }
            tracked_args = tracked(&argvals);
            inner = fold(&argvals, &mut out);
        }
        if tracked_args.is_empty() && !recv_tracked {
            return out;
        }
        out.calls += 1;
        if !tracked_args.is_empty() && is_end {
            out.sink = true;
            out.carry = inner.carry;
            out.paths = inner.paths;
            return out;
        }
        let keep = |out: &mut Val| {
            out.carry = inner.carry;
            out.paths = inner.paths.clone();
        };
        out.outside = match &effect {
            CallEffect::Unknown => Some(true),
            CallEffect::Pass | CallEffect::Clear | CallEffect::Whole | CallEffect::Model { .. } => Some(false),
            _ => None,
        };
        match effect {
            CallEffect::Unknown => {
                keep(&mut out);
                out.unmodelled.get_or_insert(callee);
            }
            CallEffect::Pass => keep(&mut out),
            CallEffect::Clear => {}
            CallEffect::Whole => {
                keep(&mut out);
                if let Some(name) = recv_node.and_then(|o| self.root_name(o)) {
                    out.whole.push(name);
                }
            }
            CallEffect::Via(name) => {
                keep(&mut out);
                out.via.get_or_insert(name);
            }
            CallEffect::Cut(name) => {
                keep(&mut out);
                out.via.get_or_insert(name);
                let tag = "stopped: max depth".to_string();
                if !out.extra.contains(&tag) {
                    out.extra.push(tag);
                }
            }
            CallEffect::Into { name, returns, writes, trees } => {
                if returns.iter().any(|r| *r) {
                    keep(&mut out);
                    if returns.len() > 1 {
                        out.positions = Some(returns);
                    }
                }
                for (slot, paths) in writes {
                    let target = match slot {
                        Slot::Arg(i) => args.get(i).copied(),
                        Slot::Receiver => recv_node,
                        _ => None,
                    };
                    let Some(t) = target else { continue };
                    if let Some(name) = self.root_name(t) {
                        let chain = self.field_chain(t);
                        let p = if chain.is_empty() { paths } else { under(&chain, &paths) };
                        out.writes.push((name, p));
                    }
                }
                out.via.get_or_insert(name);
                out.trees.extend(trees);
            }
            CallEffect::Stop { tag, rows } => {
                if !out.extra.contains(&tag) {
                    out.extra.push(tag);
                }
                out.trees.extend(rows);
            }
            CallEffect::Model { pass, .. } => {
                let all: Vec<usize> = (0..args.len()).collect();
                for p in pass {
                    let from: Vec<&Val> = match p.from {
                        Slot::Arg(i) => argvals.get(i).into_iter().collect(),
                        Slot::AllArgs => argvals.iter().collect(),
                        Slot::Receiver => recv.iter().collect(),
                        Slot::Return => vec![],
                    };
                    let Some(fv) = from.iter().find(|v| v.carry.is_some()) else {
                        continue;
                    };
                    let targets: Vec<Node> = match p.to {
                        Slot::Return => {
                            if out.carry.is_none() {
                                out.carry = fv.carry;
                                out.paths = fv.paths.clone();
                            }
                            vec![]
                        }
                        Slot::Arg(j) => args.get(j).copied().into_iter().collect(),
                        Slot::AllArgs => all.iter().map(|&j| args[j]).collect(),
                        Slot::Receiver => recv_node.into_iter().collect(),
                    };
                    for t in targets {
                        if let Some(name) = self.root_name(t) {
                            if !out.whole.contains(&name) {
                                out.whole.push(name);
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// Join the states at the end of each branch. A variable tracked on only some
/// branches stays tracked with `conditional@line`.
fn merge(states: &[State], line: usize) -> State {
    let mut out = State::new();
    for s in states {
        for (name, v) in s {
            if out.contains_key(name) {
                continue;
            }
            let present: Vec<&Var> = states.iter().filter_map(|x| x.get(name)).collect();
            let cond = present.iter().find_map(|p| p.cond);
            let cond = if present.len() == states.len() { cond } else { Some(cond.unwrap_or(line)) };
            let paths = present.iter().skip(1).fold(present[0].paths.clone(), |a, p| join_paths(&a, &p.paths));
            out.insert(name.clone(), Var { origin: v.origin, cond, paths });
        }
    }
    out
}

fn same(a: &State, b: &State) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| w.cond == v.cond && w.paths == v.paths))
}
