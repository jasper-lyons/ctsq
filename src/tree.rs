use crate::compiler::lang;
use tree_sitter::{Node, Parser};

/// One file's outline and what it left out.
pub struct Outline {
    /// Header line and entries, or empty when the whole file is hidden.
    pub text: String,
    /// Entries printed.
    pub entries: usize,
    /// Entries left out: every entry of a hidden file, or a test scope with
    /// everything nested in it.
    pub hidden: crate::footer::Hidden,
}

/// The whole outline as text: one header line, then the entries. Returned
/// rather than printed so tests can assert on it without capturing stdout.
/// `hider` decides which files and test scopes are left out, and the caller
/// prints the summed footer once (BUILD.md A11).
pub fn report(path: &str, lang_name: Option<&str>, hider: &mut crate::files::Hider) -> Outline {
    let source = match std::fs::read(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("Failed to read {}: {}", path, e); std::process::exit(1); }
    };

    let lang_name = lang_name.or_else(|| {
        std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(lang::lang_for_extension)
    });
    let lang_name = match lang_name {
        Some(n) => n,
        None => { eprintln!("Cannot detect language. Use --lang"); std::process::exit(1); }
    };
    let lang_box = match lang::for_name(lang_name) {
        Some(l) => l,
        None => { eprintln!("Unknown language: {}", lang_name); std::process::exit(1); }
    };

    let mut parser = Parser::new();
    parser.set_language(&lang_box.ts_language()).unwrap();
    let tree = parser.parse(&source, None).expect("parse failed");

    let root = tree.root_node();
    let mut items = collect_items(root, &source, lang_name);
    let mut hidden = crate::footer::Hidden::default();

    if let Some(why) = hider.file(path) {
        hidden.count(why, count_items(&items));
        return Outline { text: String::new(), entries: 0, hidden };
    }
    let scopes = hider.scope_lines_in(path, Some(&source), Some(&tree));
    if !scopes.is_empty() {
        hidden.tests += drop_scoped(&mut items, &scopes);
    }

    let line_count = String::from_utf8_lossy(&source).lines().count();
    let entries = count_items(&items);
    let mut out = format!(
        "{}  {} {}  ~{} tokens  {} {}\n",
        path,
        line_count,
        if line_count == 1 { "line" } else { "lines" },
        fmt_tokens(source.len() / 4),
        entries,
        if entries == 1 { "entry" } else { "entries" },
    );
    push_items(&mut out, &items, "");
    Outline { text: out, entries, hidden }
}

/// Remove the items that start inside a test scope, and return how many
/// entries went with them, nested ones included.
fn drop_scoped(items: &mut Vec<Item>, scopes: &[(usize, usize)]) -> usize {
    let mut dropped = 0;
    items.retain_mut(|it| {
        if scopes.iter().any(|(a, b)| *a <= it.line && it.line <= *b) {
            dropped += 1 + count_items(&it.children);
            false
        } else {
            dropped += drop_scoped(&mut it.children, scopes);
            true
        }
    });
    dropped
}

/// Rough token count, the way mdq reports it: bytes over four, thousands
/// abbreviated.
fn fmt_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn count_items(items: &[Item]) -> usize {
    items.iter().map(|i| 1 + count_items(&i.children)).sum()
}

#[derive(Debug)]
struct Item {
    kind: &'static str,   // "class", "fn", "var"
    name: String,
    line: usize,
    /// Last line of the declaration, for the printed span.
    end_line: usize,
    /// First source line of the declaration, trimmed and cut. Standing in for
    /// a per-language signature extractor: whatever a declaration is written
    /// as, its opening line is the part a reader wants.
    sig: String,
    children: Vec<Item>,
}

/// Longest signature printed before it is cut.
const SIG_MAX: usize = 100;

fn item(kind: &'static str, name: String, node: Node, source: &[u8], children: Vec<Item>) -> Item {
    Item {
        kind,
        name,
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        sig: signature(node, source),
        children,
    }
}

pub fn signature(node: Node, source: &[u8]) -> String {
    let first = first_line(node_text(node, source)).trim();
    if first.chars().count() > SIG_MAX {
        let cut: String = first.chars().take(SIG_MAX).collect();
        format!("{}...", cut)
    } else {
        first.to_string()
    }
}

fn node_text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

