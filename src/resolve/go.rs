//! Go resolver: definition indexing, file scope model, and call resolution.
//!
//! Resolution is heuristic, not full type inference. Call returns are
//! followed through indexed return types (`x := makeServer(); x.Run()`, and
//! `x, err := f()` binds the first result), falling back to the `NewType()`
//! naming convention. Interface dispatch is CHA-lite: a call through an
//! interface-typed receiver expands to one edge per project type whose
//! method set covers the interface. Receivers that are not a bare variable
//! are typed as a chain: struct fields through the indexed struct
//! declarations (`h.store.List()`, embedded fields promoted), call results
//! through the target's return type (`NewX().M()`, `x.Get().M()`). A local
//! closure (`write := func(...)`) is called with no target, never matched by
//! name. Out of scope (calls fall back to ranked name matching): embedded
//! interfaces, receivers whose type is outside the project, func-typed
//! fields and params, and plain reassignment (`x = ...` after the
//! declaration). `CTSQ_RESOLVE_DEBUG=1` prints how each call resolved.
//!
//! Go-specific shape: `pkg.Fn()` and `x.Method()` are the same syntax
//! (`selector_expression`), so call sites are classified as `Method` with a
//! `Var` receiver and disambiguated at resolve time — local bindings and
//! params first (they shadow package names), then the import map.

use super::{
    CallSite, Callee, Confidence, Def, DefId, DefIndex, DefKind, FileModel, Receiver, ResolvedCall,
    Resolver, Want,
};
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

#[derive(Default)]
pub struct GoResolver;

impl Resolver for GoResolver {
    fn collect_defs(&self, file: &str, source: &[u8]) -> Vec<Def> {
        self.collect_defs_tree(file, source, None)
    }

    fn collect_defs_tree(&self, file: &str, source: &[u8], given: Option<&tree_sitter::Tree>) -> Vec<Def> {
        let tree = match parse_or(given, file, source) { Some(t) => t, None => return vec![] };
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

    /// Types this file constructs (composite literals, `new(T)`), recorded as
    /// `(T, CONSTRUCTED)`. Interface dispatch widens only to these. Also the
    /// fields of each struct it declares, recorded as `(T, FIELD...)` (see
    /// `collect_fields`), which type field-chain receivers.
    fn collect_impls(&self, file: &str, source: &[u8]) -> Vec<(String, String)> {
        self.collect_impls_tree(file, source, None)
    }

    fn collect_impls_tree(&self, file: &str, source: &[u8], given: Option<&tree_sitter::Tree>) -> Vec<(String, String)> {
        let Some(tree) = parse_or(given, file, source) else { return vec![] };
        let mut seen: HashSet<String> = HashSet::new();
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "composite_literal" => {
                    if let Some(t) = n.child_by_field_name("type").and_then(|t| type_name(t, source)) {
                        seen.insert(t);
                    }
                }
                "type_spec" => collect_fields(n, source, file, &mut fields),
                "call_expression" => {
                    let is_new = n.child_by_field_name("function").is_some_and(|f| text(f, source) == "new");
                    if is_new {
                        let arg = n.child_by_field_name("arguments").and_then(|a| a.named_child(0));
                        if let Some(t) = arg.and_then(|a| type_name(a, source).or_else(|| {
                            // `new(T)` can parse its argument as an identifier
                            (a.kind() == "identifier").then(|| text(a, source).to_string())
                        })) {
                            seen.insert(t);
                        }
                    }
                }
                _ => {}
            }
            let mut c = n.walk();
            stack.extend(n.children(&mut c));
        }
        let mut out: Vec<(String, String)> = seen.into_iter().map(|t| (t, CONSTRUCTED.to_string())).collect();
        out.extend(fields);
        out.sort();
        out
    }

    fn model(&self, file: &str, source: &[u8], given: Option<&tree_sitter::Tree>, _own: &[Def]) -> FileModel {
        match parse_or(given, file, source) {
            Some(t) => FileModel::Go(FileScope::build(t.root_node(), source)),
            None => FileModel::None,
        }
    }

    fn resolve_model(
        &self,
        file: &str,
        model: &FileModel,
        index: &DefIndex,
        want: Option<&Want>,
    ) -> (Vec<ResolvedCall>, Vec<CallSite>) {
        let FileModel::Go(scope) = model else { return (vec![], vec![]) };
        let constructed = index.go_constructed.get_or_init(|| constructed_types(index));

        // def line -> index, for attributing calls to their enclosing function
        let def_at: HashMap<usize, usize> = index.in_file(file).iter()
            .map(|&i| (index.defs[i].id.line, i))
            .collect();

        let debug = std::env::var_os(DEBUG_ENV).is_some_and(|v| !v.is_empty() && v != "0");
        let fields = OnceCell::new();
        let mut out = Vec::new();
        let mut sites = Vec::new();
        for fun in &scope.fns {
            let Some(&caller) = def_at.get(&fun.line) else { continue };
            let ctx = Ctx { file, scope, fun, index, constructed, fields: &fields };
            let whole = want.is_none_or(|w| w.whole(fun.line));
            for call in &fun.calls {
                if !whole && !wanted_site(&call.callee.callee, fun, want) {
                    continue;
                }
                let line = call.line;
                // interface dispatch can yield several targets → one edge each
                let targets: Vec<Option<(usize, Confidence)>> =
                    match resolve_callee(&call.callee, line, 0, &ctx) {
                        t if t.is_empty() => vec![None],
                        t => t.into_iter().map(Some).collect(),
                    };
                let display = call.callee.callee.name().to_string();
                if debug {
                    debug_line(index, caller, &display, file, line, &targets);
                }
                for target in targets {
                    out.push(ResolvedCall {
                        caller, callee_display: display.clone(), line, target,
                    });
                    sites.push(CallSite {
                        caller,
                        callee_display: display.clone(),
                        target,
                        line,
                        start_byte: call.start_byte,
                        end_byte: call.end_byte,
                        args: call.args.clone(),
                    });
                }
            }
        }
        (out, sites)
    }
}

/// Environment variable that turns on one stderr line per resolved Go call
/// target, saying how it resolved. Read by `plans/explore/edges.sh`.
pub const DEBUG_ENV: &str = "CTSQ_RESOLVE_DEBUG";

/// How a call target was found: through types and scope (`by-type`), or not
/// at all (`unresolved`). There is no name-only fallback, so the `by-name`
/// class `plans/explore/edges.sh` still counts is always empty.
fn resolve_class(target: Option<(usize, Confidence)>) -> &'static str {
    match target {
        None => "unresolved",
        Some(_) => "by-type",
    }
}

/// Print one `ctsq-resolve` line per target to stderr, tab separated:
/// class, caller id, target id (the callee name when unresolved), call site
/// `file:line`, callee name. Ids are `basename:line`, the form
/// `callgraph --format edges` prints.
fn debug_line(
    index: &DefIndex,
    caller: usize,
    display: &str,
    file: &str,
    line: usize,
    targets: &[Option<(usize, Confidence)>],
) {
    let id = |d: &Def| format!("{}:{}", crate::calltree::short_path(&d.id.file), d.id.line);
    let from = id(&index.defs[caller]);
    for &t in targets {
        let to = t.map(|(i, _)| id(&index.defs[i])).unwrap_or_else(|| display.to_string());
        eprintln!("ctsq-resolve\t{}\t{}\t{}\t{}:{}\t{}", resolve_class(t), from, to, file, line, display);
    }
}

