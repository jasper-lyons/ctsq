#![allow(dead_code)]
//! The file model for TypeScript, TSX, JavaScript and JSX (STAGE-B package
//! ts-model, B2). It resolves nothing: B3's `TsResolver` reads it.
//!
//! `FileScope::build` takes the file's definitions as `DefIndex` lists them
//! (`own`, from `tree::collect_function_locs`) and records, per definition,
//! the call sites inside it with cleaned callee names (B03), plus every local
//! binding with the byte range it is visible in.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::{Node, Tree};

use super::Def;

/// The callee of a call site, cleaned (B03): type arguments are dropped,
/// `a?.b()` reads as `a.b()` and `new X()` as a call to `X`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// `f()`, `f<T>()`, `f?.()`, `new F()`.
    Bare(String),
    /// `recv.name()`, `recv?.name()`, `new recv.Name()` and a dotted JSX tag
    /// `<Recv.Name>`. `receiver` is the object's text with `?.` read as `.`
    /// when it is a plain member chain, otherwise its source text with
    /// whitespace runs collapsed to one space.
    Member { receiver: String, name: String },
    /// A JSX element with an undotted tag: `<Row />`, `<div>`.
    Jsx(String),
}

impl Callee {
    /// The name the call reaches: the bare name, the property or the tag.
    pub fn name(&self) -> &str {
        match self {
            Callee::Bare(n) | Callee::Jsx(n) | Callee::Member { name: n, .. } => n,
        }
    }
}

/// One call inside a listed definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Index into `own` (and `FileScope::defs`): the innermost listed
    /// definition holding the call (B04 as amended).
    pub def: usize,
    pub callee: Callee,
    /// 1-based line of the matched node's start.
    pub line: usize,
    /// Span of the node the TS `call` abstract type matches
    /// (`call_expression`, `jsx_self_closing_element`, `jsx_opening_element`),
    /// or of the `new_expression`.
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// A function, method or catch parameter, destructured ones included.
    Param,
    Const,
    /// `let`, and `var`.
    Let,
    /// A name bound by an array or object pattern in a `const`/`let`/`var`
    /// or a `for (… of/in …)` head.
    Destructure,
    /// A function declaration that is not a listed definition.
    Fn,
}

/// A local name. Import bindings are not here (B1 owns them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub kind: BindingKind,
    /// The block or function the name is visible in.
    pub scope: Range<usize>,
    /// Start byte of the name where it is declared.
    pub decl_byte: usize,
}

impl Binding {
    /// Whether a call starting at `byte` lies inside this binding's scope.
    pub fn encloses(&self, byte: usize) -> bool {
        self.scope.contains(&byte)
    }
}

/// One entry of `own`, in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsDef {
    pub name: String,
    pub line: usize,
    /// Exported from the module: `export function`, `export const`,
    /// `export default X`, or `export { X }` without a `from`. Class methods
    /// are never exported.
    pub exported: bool,
    /// The declaration node's span, `None` when no node in the file matched
    /// the def's name and line.
    pub span: Option<Range<usize>>,
    pub calls: Vec<CallSite>,
}

pub struct FileScope {
    pub defs: Vec<TsDef>,
    pub bindings: Vec<Binding>,
}

/// Node kinds that introduce a function with parameters.
const FUNCTION_KINDS: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "function",
    "generator_function",
    "arrow_function",
    "method_definition",
];

/// Node kinds `tree.rs` lists definitions from, in TS and JS.
const DEF_KINDS: &[&str] = &[
    "function_declaration",
    "function_signature",
    "method_definition",
    "abstract_method_signature",
    "lexical_declaration",
    "variable_declaration",
];

/// Ancestors that bound a `const`/`let` and a nested function declaration.
const BLOCK_KINDS: &[&str] =
    &["statement_block", "program", "for_statement", "for_in_statement", "class_static_block", "switch_body"];

