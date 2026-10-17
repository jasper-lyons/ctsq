//! The data-flow combinators of `ctsq path`: `=>` (one hop) and `==>`
//! (eventually). The head segment's rows give the start points, the tail
//! segment's rows (if any) say which calls are sinks. The walk inside each
//! function is `flow::analyse_function`. Calls into functions in the repo are
//! followed through `CallGraph.sites`: the walk continues in the callee from
//! the parameters that received tracked arguments, and comes back through
//! the callee's return.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

use super::print::{plural, row};
use super::{segment_rows, split_query};
use crate::config::{self, gomod, LangConfig, Slot};
use crate::flow::{template, widen};
use crate::flow::{
    self, upward, analyse_function, analyse_function_with, enclosing_function, start_at, start_in, CallEffect, CallSite,
    EndFn, FlowCtx, FlowResult, Hop, ModelOutside, Module, Outside, Paths, StartKind,
};
use crate::query::ast::{Combinator, Query};
use crate::resolve::go::{binding_type_qualified, go_imports, go_signatures, GoSignature};
use crate::resolve::scoped::FlowGraph;
use crate::resolve::{DefIndex, DefKind, TreeStore};

#[derive(Debug, Clone)]
pub struct FlowOptions {
    /// Deepest chain of in-repo calls the walk follows.
    pub max: usize,
    /// Keep only hops carrying one of these tags, with their ancestors. Only
    /// routes with such a hop count.
    pub only: Vec<String>,
    /// List the implementations of module interfaces that live outside the
    /// module (`--expand`).
    pub expand: bool,
}

impl Default for FlowOptions {
    fn default() -> Self {
        FlowOptions { max: 12, only: vec![], expand: false }
    }
}

/// The printed trees and the number of routes found.
#[derive(Debug, Clone)]
pub struct FlowReport {
    pub text: String,
    /// Sink hops, or `outside` leaves when the query has no end.
    pub found: usize,
}

/// A query string that ends in `=>`, `==>`, `<=` or `<==` has no end
/// selector. Returns the query without it and the combinator, or None when it
/// ends otherwise.
pub fn open_end(query: &str) -> Option<(String, Combinator)> {
    let t = query.trim_end();
    if let Some(head) = t.strip_suffix("<==") {
        return Some((head.to_string(), Combinator::FlowsFromEventually));
    }
    if let Some(head) = t.strip_suffix("<=") {
        return Some((head.to_string(), Combinator::FlowsFrom));
    }
    if let Some(head) = t.strip_suffix("==>") {
        return Some((head.to_string(), Combinator::FlowsEventually));
    }
    if let Some(head) = t.strip_suffix("=>") {
        if !head.ends_with('=') && !head.ends_with('<') {
            return Some((head.to_string(), Combinator::Flows));
        }
    }
    None
}