fn collect_items(root: Node, source: &[u8], lang_name: &str) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    walk_top_level(root, source, lang_name, &mut items);
    items
}

fn walk_top_level(node: Node, source: &[u8], lang_name: &str, out: &mut Vec<Item>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(item) = try_extract(child, source, lang_name, true) {
            out.push(item);
        }
    }
}

fn try_extract(node: Node, source: &[u8], lang_name: &str, top: bool) -> Option<Item> {
    match lang_name {
        "cpp" | "c"            => extract_c(node, source, lang_name, top),
        "python"               => extract_python(node, source, top),
        "javascript"           => extract_js(node, source, top),
        "typescript" | "tsx"   => extract_ts(node, source, top),
        "ruby"                 => extract_ruby(node, source, top),
        "rust"                 => extract_rust(node, source, top),
        "go"                   => extract_go(node, source, top),
        _                      => None,
    }
}

// ── C / C++ ─────────────────────────────────────────────────────────────────

fn extract_c(node: Node, source: &[u8], lang_name: &str, top: bool) -> Option<Item> {
    match node.kind() {
        "function_definition" => {
            let name = fn_name_c(node, source)?;
            Some(item("fn", name, node, source, vec![]))
        }
        "namespace_definition" if lang_name == "cpp" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_c(child, source, lang_name, false) {
                        children.push(it);
                    }
                }
            }
            Some(item("namespace", name, node, source, children))
        }
        "class_specifier" | "struct_specifier" if lang_name == "cpp" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_c(child, source, lang_name, false) {
                        children.push(it);
                    }
                    // field_declaration can contain function_declarator (prototype)
                    if child.kind() == "field_declaration" {
                        if let Some(it) = field_decl_method(child, source) {
                            children.push(it);
                        }
                    }
                }
            }
            let kind = if node.kind() == "class_specifier" { "class" } else { "struct" };
            Some(item(kind, name, node, source, children))
        }
        "declaration" if top => {
            // top-level variable declarations
            var_name_c(node, source).map(|name| item("var", name, node, source, vec![]))
        }
        _ => None,
    }
}

fn fn_name_c(node: Node, source: &[u8]) -> Option<String> {
    // function_definition → declarator field → function_declarator → declarator → identifier
    let decl = node.child_by_field_name("declarator")?;
    fn dig(n: Node, source: &[u8]) -> Option<String> {
        match n.kind() {
            "identifier" | "field_identifier" => Some(node_text(n, source).to_string()),
            _ => {
                let inner = n.child_by_field_name("declarator")
                    .or_else(|| n.child_by_field_name("name"));
                inner.and_then(|c| dig(c, source))
            }
        }
    }
    dig(decl, source)
}

fn field_decl_method(node: Node, source: &[u8]) -> Option<Item> {
    // method prototype: `int Foo(args);` inside a class body
    let decl = node.child_by_field_name("declarator")?;
    if decl.kind() != "function_declarator" {
        return None;
    }
    let name_node = decl.child_by_field_name("declarator")?;
    let name = node_text(name_node, source).to_string();
    Some(item("fn", name, node, source, vec![]))
}

fn var_name_c(node: Node, source: &[u8]) -> Option<String> {
    // simple heuristic: first identifier child that isn't a type keyword
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "identifier" {
            let text = node_text(child, source);
            return Some(text.to_string());
        }
        if child.kind() == "init_declarator" {
            if let Some(d) = child.child_by_field_name("declarator") {
                return Some(node_text(d, source).to_string());
            }
        }
    }
    None
}

// ── Python ───────────────────────────────────────────────────────────────────

fn extract_python(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        "function_definition" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "class_definition" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_python(child, source, false) {
                        children.push(it);
                    }
                }
            }
            Some(item("class", name, node, source, children))
        }
        "expression_statement" if top => {
            // x = ... at top level
            let assign = node.named_child(0).filter(|n| n.kind() == "assignment")?;
            let lhs = assign.child_by_field_name("left")?;
            if lhs.kind() == "identifier" {
                Some(item("var", node_text(lhs, source).to_string(), node, source, vec![]))
            } else { None }
        }
        _ => None,
    }
}

// ── Ruby ─────────────────────────────────────────────────────────────────────