impl FileScope {
    /// Builds the model of one parsed file. `own` is the file's definitions
    /// as `Resolver::model` receives them; `defs` keeps their order.
    pub fn build(tree: &Tree, source: &[u8], own: &[Def]) -> FileScope {
        let mut defs: Vec<TsDef> = own
            .iter()
            .map(|d| TsDef { name: d.name.clone(), line: d.id.line, exported: false, span: None, calls: vec![] })
            .collect();
        let mut by_key: HashMap<(String, usize), Vec<usize>> = HashMap::new();
        for (i, d) in own.iter().enumerate() {
            by_key.entry((d.name.clone(), d.id.line)).or_default().push(i);
        }
        let mut w = Walk { source, by_key, defs: &mut defs, method: HashSet::new(), exports: HashSet::new(), calls: vec![], bindings: vec![] };
        w.visit(tree.root_node());
        let Walk { method, exports, calls, bindings, .. } = w;

        for (i, d) in defs.iter_mut().enumerate() {
            d.exported = d.span.is_some() && !method.contains(&i) && exports.contains(&d.name);
        }
        for (callee, node_span, line) in calls {
            let holder = defs
                .iter()
                .enumerate()
                .filter_map(|(i, d)| d.span.as_ref().filter(|s| s.contains(&node_span.start)).map(|s| (i, s.len())))
                .min_by_key(|&(_, len)| len);
            let Some((def, _)) = holder else { continue };
            defs[def].calls.push(CallSite { def, callee, line, start_byte: node_span.start, end_byte: node_span.end });
        }
        FileScope { defs, bindings }
    }

    /// The line of each definition, and for each call (start byte, end byte)
    /// its (definition, call) position: the shape of
    /// `go::FileScope::call_positions`.
    pub(crate) fn call_positions(&self) -> (Vec<usize>, HashMap<(usize, usize), (usize, usize)>) {
        let mut at = HashMap::new();
        for (d, def) in self.defs.iter().enumerate() {
            for (c, call) in def.calls.iter().enumerate() {
                at.entry((call.start_byte, call.end_byte)).or_insert((d, c));
            }
        }
        (self.defs.iter().map(|d| d.line).collect(), at)
    }

    /// The innermost binding of `name` whose scope holds `byte`.
    pub fn binding_for(&self, name: &str, byte: usize) -> Option<&Binding> {
        self.bindings.iter().filter(|b| b.name == name && b.encloses(byte)).min_by_key(|b| b.scope.len())
    }
}

struct Walk<'a> {
    source: &'a [u8],
    by_key: HashMap<(String, usize), Vec<usize>>,
    defs: &'a mut Vec<TsDef>,
    /// Defs whose node is a class method.
    method: HashSet<usize>,
    /// Names the module exports from its own declarations.
    exports: HashSet<String>,
    calls: Vec<(Callee, Range<usize>, usize)>,
    bindings: Vec<Binding>,
}