/// Evaluate a data-flow path query. `open` is the combinator stripped by
/// `open_end` when the query has no end selector. `root` is the repo root the
/// language config is loaded from. The module path and Go version come from
/// the go.mod nearest `scan`, the first scanned path, when it has one.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_flows(
    query: &Query,
    open: Option<Combinator>,
    store: &TreeStore,
    graph: &FlowGraph,
    root: &Path,
    scan: &Path,
    opts: &FlowOptions,
) -> Result<FlowReport, String> {
    let split = split_query(query)?;
    let (comb, tail) = match (&open, split.joins.as_slice()) {
        (Some(c), []) => (c.clone(), None),
        (None, [c]) => (c.clone(), Some(&split.segments[1])),
        (Some(_), _) => return Err("a data-flow query has one start and at most one end".into()),
        (None, []) => return Err("no data-flow combinator in the query, use => or ==>".into()),
        (None, _) => return Err("a data-flow query has one start and at most one end".into()),
    };
    match comb {
        Combinator::Flows | Combinator::FlowsEventually => {}
        Combinator::FlowsFrom | Combinator::FlowsFromEventually => {
            if split.nots.iter().any(|n| !n.is_empty()) {
                return Err(":not() is not available in data-flow queries yet".into());
            }
            return evaluate_upward(&split.segments[0], tail, comb == Combinator::FlowsFromEventually, store, graph, root, scan, opts);
        }
        _ => return Err("data-flow combinators cannot be mixed with call combinators".into()),
    }
    if split.nots.iter().any(|n| !n.is_empty()) {
        return Err(":not() is not available in data-flow queries yet".into());
    }
    let eventually = comb == Combinator::FlowsEventually;

    let mut cfg = config::load(root, "go").map_err(|e| e.to_string())?;
    if let Some(m) = gomod::module_path(scan) {
        cfg.module_path = Some(m);
    }

    // The end: tail rows by file, or every outside call.
    let mut ends: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    if let Some(t) = tail {
        for (file, rows) in tail_rows(t, store)? {
            ends.insert(file, rows.iter().map(|r| (r.start_byte, r.end_byte)).collect());
        }
    }
    let imports_cache: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
    // A lazy store gives a module read one name at a time.
    let module = match store.finder() {
        Some(f) => Module::lazy(f, &cfg.mechanics),
        None => {
            let files: Vec<(&str, Node, &[u8])> = store
                .iter()
                .filter(|(_, pf)| pf.lang == "go")
                .filter_map(|(f, pf)| pf.tree.as_ref().map(|t| (f, t.root_node(), pf.source.as_slice())))
                .collect();
            Module::build(&files, &cfg.mechanics)
        }
    };
    let modpath = cfg.module_path.clone();
    let has_end = tail.is_some();
    let is_end = |n: Node, src: &[u8]| -> bool {
        let Some(file) = store.file_of(src) else { return false };
        let Some(f) = n.child_by_field_name("function") else { return false };
        if has_end {
            let name = f.child_by_field_name("field").unwrap_or(f);
            return ends
                .get(file)
                .is_some_and(|rs| rs.iter().any(|&(s, e)| s <= name.start_byte() && name.end_byte() <= e));
        }
        let mut root = n;
        while let Some(p) = root.parent() {
            root = p;
        }
        let cx = FlowCtx { file, src, root, mech: &cfg.mechanics, module: &module };
        is_outside_call(&cx, n, f, modpath.as_deref(), &imports_cache)
    };

    let mut walk = Walk::new(store, graph, &cfg, &module, &is_end, eventually, opts.max);
    walk.expand = opts.expand;
    walk.tail = tail;
    walk.tmpl_mech = config::load(root, "gotmpl").ok().map(|c| c.mechanics);
    let mut groups: Vec<FlowResult> = vec![];
    let mut raw: Vec<FlowResult> = vec![];

    // Start points from the head segment.
    let mut seen_vars: HashSet<(String, usize, String)> = HashSet::new();
    let mut seen_fields: Vec<(String, Option<String>)> = vec![];
    for (file, rows) in segment_rows(&split.segments[0], store)? {
        let Some(pf) = store.get(&file) else { continue };
        let Some(tree) = &pf.tree else { continue };
        if pf.lang != "go" {
            continue;
        }
        let cx = FlowCtx { file: &file, src: &pf.source, root: tree.root_node(), mech: &cfg.mechanics, module: &module };
        for r in &rows {
            let Some(start) = start_in(&cx, r.start_byte, r.end_byte) else { continue };
            match &start.kind {
                StartKind::Var(name) => {
                    let Some(func) = enclosing_function(cx.root, cx.mech, start.start_byte) else { continue };
                    if !seen_vars.insert((file.clone(), func.start_byte(), name.clone())) {
                        continue;
                    }
                    let res = walk.run(&file, || analyse_function(&cx, func, &start, walk.end, &walk));
                    raw.push(res.clone());
                    groups.push(res);
                }
                StartKind::Field { name, owner } => {
                    let key = (name.clone(), owner.clone());
                    if seen_fields.contains(&key) {
                        continue;
                    }
                    seen_fields.push(key);
                    // The declaration row is the group's source when there is one.
                    let decl = rows
                        .iter()
                        .filter_map(|x| start_in(&cx, x.start_byte, x.end_byte))
                        .find(|s| s.kind == start.kind && s.func == "field" && is_decl_row(&cx, s.start_byte))
                        .unwrap_or_else(|| start.clone());
                    let mut root: Option<FlowResult> = None;
                    for (ffile, func_byte) in field_users(store, &cfg, &module, name, owner.as_deref()) {
                        let Some(fpf) = store.get(&ffile) else { continue };
                        let Some(ftree) = &fpf.tree else { continue };
                        let fcx = FlowCtx {
                            file: &ffile,
                            src: &fpf.source,
                            root: ftree.root_node(),
                            mech: &cfg.mechanics,
                            module: &module,
                        };
                        let Some(func) = enclosing_function(fcx.root, fcx.mech, func_byte) else { continue };
                        let res = walk.run(&ffile, || analyse_function(&fcx, func, &decl, walk.end, &walk));
                        match &mut root {
                            None => root = Some(res),
                            Some(r) => r.root.children.extend(res.root.children),
                        }
                    }
                    if let Some(r) = root {
                        raw.push(r.clone());
                        groups.push(r);
                    }
                }
            }
        }
    }

    let end_tag = if has_end { "sink" } else { "outside" };
    let mut out = String::new();
    let (mut found, mut cut, mut through) = (0, 0, 0);
    for g in &groups {
        let mut root = g.root.clone();
        if !has_end {
            retag(&mut root, false);
        }
        let mut pruned = prune(&root, end_tag, eventually, 0, false).unwrap_or_else(|| Hop { children: vec![], ..root.clone() });
        let routes_through = through_unmodelled(&pruned, end_tag, false);
        if opts.only.is_empty() {
            found += pruned.walk().iter().filter(|h| h.has(end_tag)).count();
        } else {
            // Routes count when a hop on them carries a wanted tag. A group
            // with none of those prints nothing.
            let n = matching_routes(&pruned, &opts.only, end_tag, false);
            found += n;
            match only(&pruned, &opts.only, true) {
                Some(o) if !o.children.is_empty() => pruned = o,
                _ => continue,
            }
        }
        through += routes_through;
        cut += pruned.walk().iter().filter(|h| h.has(MAX_DEPTH)).count();
        print_tree(&pruned, 0, &mut out);
    }
    out.push_str(&walk.outside_rows());
    if groups.is_empty() {
        out.push_str("note: the start selector matched no variable, parameter or field\n");
    }
    if !opts.only.is_empty() && found == 0 {
        out.push_str(&format!("0 routes match --only {}\n", opts.only.join(",")));
    } else {
        out.push_str(&format!("{} to {}\n", plural(found, "route"), end_tag));
    }
    if cut > 0 {
        out.push_str(&format!("{} cut at --max-depth {}\n", plural(cut, "route"), opts.max));
    }
    if through > 0 {
        out.push_str(&format!("{} passed through unmodelled calls\n", plural(through, "route")));
    }
    let ends: Vec<&Hop> = raw.iter().flat_map(|r| r.unmodelled_ends()).collect();
    if !ends.is_empty() {
        let list: Vec<String> = ends
            .iter()
            .map(|h| format!("{}:{} {}", h.file, h.line, h.unmodelled.as_deref().unwrap_or("")))
            .collect();
        let verb = if ends.len() == 1 { "route ends" } else { "routes end" };
        out.push_str(&format!("{} {} at an unmodelled call: {}\n", ends.len(), verb, list.join(", ")));
    }
    Ok(FlowReport { text: out, found })
}

/// Evaluate `<=` (one step) and `<==` (eventually): the head segment's rows
/// are the values, the tail segment's rows (if any) are the calls an origin
/// must be. Each tree prints the value first and its origins nested under
/// it. Found counts the origins: hops tagged `source...`, or with an end,
/// the calls the end matched.
#[allow(clippy::too_many_arguments)]
fn evaluate_upward(
    head: &Query,
    tail: Option<&Query>,
    eventually: bool,
    store: &TreeStore,
    graph: &FlowGraph,
    root: &Path,
    scan: &Path,
    opts: &FlowOptions,
) -> Result<FlowReport, String> {
    let mut cfg = config::load(root, "go").map_err(|e| e.to_string())?;
    if let Some(m) = gomod::module_path(scan) {
        cfg.module_path = Some(m);
    }
    let mut ends: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    if let Some(t) = tail {
        for (file, rows) in segment_rows(t, store)? {
            ends.insert(file, rows.iter().map(|r| (r.start_byte, r.end_byte)).collect());
        }
    }
    let is_end = |file: &str, call: Node| -> bool {
        let Some(f) = call.child_by_field_name("function") else { return false };
        let name = f.child_by_field_name("field").unwrap_or(f);
        ends.get(file).is_some_and(|rs| rs.iter().any(|&(s, e)| s <= name.start_byte() && name.end_byte() <= e))
    };
    let mut up = upward::Up::new(store, graph, Some(&cfg), opts.max);
    if tail.is_some() {
        up.end = Some(&is_end);
    }
    let mut out = String::new();
    let (mut found, mut cut, mut groups) = (0, 0, 0);
    let mut seen: HashSet<(String, usize, String)> = HashSet::new();
    for (file, rows) in segment_rows(head, store)? {
        let Some(pf) = store.get(&file) else { continue };
        let Some(tree) = &pf.tree else { continue };
        if pf.lang != "go" {
            continue;
        }
        let cx_root = tree.root_node();
        let mut starts: Vec<Node> = vec![];
        for r in &rows {
            let Some(n) = cx_root.descendant_for_byte_range(r.start_byte, r.end_byte) else { continue };
            // A parameter row names its parameter, a field access its field.
            let n = match n.kind() {
                "parameter_declaration" | "variadic_parameter_declaration" => n.child_by_field_name("name").unwrap_or(n),
                _ => n,
            };
            if n.kind() != "identifier" {
                continue;
            }
            let func = upward::function_at(cx_root, n.start_byte()).map(|f| f.start_byte()).unwrap_or(0);
            if seen.insert((file.clone(), func, upward::text(n, &pf.source).to_string())) {
                starts.push(n);
            }
        }
        for n in starts {
            let Some(mut h) = up.start(&file, n) else { continue };
            if !eventually {
                h = upward::truncate(&h, 1);
            }
            if tail.is_some() {
                match upward::prune_to_end(&h) {
                    Some(p) => h = p,
                    None => continue,
                }
            }
            if !opts.only.is_empty() {
                match only(&h, &opts.only, true) {
                    Some(o) if !o.children.is_empty() || o.tags.iter().any(|t| tag_matches(t, &opts.only)) => h = o,
                    _ => continue,
                }
            }
            groups += 1;
            let hops = h.walk();
            found += if tail.is_some() {
                hops.iter().filter(|x| x.has(upward::END)).count()
            } else {
                hops.iter().filter(|x| x.tags.iter().any(|t| t.starts_with("source"))).count()
            };
            cut += hops.iter().filter(|x| x.has(MAX_DEPTH)).count();
            print_tree_with(&h, 0, true, &mut out);
        }
    }
    if groups == 0 && opts.only.is_empty() && tail.is_none() {
        out.push_str("note: the start selector matched no variable or parameter\n");
    }
    if !opts.only.is_empty() && found == 0 {
        out.push_str(&format!("0 routes match --only {}\n", opts.only.join(",")));
    } else if tail.is_some() {
        out.push_str(&format!("{} from source\n", plural(found, "route")));
    } else {
        out.push_str(&format!("{} from {}\n", plural(found, "route"), if found == 1 { "an origin" } else { "origins" }));
    }
    if cut > 0 {
        out.push_str(&format!("{} cut at --max-depth {}\n", plural(cut, "route"), opts.max));
    }
    Ok(FlowReport { text: out, found })
}

