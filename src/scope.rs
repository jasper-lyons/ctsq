//! The nearest named scope around a byte offset, shared by `--scope` and the
//! `scopeof` command.
//!
//! A named scope is a declaration that has a name and holds other code: a
//! function or method, a class, a type, a Rust `impl` or `trait`, a Ruby
//! `module`. The node kinds below mirror what the `function`, `class` and `type`
//! abstract types resolve to in each language (see `ctsq types LANG`), narrowed
//! to the ones that have a name. An arrow function or function expression is
//! named only when it is bound to a const, let or var (or a class field). A
//! closure, lambda or any other anonymous function is skipped, so the walk
//! carries on to the named parent.

use crate::compiler::lang;
use crate::tree::signature;
use std::collections::HashMap;
use tree_sitter::{Node, Parser, Tree};

/// One scope: where it is, what it is called and its first line.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub path: String,
    /// 1-based, inclusive. The span covers the whole declaration, including an
    /// `export` or decorator wrapper and the `const X =` of a bound arrow function.
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    /// `function`, `class`, `type`, `impl`, `trait`, `module`, or `block`.
    pub kind: &'static str,
    pub name: String,
    /// The declaration's first line, trimmed and cut (`tree::signature`).
    pub signature: String,
    /// Byte just past the name. A hit before this byte, inside the scope, is on
    /// the scope's own declaration and not held by it.
    pub name_end_byte: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Fam {
    Go,
    Rust,
    Ecma,
    Py,
    Ruby,
    C,
    Cpp,
}

fn family(lang_name: &str) -> Option<Fam> {
    let canon = lang::lang_for_name_canonical(lang_name).unwrap_or(lang_name);
    Some(match canon {
        "go" => Fam::Go,
        "rust" => Fam::Rust,
        "javascript" | "typescript" | "tsx" => Fam::Ecma,
        "python" => Fam::Py,
        "ruby" => Fam::Ruby,
        "c" => Fam::C,
        "cpp" => Fam::Cpp,
        _ => return None,
    })
}

/// Parse `source` with the grammar for `lang_name`.
pub fn parse(lang_name: &str, source: &[u8]) -> Option<Tree> {
    let l = lang::for_name(lang_name)?;
    let mut parser = Parser::new();
    parser.set_language(&l.ts_language()).ok()?;
    parser.parse(source, None)
}

struct Found<'t> {
    core: Node<'t>,
    outer: Node<'t>,
    kind: &'static str,
    name: String,
    name_end: usize,
}

fn text(n: Node, source: &[u8]) -> String {
    n.utf8_text(source).unwrap_or("").to_string()
}

fn c_name<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut n = node.child_by_field_name("declarator")?;
    loop {
        match n.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "operator_name" | "destructor_name" => {
                return Some(n)
            }
            _ => n = n.child_by_field_name("declarator").or_else(|| n.child_by_field_name("name"))?,
        }
    }
}

/// The name node and kind label of `node` when it is a named scope.
fn classify<'t>(node: Node<'t>, fam: Fam) -> Option<(&'static str, Node<'t>)> {
    let by_name = |kind: &'static str| node.child_by_field_name("name").map(|n| (kind, n));
    match fam {
        Fam::Go => match node.kind() {
            "function_declaration" | "method_declaration" => by_name("function"),
            "type_spec" | "type_alias" => by_name("type"),
            _ => None,
        },
        Fam::Rust => match node.kind() {
            "function_item" => by_name("function"),
            "struct_item" | "enum_item" | "union_item" | "type_item" => by_name("type"),
            "trait_item" => by_name("trait"),
            "impl_item" => node.child_by_field_name("type").map(|n| ("impl", n)),
            _ => None,
        },
        Fam::Ecma => match node.kind() {
            "function_declaration" | "generator_function_declaration" | "function_signature" | "method_definition" => {
                by_name("function")
            }
            "class_declaration" | "abstract_class_declaration" => by_name("class"),
            "interface_declaration" | "type_alias_declaration" | "enum_declaration" => by_name("type"),
            "arrow_function" | "function_expression" | "function" | "generator_function" => {
                let parent = node.parent()?;
                let value = parent.child_by_field_name("value")?;
                if value.id() != node.id() {
                    return None;
                }
                match parent.kind() {
                    "variable_declarator" => {
                        let n = parent.child_by_field_name("name")?;
                        (n.kind() == "identifier").then_some(("function", n))
                    }
                    "public_field_definition" | "field_definition" => parent
                        .child_by_field_name("name")
                        .or_else(|| parent.child_by_field_name("property"))
                        .map(|n| ("function", n)),
                    _ => None,
                }
            }
            _ => None,
        },
        Fam::Py => match node.kind() {
            "function_definition" => by_name("function"),
            "class_definition" => by_name("class"),
            _ => None,
        },
        Fam::Ruby => match node.kind() {
            "method" | "singleton_method" => by_name("function"),
            "class" => by_name("class"),
            "module" => by_name("module"),
            _ => None,
        },
        Fam::C | Fam::Cpp => match node.kind() {
            "function_definition" => c_name(node).map(|n| ("function", n)),
            "struct_specifier" | "class_specifier" | "union_specifier" => {
                node.child_by_field_name("body")?;
                by_name("class")
            }
            _ => None,
        },
    }
}