impl Walk<'_> {
    fn text(&self, n: Node) -> &str {
        n.utf8_text(self.source).unwrap_or("")
    }

    fn visit(&mut self, node: Node) {
        let kind = node.kind();
        if DEF_KINDS.contains(&kind) {
            self.match_def(node);
        }
        if kind == "export_statement" {
            self.record_export(node);
        }
        if let Some(callee) = self.callee(node) {
            self.calls.push((callee, node.byte_range(), node.start_position().row + 1));
        }
        self.record_bindings(node);
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            self.visit(child);
        }
    }

    // ── definitions ──────────────────────────────────────────────────────

    /// The name `tree.rs` lists a definition node under.
    fn def_name(&self, node: Node) -> Option<String> {
        match node.kind() {
            "lexical_declaration" | "variable_declaration" => {
                let mut cur = node.walk();
                let decl = node.named_children(&mut cur).find(|c| c.kind() == "variable_declarator")?;
                decl.child_by_field_name("name").map(|n| self.text(n).to_string())
            }
            _ => node.child_by_field_name("name").map(|n| self.text(n).to_string()),
        }
    }

    fn match_def(&mut self, node: Node) {
        let Some(name) = self.def_name(node) else { return };
        let key = (name, node.start_position().row + 1);
        let Some(cands) = self.by_key.get(&key) else { return };
        let Some(&i) = cands.iter().find(|&&i| self.defs[i].span.is_none()) else { return };
        self.defs[i].span = Some(node.byte_range());
        if matches!(node.kind(), "method_definition" | "abstract_method_signature") {
            self.method.insert(i);
        }
    }

    /// Whether `node` is the declaration of a listed definition.
    fn is_listed(&self, node: Node) -> bool {
        let r = node.byte_range();
        self.defs.iter().any(|d| d.span.as_ref() == Some(&r))
    }

    fn record_export(&mut self, node: Node) {
        if node.child_by_field_name("source").is_some() {
            return; // a re-export names another module's bindings
        }
        let mut cur = node.walk();
        let children: Vec<Node> = node.named_children(&mut cur).collect();
        for c in children {
            match c.kind() {
                "export_clause" => {
                    let mut cur = c.walk();
                    let specs: Vec<Node> = c.named_children(&mut cur).collect();
                    for s in specs {
                        if let Some(n) = s.child_by_field_name("name") {
                            self.exports.insert(self.text(n).to_string());
                        }
                    }
                }
                "identifier" => {
                    self.exports.insert(self.text(c).to_string());
                }
                "lexical_declaration" | "variable_declaration" => {
                    let mut cur = c.walk();
                    let decls: Vec<Node> = c.named_children(&mut cur).collect();
                    for d in decls {
                        if let Some(n) = d.child_by_field_name("name") {
                            let mut names = vec![];
                            pattern_names(n, &mut names);
                            for n in names {
                                self.exports.insert(self.text(n).to_string());
                            }
                        }
                    }
                }
                _ => {
                    if let Some(n) = c.child_by_field_name("name") {
                        self.exports.insert(self.text(n).to_string());
                    }
                }
            }
        }
    }

    // ── call sites ───────────────────────────────────────────────────────

    /// The cleaned callee when `node` is a call the TS `call` type matches,
    /// or a `new_expression`.
    fn callee(&self, node: Node) -> Option<Callee> {
        match node.kind() {
            "call_expression" => self.target(node.child_by_field_name("function")?, false),
            "new_expression" => self.target(node.child_by_field_name("constructor")?, false),
            "jsx_self_closing_element" | "jsx_opening_element" => self.target(node.child_by_field_name("name")?, true),
            _ => None,
        }
    }

    fn target(&self, f: Node, jsx: bool) -> Option<Callee> {
        match f.kind() {
            "identifier" if jsx => Some(Callee::Jsx(self.text(f).to_string())),
            "identifier" => Some(Callee::Bare(self.text(f).to_string())),
            "member_expression" => {
                let object = f.child_by_field_name("object")?;
                let property = f.child_by_field_name("property")?;
                Some(Callee::Member { receiver: self.clean(object), name: self.text(property).to_string() })
            }
            // Older JSX grammars spell a dotted tag as one node.
            "nested_identifier" if jsx => {
                let t = self.text(f);
                let (recv, name) = t.rsplit_once('.')?;
                Some(Callee::Member { receiver: recv.to_string(), name: name.to_string() })
            }
            "jsx_namespace_name" if jsx => Some(Callee::Jsx(self.text(f).to_string())),
            _ => None,
        }
    }

    /// Receiver text: a member chain with `?.` read as `.`, else the source
    /// text with whitespace runs collapsed.
    fn clean(&self, n: Node) -> String {
        match n.kind() {
            "member_expression" => {
                let (Some(o), Some(p)) = (n.child_by_field_name("object"), n.child_by_field_name("property")) else {
                    return self.collapsed(n);
                };
                format!("{}.{}", self.clean(o), self.text(p))
            }
            "non_null_expression" => match n.named_child(0) {
                Some(inner) => self.clean(inner),
                None => self.collapsed(n),
            },
            _ => self.collapsed(n),
        }
    }

    fn collapsed(&self, n: Node) -> String {
        self.text(n).split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // ── bindings ─────────────────────────────────────────────────────────

    fn bind(&mut self, names: Vec<Node>, kind: BindingKind, scope: Range<usize>) {
        for n in names {
            let name = self.text(n).to_string();
            self.bindings.push(Binding { name, kind, scope: scope.clone(), decl_byte: n.start_byte() });
        }
    }

    fn record_bindings(&mut self, node: Node) {
        let kind = node.kind();
        if FUNCTION_KINDS.contains(&kind) {
            let mut names = vec![];
            if let Some(p) = node.child_by_field_name("parameter") {
                pattern_names(p, &mut names);
            }
            if let Some(ps) = node.child_by_field_name("parameters") {
                let mut cur = ps.walk();
                for p in ps.named_children(&mut cur) {
                    let pat = match p.kind() {
                        "required_parameter" | "optional_parameter" => p.child_by_field_name("pattern"),
                        _ => Some(p),
                    };
                    if let Some(pat) = pat {
                        pattern_names(pat, &mut names);
                    }
                }
            }
            self.bind(names, BindingKind::Param, node.byte_range());
        }
        match kind {
            "function_declaration" | "generator_function_declaration" if !self.is_listed(node) => {
                let (Some(n), Some(block)) = (node.child_by_field_name("name"), enclosing(node, BLOCK_KINDS)) else {
                    return;
                };
                self.bind(vec![n], BindingKind::Fn, block.byte_range());
            }
            "lexical_declaration" | "variable_declaration" => {
                let is_var = kind == "variable_declaration";
                let scope = if is_var {
                    enclosing(node, FUNCTION_KINDS).or_else(|| enclosing(node, &["program"]))
                } else {
                    enclosing(node, BLOCK_KINDS)
                };
                let Some(scope) = scope else { return };
                let is_const = !is_var && node.child(0).is_some_and(|k| self.text(k) == "const");
                // A listed definition declared this way is a def, not a local.
                let listed = self.is_listed(node).then(|| self.def_name(node)).flatten();
                let mut cur = node.walk();
                let decls: Vec<Node> = node.named_children(&mut cur).filter(|c| c.kind() == "variable_declarator").collect();
                for d in decls {
                    let Some(n) = d.child_by_field_name("name") else { continue };
                    if n.kind() == "identifier" {
                        if listed.as_deref() == Some(self.text(n)) {
                            continue;
                        }
                        let k = if is_const { BindingKind::Const } else { BindingKind::Let };
                        self.bind(vec![n], k, scope.byte_range());
                    } else {
                        let mut names = vec![];
                        pattern_names(n, &mut names);
                        self.bind(names, BindingKind::Destructure, scope.byte_range());
                    }
                }
            }
            "for_in_statement" => {
                // `for (const x of xs)` binds; `for (x of xs)` assigns.
                if node.child_by_field_name("kind").is_none() {
                    return;
                }
                let Some(left) = node.child_by_field_name("left") else { return };
                let is_const = node.child_by_field_name("kind").is_some_and(|k| self.text(k) == "const");
                let k = if left.kind() != "identifier" {
                    BindingKind::Destructure
                } else if is_const {
                    BindingKind::Const
                } else {
                    BindingKind::Let
                };
                let mut names = vec![];
                pattern_names(left, &mut names);
                self.bind(names, k, node.byte_range());
            }
            "catch_clause" => {
                let Some(p) = node.child_by_field_name("parameter") else { return };
                let mut names = vec![];
                pattern_names(p, &mut names);
                self.bind(names, BindingKind::Param, node.byte_range());
            }
            _ => {}
        }
    }
}