/// Rows of the end selector. An end that only template files have, such as
/// `template_action`, is accepted when no template file was scanned: the
/// template walk matches it against the templates it links.
fn tail_rows(t: &Query, store: &TreeStore) -> Result<Vec<(String, Vec<crate::runner::MatchRow>)>, String> {
    match segment_rows(t, store) {
        Ok(r) => Ok(r),
        Err(e) => {
            let mut probe = TreeStore::default();
            probe.insert("probe.tmpl", "gotmpl", b"{{.}}".to_vec());
            match segment_rows(t, &probe) {
                Ok(_) => Ok(vec![]),
                Err(_) => Err(e),
            }
        }
    }
}

/// Tag of a call hop the walk did not enter because of `--max`.
const MAX_DEPTH: &str = "stopped: max depth";

/// End hops below `h` with a wanted tag on the way to them, the end included.
fn matching_routes(h: &Hop, wanted: &[String], end: &str, above: bool) -> usize {
    let hit = above || h.tags.iter().any(|t| tag_matches(t, wanted));
    let here = usize::from(hit && h.has(end));
    here + h.children.iter().map(|c| matching_routes(c, wanted, end, hit)).sum::<usize>()
}

/// End hops below `h` with a hop tagged `unmodelled` above them, the end
/// itself not counted.
fn through_unmodelled(h: &Hop, end: &str, above: bool) -> usize {
    let here = usize::from(above && h.has(end));
    let below = above || h.has("unmodelled");
    here + h.children.iter().map(|c| through_unmodelled(c, end, below)).sum::<usize>()
}

/// Whether the row at `byte` is a field declaration, not a read.
fn is_decl_row(cx: &FlowCtx, byte: usize) -> bool {
    let Some(n) = cx.root.descendant_for_byte_range(byte, byte) else { return false };
    n.parent().is_some_and(|p| config::Mechanics::is(&cx.mech.nodes.field_decl, p.kind()))
}