/// Whether a call outside the wanted functions can resolve to a definition
/// named in `want.names`, or stay unresolved under such a name: its callee
/// is one of the names, or it calls a local holding a function value, which
/// can reach a definition of any name.
fn wanted_site(callee: &Callee, fun: &FnScope, want: Option<&Want>) -> bool {
    let Some(w) = want else { return true };
    if w.names.contains(callee.name()) {
        return true;
    }
    match callee {
        Callee::Bare(v) => fun.bindings.iter().any(|(_, b, bind)| b == v && matches!(bind, Binding::FnValue(_))),
        _ => false,
    }
}

/// Marker trait name under which `collect_impls` records a constructed type.
pub const CONSTRUCTED: &str = "#constructed";

/// Types the program builds somewhere: composite literals and `new(T)`
/// (recorded by `collect_impls`), the return types of functions, and the
/// `NewT` naming convention. Interface calls widen only to these.
pub fn constructed_types(index: &DefIndex) -> HashSet<String> {
    let mut set: HashSet<String> = index.impls.iter()
        .filter(|(_, t)| t == CONSTRUCTED)
        .map(|(ty, _)| ty.clone())
        .collect();
    for d in &index.defs {
        if !matches!(d.kind, DefKind::Function) { continue; }
        if let Some(r) = &d.ret { set.insert(r.clone()); }
        if let Some(t) = d.name.strip_prefix("New").filter(|t| t.chars().next().is_some_and(|c| c.is_uppercase())) {
            set.insert(t.to_string());
        }
    }
    set
}

/// Marker prefix under which `collect_impls` records a struct field:
/// `(Struct, "#field\t<name>\t<base type>\t<embedded 0|1>\t<file>")`.
const FIELD: &str = "#field";

/// Record each field of `type T struct { ... }` whose type has a base name
/// (`*pkg.Store` gives `Store`; maps, slices and func types are left out).
/// An embedded field is named after its type and marked embedded, so its
/// fields are promoted.
fn collect_fields(spec: Node, source: &[u8], file: &str, out: &mut Vec<(String, String)>) {
    let (Some(name), Some(body)) = (spec.child_by_field_name("name"), spec.child_by_field_name("type"))
    else { return };
    if body.kind() != "struct_type" { return; }
    let owner = text(name, source).to_string();
    // Type parameters with a named constraint: a field of type `D` in
    // `Repo[D Driver]` has the methods of `Driver`.
    let mut tparams: HashMap<String, String> = HashMap::new();
    if let Some(list) = spec.child_by_field_name("type_parameters") {
        let mut c = list.walk();
        for p in list.named_children(&mut c) {
            let Some(con) = p.child_by_field_name("type") else { continue };
            let con = text(con, source).trim();
            if con.is_empty() || !con.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.') {
                continue;
            }
            let base = con.rsplit('.').next().unwrap_or(con).to_string();
            let mut pc = p.walk();
            for nm in p.children_by_field_name("name", &mut pc) {
                tparams.insert(text(nm, source).to_string(), base.clone());
            }
        }
    }
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() != "field_declaration" {
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
            continue;
        }
        let Some(ty) = n.child_by_field_name("type").and_then(|t| type_name(t, source)) else { continue };
        let ty = tparams.get(&ty).cloned().unwrap_or(ty);
        let mut c = n.walk();
        let names: Vec<String> = n.children_by_field_name("name", &mut c)
            .map(|x| text(x, source).to_string())
            .collect();
        if names.is_empty() {
            out.push((owner.clone(), format!("{FIELD}\t{ty}\t{ty}\t1\t{file}")));
        }
        for f in names {
            out.push((owner.clone(), format!("{FIELD}\t{f}\t{ty}\t0\t{file}")));
        }
    }
}

/// One struct field read back from `index.impls`.
#[derive(Debug)]
struct FieldRec {
    name: String,
    ty: String,
    embedded: bool,
    file: String,
}

/// Struct name to its fields, over every struct the index has read. Built
/// once per resolved file, on the first field-chain receiver.
#[derive(Debug, Default)]
struct StructFields(HashMap<String, Vec<FieldRec>>);

impl StructFields {
    fn of(index: &DefIndex) -> StructFields {
        let mut map: HashMap<String, Vec<FieldRec>> = HashMap::new();
        for (owner, rec) in &index.impls {
            let Some(rest) = rec.strip_prefix(FIELD).and_then(|r| r.strip_prefix('\t')) else { continue };
            let mut parts = rest.splitn(4, '\t');
            let (Some(name), Some(ty), Some(emb), Some(file)) = (parts.next(), parts.next(), parts.next(), parts.next())
            else { continue };
            map.entry(owner.clone()).or_default().push(FieldRec {
                name: name.to_string(), ty: ty.to_string(), embedded: emb == "1", file: file.to_string(),
            });
        }
        StructFields(map)
    }

    /// Base type of `ty.field`, promoted through embedded structs. Several
    /// structs may share the name `ty` across packages: one answer is kept
    /// when they agree, else the one declared in `from`'s directory, else none.
    fn lookup(&self, ty: &str, field: &str, from: &str, depth: usize) -> Option<String> {
        let recs = self.0.get(ty)?;
        let direct: Vec<&FieldRec> = recs.iter().filter(|r| r.name == field).collect();
        if !direct.is_empty() {
            return pick(direct.iter().map(|r| (r.ty.as_str(), r.file.as_str())), from);
        }
        if depth >= MAX_BINDING_DEPTH { return None; }
        let promoted: Vec<(String, &str)> = recs.iter()
            .filter(|r| r.embedded)
            .filter_map(|r| self.lookup(&r.ty, field, from, depth + 1).map(|t| (t, r.file.as_str())))
            .collect();
        pick(promoted.iter().map(|(t, f)| (t.as_str(), *f)), from)
    }
}

/// The one type among (type, declaring file) answers, preferring answers
/// declared in the same directory as `from` when they disagree.
fn pick<'a>(answers: impl Iterator<Item = (&'a str, &'a str)>, from: &str) -> Option<String> {
    let answers: Vec<(&str, &str)> = answers.collect();
    let one = |set: &[(&str, &str)]| -> Option<String> {
        let first = set.first()?.0;
        set.iter().all(|(t, _)| *t == first).then(|| first.to_string())
    };
    if let Some(t) = one(&answers) { return Some(t); }
    let dir = std::path::Path::new(from).parent();
    let local: Vec<(&str, &str)> = answers.into_iter()
        .filter(|(_, f)| std::path::Path::new(f).parent() == dir)
        .collect();
    one(&local)
}

#[cfg(test)]
fn parse(source: &[u8]) -> Option<tree_sitter::Tree> {
    crate::parsed::parse_text("go", source)
}