fn extract_ruby(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        "method" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "singleton_method" => {
            // `def self.foo`. The object the method hangs off belongs in the
            // name, or two singleton methods on different objects read alike.
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            let object = node.child_by_field_name("object")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "self".into());
            Some(item("fn", format!("{}.{}", object, name), node, source, vec![]))
        }
        "class" | "module" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_ruby(child, source, false) {
                        children.push(it);
                    }
                }
            }
            let kind = if node.kind() == "class" { "class" } else { "module" };
            Some(item(kind, name, node, source, children))
        }
        "assignment" if top => {
            // `X = …` at top level. Only a constant is worth an outline entry.
            // A lowercase local would list every temporary in the file.
            let lhs = node.child_by_field_name("left")?;
            if lhs.kind() == "constant" {
                Some(item("const", node_text(lhs, source).to_string(), node, source, vec![]))
            } else { None }
        }
        _ => None,
    }
}

// ── JavaScript ───────────────────────────────────────────────────────────────

fn extract_js(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        "function_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "class_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if child.kind() == "method_definition" {
                        if let Some(n) = child.child_by_field_name("name") {
                            children.push(item(
                                "fn",
                                node_text(n, source).to_string(),
                                child, source, vec![],
                            ));
                        }
                    }
                }
            }
            Some(item("class", name, node, source, children))
        }
        "lexical_declaration" | "variable_declaration" if top => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() == "variable_declarator" {
                    if let Some(n) = child.child_by_field_name("name") {
                        // only show simple vars, not arrow functions
                        let val_kind = child.child_by_field_name("value")
                            .map(|v| v.kind())
                            .unwrap_or("");
                        let kind = if val_kind == "arrow_function" || val_kind == "function" {
                            "fn"
                        } else {
                            "var"
                        };
                        return Some(item(
                            kind,
                            node_text(n, source).to_string(),
                            node, source, vec![],
                        ));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// ── TypeScript / TSX ─────────────────────────────────────────────────────────

fn extract_ts(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        // unwrap export/declare wrappers and recurse into the inner declaration
        "export_statement" | "ambient_declaration" => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if let Some(item) = extract_ts(child, source, top) {
                    return Some(item);
                }
            }
            None
        }
        "function_declaration" | "function_signature" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "class_declaration" | "abstract_class_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if matches!(child.kind(), "method_definition" | "abstract_method_signature") {
                        if let Some(n) = child.child_by_field_name("name") {
                            children.push(item(
                                "fn",
                                node_text(n, source).to_string(),
                                child, source, vec![],
                            ));
                        }
                    }
                }
            }
            Some(item("class", name, node, source, children))
        }
        "interface_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            Some(item("interface", name, node, source, vec![]))
        }
        "enum_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            Some(item("enum", name, node, source, vec![]))
        }
        "type_alias_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("type", name, node, source, vec![]))
        }
        "lexical_declaration" | "variable_declaration" if top => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() == "variable_declarator" {
                    if let Some(n) = child.child_by_field_name("name") {
                        let val_kind = child.child_by_field_name("value")
                            .map(|v| v.kind())
                            .unwrap_or("");
                        let kind = if matches!(val_kind, "arrow_function" | "function") { "fn" } else { "var" };
                        return Some(item(
                            kind,
                            node_text(n, source).to_string(),
                            node, source, vec![],
                        ));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// ── Rust ─────────────────────────────────────────────────────────────────────

fn extract_rust(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        "function_item" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "mod_item" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_rust(child, source, false) {
                        children.push(it);
                    }
                }
            }
            Some(item("mod", name, node, source, children))
        }
        "struct_item" | "enum_item" | "trait_item" | "impl_item" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| {
                    // impl Foo — name is the type
                    node.child_by_field_name("type")
                        .map(|n| first_line(node_text(n, source)).to_string())
                        .unwrap_or_else(|| "(anonymous)".into())
                });
            let kind = match node.kind() {
                "struct_item" => "struct",
                "enum_item"   => "enum",
                "trait_item"  => "trait",
                _             => "impl",
            };
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(it) = extract_rust(child, source, false) {
                        children.push(it);
                    }
                }
            }
            Some(item(kind, name, node, source, children))
        }
        "const_item" | "static_item" if top => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("var", name, node, source, vec![]))
        }
        _ => None,
    }
}

// ── Go ───────────────────────────────────────────────────────────────────────

