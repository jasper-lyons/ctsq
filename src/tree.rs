use crate::compiler::lang;
use tree_sitter::{Node, Parser};

pub fn run(path: &str, lang_name: Option<&str>) {
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
        None => { eprintln!("Cannot detect language; use --lang"); std::process::exit(1); }
    };
    let lang_box = match lang::for_name(lang_name) {
        Some(l) => l,
        None => { eprintln!("Unknown language: {}", lang_name); std::process::exit(1); }
    };

    let mut parser = Parser::new();
    parser.set_language(&lang_box.ts_language()).unwrap();
    let tree = parser.parse(&source, None).expect("parse failed");

    println!("{}", path);
    let root = tree.root_node();
    let items = collect_items(root, &source, lang_name);
    print_items(&items, "");
}

#[derive(Debug)]
struct Item {
    kind: &'static str,   // "class", "fn", "var"
    name: String,
    line: usize,
    children: Vec<Item>,
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
        "cpp" | "c" => extract_c(node, source, lang_name, top),
        "python"     => extract_python(node, source, top),
        "javascript" => extract_js(node, source, top),
        "rust"       => extract_rust(node, source, top),
        "go"         => extract_go(node, source, top),
        _            => None,
    }
}

// ── C / C++ ─────────────────────────────────────────────────────────────────

fn extract_c(node: Node, source: &[u8], lang_name: &str, top: bool) -> Option<Item> {
    match node.kind() {
        "function_definition" => {
            let name = fn_name_c(node, source)?;
            let line = node.start_position().row + 1;
            Some(Item { kind: "fn", name, line, children: vec![] })
        }
        "namespace_definition" if lang_name == "cpp" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(item) = extract_c(child, source, lang_name, false) {
                        children.push(item);
                    }
                }
            }
            Some(Item { kind: "namespace", name, line, children })
        }
        "class_specifier" | "struct_specifier" if lang_name == "cpp" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(item) = extract_c(child, source, lang_name, false) {
                        children.push(item);
                    }
                    // field_declaration can contain function_declarator (prototype)
                    if child.kind() == "field_declaration" {
                        if let Some(item) = field_decl_method(child, source) {
                            children.push(item);
                        }
                    }
                }
            }
            let kind = if node.kind() == "class_specifier" { "class" } else { "struct" };
            Some(Item { kind, name, line, children })
        }
        "declaration" if top => {
            // top-level variable declarations
            var_name_c(node, source).map(|name| Item {
                kind: "var", name, line: node.start_position().row + 1, children: vec![],
            })
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
    let line = node.start_position().row + 1;
    Some(Item { kind: "fn", name, line, children: vec![] })
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
            Some(Item { kind: "fn", name, line: node.start_position().row + 1, children: vec![] })
        }
        "class_definition" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(item) = extract_python(child, source, false) {
                        children.push(item);
                    }
                }
            }
            Some(Item { kind: "class", name, line, children })
        }
        "expression_statement" if top => {
            // x = ... at top level
            let assign = node.named_child(0).filter(|n| n.kind() == "assignment")?;
            let lhs = assign.child_by_field_name("left")?;
            if lhs.kind() == "identifier" {
                Some(Item { kind: "var", name: node_text(lhs, source).to_string(),
                    line: node.start_position().row + 1, children: vec![] })
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
            Some(Item { kind: "fn", name, line: node.start_position().row + 1, children: vec![] })
        }
        "class_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if child.kind() == "method_definition" {
                        if let Some(n) = child.child_by_field_name("name") {
                            children.push(Item {
                                kind: "fn",
                                name: node_text(n, source).to_string(),
                                line: child.start_position().row + 1,
                                children: vec![],
                            });
                        }
                    }
                }
            }
            Some(Item { kind: "class", name, line, children })
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
                        if val_kind != "arrow_function" && val_kind != "function" {
                            return Some(Item {
                                kind: "var",
                                name: node_text(n, source).to_string(),
                                line: node.start_position().row + 1,
                                children: vec![],
                            });
                        }
                        // arrow functions / function expressions: show as fn
                        return Some(Item {
                            kind: "fn",
                            name: node_text(n, source).to_string(),
                            line: node.start_position().row + 1,
                            children: vec![],
                        });
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
            Some(Item { kind: "fn", name, line: node.start_position().row + 1, children: vec![] })
        }
        "mod_item" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_else(|| "(anonymous)".into());
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(item) = extract_rust(child, source, false) {
                        children.push(item);
                    }
                }
            }
            Some(Item { kind: "mod", name, line, children })
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
            let line = node.start_position().row + 1;
            let mut children = Vec::new();
            let body_field = if node.kind() == "impl_item" { "body" } else { "body" };
            if let Some(body) = node.child_by_field_name(body_field) {
                let mut cur = body.walk();
                for child in body.children(&mut cur) {
                    if let Some(item) = extract_rust(child, source, false) {
                        children.push(item);
                    }
                }
            }
            Some(Item { kind, name, line, children })
        }
        "const_item" | "static_item" if top => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(Item { kind: "var", name, line: node.start_position().row + 1, children: vec![] })
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
            Some(Item { kind: "fn", name, line: node.start_position().row + 1, children: vec![] })
        }
        "method_declaration" => {
            let name = node.child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())?;
            Some(Item { kind: "fn", name, line: node.start_position().row + 1, children: vec![] })
        }
        "type_declaration" if top => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() == "type_spec" {
                    let name = child.child_by_field_name("name")
                        .map(|n| node_text(n, source).to_string())?;
                    return Some(Item {
                        kind: "type", name, line: node.start_position().row + 1, children: vec![],
                    });
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

fn fn_locs_recursive(items: &[Item], out: &mut Vec<(String, usize)>) {
    for item in items {
        if item.kind == "fn" {
            out.push((item.name.clone(), item.line));
        }
        fn_locs_recursive(&item.children, out);
    }
}

// ── Printer ───────────────────────────────────────────────────────────────────

fn print_items(items: &[Item], prefix: &str) {
    for (i, item) in items.iter().enumerate() {
        let last = i == items.len() - 1;
        let connector = if last { "└── " } else { "├── " };
        let child_prefix = if last { "    " } else { "│   " };
        println!("{}{}{} {}  :{}", prefix, connector, item.kind, item.name, item.line);
        print_items(&item.children, &format!("{}{}", prefix, child_prefix));
    }
}