/// The nearest proper ancestor of one of `kinds`.
fn enclosing<'t>(node: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut at = node.parent();
    while let Some(n) = at {
        if kinds.contains(&n.kind()) {
            return Some(n);
        }
        at = n.parent();
    }
    None
}

/// The identifiers a binding pattern introduces, in source order.
fn pattern_names<'t>(p: Node<'t>, out: &mut Vec<Node<'t>>) {
    match p.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(p),
        // `{ key: value }`: the value is bound, not the key.
        "pair_pattern" => {
            if let Some(v) = p.child_by_field_name("value") {
                pattern_names(v, out);
            }
        }
        // `x = 1` in a pattern or a parameter: only the left side binds.
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(l) = p.child_by_field_name("left") {
                pattern_names(l, out);
            }
        }
        "object_pattern" | "array_pattern" | "rest_pattern" => {
            let mut cur = p.walk();
            let kids: Vec<Node<'t>> = p.named_children(&mut cur).collect();
            for k in kids {
                pattern_names(k, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{DefId, DefKind};

    fn fixture(name: &str) -> String {
        format!("{}/fixtures/ts-resolve/model/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    /// The model `Resolver::model` would build: defs from the tree outline.
    fn model(name: &str, lang: &'static str) -> (FileScope, String) {
        let path = fixture(name);
        let own: Vec<Def> = crate::tree::collect_function_locs(&path, lang)
            .into_iter()
            .map(|(n, line)| Def {
                name: n.clone(),
                qualified: vec![n],
                kind: DefKind::Function,
                ret: None,
                id: DefId { file: path.clone(), line },
            })
            .collect();
        let p = crate::parsed::get(&path, lang).expect("fixture parses");
        let src = String::from_utf8(p.source.to_vec()).unwrap();
        (FileScope::build(p.tree.as_ref().unwrap(), &p.source, &own), src)
    }

    /// The byte span of `text`, which occurs once in `src`.
    fn span(src: &str, text: &str) -> (usize, usize) {
        assert_eq!(src.matches(text).count(), 1, "{text:?} occurs once");
        let s = src.find(text).unwrap();
        (s, s + text.len())
    }

    fn bare(n: &str) -> Callee {
        Callee::Bare(n.into())
    }
    fn jsx(n: &str) -> Callee {
        Callee::Jsx(n.into())
    }
    fn member(r: &str, n: &str) -> Callee {
        Callee::Member { receiver: r.into(), name: n.into() }
    }

    fn site(def: usize, callee: Callee, line: usize, (s, e): (usize, usize)) -> CallSite {
        CallSite { def, callee, line, start_byte: s, end_byte: e }
    }

    fn def_index(fs: &FileScope, name: &str) -> usize {
        fs.defs.iter().position(|d| d.name == name).unwrap()
    }

    fn binding<'a>(fs: &'a FileScope, name: &str) -> &'a Binding {
        let all: Vec<&Binding> = fs.bindings.iter().filter(|b| b.name == name).collect();
        assert_eq!(all.len(), 1, "one binding named {name}: {all:?}");
        all[0]
    }

    #[test]
    fn defs_follow_own_with_export_flags() {
        let (fs, _) = model("calls.tsx", "tsx");
        let got: Vec<(&str, usize, bool)> = fs.defs.iter().map(|d| (d.name.as_str(), d.line, d.exported)).collect();
        assert_eq!(
            got,
            vec![("load", 6, true), ("Picker", 14, true), ("save", 30, false), ("send", 36, false), ("persist", 40, false)]
        );
        assert!(fs.defs.iter().all(|d| d.span.is_some()));
        let (lines, _) = fs.call_positions();
        assert_eq!(lines, vec![6, 14, 30, 36, 40]);
    }

    #[test]
    fn generic_optional_and_new_calls() {
        let (fs, src) = model("calls.tsx", "tsx");
        let load = def_index(&fs, "load");
        assert_eq!(
            fs.defs[load].calls,
            vec![
                site(load, member("api", "get"), 7, span(&src, "api.get<Item[]>(`/items/${id}`)")),
                site(load, bare("send"), 8, span(&src, "send<void>(id)")),
                site(load, member("api", "find"), 9, span(&src, "api?.find(id)")),
                site(load, bare("Map"), 10, span(&src, "new Map<string, Item>()")),
            ]
        );
        let id = binding(&fs, "id");
        assert_eq!(id.kind, BindingKind::Param);
        assert_eq!(id.scope.start, span(&src, "export function load").0 + "export ".len());
        assert_eq!(binding(&fs, "rows").kind, BindingKind::Const);
    }

    #[test]
    fn jsx_signal_pair_and_nested_arrow() {
        let (fs, src) = model("calls.tsx", "tsx");
        let p = def_index(&fs, "Picker");
        let panel = "<Panel\n      title=\"Pick\"\n      onClick={toggle}\n    >";
        assert_eq!(
            fs.defs[p].calls,
            vec![
                site(p, bare("createSignal"), 15, span(&src, "createSignal(false)")),
                // inside the nested arrow `toggle`, still Picker's
                site(p, bare("setOpen"), 17, span(&src, "setOpen(!open())")),
                site(p, bare("open"), 17, span(&src, "open()")),
                site(p, jsx("Panel"), 20, span(&src, panel)),
                site(p, member("Menu", "Item"), 24, span(&src, "<Menu.Item label=\"one\" />")),
            ]
        );
        let body_start = span(&src, "=> {\n  const [open").0 + 3;
        let body_end = span(&src, "  );\n};\n\nclass").1 - "};\n\nclass".len() + 1;
        for name in ["open", "setOpen"] {
            let b = binding(&fs, name);
            assert_eq!(b.kind, BindingKind::Destructure);
            assert_eq!(b.scope, body_start..body_end);
        }
        assert_eq!(binding(&fs, "setOpen").decl_byte, span(&src, "setOpen]").0);
        let toggle = binding(&fs, "toggle");
        assert_eq!(toggle.kind, BindingKind::Const);
        let call = &fs.defs[p].calls[1];
        assert_eq!(fs.binding_for("setOpen", call.start_byte), Some(binding(&fs, "setOpen")));
        assert_eq!(binding(&fs, "props").kind, BindingKind::Param);
        // listed defs are not locals
        assert!(fs.bindings.iter().all(|b| !["load", "Picker", "send", "persist"].contains(&b.name.as_str())));
    }

    #[test]
    fn class_method_keeps_its_calls() {
        let (fs, src) = model("calls.tsx", "tsx");
        let s = def_index(&fs, "save");
        assert_eq!(
            fs.defs[s].calls,
            vec![
                site(s, bare("persist"), 31, span(&src, "persist(item)")),
                site(s, member("this", "flush"), 32, span(&src, "this.flush()")),
            ]
        );
        assert_eq!(binding(&fs, "item").kind, BindingKind::Param);
    }

    #[test]
    fn for_of_and_let_bindings() {
        let (fs, src) = model("calls.tsx", "tsx");
        let k = binding(&fs, "k");
        assert_eq!(k.kind, BindingKind::Const);
        assert_eq!(k.scope.start, span(&src, "for (const k").0);
        let n = binding(&fs, "n");
        assert_eq!(n.kind, BindingKind::Let);
        assert_eq!(n.scope.start, span(&src, "{\n    let n").0);
        let per = def_index(&fs, "persist");
        assert_eq!(fs.defs[per].calls, vec![site(per, member("Object", "keys"), 41, span(&src, "Object.keys(x)"))]);
    }

    #[test]
    fn call_positions_index_every_site() {
        let (fs, src) = model("calls.tsx", "tsx");
        let (_, at) = fs.call_positions();
        let total: usize = fs.defs.iter().map(|d| d.calls.len()).sum();
        assert_eq!(at.len(), total);
        assert_eq!(at.get(&span(&src, "this.flush()")), Some(&(def_index(&fs, "save"), 1)));
    }

    #[test]
    fn jsx_file_with_javascript_grammar() {
        let (fs, src) = model("widget.jsx", "javascript");
        let got: Vec<(&str, usize, bool)> = fs.defs.iter().map(|d| (d.name.as_str(), d.line, d.exported)).collect();
        assert_eq!(got, vec![("Widget", 3, true), ("format", 13, false)]);
        let w = def_index(&fs, "Widget");
        assert_eq!(
            fs.defs[w].calls,
            vec![
                site(w, member("Card", "Body"), 6, span(&src, "<Card.Body heading={title}>")),
                site(w, member("rest", "map"), 7, span(&src, "rest.map((it) => <Line key={it} />)")),
                site(w, jsx("Line"), 7, span(&src, "<Line key={it} />")),
                site(w, bare("format"), 8, span(&src, "format(first)")),
            ]
        );
        for name in ["title", "items", "it"] {
            assert_eq!(binding(&fs, name).kind, BindingKind::Param, "{name}");
        }
        for name in ["first", "rest"] {
            assert_eq!(binding(&fs, name).kind, BindingKind::Destructure, "{name}");
        }
        let f = def_index(&fs, "format");
        assert_eq!(fs.defs[f].calls, vec![site(f, bare("String"), 14, span(&src, "String(x)"))]);
    }

    #[test]
    fn assets_index_excerpt() {
        let (fs, whole) = model("AssetsIndex.tsx", "tsx");
        // The header comment quotes the lines under test: search past it.
        let skip = whole.find("import").unwrap();
        let src = whole[skip..].to_string();
        let span = |src: &str, text: &str| {
            let (s, e) = span(src, text);
            (s + skip, e + skip)
        };
        let ai = def_index(&fs, "AssetsIndex");
        assert!(fs.defs[ai].exported);
        let calls = &fs.defs[ai].calls;
        // the call starting where `text` starts
        let find = |text: &str| {
            let (s, _) = span(&src, text);
            calls.iter().find(|c| c.start_byte == s).unwrap_or_else(|| panic!("no call at {text}"))
        };
        // :343
        assert_eq!(find("<Row a={a} />"), &site(ai, jsx("Row"), 49, span(&src, "<Row a={a} />")));
        // :105, :106
        assert_eq!(find("on(query, (q) => {").callee, bare("on"));
        assert_eq!(find("q.trim()").callee, member("q", "trim"));
        assert_eq!(find("q.trim()").line, 29);
        // :300, the setter call, under the :102 pair
        let set = find("setQuery(e.currentTarget.value)");
        assert_eq!(set.line, 43);
        let b = binding(&fs, "setQuery");
        assert_eq!(b.kind, BindingKind::Destructure);
        assert!(b.encloses(set.start_byte));
        assert_eq!(fs.binding_for("setQuery", set.start_byte), Some(b));
        assert_eq!(b.decl_byte, span(&src, "setQuery]").0);
        let q = binding(&fs, "query");
        assert_eq!(q.kind, BindingKind::Destructure);
        assert!(q.encloses(find("on(query, (q) => {").start_byte));
        // Row's own JSX belongs to Row
        let row = def_index(&fs, "Row");
        assert_eq!(fs.defs[row].calls.iter().map(|c| c.callee.name()).collect::<Vec<_>>(), vec!["li", "span"]);
    }
}