fn extract_go(node: Node, source: &[u8], top: bool) -> Option<Item> {
    match node.kind() {
        "function_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "method_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(item("fn", name, node, source, vec![]))
        }
        "type_declaration" if top => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() == "type_spec" {
                    let name = child.child_by_field_name("name")
                        .map(|n| node_text(n, source).to_string())?;
                    return Some(item("type", name, node, source, vec![]));
                }
            }
            None
        }
        _ => None,
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn collect_function_locs(path: &str, lang_name: &str) -> Vec<(String, usize)> {
    let source = match std::fs::read(path) { Ok(s) => s, Err(_) => return vec![] };
    let lang_box = match lang::for_name(lang_name) { Some(l) => l, None => return vec![] };
    let mut parser = Parser::new();
    if parser.set_language(&lang_box.ts_language()).is_err() { return vec![]; }
    let tree = match parser.parse(&source, None) { Some(t) => t, None => return vec![] };
    let items = collect_items(tree.root_node(), &source, lang_name);
    let mut locs = Vec::new();
    fn_locs_recursive(&items, &mut locs);
    locs
}

/// Every outline item as (kind, name, line), where kind is "class", "fn" or
/// "var" — the same vocabulary `tree` prints. Used by `explain` to resolve
/// names the call-graph index does not hold, such as classes and variables.
pub fn collect_def_locs(path: &str, lang_name: &str) -> Vec<(&'static str, String, usize)> {
    let source = match std::fs::read(path) { Ok(s) => s, Err(_) => return vec![] };
    let lang_box = match lang::for_name(lang_name) { Some(l) => l, None => return vec![] };
    let mut parser = Parser::new();
    if parser.set_language(&lang_box.ts_language()).is_err() { return vec![]; }
    let tree = match parser.parse(&source, None) { Some(t) => t, None => return vec![] };
    let items = collect_items(tree.root_node(), &source, lang_name);
    let mut locs = Vec::new();
    def_locs_recursive(&items, &mut locs);
    locs
}

