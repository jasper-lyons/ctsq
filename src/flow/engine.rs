//! The statement walk of decision 10.

use std::collections::{BTreeMap, HashMap};

use tree_sitter::Node;

use super::models::qualify_type;
use super::{CallEffect, CallSite, EndFn, FlowCtx, FlowResult, Hop, Outside, Start, StartKind};
use crate::config::{Mechanics, Slot};
use crate::resolve::go::{binding_type, binding_type_qualified, go_imports};
use crate::resolve::DefIndex;

const MAX_LOOP_PASSES: usize = 32;

fn text<'s>(n: Node, src: &'s [u8]) -> &'s str {
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
fn kids<'t>(n: Node<'t>, names: &[String]) -> Vec<Node<'t>> {
    let mut out = vec![];
    for name in names {
        let mut c = n.walk();
        out.extend(n.children_by_field_name(name, &mut c));
    }
    out.sort_by_key(|k| k.start_byte());
    out.dedup_by_key(|k| k.id());
    out
}

fn named<'t>(n: Node<'t>) -> Vec<Node<'t>> {
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
            kids(p, &m.fields.operand)
                .first()
                .filter(|o| Mechanics::is(&m.nodes.name, o.kind()))
                .and_then(|o| binding_type(cx.root, cx.src, cx.file, text(*o, cx.src), line, &DefIndex::default()))
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

#[derive(Debug, Clone, PartialEq)]
struct Var {
    origin: usize,
    cond: Option<usize>,
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
}

