use crate::query::ast::Sigil;

pub mod c;
pub mod cpp;
pub mod go;
pub mod gotmpl;
pub mod javascript;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod typescript;

/// A single concrete tree-sitter node variant that an abstract type maps to.
#[derive(Debug, Clone)]
pub struct TsVariant {
    pub node: &'static str,
    /// The field containing the callee/name identifier (e.g. "function" for call_expression).
    /// None means the node itself IS the name (e.g. identifier).
    /// `Some("")` means the name lives in a positional child with no field name
    /// (e.g. the variable_declarator inside a JS/TS lexical_declaration); use it
    /// with `name_path` to say which child.
    pub name_field: Option<&'static str>,
    /// The TS node type of the name child (e.g. "identifier").
    pub name_node: &'static str,
    /// TS field name for parameters / arguments (for .params() field access).
    pub params_field: Option<&'static str>,
    /// The container node that wraps parameter children (e.g. "argument_list").
    pub params_container: Option<&'static str>,
    /// TS field name for the body (for .body() field access).
    pub body_field: Option<&'static str>,
    /// The container node wrapping body children (e.g. "compound_statement" for C/C++).
    pub body_container: Option<&'static str>,
    /// Chain of (outer_node, inner_field) pairs from name_field down to name_node.
    /// Empty = name_node is a direct child of name_field.
    /// Example: &[("function_declarator", "declarator")] means
    ///   name_field: (function_declarator declarator: (name_node) @cap)
    /// An empty inner_field means the next node down is a positional child with
    /// no field name, e.g. &[("expression_list", "")] for a Go `x := 1`.
    pub name_path: &'static [(&'static str, &'static str)],
    /// When true, .params() injects via a wildcard bridge node (`_ params_field: ...`)
    /// instead of directly. Needed for C/C++ function_definition where `parameters`
    /// is a field of `function_declarator`, not of `function_definition` itself.
    pub params_needs_bridge: bool,
    /// Structural constraint appended to node_body when no name filter is present.
    /// Used to narrow broad node types (e.g. field_declaration → only method prototypes).
    pub always_constraint: Option<&'static str>,
    /// When the node's name lives in a parent (e.g. arrow_function inside variable_declarator),
    /// set this to emit a parent-scoped pattern that filters by name.
    pub parent_name_wrap: Option<ParentNameWrap>,
    /// Force a callee-name predicate even when the query carries no name filter.
    /// Used to match `require(...)` for the `import` abstract type: emits
    /// `<name_field>: (<name_node>) @c (#eq? @c "<value>")`.
    pub always_callee_eq: Option<&'static str>,
}

/// Describes how to find a node's name via its parent when name_field is None.
/// Used for arrow functions whose name lives in the parent variable_declarator.
#[derive(Debug, Clone, Copy)]
pub struct ParentNameWrap {
    /// Parent node type (e.g. "variable_declarator").
    pub parent: &'static str,
    /// Field on parent holding the name (e.g. "name").
    pub name_field: &'static str,
    /// Node type of the name child (e.g. "identifier").
    pub name_node: &'static str,
    /// Field on parent holding self (e.g. "value").
    pub self_field: &'static str,
}

pub trait Lang: Send + Sync {
    /// Resolve an abstract node type + sigil to zero or more TS variants.
    /// - None  → type is unknown; pass through as a concrete TS node name
    /// - Some([]) → type is known but has no mapping in this language (silent empty)
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>>;
    fn ts_language(&self) -> tree_sitter::Language;
}

pub fn for_name(name: &str) -> Option<Box<dyn Lang>> {
    let inner: Box<dyn Lang> = match name {
        "c" => Box::new(c::CLang),
        "cpp" | "c++" => Box::new(cpp::CppLang),
        "javascript" | "js" => Box::new(javascript::JsLang),
        "typescript" | "ts" => Box::new(typescript::TsLang),
        "tsx" => Box::new(typescript::TsxLang),
        "python" | "py" => Box::new(python::PyLang),
        "ruby" | "rb" => Box::new(ruby::RubyLang),
        "rust" | "rs" => Box::new(rust::RustLang),
        "go" => Box::new(go::GoLang),
        "gotmpl" => Box::new(gotmpl::GotmplLang),
        _ => return None,
    };
    Some(Box::new(Aliased(inner)))
}