/// The tree a `TreeStore` already holds, else the parse cache's tree of
/// `file` when `source` is that file's text, else a fresh parse. Cloning a
/// tree is a cheap reference count.
fn parse_or(given: Option<&tree_sitter::Tree>, file: &str, source: &[u8]) -> Option<tree_sitter::Tree> {
    given.cloned().or_else(|| crate::parsed::tree_for(file, "go", source))
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
    FnValue(GoCallee),
    /// `var x Server` / `x := &Server{}`.
    Typed(String),
    /// `x := f()` / `x, err := pkg.F()` — type comes from the callee's
    /// return type at resolve time (or the `NewT` naming convention).
    Call(GoCallee),
    /// `write := func(...) {...}` or `var write func(...)`: a local closure.
    /// Calling it reaches no definition, so it never matches by name.
    Closure,
    /// A type switch variable in a `default` or multi-type case: no single type.
    Unknown,
}

/// A callee as `Callee` classifies it, plus the shape of the receiver
/// expression when a method is called on anything but a bare identifier
/// (`h.store.List()`, `NewX().M()`), which `Receiver::Opaque` only names.
#[derive(Debug, Clone)]
struct GoCallee {
    callee: Callee,
    recv: Option<Chain>,
}

impl GoCallee {
    fn plain(callee: Callee) -> GoCallee {
        GoCallee { callee, recv: None }
    }
}

/// A receiver expression that can be typed: a variable, a field of a typed
/// expression, the result of a call, or an expression naming its type
/// (`x.(*T)`, `T{}`).
#[derive(Debug, Clone)]
enum Chain {
    Name(String),
    Field(Box<Chain>, String),
    Call(Box<GoCallee>),
    Type(String),
}

#[derive(Debug)]
struct FnScope {
    /// Start line of the declaration — matches `Def.id.line`.
    line: usize,
    /// (var, type) pairs: the method receiver (if any) plus the parameter list.
    params: Vec<(String, String)>,
    /// (line, var, binding) in document order.
    bindings: Vec<(usize, String, Binding)>,
    /// Call sites in document order.
    calls: Vec<CallRec>,
}

#[derive(Debug)]
struct CallRec {
    callee: GoCallee,
    line: usize,
    start_byte: usize,
    end_byte: usize,
    /// Byte range of each argument.
    args: Vec<(usize, usize)>,
}

/// The calls, bindings and imports of one Go file, read from its tree.
#[derive(Debug)]
pub struct FileScope {
    /// local package name (alias included) → import path segments.
    imports: HashMap<String, Vec<String>>,
    /// `import . "path"` dot-import path segments.
    globs: Vec<Vec<String>>,
    fns: Vec<FnScope>,
    /// Package-level `var x T` declarations in this file: name to base type.
    vars: HashMap<String, String>,
}

impl FileScope {
    /// The declaration line of each function, and for each call (start byte,
    /// end byte) its (function, call) position, the order `resolve_model`
    /// emits sites in.
    pub(crate) fn call_positions(&self) -> (Vec<usize>, HashMap<(usize, usize), (usize, usize)>) {
        let mut at = HashMap::new();
        for (f, fun) in self.fns.iter().enumerate() {
            for (c, call) in fun.calls.iter().enumerate() {
                at.entry((call.start_byte, call.end_byte)).or_insert((f, c));
            }
        }
        (self.fns.iter().map(|f| f.line).collect(), at)
    }
}