impl Val {
    fn join(&mut self, o: Val) {
        if self.carry.is_none() {
            self.carry = o.carry;
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
    /// Whether a return statement carries tracked data.
    returns: bool,
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
    also: &[String],
    is_end: &EndFn,
    outside: &dyn Outside,
) -> FlowResult {
    let m = cx.mech;
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
        returns: false,
    };
    let mut st = State::new();
    for name in also {
        st.insert(name.clone(), Var { origin: 0, cond: None });
    }
    let body = kids(func, &m.fields.body);
    if let StartKind::Var(name) = &start.kind {
        let in_body = start.file == cx.file
            && body.iter().any(|b| b.start_byte() <= start.start_byte && start.start_byte < b.end_byte());
        if in_body {
            w.pending = Some(start.start_byte);
            w.start_is_decl = w.is_decl();
        } else {
            st.insert(name.clone(), Var { origin: 0, cond: None });
        }
    }
    for b in body {
        w.walk_stmt(b, &mut st);
    }
    FlowResult { root: w.tree(), returns: w.returns }
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
                st.insert(name.clone(), Var { origin: 0, cond: None });
            }
        }
    }

    // ── Hops ───────────────────────────────────────────────────────────────

    /// Record a hop for statement `n`, merging tags when a loop pass sees it again.
    fn hop(&mut self, n: Node, v: &Val, primary: &[String]) -> usize {
        let mut tags: Vec<String> = primary.to_vec();
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
            for c in named(n) {
                self.walk_stmt(c, st);
            }
        } else if self.is(&m.nodes.branch, n) {
            self.branch(n, st);
        } else if self.is(&m.nodes.loop_, n) {
            self.lp(n, st);
        } else if self.is(&m.nodes.function, n) {
            // Closures are not walked.
        } else {
            self.leaf(n, st);
        }
    }

    fn leaf(&mut self, n: Node<'t>, st: &mut State) {
        let p = self.pending_in(n);
        if p == Some(false) {
            self.begin(st);
        }
        if self.is(&self.m.nodes.assign, n) {
            self.assign(n, st, false);
        } else if self.is(&self.m.nodes.return_, n) {
            let mut v = Val::default();
            for c in named(n) {
                v.join(self.eval(c, st));
            }
            self.returns |= v.carry.is_some();
            if v.carry.is_some() || v.sink || !v.trees.is_empty() {
                let prim: Vec<String> =
                    if v.carry.is_some() || !v.trees.is_empty() { vec!["moved".into()] } else { vec![] };
                let id = self.hop(n, &v, &prim);
                self.hops[id].returns |= v.carry.is_some();
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
                    st.insert(name.clone(), Var { origin: id, cond: v.cond });
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
    /// element of the right side, so they are never `exact`.
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
            Set(String, Option<usize>),
            Drop(String),
        }
        let mut acts = vec![];
        let (mut any_carry, mut any_derived) = (false, false);
        let mut field_tag: Option<String> = None;
        let mut summary = all.clone();
        for (i, l) in lefts.iter().enumerate() {
            let lt = self.t(*l);
            if m.tokens.blank.iter().any(|b| b == lt) {
                continue;
            }
            let mut v = if pairwise { rvals[i].clone() } else { all.clone() };
            if !pairwise {
                v.exact = false;
            }
            if compound {
                if let Some(old) = st.get(lt) {
                    v.join(Val { carry: Some(old.origin), used: Some(old.origin), cond: old.cond, ..Val::default() });
                    summary.join(Val { carry: Some(old.origin), used: Some(old.origin), cond: old.cond, ..Val::default() });
                }
            }
            if v.carry.is_none() {
                if self.is(&m.nodes.name, *l) {
                    acts.push(Act::Drop(lt.to_string()));
                }
                continue;
            }
            let tag = if v.exact && !compound { "exact" } else { "derived" };
            if self.is(&m.nodes.name, *l) {
                acts.push(Act::Set(lt.to_string(), v.cond));
            } else if let Some(obj) = self.root_name(*l) {
                if self.is(&m.nodes.field, *l) {
                    if let Some(f) = kids(*l, &m.fields.field).first() {
                        field_tag.get_or_insert(format!("field {}", self.t(*f)));
                    }
                }
                acts.push(Act::Set(obj, v.cond));
            }
            any_carry = true;
            any_derived |= tag == "derived";
        }
        let carried = acts.iter().any(|a| matches!(a, Act::Set(..)));
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
            if let Some(f) = field_tag {
                prim.push(f);
            }
            id = Some(self.hop(n, &summary, &prim));
        }
        for a in acts {
            match a {
                Act::Set(name, cond) => {
                    st.insert(name, Var { origin: id.unwrap_or(0), cond });
                }
                Act::Drop(name) => {
                    st.remove(&name);
                }
            }
        }
        if let Some(id) = id {
            for name in &summary.whole {
                st.insert(name.clone(), Var { origin: id, cond: summary.cond });
            }
        }
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
                match id.filter(|_| v.carry.is_some()) {
                    Some(id) => {
                        st.insert(name, Var { origin: id, cond: v.cond });
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
                for c in named(case).into_iter().filter(|c| !labels.contains(&c.id())) {
                    self.walk_stmt(c, &mut s);
                }
                outs.push(s);
            }
            if !has_default {
                outs.push(st.clone());
            }
        }
        *st = merge(&outs, line);
    }

    fn lp(&mut self, n: Node<'t>, st: &mut State) {
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
        for _ in 0..MAX_LOOP_PASSES {
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
                break;
            }
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
                Some(v) => Val { carry: Some(v.origin), used: Some(v.origin), cond: v.cond, exact: true, ..Val::default() },
                None => Val::default(),
            };
        }
        if self.is(&m.nodes.field, n) {
            if self.field_read(n) {
                return Val { carry: Some(0), used: Some(0), exact: true, ..Val::default() };
            }
            let mut v = Val::default();
            for o in kids(n, &m.fields.operand) {
                v.join(self.eval(o, st));
            }
            return v;
        }
        if self.is(&m.nodes.call, n) {
            return self.call(n, st);
        }
        let parts = named(n);
        if parts.len() == 1 && self.is(&m.nodes.list, n) {
            return self.eval(parts[0], st);
        }
        let mut v = Val::default();
        for c in parts {
            v.join(self.eval(c, st));
        }
        v
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
        match kids(n, &self.m.fields.operand).first() {
            Some(o) if self.is(&self.m.nodes.name, *o) => {
                match binding_type(self.cx.root, self.cx.src, self.cx.file, self.t(*o), line_of(n), &DefIndex::default()) {
                    Some(t) => &t == owner,
                    None => true,
                }
            }
            _ => true,
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
                    binding_type(self.cx.root, self.cx.src, self.cx.file, &v, line, &DefIndex::default()).as_deref()
                        == Some(owner.as_str())
                })
            })
            .collect()
    }

    fn call(&self, n: Node<'t>, st: &State) -> Val {
        let m = self.m;
        let func = kids(n, &m.fields.function).first().copied();
        let args: Vec<Node> = kids(n, &m.fields.arguments).first().map(|a| named(*a)).unwrap_or_default();
        let mut argvals: Vec<Val> = args.iter().map(|a| self.eval(*a, st)).collect();
        let mut recv_node = None;
        let recv = match func {
            Some(f) if self.is(&m.nodes.field, f) => {
                recv_node = kids(f, &m.fields.operand).first().copied();
                // A read of the start field as a callee is a receiver use.
                Some(self.eval(f, st))
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
                ..Val::default()
            };
            inner
        };
        let mut inner = fold(&argvals, &mut out);
        if tracked_args.is_empty() && !recv_tracked && whole_args.is_empty() {
            return out;
        }
        let is_end = (self.is_end)(n, self.cx.src);
        if !tracked_args.is_empty() && is_end {
            out.calls += 1;
            out.sink = true;
            out.carry = inner.carry;
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
            recv_tracked,
            whole_args: whole_args.clone(),
            local: func.is_some_and(|f| self.is(&m.nodes.name, f)) && self.local_fns.iter().any(|x| *x == callee),
        };
        if let (Some(f), Some(o)) = (func, recv_node) {
            site.method = kids(f, &m.fields.field).first().map(|x| self.t(*x).to_string());
            if self.is(&m.nodes.name, o) {
                let on = self.t(o);
                match self.imports.get(on) {
                    Some(path) if !st.contains_key(on) => site.package = Some(path.clone()),
                    _ => {
                        let q = binding_type_qualified(
                            self.cx.root,
                            self.cx.src,
                            self.cx.file,
                            on,
                            line_of(n),
                            &DefIndex::default(),
                        );
                        site.recv_path = q.as_deref().and_then(|t| qualify_type(t, &self.imports));
                        site.recv_type = q.map(|t| t.rsplit('.').next().unwrap_or("").to_string());
                    }
                }
            }
        }
        let effect = self.outside.effect(&site);
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
            return out;
        }
        match effect {
            CallEffect::Unknown => {
                out.carry = inner.carry;
                out.unmodelled.get_or_insert(callee);
            }
            CallEffect::Pass => out.carry = inner.carry,
            CallEffect::Clear => {}
            CallEffect::Whole => {
                out.carry = inner.carry;
                if let Some(name) = recv_node.and_then(|o| self.root_name(o)) {
                    out.whole.push(name);
                }
            }
            CallEffect::Via(name) => {
                out.carry = inner.carry;
                out.via.get_or_insert(name);
            }
            CallEffect::Into { name, returns, trees } => {
                if returns {
                    out.carry = inner.carry;
                }
                out.via.get_or_insert(name);
                out.trees.extend(trees);
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
                    let Some(carry) = from.iter().find_map(|v| v.carry) else {
                        continue;
                    };
                    let targets: Vec<Node> = match p.to {
                        Slot::Return => {
                            out.carry.get_or_insert(carry);
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
            out.insert(name.clone(), Var { origin: v.origin, cond });
        }
    }
    out
}

fn same(a: &State, b: &State) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| w.cond == v.cond))
}