fn found<'t>(node: Node<'t>, fam: Fam, source: &[u8]) -> Option<Found<'t>> {
    let (kind, name_node) = classify(node, fam)?;
    // The node whose span and first line describe the declaration.
    let mut outer = node;
    match fam {
        Fam::Ecma => {
            if let Some(p) = node.parent() {
                if p.kind() == "variable_declarator" {
                    if let Some(decl) = p.parent() {
                        outer = decl;
                    }
                } else if matches!(p.kind(), "public_field_definition" | "field_definition") {
                    outer = p;
                }
            }
            if let Some(p) = outer.parent() {
                if p.kind() == "export_statement" {
                    outer = p;
                }
            }
        }
        Fam::Py => {
            if let Some(p) = node.parent() {
                if p.kind() == "decorated_definition" {
                    outer = p;
                }
            }
        }
        Fam::Go => {
            if let Some(p) = node.parent() {
                if p.kind() == "type_declaration" && p.named_child_count() == 1 {
                    outer = p;
                }
            }
        }
        _ => {}
    }
    Some(Found { core: node, outer, kind, name: text(name_node, source), name_end: name_node.end_byte() })
}

fn to_scope(path: &str, f: &Found, source: &[u8]) -> Scope {
    Scope {
        path: path.to_string(),
        start_line: f.outer.start_position().row + 1,
        end_line: f.outer.end_position().row + 1,
        start_byte: f.outer.start_byte(),
        end_byte: f.outer.end_byte(),
        kind: f.kind,
        name: f.name.clone(),
        signature: signature(f.outer, source),
        name_end_byte: f.name_end,
    }
}

fn node_at<'t>(tree: &'t Tree, source: &[u8], byte: usize) -> Option<Node<'t>> {
    let b = byte.min(source.len().saturating_sub(1));
    tree.root_node().descendant_for_byte_range(b, b)
}

/// Every named scope that holds `byte`, innermost first. A scope holds a byte
/// that sits anywhere in the declaration itself, name and header included.
pub fn enclosing(path: &str, lang_name: &str, source: &[u8], tree: &Tree, byte: usize) -> Vec<Scope> {
    let Some(fam) = family(lang_name) else { return vec![] };
    let mut out = Vec::new();
    let mut cur = node_at(tree, source, byte);
    while let Some(n) = cur {
        if let Some(f) = found(n, fam, source) {
            out.push(to_scope(path, &f, source));
        }
        cur = n.parent();
    }
    out
}

/// The innermost named scope that holds `byte`, or None at the top level.
pub fn nearest(path: &str, lang_name: &str, source: &[u8], tree: &Tree, byte: usize) -> Option<Scope> {
    enclosing(path, lang_name, source, tree, byte).into_iter().next()
}

/// The scope that holds a search hit at `byte`. A hit on a scope's own
/// declaration (its keyword, its name) is not held by that scope, so the walk
/// moves on to the one around it. This is what `--scope` prints.
pub fn for_hit(path: &str, lang_name: &str, source: &[u8], tree: &Tree, byte: usize) -> Option<Scope> {
    enclosing(path, lang_name, source, tree, byte)
        .into_iter()
        .find(|s| byte >= s.name_end_byte)
}

/// Node kinds that are a block of statements.
fn is_block(kind: &str) -> bool {
    matches!(
        kind,
        "block" | "statement_block" | "compound_statement" | "do_block" | "body_statement" | "declaration_list"
    )
}