/// (file, byte inside the function) for every function that reads or writes
/// a field called `name`, by a selector or a keyed composite element, once
/// per function, in file order. With an `owner`, a selector whose operand
/// has another known type (`b.Contact` in `b.Contact.Phone`) and a literal
/// of another known type do not count, the rule `flow::operand_type` gives.
fn field_users(
    store: &TreeStore,
    cfg: &LangConfig,
    module: &Module,
    name: &str,
    owner: Option<&str>,
) -> Vec<(String, usize)> {
    let m = &cfg.mechanics;
    let mut out = vec![];
    // A file without the name has no field called `name`.
    for (file, pf) in store.holding(&[name]) {
        if pf.lang != "go" {
            continue;
        }
        if !crate::needle::holds(&pf.source, name) {
            continue;
        }
        let Some(tree) = &pf.tree else { continue };
        let cx = FlowCtx { file, src: &pf.source, root: tree.root_node(), mech: m, module };
        let other_owner = |t: Option<String>| match (t, owner) {
            (Some(t), Some(o)) => t != o && !module.promotes(&t, o),
            _ => false,
        };
        let mut seen: HashSet<usize> = HashSet::new();
        let mut stack = vec![tree.root_node()];
        let mut hits: Vec<(usize, usize)> = vec![];
        while let Some(n) = stack.pop() {
            let field = if config::Mechanics::is(&m.nodes.field, n.kind()) {
                m.fields.field.iter().find_map(|f| n.child_by_field_name(f))
            } else if config::Mechanics::is(&m.nodes.keyed, n.kind()) {
                m.fields.key.iter().find_map(|f| n.child_by_field_name(f))
            } else {
                None
            };
            // The owner check types the operand, so it runs only on a field called `name`.
            let named_hit = field.is_some_and(|f| f.utf8_text(&pf.source).unwrap_or("") == name);
            let other = named_hit && owner.is_some() && {
                if config::Mechanics::is(&m.nodes.field, n.kind()) {
                    let line = n.start_position().row + 1;
                    other_owner(m.fields.operand.iter().find_map(|f| n.child_by_field_name(f)).and_then(|o| flow::operand_type(&cx, o, line)))
                } else {
                    // keyed element, in a literal body, in a composite literal
                    let comp = n.parent().and_then(|b| b.parent()).filter(|c| config::Mechanics::is(&m.nodes.composite, c.kind()));
                    let ty = comp.and_then(|c| m.fields.ty.iter().find_map(|f| c.child_by_field_name(f)));
                    other_owner(ty.map(|t| flow::base_type(t.utf8_text(&pf.source).unwrap_or(""))).filter(|t| !t.is_empty()))
                }
            };
            if named_hit && !other {
                if let Some(func) = enclosing_function(tree.root_node(), m, n.start_byte()) {
                    if seen.insert(func.start_byte()) {
                        hits.push((func.start_byte(), n.start_byte()));
                    }
                }
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        hits.sort();
        out.extend(hits.into_iter().map(|(_, b)| (file.to_string(), b)));
    }
    out
}

/// Whether call `n` (function part `f`) goes to code outside the module: a
/// `pkg.F` call on an import outside it, or a method on a variable whose
/// type comes from such a package.
fn is_outside_call(
    cx: &FlowCtx,
    n: Node,
    f: Node,
    module: Option<&str>,
    cache: &RefCell<HashMap<String, HashMap<String, String>>>,
) -> bool {
    let (src, file) = (cx.src, cx.file);
    let Some(mut operand) = f.child_by_field_name("operand") else { return false };
    // `d.db.Exec`: the type of field `db` as the struct declares it.
    if operand.kind() == "selector_expression" {
        if let Some((_, path)) = flow::field_type_path(cx, operand, n.start_position().row + 1) {
            return match (path, module) {
                (Some(p), Some(m)) => gomod::is_outside(m, p.rsplit_once('.').map(|(p, _)| p).unwrap_or(&p)),
                (Some(_), None) => true,
                // A type without a package part is declared in the module or unknown.
                (None, _) => false,
            };
        }
    }
    // `json.NewEncoder(w).Encode` is outside when `json` is.
    while operand.kind() != "identifier" {
        let next = match operand.kind() {
            "call_expression" => operand.child_by_field_name("function"),
            "selector_expression" => operand.child_by_field_name("operand"),
            _ => None,
        };
        match next {
            Some(n) => operand = n,
            None => return false,
        }
    }
    let root = cx.root;
    let imports = cache.borrow_mut().entry(file.to_string()).or_insert_with(|| go_imports(root, src)).clone();
    let on = operand.utf8_text(src).unwrap_or("");
    let line = n.start_position().row + 1;
    let typed = binding_type_qualified(root, src, file, on, line, &DefIndex::default());
    let path = match &typed {
        Some(t) => flow::qualify_type(t, &imports).and_then(|q| q.rsplit_once('.').map(|(p, _)| p.to_string())),
        None => imports.get(on).cloned(),
    };
    // A variable of unknown or undeclared type bound from `pkg.F(...)` on an imported
    // package holds a value of that package (`c := ses.NewFromConfig(cfg)`).
    let path = path.or_else(|| {
        // A type the module declares is not outside.
        if typed.as_deref().is_some_and(|t| t.contains('.') || cx.module.has_type(flow::base_type(t).as_str())) {
            return None;
        }
        flow::bound_from(cx.mech, src, operand)
            .and_then(|c| c.child_by_field_name("function"))
            .and_then(|f| f.child_by_field_name("operand"))
            .filter(|o| o.kind() == "identifier")
            .and_then(|o| imports.get(o.utf8_text(src).unwrap_or("")).cloned())
    });
    match (path, module) {
        (Some(p), Some(m)) => gomod::is_outside(m, &p),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// With no end selector the engine marks outside calls `sink`, shown as
/// `outside`. `outside` ends a route (decisions 09, 16), so an outside call
/// whose result goes on to another end keeps only its effect tags
/// (`derived`, `unmodelled`) and is not counted as a route. The result goes
/// on when a hop reading it reaches an end, or when it is returned and the
/// caller's use of the call reaches one. `up` says the latter for returns of
/// the function `h` sits in. Returns whether the data reaches an end from
/// `h`, by its readers or by returning.
fn retag(h: &mut Hop, up: bool) -> bool {
    let mut cont = h.returns && up;
    for c in h.children.iter_mut().filter(|c| !c.into) {
        cont |= retag(c, up);
    }
    let mut below = false;
    for c in h.children.iter_mut().filter(|c| c.into) {
        below |= retag(c, cont);
    }
    let end = h.has("sink");
    if cont {
        h.tags.retain(|t| t != "sink");
    } else {
        for t in h.tags.iter_mut() {
            if t == "sink" {
                *t = "outside".into();
            }
        }
    }
    cont || below || end
}

/// Keep branches that reach an `end` hop. For `=>` only ends one step from
/// the start count. A walk inside a callee is kept where it reaches an end,
/// and also along its returns when the caller's hop is kept, so a route
/// through a function that hands the value back shows that function.
fn prune(h: &Hop, end: &str, eventually: bool, depth: usize, need_return: bool) -> Option<Hop> {
    // A stop at shared state is kept with its reader rows (decision 12).
    if h.tags.iter().any(|t| t.starts_with("stopped:")) {
        return Some(h.clone());
    }
    let is_end = h.has(end) && (eventually || depth == 1);
    let mut kept: Vec<Option<Hop>> = h
        .children
        .iter()
        .map(|c| if c.into { None } else { prune(c, end, eventually, depth + 1, need_return) })
        .collect();
    // A callee walk that reaches an end on its own makes this hop needed, so
    // the walks of calls nested in its arguments keep their return paths.
    for (i, c) in h.children.iter().enumerate() {
        if c.into {
            kept[i] = prune(c, end, eventually, depth + 1, false);
        }
    }
    let needed = is_end || (need_return && h.returns) || kept.iter().any(|k| k.is_some());
    for (i, c) in h.children.iter().enumerate() {
        if c.into && needed {
            kept[i] = prune(c, end, eventually, depth + 1, true);
        }
    }
    let children: Vec<Hop> = kept.into_iter().flatten().collect();
    if !needed && children.is_empty() {
        return None;
    }
    let mut out = with_children(h, children);
    if h.has(end) && !is_end {
        out.tags.retain(|t| t != end);
    }
    Some(out)
}

/// Whether a hop's tag is one of `wanted`: the whole tag, or its first word
/// (`via` keeps `via normalise`, `conditional` keeps `conditional@4`).
fn tag_matches(tag: &str, wanted: &[String]) -> bool {
    wanted.iter().any(|w| {
        tag == w
            || tag.starts_with(&format!("{} ", w))
            || tag.starts_with(&format!("{}@", w))
            || tag.starts_with(&format!("{}:", w))
    })
}

/// Keep hops carrying one of the `wanted` tags and their ancestors.
fn only(h: &Hop, wanted: &[String], root: bool) -> Option<Hop> {
    let children: Vec<Hop> = h.children.iter().filter_map(|c| only(c, wanted, false)).collect();
    let keep = root || h.tags.iter().any(|t| tag_matches(t, wanted));
    if !keep && children.is_empty() {
        return None;
    }
    Some(with_children(h, children))
}

/// `h` with `children` in place of its own. `Hop { children, ..h.clone() }`
/// copied the whole subtree under `h` only to drop it, once per hop.
fn with_children(h: &Hop, children: Vec<Hop>) -> Hop {
    Hop {
        file: h.file.clone(),
        line: h.line,
        func: h.func.clone(),
        text: h.text.clone(),
        tags: h.tags.clone(),
        unmodelled: h.unmodelled.clone(),
        returns: h.returns,
        into: h.into,
        children,
    }
}

/// Print `h` and its children. A downward root drops its `source` tag (it
/// is the start), an upward root keeps it (`upward`).
fn print_tree(h: &Hop, depth: usize, out: &mut String) {
    print_tree_with(h, depth, false, out)
}

fn print_tree_with(h: &Hop, depth: usize, upward: bool, out: &mut String) {
    let tags: Vec<String> = h
        .tags
        .iter()
        .filter(|t| !(depth == 0 && !upward && *t == "source") && *t != upward::END)
        .cloned()
        .collect();
    out.push_str(&"  ".repeat(depth));
    out.push_str(&row(&h.file, h.line, &h.func, &h.text, &tags));
    out.push('\n');
    for c in &h.children {
        print_tree_with(c, depth + 1, upward, out);
    }
}

/// The `Outside` hook that walks into in-repo callees.
struct Walk<'a> {
    store: &'a TreeStore,
    graph: &'a FlowGraph,
    cfg: &'a LangConfig,
    module: &'a Module,
    models: ModelOutside<'a>,
    /// Whether calls are followed into callees (`==>`).
    follow: bool,
    max: usize,
    sigs: RefCell<HashMap<String, Vec<GoSignature>>>,
    /// Files of the functions being walked, innermost last.
    files: RefCell<Vec<String>>,
    /// (def, tracked parameter positions) being walked, to stop recursion.
    active: RefCell<Vec<(usize, Vec<usize>)>>,
    /// Effects already worked out, by file, call start, tracked arguments and
    /// their field paths.
    memo: RefCell<HashMap<(String, usize, Vec<usize>, bool, String), CallEffect>>,
    /// The end predicate, used inside callees too.
    end: &'a EndFn<'a>,
    /// Outside implementations of module interfaces, by interface, found on
    /// first use.
    outside: RefCell<HashMap<String, Vec<widen::OutsideImpl>>>,
    /// Print the outside implementations of the interfaces widened (`--expand`).
    expand: bool,
    /// Interfaces whose widened calls were followed and have outside implementations.
    widened_ifaces: RefCell<Vec<String>>,
    /// The end selector, matched against linked templates too.
    tail: Option<&'a Query>,
    /// Mechanics of the template language, None when it cannot be loaded.
    tmpl_mech: Option<config::Mechanics>,
}

impl<'a> Walk<'a> {
    fn new(
        store: &'a TreeStore,
        graph: &'a FlowGraph,
        cfg: &'a LangConfig,
        module: &'a Module,
        end: &'a EndFn<'a>,
        follow: bool,
        max: usize,
    ) -> Walk<'a> {
        Walk {
            store,
            graph,
            cfg,
            module,
            models: ModelOutside::with_module(cfg, module),
            follow,
            max,
            sigs: RefCell::new(HashMap::new()),
            files: RefCell::new(vec![]),
            active: RefCell::new(vec![]),
            memo: RefCell::new(HashMap::new()),
            end,
            outside: RefCell::new(HashMap::new()),
            expand: false,
            widened_ifaces: RefCell::new(vec![]),
            tail: None,
            tmpl_mech: None,
        }
    }

    /// Run a top-level walk in `file`.
    fn run<R>(&self, file: &str, f: impl FnOnce() -> R) -> R {
        self.files.borrow_mut().push(file.to_string());
        let r = f();
        self.files.borrow_mut().pop();
        r
    }

    fn signature(&self, file: &str, line: usize) -> Option<GoSignature> {
        let mut sigs = self.sigs.borrow_mut();
        let list = sigs.entry(file.to_string()).or_insert_with(|| {
            let pf = self.store.get(file);
            match pf.and_then(|p| p.tree.as_ref().map(|t| (p, t))) {
                Some((p, t)) => go_signatures(t.root_node(), &p.source),
                None => vec![],
            }
        });
        list.iter().find(|s| s.line == line).cloned()
    }

    /// Walk callee `def` from the parameters that receive the tracked
    /// arguments of `site` (and its receiver when tracked), each holding the
    /// data where its argument held it. None when no parameter can be named.
    /// Also gives the pointer parameters and pointer receiver the callee
    /// stored the data in, as `CallEffect::Into` writes.
    fn into_callee(&self, def: usize, site: &CallSite) -> Option<(FlowResult, Vec<(Slot, Paths)>)> {
        let d = self.graph.def(def);
        let sig = self.signature(&d.id.file, d.id.line)?;
        let mut params: Vec<(&crate::resolve::go::GoParam, Paths)> = vec![];
        for (k, &i) in site.tracked_args.iter().enumerate() {
            let p = sig.params.get(i).or_else(|| sig.params.last().filter(|p| p.ty.starts_with("...")));
            let paths = site.arg_paths.get(k).cloned().flatten();
            if let Some(p) = p {
                match params.iter_mut().find(|(q, _)| q.position == p.position) {
                    Some(e) => e.1 = flow::join_paths(&e.1, &paths),
                    None => params.push((p, paths)),
                }
            }
        }
        if site.recv_tracked {
            params.extend(sig.receiver.as_ref().map(|r| (r, site.recv_paths.clone())));
        }
        params.retain(|(p, _)| !p.name.is_empty() && p.name != "_");
        let (first, first_paths) = params.first()?;
        let pf = self.store.get(&d.id.file)?;
        let tree = pf.tree.as_ref()?;
        let cx = FlowCtx {
            file: &d.id.file,
            src: &pf.source,
            root: tree.root_node(),
            mech: &self.cfg.mechanics,
            module: self.module,
        };
        let mut start = start_at(&cx, first.start_byte, first.end_byte)?;
        start.paths = first_paths.clone();
        let func = enclosing_function(cx.root, cx.mech, first.start_byte)?;
        let also: Vec<(String, Paths)> = params[1..].iter().map(|(p, ps)| (p.name.clone(), ps.clone())).collect();
        self.files.borrow_mut().push(d.id.file.clone());
        let r = analyse_function_with(&cx, func, &start, &also, self.end, self);
        self.files.borrow_mut().pop();
        let is_ptr = |p: &crate::resolve::go::GoParam, name: &str| p.name == name && p.ty.starts_with('*');
        let mut writes = vec![];
        for (name, paths) in &r.exits {
            let slot = if sig.receiver.as_ref().is_some_and(|p| is_ptr(p, name)) {
                Some(Slot::Receiver)
            } else {
                sig.params.iter().find(|p| is_ptr(p, name)).map(|p| Slot::Arg(p.position))
            };
            if let Some(s) = slot {
                writes.push((s, paths.clone()));
            }
        }
        Some((r, writes))
    }

    fn callee_effect(&self, site: &CallSite, file: &str) -> Option<CallEffect> {
        // A method on a receiver whose type comes from outside the module is
        // not a call into the repo, whatever a name match in the graph says.
        if let Some(pkg) = site.recv_path.as_deref().and_then(|p| p.rsplit_once('.')).map(|(p, _)| p) {
            let outside = match &self.cfg.module_path {
                Some(m) => gomod::is_outside(m, pkg),
                None => true,
            };
            if outside {
                return None;
            }
        }
        let targets = self.graph.site_targets(file, site.node.start_byte(), site.node.end_byte())?;
        if targets.is_empty() {
            return None;
        }
        if site.method.is_some()
            && targets.iter().all(|&t| matches!(self.graph.def(t).kind, DefKind::InterfaceMethod { .. }))
        {
            // An interface with no implementation in the scanned files.
            return Some(CallEffect::Stop { tag: "stopped: interface".into(), rows: vec![] });
        }
        // A field receiver (`r.driver.Insert`) keeps the callee text.
        let field_recv = site
            .node
            .child_by_field_name("function")
            .and_then(|f| f.child_by_field_name("operand"))
            .is_some_and(|o| o.kind() == "selector_expression");
        let name = match (&site.recv_type, &site.method) {
            (Some(t), Some(m)) if !field_recv => format!("{}.{}", t, m),
            _ => site.callee.clone(),
        };
        Some(self.walk_targets(site, file, &targets, name))
    }

    /// Walk each of `targets` from the tracked arguments of `site`, or give
    /// `Via(name)` when calls are not followed here.
    fn walk_targets(&self, site: &CallSite, file: &str, targets: &[usize], name: String) -> CallEffect {
        let depth = self.active.borrow().len();
        if !self.follow {
            return CallEffect::Via(name);
        }
        if depth >= self.max {
            // Not entered: the hop says so instead of passing as a plain via.
            return CallEffect::Cut(name);
        }
        let key = (
            file.to_string(),
            site.node.start_byte(),
            site.tracked_args.clone(),
            site.recv_tracked,
            format!("{:?} {:?}", site.arg_paths, site.recv_paths),
        );
        if let Some(e) = self.memo.borrow().get(&key) {
            return e.clone();
        }
        let mut trees = vec![];
        let mut returns: Vec<bool> = vec![];
        let mut writes: Vec<(Slot, Paths)> = vec![];
        let mut walked = false;
        for &t in targets {
            let mark = (t, site.tracked_args.clone());
            if self.active.borrow().contains(&mark) {
                continue;
            }
            self.active.borrow_mut().push(mark);
            let r = self.into_callee(t, site);
            self.active.borrow_mut().pop();
            if let Some((mut r, w)) = r {
                walked = true;
                if returns.len() < r.returns.len() {
                    returns.resize(r.returns.len(), false);
                }
                for (i, x) in r.returns.iter().enumerate() {
                    returns[i] |= *x;
                }
                for e in w {
                    if !writes.contains(&e) {
                        writes.push(e);
                    }
                }
                r.root.tags = vec!["moved".into()];
                r.root.into = true;
                if targets.len() > 1 || self.has_outside(site) {
                    r.root.tags.push("widened".into());
                }
                trees.push(r.root);
            }
        }
        let iface = site.recv_type.as_deref().map(|t| t.rsplit('.').next().unwrap_or(t).to_string());
        let outside = iface.as_deref().map(|i| self.outside_of(i)).unwrap_or_default();
        let in_repo = targets.len();
        if walked && (in_repo > 1 || !outside.is_empty()) {
            let tag = widen::widened_tag(in_repo, outside.len());
            for t in &mut trees {
                if let Some(w) = t.tags.iter_mut().find(|x| x.as_str() == "widened") {
                    *w = tag.clone();
                }
            }
            if let (Some(i), false) = (iface, outside.is_empty()) {
                let mut seen = self.widened_ifaces.borrow_mut();
                if !seen.contains(&i) {
                    seen.push(i);
                }
            }
        }
        let effect = if walked { CallEffect::Into { name, returns, writes, trees } } else { CallEffect::Via(name) };
        self.memo.borrow_mut().insert(key, effect.clone());
        effect
    }

    /// Outside implementations of the interface named `iface`.
    fn outside_of(&self, iface: &str) -> Vec<widen::OutsideImpl> {
        if let Some(v) = self.outside.borrow().get(iface) {
            return v.clone();
        }
        let v = widen::outside_impls_for(iface, self.store, self.graph, self.cfg.module_path.as_deref());
        self.outside.borrow_mut().insert(iface.to_string(), v.clone());
        v
    }

    fn has_outside(&self, site: &CallSite) -> bool {
        site.recv_type.as_deref().is_some_and(|t| !self.outside_of(t.rsplit('.').next().unwrap_or(t)).is_empty())
    }

    /// Rows for the outside implementations of the interfaces this run
    /// widened, printed only with `--expand`.
    fn outside_rows(&self) -> String {
        let mut out = String::new();
        if !self.expand {
            return out;
        }
        let mut names = self.widened_ifaces.borrow().clone();
        names.sort();
        for i in names {
            for o in self.outside_of(&i) {
                let tags = vec![format!("outside implementation of {}", i)];
                out.push_str(&row(&o.file, o.line, &o.ty, &o.text, &tags));
                out.push('\n');
            }
        }
        out
    }

    /// A call through a variable holding a function (`send(x)` after
    /// `var send = sendEmail` or `f := helper`). None when the callee is not
    /// such a variable. One assignment that names a function in the repo is
    /// walked like a direct call. Several assignments, or one whose value is
    /// not a named function, stop with the assignments as rows.
    fn fn_value_effect(&self, site: &CallSite, file: &str) -> Option<CallEffect> {
        if site.method.is_some() || !site.callee.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let m = &self.cfg.mechanics;
        let name = site.callee.as_str();
        let outer = enclosing_function_outermost(site.node, m)?;
        let mut assigns = vec![];
        let pf = self.store.get(file)?;
        let local = declares(outer, &pf.source, m, name);
        if local {
            fn_assignments(outer, &pf.source, m, name, Some(site.node.start_byte()), file, &mut assigns);
        } else if self.module.global(name).is_some() {
            // An assignment to `name` is in a file holding it.
            for (f, fpf) in self.store.holding(&[name]) {
                if fpf.lang != "go" || f.ends_with("_test.go") {
                    continue;
                }
                let Some(tree) = &fpf.tree else { continue };
                let root = tree.root_node();
                for top in named(root) {
                    if config::Mechanics::is(&m.nodes.function, top.kind()) {
                        if declares(top, &fpf.source, m, name) {
                            continue;
                        }
                    } else if !config::Mechanics::is(&m.nodes.global, top.kind()) {
                        continue;
                    }
                    fn_assignments(top, &fpf.source, m, name, None, f, &mut assigns);
                }
            }
        }
        if assigns.is_empty() {
            return None;
        }
        if let [a] = assigns.as_slice() {
            if let Some(def) = self.function_def(&a.value, &a.file) {
                let target = self.graph.def(def).name.clone();
                return Some(self.walk_targets(site, file, &[def], target));
            }
            if local {
                // A method value or other binding: the call graph may know it.
                return None;
            }
        }
        let rows = assigns
            .iter()
            .map(|a| Hop {
                file: a.file.clone(),
                line: a.line,
                func: a.func.clone(),
                text: a.text.clone(),
                tags: vec![],
                unmodelled: None,
                returns: false,
                into: false,
                children: vec![],
            })
            .collect();
        Some(CallEffect::Stop { tag: "stopped: function value".into(), rows })
    }

    /// The function a value names (`sendEmail` or `pkg.SendEmail`), the one in
    /// the directory of `file` when several share the name.
    fn function_def(&self, value: &str, file: &str) -> Option<usize> {
        let name = value.rsplit('.').next()?;
        let cands: Vec<usize> = self
            .graph
            .by_name(name)
            .into_iter()
            .filter(|&i| matches!(self.graph.def(i).kind, DefKind::Function))
            .filter(|&i| self.graph.def(i).id.file.ends_with(".go"))
            .collect();
        let dir = |f: &str| Path::new(f).parent().map(|p| p.to_path_buf());
        if !value.contains('.') {
            return cands.iter().copied().find(|&i| dir(&self.graph.def(i).id.file) == dir(file));
        }
        match cands.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }
}

impl Walk<'_> {
    /// A modelled `Execute` or `ExecuteTemplate` whose data argument is
    /// tracked: the templates parsed into its receiver are walked with the
    /// data as `.`, each tree hung under the call. None when the call is not
    /// one or no template could be linked.
    fn template_effect(&self, site: &CallSite, file: &str) -> Option<CallEffect> {
        let mech = self.tmpl_mech.as_ref()?;
        let method = site.method.as_deref()?;
        let model = flow::model_key(self.cfg, site).and_then(|k| self.cfg.model(&k)).and_then(|m| m.template.clone());
        let data_arg = match (model, method) {
            (Some(t), _) => t.data_arg,
            (None, "Execute") => 1,
            (None, "ExecuteTemplate") => 2,
            _ => return None,
        };
        let k = site.tracked_args.iter().position(|&a| a == data_arg)?;
        let paths = site.arg_paths.get(k).cloned().flatten();
        let linker = template::Linker { store: self.store, graph: self.graph, cfg: self.cfg };
        let name_arg = (data_arg == 2).then_some(1);
        let sets = linker.link(file, site.node, name_arg);
        if sets.is_empty() {
            return None;
        }
        let trees: Vec<Hop> = sets.iter().flat_map(|s| template::render(s, mech, paths.clone(), self.tail)).collect();
        // The writer (argument 0) holds the rendered text, so it carries the
        // data when any action prints it.
        let rendered = trees.iter().any(|t| t.walk().iter().any(|h| h.has("rendered")));
        let writes = if rendered { vec![(Slot::Arg(0), None)] } else { vec![] };
        Some(CallEffect::Into { name: site.callee.clone(), returns: vec![], writes, trees })
    }
}

/// One assignment to a function-valued variable.
struct FnAssign {
    file: String,
    line: usize,
    func: String,
    /// Source text of the assigned value.
    value: String,
    /// Trimmed source line.
    text: String,
}

fn named(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

fn field_kids<'t>(n: Node<'t>, names: &[String]) -> Vec<Node<'t>> {
    let mut out = vec![];
    for f in names {
        let mut c = n.walk();
        out.extend(n.children_by_field_name(f, &mut c));
    }
    out
}