impl FileScope {
    fn build(root: Node, source: &[u8]) -> FileScope {
        let mut s = FileScope { imports: HashMap::new(), globs: vec![], fns: vec![], vars: HashMap::new() };
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
        // `switch t := x.(type) { case *T: ... }` narrows `t` to T in the case.
        "type_case" | "default_case" if node.parent().is_some_and(|p| p.kind() == "type_switch_statement") => {
            if let (Some(&f), Some(alias)) = (
                fn_stack.last(),
                node.parent().and_then(|p| p.child_by_field_name("alias")),
            ) {
                let types: Vec<Node> = if node.kind() == "type_case" {
                    let mut c = node.walk();
                    node.children_by_field_name("type", &mut c).filter(|t| t.is_named()).collect()
                } else {
                    vec![]
                };
                let binding = match types.as_slice() {
                    [t] => type_name(*t, source).map(Binding::Typed).unwrap_or(Binding::Unknown),
                    _ => Binding::Unknown,
                };
                if let Some(var) = alias.named_child(0).filter(|v| v.kind() == "identifier") {
                    s.fns[f].bindings.push((node.start_position().row + 1, text(var, source).to_string(), binding));
                }
            }
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
            } else {
                for (var, b) in classify_var_spec(node, source) {
                    if let Binding::Typed(t) = b {
                        s.vars.insert(var, t);
                    }
                }
            }
            // fall through: the value expression may contain calls
        }
        "call_expression" => {
            if let Some(&f) = fn_stack.last() {
                if let Some(callee) = classify_call(node, source) {
                    let mut args = Vec::new();
                    if let Some(list) = node.child_by_field_name("arguments") {
                        let mut c = list.walk();
                        for a in list.named_children(&mut c) {
                            if a.kind() != "comment" {
                                args.push((a.start_byte(), a.end_byte()));
                            }
                        }
                    }
                    s.fns[f].calls.push(CallRec {
                        callee,
                        line: node.start_position().row + 1,
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                        args,
                    });
                }
            }
            // fall through: arguments and receivers may contain calls
        }
        // `F[T](x)` parses as a conversion to a generic type: it calls `F`.
        "type_conversion_expression" => {
            if let (Some(&f), Some(callee)) = (fn_stack.last(), generic_conversion_callee(node, source).map(GoCallee::plain)) {
                let args = node.child_by_field_name("operand")
                    .map(|a| vec![(a.start_byte(), a.end_byte())])
                    .unwrap_or_default();
                s.fns[f].calls.push(CallRec {
                    callee,
                    line: node.start_position().row + 1,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    args,
                });
            }
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
    if left.named_child_count() > 1 && !matches!(value.kind(), "call_expression" | "type_assertion_expression") {
        return None;
    }
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
    if node.child_by_field_name("type").is_some_and(|t| t.kind() == "function_type") {
        return names.into_iter().map(|v| (v, Binding::Closure)).collect();
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
        "func_literal" => Some(Binding::Closure),
        "composite_literal" => {
            type_name(value.child_by_field_name("type")?, source).map(Binding::Typed)
        }
        // `v, ok := x.(*T)` narrows v to T
        "type_assertion_expression" => {
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

/// The function a `F[T](x)` conversion calls: `F`, or `pkg.F` for a
/// qualified generic. None for any other conversion.
fn generic_conversion_callee(node: Node, source: &[u8]) -> Option<Callee> {
    let ty = node.child_by_field_name("type").filter(|t| t.kind() == "generic_type")?;
    let base = ty.child_by_field_name("type")?;
    match base.kind() {
        "type_identifier" => Some(Callee::Bare(text(base, source).to_string())),
        "qualified_type" => Some(Callee::Method {
            receiver: Receiver::Var(text(base.child_by_field_name("package")?, source).to_string()),
            name: text(base.child_by_field_name("name")?, source).to_string(),
        }),
        _ => None,
    }
}

fn classify_call(node: Node, source: &[u8]) -> Option<GoCallee> {
    classify_callee_expr(node.child_by_field_name("function")?, source)
}

fn classify_callee_expr(f: Node, source: &[u8]) -> Option<GoCallee> {
    match f.kind() {
        "identifier" => Some(GoCallee::plain(Callee::Bare(text(f, source).to_string()))),
        // `pkg.Fn()` and `x.Method()` are syntactically identical; classify as
        // a method with a Var receiver and let resolution disambiguate.
        "selector_expression" => {
            let name = text(f.child_by_field_name("field")?, source).to_string();
            let operand = f.child_by_field_name("operand")?;
            let (receiver, recv) = match operand.kind() {
                "identifier" => (Receiver::Var(text(operand, source).to_string()), None),
                _ => (
                    Receiver::Opaque(text(operand, source).lines().next().unwrap_or("").to_string()),
                    chain(operand, source),
                ),
            };
            Some(GoCallee { callee: Callee::Method { receiver, name }, recv })
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

/// The shape of a receiver expression, or None when it is not one `Chain`
/// can type (an index, a func literal, an operator).
fn chain(n: Node, source: &[u8]) -> Option<Chain> {
    match n.kind() {
        "identifier" => Some(Chain::Name(text(n, source).to_string())),
        "selector_expression" => {
            let base = chain(n.child_by_field_name("operand")?, source)?;
            let field = text(n.child_by_field_name("field")?, source).to_string();
            Some(Chain::Field(Box::new(base), field))
        }
        "call_expression" => classify_call(n, source).map(|c| Chain::Call(Box::new(c))),
        "parenthesized_expression" => chain(n.named_child(0)?, source),
        // `(*p).M()`, `(&x).M()`: the same base type
        "unary_expression" if n.child_by_field_name("operator").is_some_and(|o| matches!(text(o, source), "*" | "&")) =>
            chain(n.child_by_field_name("operand")?, source),
        "type_assertion_expression" | "composite_literal" =>
            type_name(n.child_by_field_name("type")?, source).map(Chain::Type),
        _ => None,
    }
}

// ── Resolution ────────────────────────────────────────────────────────────────

struct Ctx<'a> {
    file: &'a str,
    scope: &'a FileScope,
    fun: &'a FnScope,
    index: &'a DefIndex,
    /// Types constructed somewhere in the scanned files.
    constructed: &'a HashSet<String>,
    /// Struct fields of the indexed files, built on first use.
    fields: &'a OnceCell<StructFields>,
}

/// How many bindings and calls a receiver may be typed through. With no
/// name-only fallback a receiver typed through `hs := newHarness()` then
/// `ws := hs.H.Store.Workspace(id)` needs every link, so this is 6, not 4.
const MAX_BINDING_DEPTH: usize = 6;

/// Resolve a call site to its target definitions. Every path yields zero or
/// one target except interface dispatch, which expands to one edge per
/// implementing method.
fn resolve_callee(callee: &GoCallee, line: usize, depth: usize, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    if depth > MAX_BINDING_DEPTH { return vec![]; }
    match &callee.callee {
        Callee::Path(segs) => {
            let Some((name, pkg)) = segs.split_last() else { return vec![] };
            resolve_pkg_fn(pkg, name, ctx).into_iter().collect()
        }
        Callee::Bare(name) => one(resolve_bare(name, line, depth, ctx)),
        Callee::Method { receiver, name } =>
            resolve_method(receiver, callee.recv.as_ref(), name, line, depth, ctx),
    }
}

fn one(hit: Option<(usize, Confidence)>) -> Vec<(usize, Confidence)> {
    hit.into_iter().collect()
}

fn resolve_bare(name: &str, line: usize, depth: usize, ctx: &Ctx) -> Option<(usize, Confidence)> {
    // (a) local binding holding a function value, or a local closure, which
    // reaches no definition
    match last_binding(ctx.fun, name, line) {
        Some(Binding::FnValue(inner)) => {
            if let Some(&hit) = resolve_callee(inner, line, depth + 1, ctx).first() {
                return Some(hit);
            }
        }
        Some(Binding::Closure) => return None,
        _ => {}
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
    // nothing in scope by that name: unresolved, never a same-named def
    // elsewhere in the project
    None
}

fn resolve_method(
    receiver: &Receiver,
    recv: Option<&Chain>,
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
        // `h.store.List()`, `NewX().M()`: typed through the chain
        Receiver::Opaque(_) => {
            if let Some(ty) = recv.and_then(|c| chain_type(c, line, depth + 1, ctx)) {
                if let Some(hit) = method_on(&ty, name, ctx) { return one(Some(hit)); }
                let impls = interface_impls(&ty, name, ctx);
                if !impls.is_empty() { return impls; }
            }
        }
        Receiver::SelfRecv => {}
    }
    // a receiver with no known type, or a type with no project method of
    // this name (an outside type, a promoted method): unresolved
    vec![]
}

/// Base type of a receiver chain at `line`: a variable through `var_type`, a
/// field through the struct declarations in the index, a call through its
/// target's return type. None when any link is unknown.
fn chain_type(c: &Chain, line: usize, depth: usize, ctx: &Ctx) -> Option<String> {
    if depth > MAX_BINDING_DEPTH { return None; }
    match c {
        Chain::Name(v) => var_type(v, line, depth, ctx),
        // a field link is finite syntax, so it does not spend binding depth
        Chain::Field(base, field) => {
            let ty = chain_type(base, line, depth, ctx)?;
            ctx.index.ask(&ty);
            ctx.fields.get_or_init(|| StructFields::of(ctx.index)).lookup(&ty, field, ctx.file, 0)
        }
        Chain::Call(callee) => {
            let hit = resolve_callee(callee, line, depth + 1, ctx).first().copied();
            ctx.index.defs[hit?.0].ret.clone()
        }
        Chain::Type(t) => Some(t.clone()),
    }
}

/// CHA-lite: a call through interface `iface` expands to `name` on every
/// project type whose method set covers all of the interface's methods. A
/// single implementor is exact; several are each a guess. Only types the
/// program constructs somewhere count (rapid type analysis). With no such
/// implementor the edge points at the signature itself.
fn interface_impls(iface: &str, name: &str, ctx: &Ctx) -> Vec<(usize, Confidence)> {
    // the interface's methods are declared in a file holding its name
    ctx.index.ask_interface(iface);
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
            DefKind::Method { receiver, .. } if constructed(receiver, ctx) && implements(receiver)
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

/// Whether the program constructs `ty` somewhere. Every piece of evidence (a
/// composite literal, `new(T)`, a function returning T, a function named
/// `NewT`) holds T's name in its file.
fn constructed(ty: &str, ctx: &Ctx) -> bool {
    // More files only add constructed types, so a yes needs no more files.
    if ctx.constructed.contains(ty) {
        return true;
    }
    ctx.index.ask(ty);
    // `constructed_types` also counts a function `NewT` as constructing T,
    // and a file holding only `NewT` never holds the word T.
    ctx.index.ask(&format!("New{}", ty));
    false
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
    ctx.index.named(name).iter().copied()
}

/// Most recent binding of `var` at or before `line` in this function.
fn last_binding<'a>(fun: &'a FnScope, var: &str, line: usize) -> Option<&'a Binding> {
    fun.bindings.iter().rev()
        .find(|(l, v, _)| *l <= line && v == var)
        .map(|(_, _, b)| b)
}

/// Type of `var` at `line`: last typed binding — for `x := f()` the resolved
/// callee's return type, else the `NewT` convention — else receiver or
/// parameter type, else a package-level `var x T` in the same file.
fn var_type(var: &str, line: usize, depth: usize, ctx: &Ctx) -> Option<String> {
    match last_binding(ctx.fun, var, line) {
        Some(Binding::Typed(t)) => return Some(t.clone()),
        Some(Binding::Call(callee)) => {
            if let Some(&(t, _)) = resolve_callee(callee, line, depth + 1, ctx).first() {
                if let Some(ret) = &ctx.index.defs[t].ret {
                    return Some(ret.clone());
                }
            }
            return new_convention_type(&callee.callee);
        }
        Some(Binding::FnValue(_)) | Some(Binding::Closure) | Some(Binding::Unknown) | None => {}
    }
    ctx.fun.params.iter().find(|(v, _)| v == var).map(|(_, t)| t.clone())
        .or_else(|| ctx.scope.vars.get(var).cloned())
}

// ── Public helpers for the attribute and flow stages ──────────────────────────

/// Local package name to full import path for one Go file: `"log/slog"` is
/// keyed `slog`, `l "log"` is keyed `l`. Dot and blank imports bind no name
/// and are left out. `root` is the file's root node.
pub fn go_imports(root: Node, src: &[u8]) -> HashMap<String, String> {
    let mut s = FileScope { imports: HashMap::new(), globs: vec![], fns: vec![], vars: HashMap::new() };
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_declaration" {
            collect_imports(child, src, &mut s);
        }
    }
    s.imports.into_iter().map(|(k, v)| (k, v.join("/"))).collect()
}

/// Key of a `SCOPES` entry: the source's address and length, and the root
/// node's byte span and kind.
type ScopeKey = (usize, usize, usize, usize, u16);

thread_local! {
    /// `FileScope`s that `binding_type` built, each with a copy of the source
    /// it was built from. The data-flow engine asks for one variable's type
    /// per operand it meets, so without this the same file's scope was
    /// rebuilt from its whole tree once per question.
    static SCOPES: std::cell::RefCell<HashMap<ScopeKey, (Vec<u8>, std::rc::Rc<FileScope>)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// `FileScope::build(root, src)`, reused while the same bytes come back. A
/// scope depends only on the subtree under `root`, which a parse of the same
/// bytes reproduces, so a hit is confirmed by comparing the bytes, never by
/// the address alone.
fn scope_for(root: Node, src: &[u8]) -> std::rc::Rc<FileScope> {
    let key = (src.as_ptr() as usize, src.len(), root.start_byte(), root.end_byte(), root.kind_id());
    let hit = SCOPES.with(|c| {
        c.borrow().get(&key).filter(|(bytes, _)| bytes.as_slice() == src).map(|(_, s)| s.clone())
    });
    if let Some(s) = hit {
        return s;
    }
    let s = std::rc::Rc::new(FileScope::build(root, src));
    SCOPES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 4096 {
            c.clear();
        }
        c.insert(key, (src.to_vec(), s.clone()));
    });
    s
}

/// Base type name of `var` as seen at 1-based `line` inside the function
/// declared nearest above that line: its last binding (`var x T`, `x := &T{}`,
/// `x := f()` through `f`'s indexed return type or the `NewT` convention),
/// else the receiver or parameter type. `index` may be empty, which leaves
/// only the call-return case unresolved. `file` is the path `index` uses.
pub fn binding_type(
    root: Node,
    src: &[u8],
    file: &str,
    var: &str,
    line: usize,
    index: &DefIndex,
) -> Option<String> {
    let scope = scope_for(root, src);
    let fun = scope.fns.iter().filter(|f| f.line <= line).max_by_key(|f| f.line)?;
    let constructed = HashSet::new();
    let fields = OnceCell::new();
    let ctx = Ctx { file, scope: &scope, fun, index, constructed: &constructed, fields: &fields };
    var_type(var, line, 0, &ctx)
}

/// `binding_type` with the package qualifier kept: `strings.Builder` where
/// `binding_type` gives `Builder`. The type is read from the binding that
/// `binding_type` used (a `var` type, a composite literal, a receiver or a
/// parameter). When that binding names no qualified type, as for a call
/// result, the answer is the same as `binding_type`.
pub fn binding_type_qualified(
    root: Node,
    src: &[u8],
    file: &str,
    var: &str,
    line: usize,
    index: &DefIndex,
) -> Option<String> {
    let base = binding_type(root, src, file, var, line, index)?;
    let mut decl = None;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if matches!(child.kind(), "function_declaration" | "method_declaration")
            && child.start_position().row < line
        {
            decl = Some(child);
        }
    }
    let decl = decl?;
    let mut types: Vec<Node> = Vec::new();
    for list in ["receiver", "parameters"] {
        if let Some(l) = decl.child_by_field_name(list) {
            let mut c = l.walk();
            for p in l.named_children(&mut c) {
                let mut nc = p.walk();
                if p.children_by_field_name("name", &mut nc).any(|n| text(n, src) == var) {
                    types.extend(p.child_by_field_name("type"));
                }
            }
        }
    }
    if let Some(body) = decl.child_by_field_name("body") {
        let mut stack = vec![body];
        let mut found: Vec<(usize, Node)> = Vec::new();
        while let Some(n) = stack.pop() {
            if n.start_position().row >= line {
                continue;
            }
            match n.kind() {
                "var_spec" => {
                    let mut c = n.walk();
                    if n.children_by_field_name("name", &mut c).any(|x| text(x, src) == var) {
                        let ty = n.child_by_field_name("type").or_else(|| {
                            n.child_by_field_name("value").and_then(|v| v.named_child(0)).and_then(literal_type)
                        });
                        found.extend(ty.map(|t| (n.start_byte(), t)));
                    }
                }
                "short_var_declaration" => {
                    let left = n.child_by_field_name("left");
                    if left.and_then(|l| l.named_child(0)).is_some_and(|x| text(x, src) == var) {
                        let ty = n.child_by_field_name("right").and_then(|v| v.named_child(0)).and_then(literal_type);
                        found.extend(ty.map(|t| (n.start_byte(), t)));
                    }
                }
                _ => {}
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        found.sort_by_key(|(b, _)| *b);
        if let Some((_, t)) = found.pop() {
            types = vec![t];
        }
    }
    let qualified = types.last().and_then(|t| qualified_type_name(*t, src));
    match qualified {
        Some(q) if q.rsplit('.').next() == Some(base.as_str()) => Some(q),
        _ => Some(base),
    }
}

/// The type node of a composite literal value, through `&`.
fn literal_type(value: Node) -> Option<Node> {
    match value.kind() {
        "composite_literal" => value.child_by_field_name("type"),
        "unary_expression" => literal_type(value.child_by_field_name("operand")?),
        _ => None,
    }
}

/// `type_name` that keeps a package qualifier: `pkg.Name`.
fn qualified_type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "qualified_type" => {
            let pkg = node.child_by_field_name("package")?;
            let name = node.child_by_field_name("name")?;
            Some(format!("{}.{}", text(pkg, source), text(name, source)))
        }
        "pointer_type" => node.named_child(0).and_then(|t| qualified_type_name(t, source)),
        "generic_type" => node.child_by_field_name("type").and_then(|t| qualified_type_name(t, source)),
        _ => type_name(node, source),
    }
}

#[allow(dead_code)] // wired by the path command
/// One parameter of a Go function or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoParam {
    /// Empty for an unnamed parameter (`func f(int)`) and `_` stays `_`.
    pub name: String,
    /// 0-based position in the parameter list, counting every name in
    /// `a, b int` and unnamed parameters. The receiver is not counted.
    pub position: usize,
    /// Type text as written, `...T` for a variadic parameter.
    pub ty: String,
    /// Byte range of the name, or of the type when unnamed.
    pub start_byte: usize,
    pub end_byte: usize,
}