/// The innermost block of statements around `byte`, as a scope of kind `block`.
pub fn innermost_block(path: &str, source: &[u8], tree: &Tree, byte: usize) -> Option<Scope> {
    let mut cur = node_at(tree, source, byte);
    while let Some(n) = cur {
        if is_block(n.kind()) {
            return Some(Scope {
                path: path.to_string(),
                start_line: n.start_position().row + 1,
                end_line: n.end_position().row + 1,
                start_byte: n.start_byte(),
                end_byte: n.end_byte(),
                kind: "block",
                name: String::new(),
                signature: signature(n, source),
                name_end_byte: n.start_byte(),
            });
        }
        cur = n.parent();
    }
    None
}

/// The other named scopes that share `scope`'s parent (or the file, at the top
/// level), in source order, `scope` itself left out.
pub fn siblings(path: &str, lang_name: &str, source: &[u8], tree: &Tree, scope: &Scope) -> Vec<Scope> {
    let Some(fam) = family(lang_name) else { return vec![] };
    // The container is the scope just outside this one, or the whole file.
    let parent = enclosing(path, lang_name, source, tree, scope.start_byte)
        .into_iter()
        .find(|s| s.start_byte < scope.start_byte || s.end_byte > scope.end_byte);
    let root = tree.root_node();
    let container = match &parent {
        Some(p) => root
            .descendant_for_byte_range(p.start_byte, p.end_byte)
            .and_then(|mut n| loop {
                if found(n, fam, source).is_some_and(|f| f.outer.start_byte() == p.start_byte) {
                    break Some(n);
                }
                n = n.parent()?;
            })
            .unwrap_or(root),
        None => root,
    };
    fn walk(n: Node, fam: Fam, source: &[u8], path: &str, skip: usize, top: bool, out: &mut Vec<Scope>) {
        if !top {
            if let Some(f) = found(n, fam, source) {
                if f.outer.start_byte() != skip {
                    out.push(to_scope(path, &f, source));
                }
                return;
            }
        }
        let mut c = n.walk();
        for child in n.children(&mut c) {
            walk(child, fam, source, path, skip, false, out);
        }
    }
    let mut out = Vec::new();
    walk(container, fam, source, path, scope.start_byte, true, &mut out);
    out.sort_by_key(|s| s.start_byte);
    out
}

// ── Byte offsets from lines ──────────────────────────────────────────────────

/// Byte offset of the first non-blank character on 1-based `line`.
pub fn line_start_byte(source: &[u8], line: usize) -> usize {
    let mut start = 0usize;
    for _ in 1..line {
        match source[start..].iter().position(|&b| b == b'\n') {
            Some(i) => start += i + 1,
            None => return source.len(),
        }
    }
    let mut b = start;
    while b < source.len() && (source[b] == b' ' || source[b] == b'\t') {
        b += 1;
    }
    b
}

// ── What --scope prints ──────────────────────────────────────────────────────

/// Parses each file once and answers scope questions about it.
#[derive(Default)]
pub struct Scopes {
    trees: HashMap<String, Option<Tree>>,
}

impl Scopes {
    pub fn new() -> Scopes {
        Scopes::default()
    }

    fn tree(&mut self, key: &str, lang_name: &str, source: &[u8]) -> Option<&Tree> {
        self.trees
            .entry(key.to_string())
            .or_insert_with(|| parse(lang_name, source))
            .as_ref()
    }

    /// The scope holding a hit at `byte` (see `for_hit`), labelled with `path`.
    pub fn hit(&mut self, path: &str, lang_name: &str, source: &[u8], byte: usize) -> Option<Scope> {
        let tree = self.tree(path, lang_name, source)?;
        for_hit(path, lang_name, source, tree, byte)
    }

    /// The scope holding the call written on `line`: plain containment, since a
    /// call line is never a declaration the walk should step over.
    pub fn call_line(&mut self, path: &str, lang_name: &str, source: &[u8], line: usize) -> Option<Scope> {
        let tree = self.tree(path, lang_name, source)?;
        nearest(path, lang_name, source, tree, line_start_byte(source, line))
    }

    /// The scope holding a definition on `line`, stepping over a declaration
    /// that starts there.
    pub fn def_line(&mut self, path: &str, lang_name: &str, source: &[u8], line: usize) -> Option<Scope> {
        self.hit(path, lang_name, source, line_start_byte(source, line))
    }
}