fn def_locs_recursive(items: &[Item], out: &mut Vec<(&'static str, String, usize)>) {
    for item in items {
        out.push((item.kind, item.name.clone(), item.line));
        def_locs_recursive(&item.children, out);
    }
}

fn fn_locs_recursive(items: &[Item], out: &mut Vec<(String, usize)>) {
    for item in items {
        if item.kind == "fn" {
            out.push((item.name.clone(), item.line));
        }
        fn_locs_recursive(&item.children, out);
    }
}

// ── Printer ───────────────────────────────────────────────────────────────────

fn push_items(out: &mut String, items: &[Item], prefix: &str) {
    for (i, it) in items.iter().enumerate() {
        let last = i == items.len() - 1;
        let connector = if last { "└── " } else { "├── " };
        let child_prefix = if last { "    " } else { "│   " };
        let span = it.end_line.saturating_sub(it.line) + 1;
        let span_note = if span > 10 { format!("  ({} lines)", span) } else { String::new() };
        // The signature already carries the language's own keyword (`class`,
        // `func`, `const`), so printing the kind as well only stutters.
        out.push_str(&format!("{}{}{}  :{}{}\n", prefix, connector, it.sig, it.line, span_note));
        push_items(out, &it.children, &format!("{}{}", prefix, child_prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
    }

    fn outline(name: &str) -> String {
        report(&fixture(name), None, &mut crate::files::Hider::none()).text
    }

    #[test]
    fn the_header_reports_size_and_entry_count() {
        let out = outline("example.cpp");
        let header = out.lines().next().unwrap();
        assert!(header.contains("example.cpp"), "{}", header);
        assert!(header.contains(" lines  "), "line count missing: {}", header);
        assert!(header.contains("~"), "token estimate missing: {}", header);
        assert!(header.contains("entries"), "entry count missing: {}", header);
        let entries = out.lines().count() - 1;
        assert!(
            header.contains(&format!("{} entries", entries)),
            "count must match the printed entries ({}): {}",
            entries,
            header
        );
    }

    #[test]
    fn an_entry_prints_its_declaration_line_not_its_name() {
        let out = outline("example.cpp");
        assert!(out.contains("class Buffer {"), "expected the declaration: {}", out);
        assert!(out.contains("int main(int argc, char **argv) {"), "{}", out);
        assert!(!out.contains("── Buffer  :"), "the bare name should be gone: {}", out);
    }

    #[test]
    fn entries_over_ten_lines_report_their_span() {
        let out = outline("example.cpp");
        let buffer = out.lines().find(|l| l.contains("class Buffer")).unwrap();
        assert!(buffer.contains(" lines)"), "expected a span note: {}", buffer);
        let ctor = out.lines().find(|l| l.contains("Buffer(int size)")).unwrap();
        assert!(!ctor.contains(" lines)"), "short entries take no span note: {}", ctor);
    }

    #[test]
    fn every_entry_keeps_its_start_line() {
        let out = outline("example.go");
        assert!(out.contains("func NewServer(host string, port int) *Server {  :14"), "{}", out);
    }

    #[test]
    fn indentation_still_nests_children_under_their_parent() {
        let out = outline("example.cpp");
        assert!(out.contains("│   ├── ") || out.contains("    └── "), "{}", out);
    }

    #[test]
    fn a_ruby_class_nests_its_methods_and_names_the_singleton() {
        let out = outline("example.rb");
        assert!(out.contains("├── class MyClass  :11"), "{}", out);
        assert!(out.contains("│   ├── def run  :12"), "methods nest under the class: {}", out);
        assert!(out.contains("def self.build"), "a singleton method keeps its object: {}", out);
        assert!(out.contains("module Shapes"), "a module is an entry too: {}", out);
        assert!(out.contains("MAX_SIZE = 64"), "a top-level constant is an entry: {}", out);
    }

    #[test]
    fn a_long_declaration_is_cut_to_one_readable_line() {
        let dir = std::env::temp_dir().join("ctsq_tree_sig_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.go");
        let long = format!("package main\n\nfunc F({}) {{\n}}\n", "a, b string, ".repeat(20));
        std::fs::write(&path, long).unwrap();
        let out = report(path.to_str().unwrap(), None, &mut crate::files::Hider::none()).text;
        let entry = out.lines().nth(1).unwrap();
        assert!(entry.contains("..."), "expected a cut marker: {}", entry);
        assert!(entry.len() < 160, "signature must stay on one readable line: {}", entry);
    }

    #[test]
    fn test_scopes_are_left_out_and_counted_with_what_they_hold() {
        let dir = std::env::temp_dir().join(format!("ctsq_tree_scopes_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(
            &path,
            "fn real() {}\n\n#[cfg(test)]\nfn helper() {}\n\n#[cfg(test)]\nmod tests {\n    fn a() {}\n    fn b() {}\n}\n",
        )
        .unwrap();
        let root = dir.to_str().unwrap();
        let file = path.to_str().unwrap();
        let jobs = crate::files::resolve_jobs(root, None, true);
        let mut hider = crate::files::Hider::new(&Default::default());
        hider.add_jobs(root, &jobs);
        let o = report(file, None, &mut hider);
        assert!(o.text.contains("fn real()"), "{}", o.text);
        assert!(!o.text.contains("mod tests"), "{}", o.text);
        assert!(!o.text.contains("helper"), "{}", o.text);
        assert_eq!(o.entries, 1);
        assert_eq!(o.hidden.tests, 4, "helper, mod tests and its two functions");
        assert!(o.text.lines().next().unwrap().ends_with(" 1 entry"), "{}", o.text);

        let mut all = crate::files::Hider::new(&crate::files::HideOpts { tests: true, exclude: vec![] });
        all.add_jobs(root, &jobs);
        let o = report(file, None, &mut all);
        assert!(o.text.contains("mod tests"), "--tests keeps them: {}", o.text);
        assert_eq!(o.hidden, Default::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hidden_file_prints_nothing_and_counts_its_entries() {
        let dir = std::env::temp_dir().join(format!("ctsq_tree_hidden_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a_test.go"), "package a\n\nfunc A() {}\nfunc B() {}\n").unwrap();
        let root = dir.to_str().unwrap();
        let jobs = crate::files::resolve_jobs(root, None, true);
        let mut hider = crate::files::Hider::new(&Default::default());
        hider.add_jobs(root, &jobs);
        let o = report(&jobs[0].0, None, &mut hider);
        assert_eq!(o.text, "");
        assert_eq!(o.hidden.tests, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_read_as_thousands_once_past_a_thousand() {
        assert_eq!(fmt_tokens(907), "907");
        assert_eq!(fmt_tokens(3400), "3.4k");
    }
}