#[allow(dead_code)] // wired by the path command
/// Receiver and parameters of one Go function, method or interface method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoSignature {
    /// Start line of the declaration, which equals `Def.id.line`.
    pub line: usize,
    pub receiver: Option<GoParam>,
    pub params: Vec<GoParam>,
}

#[allow(dead_code)] // wired by the path command
/// Signatures of every top-level function and method and every interface
/// method in one Go file, in document order. `root` is the file's root node.
pub fn go_signatures(root: Node, src: &[u8]) -> Vec<GoSignature> {
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_declaration" | "method_declaration" => {
                let receiver = child.child_by_field_name("receiver")
                    .and_then(|r| signature_params(r, src).into_iter().next());
                let params = child.child_by_field_name("parameters")
                    .map(|p| signature_params(p, src))
                    .unwrap_or_default();
                out.push(GoSignature { line: child.start_position().row + 1, receiver, params });
            }
            "type_declaration" => {
                let mut stack = vec![child];
                while let Some(n) = stack.pop() {
                    if n.kind() == "method_elem" {
                        let params = n.child_by_field_name("parameters")
                            .map(|p| signature_params(p, src))
                            .unwrap_or_default();
                        out.push(GoSignature { line: n.start_position().row + 1, receiver: None, params });
                        continue;
                    }
                    let mut c = n.walk();
                    let kids: Vec<Node> = n.named_children(&mut c).collect();
                    stack.extend(kids.into_iter().rev());
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|s| s.line);
    out
}