/// The top-level function holding `n`, through any literals.
fn enclosing_function_outermost<'t>(n: Node<'t>, m: &config::Mechanics) -> Option<Node<'t>> {
    let mut found = None;
    let mut x = n;
    while let Some(p) = x.parent() {
        if config::Mechanics::is(&m.nodes.function, p.kind()) {
            found = Some(p);
        }
        x = p;
    }
    found
}

/// Names bound on the left of assignment node `a`, with the value assigned
/// to each when the sides pair up.
fn bound<'t>(a: Node<'t>, m: &config::Mechanics) -> Vec<(Node<'t>, Option<Node<'t>>)> {
    let flat = |ns: Vec<Node<'t>>| -> Vec<Node<'t>> {
        ns.into_iter()
            .flat_map(|x| if config::Mechanics::is(&m.nodes.list, x.kind()) { named(x) } else { vec![x] })
            .collect()
    };
    let lefts = flat(field_kids(a, &m.fields.left));
    let rights = flat(field_kids(a, &m.fields.right));
    let pair = lefts.len() == rights.len();
    lefts.into_iter().enumerate().map(|(i, l)| (l, if pair { Some(rights[i]) } else { None })).collect()
}

/// Whether function `func` declares `name` itself: a parameter, `:=` or `var`.
fn declares(func: Node, src: &[u8], m: &config::Mechanics, name: &str) -> bool {
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        let is_decl = config::Mechanics::is(&m.nodes.param, n.kind())
            || (config::Mechanics::is(&m.nodes.assign, n.kind()) && field_kids(n, &m.fields.operator).is_empty());
        if is_decl && bound(n, m).iter().any(|(l, _)| l.utf8_text(src).unwrap_or("") == name) {
            return true;
        }
        stack.extend(named(n));
    }
    false
}