/// The `--scope` row: two spaces, `in `, `path:START-END`, two spaces, the
/// signature.
pub fn row(s: &Scope) -> String {
    format!("  in {}:{}-{}  {}", s.path, s.start_line, s.end_line, s.signature)
}

/// The row, then with `body` the scope's source lines each indented four
/// spaces. Every line of the result ends in a newline and starts with `indent`
/// (a call tree passes its branch prefix). `source` is the scope's file.
pub fn render(s: &Scope, source: &[u8], body: bool, indent: &str) -> String {
    let mut out = format!("{}{}\n", indent, row(s));
    if body {
        let text = String::from_utf8_lossy(source);
        for line in text.lines().skip(s.start_line - 1).take(s.end_line - s.start_line + 1) {
            if line.trim().is_empty() {
                out.push('\n');
            } else {
                out.push_str(&format!("{}    {}\n", indent, line));
            }
        }
    }
    out
}

/// What `--scope` was asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// `--scope --body`: print the scope's body under its row.
    pub body: bool,
}

/// Scope rows for commands that know a file and a line or byte but hold no
/// source: it reads and parses each file once, on first use.
pub struct Printer {
    opts: Opts,
    scopes: Scopes,
    langs: HashMap<String, &'static str>,
    sources: HashMap<String, Option<Vec<u8>>>,
}