#[allow(dead_code)] // wired by the path command
fn signature_params(list: Node, src: &[u8]) -> Vec<GoParam> {
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for p in list.named_children(&mut cursor) {
        if !matches!(p.kind(), "parameter_declaration" | "variadic_parameter_declaration") {
            continue;
        }
        let ty_node = p.child_by_field_name("type");
        let ty_text = ty_node.map(|t| text(t, src).to_string()).unwrap_or_default();
        let ty = if p.kind() == "variadic_parameter_declaration" {
            format!("...{}", ty_text)
        } else {
            ty_text
        };
        let mut names = p.walk();
        let named: Vec<Node> = p.children_by_field_name("name", &mut names).collect();
        if named.is_empty() {
            let span = ty_node.unwrap_or(p);
            out.push(GoParam {
                name: String::new(), position: out.len(), ty: ty.clone(),
                start_byte: span.start_byte(), end_byte: span.end_byte(),
            });
        }
        for n in named {
            out.push(GoParam {
                name: text(n, src).to_string(), position: out.len(), ty: ty.clone(),
                start_byte: n.start_byte(), end_byte: n.end_byte(),
            });
        }
    }
    out
}

/// Result names of a Go function, method or literal node, one per result
/// position. An unnamed result gives an empty name, so `(string, error)`
/// gives two empty names and a single result type gives one.
/// Declared result types of a function node, one per position: `error`
/// for `(T, error)`. Empty when it has no results.
pub fn go_result_types(func: Node, src: &[u8]) -> Vec<String> {
    let Some(r) = func.child_by_field_name("result") else { return vec![] };
    if r.kind() == "parameter_list" {
        signature_params(r, src).into_iter().map(|p| p.ty).collect()
    } else {
        vec![text(r, src).to_string()]
    }
}

pub fn go_result_names(func: Node, src: &[u8]) -> Vec<String> {
    let Some(r) = func.child_by_field_name("result") else { return vec![] };
    if r.kind() == "parameter_list" {
        signature_params(r, src).into_iter().map(|p| p.name).collect()
    } else {
        vec![String::new()]
    }
}