/// Every assignment to `name` inside `scope`, before byte `before` when given.
fn fn_assignments(
    scope: Node,
    src: &[u8],
    m: &config::Mechanics,
    name: &str,
    before: Option<usize>,
    file: &str,
    out: &mut Vec<FnAssign>,
) {
    let mut stack = vec![scope];
    let mut found = vec![];
    while let Some(n) = stack.pop() {
        if config::Mechanics::is(&m.nodes.assign, n.kind()) && before.is_none_or(|b| n.start_byte() < b) {
            for (l, r) in bound(n, m) {
                if config::Mechanics::is(&m.nodes.name, l.kind()) && l.utf8_text(src).unwrap_or("") == name {
                    found.push((n, r));
                }
            }
        }
        stack.extend(named(n));
    }
    found.sort_by_key(|(n, _)| n.start_byte());
    let text = String::from_utf8_lossy(src);
    for (n, r) in found {
        let line = n.start_position().row + 1;
        let func = if config::Mechanics::is(&m.nodes.function, scope.kind()) {
            flow::function_name(scope, src, m)
        } else {
            "var".to_string()
        };
        out.push(FnAssign {
            file: file.to_string(),
            line,
            func,
            value: r.map(|r| r.utf8_text(src).unwrap_or("").to_string()).unwrap_or_default(),
            text: text.lines().nth(line - 1).unwrap_or("").trim().to_string(),
        });
    }
}