impl Printer {
    pub fn new(opts: Opts, jobs: &[(String, &'static str)]) -> Printer {
        Printer {
            opts,
            scopes: Scopes::new(),
            langs: jobs.iter().map(|(f, l)| (f.clone(), *l)).collect(),
            sources: HashMap::new(),
        }
    }

    fn load(&mut self, file: &str) -> Option<(&'static str, &mut Scopes, &[u8])> {
        let lang = *self.langs.get(file)?;
        let src = self.sources.entry(file.to_string()).or_insert_with(|| std::fs::read(file).ok());
        Some((lang, &mut self.scopes, src.as_deref()?))
    }

    /// The rows for the scope holding the call written on `line` of `file`.
    /// Empty at the top level. Each line starts with `indent`.
    pub fn call_rows(&mut self, file: &str, line: usize, indent: &str) -> String {
        let body = self.opts.body;
        match self.load(file) {
            Some((lang, scopes, src)) => match scopes.call_line(file, lang, src, line) {
                Some(s) => render(&s, src, body, indent),
                None => String::new(),
            },
            None => String::new(),
        }
    }

    /// The rows for the scope holding the definition on `line` of `file`.
    pub fn def_rows(&mut self, file: &str, line: usize, indent: &str) -> String {
        let body = self.opts.body;
        match self.load(file) {
            Some((lang, scopes, src)) => match scopes.def_line(file, lang, src, line) {
                Some(s) => render(&s, src, body, indent),
                None => String::new(),
            },
            None => String::new(),
        }
    }

    /// The scope holding a hit at `byte` of `file`.
    pub fn hit(&mut self, file: &str, byte: usize) -> Option<Scope> {
        let (lang, scopes, src) = self.load(file)?;
        scopes.hit(file, lang, src, byte)
    }

    /// The rows for `scope`, which sits in `file`.
    pub fn rows(&mut self, file: &str, scope: &Scope, indent: &str) -> String {
        let body = self.opts.body;
        match self.load(file) {
            Some((_, _, src)) => render(scope, src, body, indent),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (scope names innermost first) for the byte at `needle`'s first occurrence.
    fn names(lang_name: &str, src: &str, needle: &str) -> Vec<String> {
        let tree = parse(lang_name, src.as_bytes()).unwrap();
        let b = src.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        enclosing("f", lang_name, src.as_bytes(), &tree, b).into_iter().map(|s| s.name).collect()
    }

    fn near(lang_name: &str, src: &str, needle: &str) -> Option<Scope> {
        let tree = parse(lang_name, src.as_bytes()).unwrap();
        nearest("f", lang_name, src.as_bytes(), &tree, src.find(needle).unwrap())
    }

    const GO: &str = "package p\n\nvar top = 1\n\ntype Server struct {\n\tHost string\n}\n\nfunc (s *Server) Start() {\n\trun := func() {\n\t\tinner()\n\t}\n\trun()\n}\n";

    #[test]
    fn go_closure_resolves_to_its_named_method() {
        let s = near("go", GO, "inner()").unwrap();
        assert_eq!((s.name.as_str(), s.kind, s.start_line, s.end_line), ("Start", "function", 9, 14));
        assert_eq!(s.signature, "func (s *Server) Start() {");
        assert_eq!(names("go", GO, "Host"), vec!["Server"]);
    }

    #[test]
    fn go_top_level_has_no_scope() {
        assert_eq!(near("go", GO, "top = 1"), None);
        assert_eq!(near("go", GO, "package"), None);
    }

    #[test]
    fn go_type_span_includes_the_type_keyword() {
        let s = near("go", GO, "Host").unwrap();
        assert_eq!((s.start_line, s.end_line), (5, 7));
        assert_eq!(s.signature, "type Server struct {");
    }

    const RS: &str = "struct S;\nimpl S {\n    fn m(&self) {\n        let c = |x| helper(x);\n    }\n}\ntrait T {\n    fn t(&self) { body_in_trait(); }\n}\nfn top() {}\n";

    #[test]
    fn rust_closure_resolves_to_method_and_impl_follows() {
        assert_eq!(names("rust", RS, "helper"), vec!["m", "S"]);
        let s = near("rust", RS, "helper").unwrap();
        assert_eq!(s.kind, "function");
        let tree = parse("rust", RS.as_bytes()).unwrap();
        let all = enclosing("f", "rust", RS.as_bytes(), &tree, RS.find("helper").unwrap());
        assert_eq!(all[1].kind, "impl");
        assert_eq!(all[1].signature, "impl S {");
    }

    #[test]
    fn rust_trait_is_a_scope_and_top_level_is_not() {
        assert_eq!(names("rust", RS, "body_in_trait"), vec!["t", "T"]);
        assert_eq!(near("rust", RS, "struct S").map(|s| s.kind), Some("type"));
        assert_eq!(near("rust", RS, "fn top").map(|s| s.name), Some("top".to_string()));
        assert_eq!(near("rust", "const A: u8 = 1;\n", "A"), None);
        assert_eq!(near("rust", RS, "impl S").map(|s| s.name), Some("S".to_string()));
        assert_eq!(near("rust", RS, "trait T").map(|s| s.name), Some("T".to_string()));
    }

    const TSX: &str = "import x from 'y'\n\nexport const AssetEditor = (props: Props) => {\n  const saveAssetNow = async () => {\n    persist(asset())\n  }\n  const items = list.map((i) => <li>{i}</li>)\n  return <div onClick={() => saveAssetNow()} />\n}\n\nconst plain = 1\n\nfunction Brand() {\n  return null\n}\n";

    #[test]
    fn tsx_const_arrow_component_is_a_scope_with_export_in_its_signature() {
        let s = near("tsx", TSX, "const saveAssetNow").unwrap();
        assert_eq!(s.name, "AssetEditor");
        assert_eq!(s.signature, "export const AssetEditor = (props: Props) => {");
        assert_eq!((s.start_line, s.end_line), (3, 9));
    }

    #[test]
    fn tsx_nested_bound_arrow_is_a_scope_and_inline_callbacks_are_not() {
        assert_eq!(names("tsx", TSX, "persist"), vec!["saveAssetNow", "AssetEditor"]);
        assert_eq!(names("tsx", TSX, "<li>"), vec!["AssetEditor"]);
        assert_eq!(names("tsx", TSX, "saveAssetNow()"), vec!["AssetEditor"]);
        assert_eq!(near("tsx", TSX, "plain = 1"), None);
        assert_eq!(names("tsx", TSX, "return null"), vec!["Brand"]);
    }

    #[test]
    fn a_hit_on_a_scopes_own_name_is_held_by_the_scope_around_it() {
        let src = TSX;
        let tree = parse("tsx", src.as_bytes()).unwrap();
        let on_name = src.find("saveAssetNow = ").unwrap();
        // a bound arrow's name sits outside the arrow node, so the component holds it
        assert_eq!(for_hit("f", "tsx", src.as_bytes(), &tree, on_name).unwrap().name, "AssetEditor");
        let on_fn = src.find("Brand").unwrap();
        assert_eq!(nearest("f", "tsx", src.as_bytes(), &tree, on_fn).unwrap().name, "Brand");
        assert_eq!(for_hit("f", "tsx", src.as_bytes(), &tree, on_fn), None);
        // the keyword start of a declaration is not held by it either
        let on_kw = src.find("function Brand").unwrap();
        assert_eq!(for_hit("f", "tsx", src.as_bytes(), &tree, on_kw), None);
    }

    #[test]
    fn tsx_class_method_has_its_class_next() {
        let src = "class Box {\n  open() {\n    go()\n  }\n}\ninterface Shape { area: number }\n";
        assert_eq!(names("typescript", src, "go()"), vec!["open", "Box"]);
        assert_eq!(names("typescript", src, "area"), vec!["Shape"]);
    }

    const PY: &str = "x = 1\n\nclass A:\n    def m(self):\n        f = lambda: target()\n        def inner():\n            deep()\n        return f\n";

    #[test]
    fn python_lambda_resolves_to_its_def_and_nested_def_is_a_scope() {
        assert_eq!(names("python", PY, "target"), vec!["m", "A"]);
        assert_eq!(names("python", PY, "deep"), vec!["inner", "m", "A"]);
        assert_eq!(near("python", PY, "x = 1"), None);
    }

    const RB: &str = "module Shop\n  class Cart\n    def add(item)\n      items.each { |i| tally(i) }\n    end\n    def self.build\n      new\n    end\n  end\nend\n";

    #[test]
    fn ruby_block_resolves_to_method_then_class_then_module() {
        assert_eq!(names("ruby", RB, "tally"), vec!["add", "Cart", "Shop"]);
        assert_eq!(names("ruby", RB, "new\n"), vec!["build", "Cart", "Shop"]);
        let s = near("ruby", RB, "class Cart").unwrap();
        assert_eq!(s.name, "Cart");
    }

    #[test]
    fn c_function_name_is_dug_out_of_the_declarator() {
        let src = "int add(int a, int b) {\n  return a + b;\n}\nint v = 3;\n";
        assert_eq!(names("c", src, "return"), vec!["add"]);
        assert_eq!(near("c", src, "v = 3"), None);
    }

    #[test]
    fn the_innermost_block_is_the_nearest_braces() {
        let tree = parse("go", GO.as_bytes()).unwrap();
        let b = innermost_block("f", GO.as_bytes(), &tree, GO.find("inner()").unwrap()).unwrap();
        assert_eq!((b.kind, b.start_line, b.end_line), ("block", 10, 12));
        assert!(innermost_block("f", GO.as_bytes(), &tree, GO.find("top").unwrap()).is_none());
    }

    #[test]
    fn siblings_are_the_other_scopes_in_the_same_parent() {
        let src = "class A:\n    def one(self): pass\n    def two(self): pass\n    class In:\n        def deep(self): pass\n    def three(self): pass\n\ndef top(): pass\n";
        let tree = parse("python", src.as_bytes()).unwrap();
        let one = nearest("f", "python", src.as_bytes(), &tree, src.find("one").unwrap()).unwrap();
        let sibs: Vec<String> = siblings("f", "python", src.as_bytes(), &tree, &one).into_iter().map(|s| s.name).collect();
        assert_eq!(sibs, vec!["two", "In", "three"]);
        let a = nearest("f", "python", src.as_bytes(), &tree, 0).unwrap();
        let top: Vec<String> = siblings("f", "python", src.as_bytes(), &tree, &a).into_iter().map(|s| s.name).collect();
        assert_eq!(top, vec!["top"]);
    }

    #[test]
    fn the_row_and_body_have_the_output_format_shape() {
        let tree = parse("go", GO.as_bytes()).unwrap();
        let s = nearest("a.go", "go", GO.as_bytes(), &tree, GO.find("inner()").unwrap()).unwrap();
        assert_eq!(row(&s), "  in a.go:9-14  func (s *Server) Start() {");
        let r = render(&s, GO.as_bytes(), true, "");
        assert!(r.starts_with("  in a.go:9-14  func (s *Server) Start() {\n    func (s *Server) Start() {\n"), "{}", r);
        assert!(r.contains("\n    \trun := func() {\n"), "{}", r);
        assert_eq!(r.lines().count(), 1 + 6, "{}", r);
        assert_eq!(render(&s, GO.as_bytes(), false, "│   ").lines().count(), 1);
    }

    #[test]
    fn line_start_byte_skips_indentation() {
        let src = b"a\n   b\n";
        assert_eq!(line_start_byte(src, 1), 0);
        assert_eq!(line_start_byte(src, 2), 5);
    }
}