/// Wraps a language so the names agents reach for first resolve in every
/// language: `method` is `function` (a language with no separate method kind
/// has nothing else to mean) and `string` is the string part of `literal`.
struct Aliased(Box<dyn Lang>);

impl Lang for Aliased {
    fn resolve(&self, abstract_type: &str, sigil: Option<&Sigil>) -> Option<Vec<TsVariant>> {
        match abstract_type {
            "method" => self.0.resolve("function", sigil),
            // Where the grammar has a node called `string` (JavaScript, TypeScript,
            // Python, Ruby) that node is the string, so it passes through and the
            // raw `string` in a candidate query keeps meaning that node. Elsewhere
            // `string` is the string members of `literal`, never the numbers.
            "string" => {
                let strings: Vec<TsVariant> = self
                    .0
                    .resolve("literal", sigil)?
                    .into_iter()
                    .filter(|v| v.node.contains("string"))
                    .collect();
                if strings.iter().any(|v| v.node == "string") { None } else { Some(strings) }
            }
            other => self.0.resolve(other, sigil),
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        self.0.ts_language()
    }
}

pub fn lang_for_name_canonical(name: &str) -> Option<&'static str> {
    match name {
        "c" => Some("c"),
        "cpp" | "c++" => Some("cpp"),
        "javascript" | "js" => Some("javascript"),
        "typescript" | "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "python" | "py" => Some("python"),
        "ruby" | "rb" => Some("ruby"),
        "rust" | "rs" => Some("rust"),
        "go" => Some("go"),
        "gotmpl" => Some("gotmpl"),
        _ => None,
    }
}

pub fn extensions_for_name(name: &str) -> &'static [&'static str] {
    match name {
        "c" => &["c", "h"],
        "cpp" | "c++" => &["cpp", "cc", "cxx", "h", "hpp", "hxx"],
        "javascript" | "js" => &["js", "mjs", "cjs", "jsx"],
        "typescript" | "ts" => &["ts", "tsx"],
        "tsx" => &["tsx"],
        "python" | "py" => &["py"],
        "ruby" | "rb" => &["rb", "rake", "gemspec"],
        "rust" | "rs" => &["rs"],
        "go" => &["go"],
        "gotmpl" => &["tmpl", "gotmpl"],
        _ => &[],
    }
}

pub fn all_known_extensions() -> &'static [&'static str] {
    &["c", "h", "cpp", "cc", "cxx", "hpp", "hxx", "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rb", "rake", "gemspec", "rs", "go", "tmpl", "gotmpl"]
}

pub fn lang_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "py" => Some("python"),
        "rb" | "rake" | "gemspec" => Some("ruby"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "tmpl" | "gotmpl" => Some("gotmpl"),
        _ => None,
    }
}

/// The language for a path: its extension first, then, for a file with no
/// extension, the shebang line.
pub fn lang_for_path(path: &str) -> Option<&'static str> {
    match std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        Some(ext) => lang_for_extension(ext),
        None => lang_for_shebang_file(path),
    }
}

/// `-l ts` means the TypeScript family, so a `.tsx` file is parsed with the TSX
/// grammar, which the TypeScript one cannot read. Every other language is as named.
pub fn refine_for_path(canonical: &'static str, path: &str) -> &'static str {
    let tsx = std::path::Path::new(path).extension().and_then(|e| e.to_str()) == Some("tsx");
    if canonical == "typescript" && tsx { "tsx" } else { canonical }
}

/// The interpreter named on a shebang line, version suffix dropped, through
/// `env` or a direct path: `#!/usr/bin/env python3.11` gives `python`.
pub fn shebang_interpreter(first_line: &str) -> Option<&str> {
    let rest = first_line.strip_prefix("#!")?.trim();
    let mut parts = rest.split_whitespace();
    let prog = parts.next()?;
    let mut name = after_last_slash(prog);
    if name == "env" {
        let arg = parts.find(|a| !a.starts_with('-') && !a.contains('='))?;
        name = after_last_slash(arg);
    }
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if name.is_empty() { None } else { Some(name) }
}

fn after_last_slash(p: &str) -> &str {
    p.rfind('/').map(|i| &p[i + 1..]).unwrap_or(p)
}

/// The language a shebang line selects. Shell interpreters are recognised by
/// `shebang_interpreter` but have no grammar here, so they select nothing.
pub fn lang_for_shebang(first_line: &str) -> Option<&'static str> {
    match shebang_interpreter(first_line)? {
        "ruby" => Some("ruby"),
        "python" => Some("python"),
        "node" | "nodejs" => Some("javascript"),
        _ => None,
    }
}