/// Go's predeclared types: a call to one is a conversion, which keeps the data.
const CONVERSIONS: &[&str] = &[
    "string", "[]byte", "[]rune", "byte", "rune", "bool", "int", "int8", "int16", "int32", "int64", "uint",
    "uint8", "uint16", "uint32", "uint64", "uintptr", "float32", "float64", "complex64", "complex128",
];

impl Outside for Walk<'_> {
    fn effect(&self, site: &CallSite) -> CallEffect {
        if site.method.is_none() && CONVERSIONS.contains(&site.callee.as_str()) {
            return CallEffect::Pass;
        }
        let file = self.files.borrow().last().cloned().unwrap_or_default();
        if let Some(e) = self.fn_value_effect(site, &file) {
            return e;
        }
        if let Some(e) = self.callee_effect(site, &file) {
            return e;
        }
        if let Some(e) = self.template_effect(site, &file) {
            return e;
        }
        self.models.effect(site)
    }

    /// With no end selector every end is an outside call, so its model
    /// decides whether the walk follows the result on.
    fn end_effect(&self, site: &CallSite) -> Option<CallEffect> {
        if self.tail.is_some() {
            return None;
        }
        Some(self.models.effect(site))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::parse;

    fn hop(tags: &[&str], children: Vec<Hop>) -> Hop {
        Hop {
            file: "a.go".into(),
            line: 1,
            func: "f".into(),
            text: String::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            unmodelled: None,
            returns: false,
            into: false,
            children,
        }
    }

    fn eval(files: &[(&str, &str)], q: &str) -> FlowReport {
        let d = std::env::temp_dir().join(format!("ctsq-flows-{}-{}", std::process::id(), q.len()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("go.mod"), "module example.com/t\n\ngo 1.23\n").unwrap();
        let mut store = TreeStore::default();
        for (f, src) in files {
            store.insert(f, "go", src.as_bytes().to_vec());
        }
        let g = FlowGraph::whole(&crate::resolve::graph::build_from_store(&store));
        let (head, open) = match open_end(q) {
            Some((h, c)) => (h, Some(c)),
            None => (q.to_string(), None),
        };
        evaluate_flows(&parse(&head).unwrap(), open, &store, &g, &d, &d, &FlowOptions::default()).unwrap()
    }

    #[test]
    fn open_end_strips_a_trailing_flow_combinator() {
        assert_eq!(open_end("a ==> "), Some(("a ".to_string(), Combinator::FlowsEventually)));
        assert_eq!(open_end("a =>"), Some(("a ".to_string(), Combinator::Flows)));
        assert_eq!(open_end("a ==> b"), None);
        assert_eq!(open_end("a <=="), Some(("a ".to_string(), Combinator::FlowsFromEventually)));
        assert_eq!(open_end("a <= "), Some(("a ".to_string(), Combinator::FlowsFrom)));
        assert_eq!(open_end("a <== b"), None);
    }

    #[test]
    fn prune_keeps_branches_to_an_end_and_one_hop_only_at_depth_one() {
        let t = hop(&["source"], vec![hop(&["exact"], vec![hop(&["sink"], vec![])]), hop(&["moved"], vec![])]);
        let p = prune(&t, "sink", true, 0, false).unwrap();
        assert_eq!(p.children.len(), 1);
        assert_eq!(p.children[0].children[0].tags, vec!["sink"]);
        // `=>` drops a sink two steps away
        assert!(prune(&t, "sink", false, 0, false).is_none());
    }

    #[test]
    fn only_matches_whole_tags_and_their_first_word() {
        let w = vec!["via".to_string(), "conditional".to_string()];
        assert!(tag_matches("via clean", &w));
        assert!(tag_matches("conditional@4", &w));
        assert!(!tag_matches("viaduct", &w));
        let t = hop(&["source"], vec![hop(&["exact"], vec![hop(&["via x"], vec![])]), hop(&["sink"], vec![])]);
        let o = only(&t, &["via".to_string()], true).unwrap();
        assert_eq!(o.children.len(), 1, "the ancestor of the via hop stays");
        assert_eq!(o.children[0].children[0].tags, vec!["via x"]);
    }

    #[test]
    fn calls_in_another_file_are_walked_and_returns_come_back() {
        let a = "package t\nimport \"log\"\nfunc F(x string) {\n\ty := clean(x)\n\tlog.Println(y)\n}\n";
        let b = "package t\nfunc clean(s string) string {\n\tr := s + \"!\"\n\treturn r\n}\n";
        let r = eval(&[("t/a.go", a), ("t/b.go", b)], "function#F param#x ==> function#log.Println");
        assert_eq!(r.found, 1, "{}", r.text);
        assert!(r.text.contains("t/b.go:2"), "{}", r.text);
        assert!(r.text.contains("return r  moved"), "{}", r.text);
    }

    #[test]
    fn a_callee_that_drops_the_value_ends_the_route() {
        let a = "package t\nimport \"log\"\nfunc F(x string) {\n\ty := drop(x)\n\tlog.Println(y)\n}\n";
        let b = "package t\nfunc drop(s string) string {\n\treturn \"\"\n}\n";
        let r = eval(&[("t/a.go", a), ("t/b.go", b)], "function#F param#x ==> function#log.Println");
        assert_eq!(r.found, 0, "{}", r.text);
    }

    #[test]
    fn an_object_carries_its_field_path_into_and_out_of_callees() {
        let a = "package t\nimport \"log\"\ntype B struct {\n\tP string\n\tQ string\n}\nfunc F(x string) {\n\tb := B{P: x}\n\tc := keep(b)\n\tlog.Println(c.Q)\n\tlog.Println(c.P)\n}\n";
        let b = "package t\nimport \"log\"\nfunc keep(o B) B {\n\tlog.Print(o.Q)\n\tlog.Print(o.P)\n\treturn o\n}\n";
        let r = eval(&[("t/a.go", a), ("t/b.go", b)], "function#F param#x ==> function#/^Print/");
        // in the callee only the stored field is a route
        assert!(r.text.contains("log.Print(o.P)  sink"), "{}", r.text);
        assert!(!r.text.contains("log.Print(o.Q)"), "{}", r.text);
        // the returned object still holds P, not Q
        assert!(r.text.contains("log.Println(c.P)  sink"), "{}", r.text);
        assert!(!r.text.contains("log.Println(c.Q)"), "{}", r.text);
        assert_eq!(r.found, 2, "{}", r.text);
    }

    #[test]
    fn a_generic_conversion_call_is_walked() {
        let a = "package t\nimport \"log\"\nfunc F(x string) {\n\tv := Id[string](x)\n\tlog.Println(v)\n}\nfunc Id[T any](v T) T {\n\treturn v\n}\n";
        let r = eval(&[("t/a.go", a)], "function#F param#x ==> function#log.Println");
        assert!(r.text.contains("v := Id[string](x)  derived, via Id"), "{}", r.text);
        assert!(r.text.contains("func Id[T any](v T) T {  moved"), "{}", r.text);
        assert_eq!(r.found, 1, "{}", r.text);
    }

    #[test]
    fn literals_called_with_an_argument_or_passed_to_a_call_are_walked() {
        let a = "package t\nimport \"log\"\nfunc F(x string) {\n\tfunc(s string) {\n\t\tlog.Println(s)\n\t}(x)\n\trun(func() {\n\t\tlog.Println(x)\n\t})\n}\nfunc run(f func()) {}\n";
        let r = eval(&[("t/a.go", a)], "function#F param#x ==> function#log.Println");
        assert_eq!(r.found, 2, "{}", r.text);
        assert!(r.text.contains("func(s string) {  moved, closure"), "{}", r.text);
        assert!(r.text.contains("run(func() {  moved, closure"), "{}", r.text);
        assert!(!r.text.contains("unmodelled"), "{}", r.text);
    }

    #[test]
    fn a_parameter_of_function_type_is_not_a_function_value_stop() {
        let a = "package t\nimport \"log\"\nfunc F(x string, f func(string)) {\n\tf(x)\n\tlog.Println(x)\n}\n";
        let r = eval(&[("t/a.go", a)], "function#F param#x ==> function#log.Println");
        assert!(!r.text.contains("stopped"), "{}", r.text);
        assert_eq!(r.found, 1, "{}", r.text);
    }

    #[test]
    fn recursion_stops() {
        let a = "package t\nimport \"log\"\nfunc F(x string) {\n\tlog.Println(loop(x))\n}\nfunc loop(s string) string {\n\treturn loop(s)\n}\n";
        let r = eval(&[("t/a.go", a)], "function#F param#x ==> function#log.Println");
        assert_eq!(r.found, 1, "{}", r.text);
    }
}