/// Base type name (no pointer, package or type arguments) of field `field`
/// of struct `ty`, from the struct declarations in this file. Fields of
/// embedded structs are promoted.
pub fn go_field_type(root: Node, src: &[u8], ty: &str, field: &str) -> Option<String> {
    fn base(t: &str) -> String {
        let t = t.trim().trim_start_matches(['*', '&']);
        let t = t.split('[').next().unwrap_or(t);
        t.rsplit('.').next().unwrap_or(t).to_string()
    }
    fn struct_of<'t>(n: Node<'t>, src: &[u8], name: &str) -> Option<Node<'t>> {
        if n.kind() == "type_spec"
            && n.child_by_field_name("name").is_some_and(|x| text(x, src) == name)
        {
            return n.child_by_field_name("type").filter(|t| t.kind() == "struct_type");
        }
        if matches!(n.kind(), "function_declaration" | "method_declaration") {
            return None;
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.named_children(&mut c).collect();
        kids.into_iter().find_map(|k| struct_of(k, src, name))
    }
    fn find(root: Node, src: &[u8], ty: &str, field: &str, depth: usize) -> Option<String> {
        let st = struct_of(root, src, ty)?;
        let mut stack = vec![st];
        let mut decls = vec![];
        while let Some(n) = stack.pop() {
            if n.kind() == "field_declaration" {
                decls.push(n);
                continue;
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        let mut embedded = vec![];
        for d in decls {
            let t = d.child_by_field_name("type").map(|t| base(text(t, src))).unwrap_or_default();
            let mut c = d.walk();
            let names: Vec<Node> = d.children_by_field_name("name", &mut c).collect();
            if names.is_empty() {
                if t == field {
                    return Some(t);
                }
                embedded.push(t);
            } else if names.iter().any(|n| text(*n, src) == field) {
                return Some(t);
            }
        }
        if depth > 4 {
            return None;
        }
        embedded.into_iter().find_map(|e| find(root, src, &e, field, depth + 1))
    }
    find(root, src, &base(ty), field, 0)
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
            idx.impls.extend(r.collect_impls(f, src.as_bytes()));
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
    fn generic_calls_with_type_arguments_resolve() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\nfunc Extract[T any](r int) T { var v T; return v }\nfunc JSON[T any](w int, v T) {}\nfunc Go() {\n\tf := Extract[string](1)\n\tJSON[string](1, f)\n}\n",
        )]);
        assert_eq!(target_of(&results, "Go", "Extract").map(|t| t.1), Some(2));
        assert_eq!(target_of(&results, "Go", "JSON").map(|t| t.1), Some(3));
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
            "package a\ntype Writer interface {\n  Write()\n}\ntype File struct{}\nfunc (f *File) Write() {}\ntype Buf struct{}\nfunc (b *Buf) Write() {}\ntype NotWriter struct{}\nfunc (n *NotWriter) Other() {}\nfunc Save(w Writer) {\n  w.Write()\n}\nfunc mk() {\n  _ = &File{}\n  _ = &Buf{}\n}\n",
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
            "package a\ntype Writer interface {\n  Write()\n}\ntype File struct{}\nfunc (f *File) Write() {}\nfunc Save(w Writer) {\n  w.Write()\n}\nfunc mk() { _ = &File{} }\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact));
    }

    #[test]
    fn interface_dispatch_requires_full_method_set() {
        // Half implements only one of Writer's two methods — not an implementor.
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Writer interface {\n  Write()\n  Close()\n}\ntype Full struct{}\nfunc (f *Full) Write() {}\nfunc (f *Full) Close() {}\ntype Half struct{}\nfunc (h *Half) Write() {}\nfunc Save(w Writer) {\n  w.Write()\n}\nfunc mk() { _, _ = &Full{}, &Half{} }\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (7, Confidence::Exact), "only Full implements Writer");
    }

    #[test]
    fn interface_dispatch_skips_types_never_constructed() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype W interface {\n  Write()\n}\ntype Made struct{}\nfunc (m *Made) Write() {}\ntype Ghost struct{}\nfunc (g *Ghost) Write() {}\nfunc Save(w W) {\n  w.Write()\n}\nfunc mk() { _ = new(Made) }\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (6, Confidence::Exact), "Ghost is never built");
    }

    #[test]
    fn implementations_that_are_never_constructed_leave_the_signature() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype W interface {\n  Write()\n}\ntype Ghost struct{}\nfunc (g *Ghost) Write() {}\nfunc Save(w W) {\n  w.Write()\n}\n",
        )]);
        let (_, line, _) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!(line, 3);
    }

    #[test]
    fn type_assertion_narrows_to_one_type() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype W interface {\n  Write()\n}\ntype F struct{}\nfunc (f *F) Write() {}\ntype B struct{}\nfunc (b *B) Write() {}\nfunc mk() { _, _ = &F{}, &B{} }\nfunc Save(w W) {\n  if b, ok := w.(*B); ok {\n    b.Write()\n  }\n}\n",
        )]);
        let (_, line, conf) = target_of(&results, "Save", "Write").unwrap();
        assert_eq!((line, conf), (8, Confidence::Exact));
    }

    #[test]
    fn type_switch_case_narrows_and_default_does_not() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype W interface {\n  Write()\n}\ntype F struct{}\nfunc (f *F) Write() {}\ntype B struct{}\nfunc (b *B) Write() {}\nfunc mk() { _, _ = &F{}, &B{} }\nfunc Save(w W) {\n  switch t := w.(type) {\n  case *F:\n    t.Write()\n  default:\n    w.Write()\n  }\n}\n",
        )]);
        let lines: Vec<(usize, usize)> = results.iter()
            .filter(|(cr, ce, _)| cr == "Save" && ce == "Write")
            .filter_map(|(_, _, t)| t.clone().map(|(_, l, _)| l))
            .enumerate().collect();
        assert_eq!(lines, vec![(0, 6), (1, 6), (2, 8)], "case F calls F only, default calls both: {:?}", lines);
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
    fn method_on_an_untyped_receiver_is_unresolved() {
        // `x` has no type, so neither same-named function is a target
        let results = resolve_all(&[
            ("a/near.go", "package a\nfunc Helper() {}\n"),
            ("far/far.go", "package far\nfunc Helper() {}\n"),
            ("a/main.go", "package a\nfunc Go() { x.Helper() }\n"),
        ]);
        assert_eq!(target_of(&results, "Go", "Helper"), None);
    }

    #[test]
    fn go_imports_maps_local_name_to_full_path() {
        let src = "package main\nimport (\n  \"fmt\"\n  \"log/slog\"\n  l \"log\"\n  . \"proj/dot\"\n  _ \"side\"\n)\n";
        let tree = parse(src.as_bytes()).unwrap();
        let m = go_imports(tree.root_node(), src.as_bytes());
        assert_eq!(m.get("fmt").map(String::as_str), Some("fmt"));
        assert_eq!(m.get("slog").map(String::as_str), Some("log/slog"));
        assert_eq!(m.get("l").map(String::as_str), Some("log"));
        assert_eq!(m.len(), 3, "dot and blank imports bind no name: {:?}", m);
    }

    #[test]
    fn binding_type_reads_params_literals_and_returns() {
        let src = "package a\ntype S struct{}\nfunc makeIt() *S { return &S{} }\nfunc Go(p *S) {\n  var v S\n  x := &S{}\n  y := makeIt()\n  z := NewT()\n  _ = v\n}\n";
        let tree = parse(src.as_bytes()).unwrap();
        let root = tree.root_node();
        let idx = build_index(&[("a/a.go", src)]);
        let ty = |var: &str| binding_type(root, src.as_bytes(), "a/a.go", var, 9, &idx);
        assert_eq!(ty("p").as_deref(), Some("S"));
        assert_eq!(ty("v").as_deref(), Some("S"));
        assert_eq!(ty("x").as_deref(), Some("S"));
        assert_eq!(ty("y").as_deref(), Some("S"));
        assert_eq!(ty("z").as_deref(), Some("T"));
        assert_eq!(ty("nope"), None);
        // before the binding line the variable has no type yet
        assert_eq!(binding_type(root, src.as_bytes(), "a/a.go", "x", 5, &idx), None);
    }

    #[test]
    fn binding_type_qualified_keeps_the_package() {
        let src = "package a\nimport \"strings\"\nfunc Go(w *http.Writer, s S) {\n  var b strings.Builder\n  p := &bytes.Buffer{}\n  _ = b\n}\n";
        let tree = parse(src.as_bytes()).unwrap();
        let root = tree.root_node();
        let idx = DefIndex::default();
        let ty = |var: &str| binding_type_qualified(root, src.as_bytes(), "a/a.go", var, 6, &idx);
        assert_eq!(ty("b").as_deref(), Some("strings.Builder"));
        assert_eq!(ty("p").as_deref(), Some("bytes.Buffer"));
        assert_eq!(ty("w").as_deref(), Some("http.Writer"));
        assert_eq!(ty("s").as_deref(), Some("S"));
        assert_eq!(ty("nope"), None);
    }

    #[test]
    fn go_signatures_list_receiver_and_positioned_params() {
        let src = "package a\ntype W interface {\n  Write(p []byte, n int) error\n}\nfunc (s *S) Run(a, b int, _ string, rest ...any) {}\nfunc F(int, *S) {}\n";
        let tree = parse(src.as_bytes()).unwrap();
        let sigs = go_signatures(tree.root_node(), src.as_bytes());
        let lines: Vec<usize> = sigs.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![3, 5, 6], "{:?}", sigs);
        let write: Vec<(&str, usize, &str)> =
            sigs[0].params.iter().map(|p| (p.name.as_str(), p.position, p.ty.as_str())).collect();
        assert_eq!(write, vec![("p", 0, "[]byte"), ("n", 1, "int")]);
        let run = &sigs[1];
        let recv = run.receiver.as_ref().unwrap();
        assert_eq!((recv.name.as_str(), recv.ty.as_str()), ("s", "*S"));
        assert_eq!(&src[recv.start_byte..recv.end_byte], "s");
        let params: Vec<(&str, usize, &str)> =
            run.params.iter().map(|p| (p.name.as_str(), p.position, p.ty.as_str())).collect();
        assert_eq!(
            params,
            vec![("a", 0, "int"), ("b", 1, "int"), ("_", 2, "string"), ("rest", 3, "...any")]
        );
        let unnamed: Vec<(&str, usize, &str)> =
            sigs[2].params.iter().map(|p| (p.name.as_str(), p.position, p.ty.as_str())).collect();
        assert_eq!(unnamed, vec![("", 0, "int"), ("", 1, "*S")]);
    }

    #[test]
    fn zero_candidates_stay_unresolved() {
        let results = resolve_all(&[("a/a.go", "package a\nfunc Go() { println() }\n")]);
        assert_eq!(target_of(&results, "Go", "println"), None);
    }

    /// The `fixtures/resolve-strict/go` package pair: `api` calls through
    /// receivers whose methods share names with `api.Cache`'s, which the
    /// name-only fallback would pick.
    fn strict_fixture() -> Vec<(String, String, Option<(String, usize, Confidence)>)> {
        resolve_all(&[
            ("resolve-strict/go/store/store.go", include_str!("../../fixtures/resolve-strict/go/store/store.go")),
            ("resolve-strict/go/api/cache.go", include_str!("../../fixtures/resolve-strict/go/api/cache.go")),
            ("resolve-strict/go/api/handler.go", include_str!("../../fixtures/resolve-strict/go/api/handler.go")),
        ])
    }

    fn all_targets(
        results: &[(String, String, Option<(String, usize, Confidence)>)],
        caller: &str,
        callee: &str,
    ) -> Vec<Option<(String, usize, Confidence)>> {
        results.iter()
            .filter(|(cr, ce, _)| cr == caller && ce == callee)
            .map(|(_, _, t)| t.clone())
            .collect()
    }

    const STORE: &str = "resolve-strict/go/store/store.go";
    const CACHE: &str = "resolve-strict/go/api/cache.go";

    #[test]
    fn field_chain_receiver_resolves_through_the_field_type() {
        let r = strict_fixture();
        assert_eq!(target_of(&r, "Index", "List"), Some((STORE.into(), 9, Confidence::Exact)));
    }

    #[test]
    fn field_chain_promotes_fields_of_embedded_structs() {
        let r = strict_fixture();
        assert_eq!(target_of(&r, "Promoted", "List"), Some((STORE.into(), 9, Confidence::Exact)));
    }

    #[test]
    fn call_result_receiver_resolves_through_the_return_type() {
        let r = strict_fixture();
        assert_eq!(target_of(&r, "Rows", "Close"), Some((STORE.into(), 13, Confidence::Exact)));
        assert_eq!(target_of(&r, "Rows", "Query"), Some((STORE.into(), 11, Confidence::Exact)));
        assert_eq!(all_targets(&r, "Rows", "List"), vec![Some((STORE.into(), 9, Confidence::Exact))]);
    }

    #[test]
    fn closure_named_like_a_method_has_no_target() {
        let r = strict_fixture();
        let calls = all_targets(&r, "Footer", "write");
        assert_eq!(calls, vec![None], "the closure must not reach Cache.write by name");
    }

    #[test]
    fn same_package_method_on_typed_variable_still_resolves() {
        let r = strict_fixture();
        assert_eq!(target_of(&r, "Control", "List"), Some((CACHE.into(), 8, Confidence::Exact)));
    }

    #[test]
    fn func_typed_var_is_a_closure() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype T struct{}\nfunc (t *T) run() {}\nfunc Go() {\n  var run func()\n  run()\n}\n",
        )]);
        assert_eq!(all_targets(&results, "Go", "run"), vec![None]);
    }

    #[test]
    fn call_result_of_an_outside_call_types_nothing() {
        // `lib.Get()` is outside the project, so its result types nothing
        // and `.Close()` stays unresolved rather than reaching `R.Close`.
        let results = resolve_all(&[
            ("a/a.go", "package a\nimport \"ext/lib\"\ntype R struct{}\nfunc (r *R) Close() {}\nfunc Go() { lib.Get().Close() }\n"),
        ]);
        assert_eq!(all_targets(&results, "Go", "Close"), vec![None]);
    }

    #[test]
    fn outside_receiver_type_does_not_reach_a_project_method_by_name() {
        let r = strict_fixture();
        assert_eq!(all_targets(&r, "Header", "Set"), vec![None], "w.Header().Set must not reach Cache.Set");
        assert_eq!(all_targets(&r, "Header", "Header"), vec![None]);
        // positive control: the same name on a typed receiver
        assert_eq!(target_of(&r, "Control", "Set"), Some((CACHE.into(), 14, Confidence::Exact)));
    }

    #[test]
    fn bare_call_out_of_scope_has_no_target() {
        let r = strict_fixture();
        assert_eq!(all_targets(&r, "Bare", "Open"), vec![None], "store.Open is not in scope in package api");
    }

    #[test]
    fn package_level_typed_var_types_its_receiver() {
        let results = resolve_all(&[
            ("a/a.go", "package a\ntype T struct{}\nfunc (t *T) Run() {}\nvar global *T\nfunc Go() { global.Run() }\n"),
        ]);
        assert_eq!(target_of(&results, "Go", "Run"), Some(("a/a.go".into(), 3, Confidence::Exact)));
    }

    #[test]
    fn generic_field_takes_its_constraint_type() {
        let results = resolve_all(&[(
            "a/a.go",
            "package a\ntype Driver interface{ Exec() }\ntype S struct{}\nfunc (s *S) Exec() {}\n\
             type Repo[D Driver] struct{ d D }\nfunc (r *Repo[D]) Add() { r.d.Exec() }\nfunc New() { _ = &S{} }\n",
        )]);
        assert_eq!(target_of(&results, "Add", "Exec"), Some(("a/a.go".into(), 4, Confidence::Exact)));
    }

    #[test]
    fn receiver_typed_through_a_binding_a_field_chain_and_a_call() {
        let results = resolve_all(&[
            ("s/s.go", "package s\ntype Store struct{}\ntype WS struct{}\nfunc (s *Store) Workspace(id string) *WS { return nil }\nfunc (w *WS) GetEvent() {}\n"),
            ("a/a.go", "package a\nimport \"x/s\"\ntype Handler struct{ Store *s.Store }\ntype harness struct{ H *Handler }\n\
              func newHarness() *harness { return nil }\nfunc Go() {\n  hs := newHarness()\n  ws := hs.H.Store.Workspace(\"id\")\n  ws.GetEvent()\n}\n"),
        ]);
        assert_eq!(target_of(&results, "Go", "GetEvent"), Some(("s/s.go".into(), 5, Confidence::Exact)));
    }

    #[test]
    fn untyped_receiver_has_no_target() {
        let results = resolve_all(&[
            ("a/a.go", "package a\ntype T struct{}\nfunc (t *T) Run() {}\nvar global = load()\nfunc Go() { global.Run() }\n"),
        ]);
        assert_eq!(all_targets(&results, "Go", "Run"), vec![None]);
    }

    #[test]
    fn struct_fields_pick_the_local_declaration_when_names_clash() {
        let idx = build_index(&[
            ("x/a.go", "package x\ntype H struct{ s *A }\n"),
            ("y/b.go", "package y\ntype H struct{ s *B }\n"),
        ]);
        let f = StructFields::of(&idx);
        assert_eq!(f.lookup("H", "s", "x/main.go", 0).as_deref(), Some("A"));
        assert_eq!(f.lookup("H", "s", "z/main.go", 0), None);
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