/// True when the shebang names a shell, which has no grammar here.
pub fn shebang_is_shell(first_line: &str) -> bool {
    matches!(shebang_interpreter(first_line), Some("bash" | "sh" | "zsh" | "dash"))
}

/// First line of a file, read from its first 256 bytes. Empty when unreadable.
pub fn first_line_of(path: &str) -> String {
    use std::io::Read;
    let mut buf = [0u8; 256];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let line = buf[..n].split(|&b| b == b'\n').next().unwrap_or(&[]);
    String::from_utf8_lossy(line).trim_end_matches('\r').to_string()
}

pub fn lang_for_shebang_file(path: &str) -> Option<&'static str> {
    lang_for_shebang(&first_line_of(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source lines (trimmed) a query matches in an inline source.
    fn hits(query: &str, lang_name: &str, source: &str) -> Vec<String> {
        let ast = crate::query::parse(query).unwrap();
        crate::compiler::validate(&ast, for_name(lang_name).unwrap().as_ref(), lang_name)
            .unwrap_or_else(|e| panic!("{} rejected for {}: {}", query, lang_name, e));
        let l = for_name(lang_name).unwrap();
        let compiled = crate::compiler::compile(&ast, l.as_ref());
        crate::runner::run_rows(&compiled, l.as_ref(), source.as_bytes(), false)
            .unwrap()
            .into_iter()
            .filter_map(|r| r.src_line)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    // --- decision 08: function includes arrow functions and function expressions ---

    #[test]
    fn function_includes_arrow_functions_bound_to_a_const_in_js_ts_and_tsx() {
        let src = "const Topbar = () => {\n  return 1;\n};\nlet inner = function () { return 2; };\nvar old = function named() {};\n";
        for l in ["javascript", "typescript", "tsx"] {
            assert_eq!(hits("*function#Topbar", l, src).len(), 1, "{}", l);
        }
        let tsx = "const Topbar = () => {\n  return <div className=\"x\" />;\n};\n";
        assert_eq!(hits("*function#Topbar", "tsx", tsx), vec!["const Topbar = () => {"]);
        // The name lives in the binding, so a function expression is found by the same name.
        assert_eq!(hits("*function#inner", "javascript", src).len(), 1);
    }

    // --- decision 08: method is accepted in every language ---

    #[test]
    fn method_is_an_alias_of_function_in_every_language() {
        let cases: &[(&str, &str, &str)] = &[
            ("go", "package p\nfunc (t T) Run() {}\nfunc Free() {}\n", "Run"),
            ("ruby", "class A\n  def run\n  end\nend\n", "run"),
            ("python", "class A:\n    def run(self):\n        pass\n", "run"),
            ("javascript", "class A {\n  run() {}\n}\n", "run"),
            ("typescript", "class A {\n  run(): void {}\n}\n", "run"),
            ("rust", "impl A {\n    fn run(&self) {}\n}\n", "run"),
            ("cpp", "int A::run() { return 1; }\n", "run"),
            ("c", "int run(void) { return 1; }\n", "run"),
        ];
        for (l, src, name) in cases {
            let by_method = hits(&format!("*method#{}", name), l, src);
            let by_function = hits(&format!("*function#{}", name), l, src);
            assert_eq!(by_method.len(), 1, "{}: {:?}", l, by_method);
            assert_eq!(by_method, by_function, "{}", l);
        }
    }

    // --- decision 08: string is an alias of literal ---

    #[test]
    fn string_is_an_alias_of_literal() {
        let cases: &[(&str, &str)] = &[
            ("go", "package p\nvar a = \"hello\"\n"),
            ("rust", "fn f() { let a = \"hello\"; }\n"),
            ("python", "a = \"hello\"\n"),
            ("ruby", "a = \"hello\"\n"),
            ("javascript", "const a = \"hello\";\n"),
            ("typescript", "const a = \"hello\";\n"),
            ("c", "char *a = \"hello\";\n"),
            ("cpp", "const char *a = \"hello\";\n"),
        ];
        for (l, src) in cases {
            let s = hits("string#/hello/", l, src);
            assert_eq!(s.len(), 1, "{}: {:?}", l, s);
            assert_eq!(s, hits("literal#/hello/", l, src), "{}", l);
        }
    }

    // --- decision 08: id includes property, field and JSX attribute names ---

    #[test]
    fn id_includes_property_identifiers_and_jsx_attribute_names() {
        let tsx = "const a = <div className=\"x\" />;\n";
        assert_eq!(hits("id#className", "tsx", tsx), vec![tsx.trim()]);
        assert_eq!(hits("id#className", "javascript", tsx), vec![tsx.trim()]);
        let obj = "const o = { retries: 3 };\no.retries;\n";
        assert_eq!(hits("id#retries", "typescript", obj).len(), 2);
        assert_eq!(hits("id#retries", "javascript", obj).len(), 2);
    }

    #[test]
    fn id_includes_field_identifiers_in_rust_go_c_and_cpp() {
        let rs = "struct S { f: i32 }\nfn g(s: S) -> i32 { s.f }\n";
        assert_eq!(hits("id#f", "rust", rs).len(), 2);
        let go = "package p\ntype S struct{ F int }\nfunc g(s S) int { return s.F }\n";
        assert_eq!(hits("id#F", "go", go).len(), 2);
        let c = "struct S { int f; };\nint g(struct S *s) { return s->f; }\n";
        assert_eq!(hits("id#f", "c", c).len(), 2);
        assert_eq!(hits("id#f", "cpp", c).len(), 2);
    }

    // --- decision 08: call matches the last segment of a member call ---

    #[test]
    fn call_matches_plain_and_member_calls_by_their_last_segment() {
        let js = "it(\"works\", () => { expect(1).toBe(1); });\nobj.deep.run(2);\n";
        for l in ["javascript", "typescript", "tsx"] {
            assert_eq!(hits("call#it", l, js), vec![js.lines().next().unwrap()], "{}", l);
            assert_eq!(hits("call#toBe", l, js).len(), 1, "{}", l);
            assert_eq!(hits("call#run", l, js), vec!["obj.deep.run(2);"], "{}", l);
            assert_eq!(hits("call", l, js).len(), 2, "{}", l);
        }
        let go = "package p\nfunc f() { fmt.Println(1); Plain() }\n";
        assert_eq!(hits("call#Println", "go", go).len(), 1);
        assert_eq!(hits("call#Plain", "go", go).len(), 1);
        let rs = "fn f() { a::b::go(); x.run(); helper(); }\n";
        for n in ["go", "run", "helper"] {
            assert_eq!(hits(&format!("call#{}", n), "rust", rs).len(), 1, "{}", n);
        }
        let py = "os.path.join(a)\nfoo(1)\n";
        assert_eq!(hits("call#join", "python", py), vec!["os.path.join(a)"]);
        assert_eq!(hits("call#foo", "python", py), vec!["foo(1)"]);
        let rb = "x.foo(2)\nbar(3)\n";
        assert_eq!(hits("call#foo", "ruby", rb), vec!["x.foo(2)"]);
        assert_eq!(hits("call#bar", "ruby", rb), vec!["bar(3)"]);
        let c = "int main(void) { s->cb(1); f(2); }\n";
        assert_eq!(hits("call#cb", "c", c).len(), 1);
        assert_eq!(hits("call#f", "c", c).len(), 1);
        let cpp = "int main() { s->cb(1); o.m(); f(2); ns::g(3); }\n";
        for n in ["cb", "m", "f", "g"] {
            assert_eq!(hits(&format!("call#{}", n), "cpp", cpp).len(), 1, "{}", n);
        }
    }

    #[test]
    fn call_does_not_match_a_definition_or_a_lookalike_name() {
        let go = "package p\nfunc Run() {}\nfunc f() { RunAll() }\n";
        assert!(hits("call#Run", "go", go).is_empty());
    }

    // --- decision 08: -l ts includes .tsx, -l js includes .jsx ---
    // (the walk is covered in files.rs, the grammar choice here)

    #[test]
    fn the_tsx_grammar_reads_jsx_the_ts_grammar_cannot() {
        let src = "const B = () => <div id=\"a\" />;\n";
        assert_eq!(hits("*function#B", "tsx", src).len(), 1);
        assert_eq!(hits("*function#B", "javascript", src).len(), 1);
    }

    // --- decision 08: comment is a kind in every language ---

    #[test]
    fn comment_is_a_kind_in_every_language() {
        let cases: &[(&str, &str)] = &[
            ("c", "/* block */\nint x; // tail\n"),
            ("cpp", "/* block */\nint x; // tail\n"),
            ("go", "package p\n// line\nvar x int\n"),
            ("javascript", "// line\nconst x = 1;\n"),
            ("typescript", "// line\nconst x = 1;\n"),
            ("tsx", "// line\nconst x = 1;\n"),
            ("python", "# line\nx = 1\n"),
            ("ruby", "# line\nx = 1\n"),
            ("rust", "// line\nfn f() {}\n/* block */\n"),
        ];
        for (l, src) in cases {
            assert!(!hits("comment", l, src).is_empty(), "{}", l);
        }
        // Rust has two comment node kinds, and both are reached.
        assert_eq!(hits("comment", "rust", "// line\nfn f() {}\n/* block */\n").len(), 2);
        assert_eq!(hits("comment#/TODO/", "go", "package p\n// TODO fix\n// ok\n").len(), 1);
    }

    // --- decision 08: C++ definitions behind export macros are functions ---

    #[test]
    fn cpp_definitions_behind_export_macros_parse_as_functions() {
        let src = "extern \"C\" __declspec(dllexport) int Foo(int a) { return a; }\n#define API __declspec(dllexport)\nAPI int Bar(int b) { return b; }\n";
        assert_eq!(hits("*function#Foo", "cpp", src).len(), 1);
        assert_eq!(hits("*function#Bar", "cpp", src).len(), 1);
    }

    #[test]
    fn the_new_kinds_are_listed_as_valid() {
        let l = for_name("go").unwrap();
        let kinds = crate::compiler::known_kinds(l.as_ref());
        for k in ["method", "string", "comment", "call"] {
            assert!(kinds.contains(&k), "{} missing from {:?}", k, kinds);
        }
    }

    #[test]
    fn a_direct_path_shebang_picks_the_language() {
        assert_eq!(lang_for_shebang("#!/usr/bin/ruby"), Some("ruby"));
        assert_eq!(lang_for_shebang("#!/usr/bin/python"), Some("python"));
        assert_eq!(lang_for_shebang("#!/usr/local/bin/node"), Some("javascript"));
    }

    #[test]
    fn env_shebangs_skip_flags_and_assignments() {
        assert_eq!(lang_for_shebang("#!/usr/bin/env ruby"), Some("ruby"));
        assert_eq!(lang_for_shebang("#!/usr/bin/env python3"), Some("python"));
        assert_eq!(lang_for_shebang("#!/usr/bin/env -S node --harmony"), Some("javascript"));
        assert_eq!(lang_for_shebang("#!/usr/bin/env FOO=1 ruby -w"), Some("ruby"));
        assert_eq!(lang_for_shebang("#! /usr/bin/python3.11 -u"), Some("python"));
    }

    #[test]
    fn shell_shebangs_are_recognised_but_have_no_grammar() {
        for line in ["#!/bin/bash", "#!/bin/sh", "#!/usr/bin/env bash", "#!/usr/bin/env sh"] {
            assert!(shebang_is_shell(line), "{}", line);
            assert_eq!(lang_for_shebang(line), None, "{}", line);
        }
    }

    #[test]
    fn a_line_that_is_not_a_shebang_selects_nothing() {
        assert_eq!(lang_for_shebang("# ruby"), None);
        assert_eq!(lang_for_shebang("#!"), None);
        assert_eq!(lang_for_shebang("#!/usr/bin/env"), None);
        assert_eq!(lang_for_shebang(""), None);
    }

    #[test]
    fn ts_includes_tsx_and_picks_the_grammar_per_file() {
        assert!(extensions_for_name("ts").contains(&"tsx"));
        assert!(extensions_for_name("typescript").contains(&"tsx"));
        assert_eq!(refine_for_path("typescript", "a/b.tsx"), "tsx");
        assert_eq!(refine_for_path("typescript", "a/b.ts"), "typescript");
        assert_eq!(refine_for_path("javascript", "a/b.jsx"), "javascript");
        assert!(extensions_for_name("js").contains(&"jsx"));
    }
}
