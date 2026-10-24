//! TypeScript and JavaScript parameter lists, with destructured parameters
//! split into the names they bind. The TS analogue of `resolve::go`'s
//! `GoParam`, `GoSignature` and `go_signatures`.
//!
//! JS `formal_parameters` holds bare patterns. TS and TSX wrap each one in a
//! `required_parameter` or `optional_parameter` with `pattern`, `type` and
//! `value` fields. Both shapes give the same `TsParam` list.
#![allow(dead_code)] // wired by C3

use tree_sitter::Node;

/// One name bound by a parameter. `function f(a, {b, c: d}: P, ...rest)`
/// gives `a` (position 0, path []), `b` (1, [b]), `d` (1, [c]) and `rest`
/// (2, [], rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsParam {
    /// The bound name. Empty when nothing is bound, as with an empty pattern `{}`.
    pub name: String,
    /// 0-based position in the parameter list.
    pub position: usize,
    /// Where the name sits inside the argument: property names for object
    /// patterns and decimal indices for array patterns, outermost first.
    /// Empty when the name is the whole argument.
    pub path: Vec<String>,
    /// The name is a `...rest`: the remaining arguments at the top level, or
    /// the remaining properties or elements inside a pattern.
    pub rest: bool,
    /// The name has a default value (`a = 1`, `{b = 2}`), or the parameter is
    /// optional (`a?: T`).
    pub default: bool,
    /// Type annotation of the whole parameter as written, without the `:`.
    /// Empty in JS and when unannotated.
    pub ty: String,
    /// Byte range of the bound name, or of the parameter when nothing is bound.
    pub start_byte: usize,
    pub end_byte: usize,
}

/// The parameters of one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsSignature {
    /// Start line of the function node, 1-based.
    pub line: usize,
    /// Function name: its own `name` field, else the variable or property
    /// it is bound to (`const f = () => {}`). Empty for anonymous callbacks.
    pub name: String,
    pub params: Vec<TsParam>,
    /// Byte range of the function node.
    pub start_byte: usize,
    pub end_byte: usize,
}

const FUNCTION_KINDS: &[&str] = &[
    "function_declaration",
    "function_expression",
    "function",
    "arrow_function",
    "method_definition",
    "generator_function_declaration",
    "generator_function",
];

fn text(n: Node, src: &[u8]) -> String {
    n.utf8_text(src).unwrap_or("").to_string()
}

/// Every function in the file, ordered by start byte.
pub fn ts_signatures(root: Node, src: &[u8]) -> Vec<TsSignature> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if let Some(s) = ts_signature(n, src) {
            out.push(s);
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.named_children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    out.sort_by_key(|s| s.start_byte);
    out
}

/// The signature of `func` when it is a function node, else None.
pub fn ts_signature(func: Node, src: &[u8]) -> Option<TsSignature> {
    if !FUNCTION_KINDS.contains(&func.kind()) {
        return None;
    }
    let params = if let Some(p) = func.child_by_field_name("parameters") {
        ts_params(p, src)
    } else if let Some(p) = func.child_by_field_name("parameter") {
        // `x => ...`: a single bare identifier.
        let mut out = Vec::new();
        bind(p, src, 0, Vec::new(), false, String::new(), &mut out);
        out
    } else {
        Vec::new()
    };
    Some(TsSignature {
        line: func.start_position().row + 1,
        name: function_name(func, src),
        params,
        start_byte: func.start_byte(),
        end_byte: func.end_byte(),
    })
}

fn function_name(func: Node, src: &[u8]) -> String {
    if let Some(n) = func.child_by_field_name("name") {
        return text(n, src);
    }
    let Some(parent) = func.parent() else { return String::new() };
    let field = match parent.kind() {
        "variable_declarator" => "name",
        "assignment_expression" => "left",
        "pair" => "key",
        "public_field_definition" | "field_definition" => {
            return parent
                .child_by_field_name("name")
                .or_else(|| parent.child_by_field_name("property"))
                .map(|n| text(n, src))
                .unwrap_or_default()
        }
        _ => return String::new(),
    };
    match parent.child_by_field_name(field) {
        Some(n) if n.id() != func.id() => text(n, src),
        _ => String::new(),
    }
}

/// The parameters of a `formal_parameters` node, JS or TS.
pub fn ts_params(list: Node, src: &[u8]) -> Vec<TsParam> {
    let mut out = Vec::new();
    let mut position = 0;
    let mut c = list.walk();
    for p in list.named_children(&mut c) {
        match p.kind() {
            "comment" => continue,
            "required_parameter" | "optional_parameter" => {
                let ty = p
                    .child_by_field_name("type")
                    .map(|t| text(t, src).trim_start_matches(':').trim().to_string())
                    .unwrap_or_default();
                let optional = p.kind() == "optional_parameter" || p.child_by_field_name("value").is_some();
                match p.child_by_field_name("pattern") {
                    Some(pat) => {
                        let before = out.len();
                        bind(pat, src, position, Vec::new(), optional, ty, &mut out);
                        if out.len() == before {
                            out.push(unbound(p, position));
                        }
                    }
                    None => out.push(unbound(p, position)),
                }
            }
            _ => {
                let before = out.len();
                bind(p, src, position, Vec::new(), false, String::new(), &mut out);
                if out.len() == before {
                    out.push(unbound(p, position));
                }
            }
        }
        position += 1;
    }
    out
}

fn unbound(n: Node, position: usize) -> TsParam {
    TsParam {
        name: String::new(),
        position,
        path: Vec::new(),
        rest: false,
        default: false,
        ty: String::new(),
        start_byte: n.start_byte(),
        end_byte: n.end_byte(),
    }
}

/// Push every name bound by pattern `n`, which sits at `path` inside
/// parameter `position`.
fn bind(n: Node, src: &[u8], position: usize, path: Vec<String>, default: bool, ty: String, out: &mut Vec<TsParam>) {
    let leaf = |name: Node, rest: bool, default: bool, path: Vec<String>, out: &mut Vec<TsParam>| {
        out.push(TsParam {
            name: text(name, src),
            position,
            path,
            rest,
            default,
            ty: ty.clone(),
            start_byte: name.start_byte(),
            end_byte: name.end_byte(),
        })
    };
    match n.kind() {
        "identifier" | "this" => leaf(n, false, default, path, out),
        "shorthand_property_identifier_pattern" => {
            let mut p = path;
            p.push(text(n, src));
            leaf(n, false, default, p, out)
        }
        "assignment_pattern" => {
            if let Some(l) = n.child_by_field_name("left") {
                bind(l, src, position, path, true, ty.clone(), out);
            }
        }
        "object_assignment_pattern" => {
            // `{b = 2}`: the left is a shorthand name, or a nested pattern.
            if let Some(l) = n.child_by_field_name("left") {
                bind(l, src, position, path, true, ty.clone(), out);
            }
        }
        "rest_pattern" => {
            let mut c = n.walk();
            let inner: Vec<Node> = n.named_children(&mut c).collect();
            for i in inner {
                let before = out.len();
                bind(i, src, position, path.clone(), default, ty.clone(), out);
                if before < out.len() {
                    out[before].rest = true;
                }
            }
        }
        "object_pattern" => {
            let mut c = n.walk();
            let kids: Vec<Node> = n.named_children(&mut c).collect();
            for k in kids {
                match k.kind() {
                    "pair_pattern" => {
                        let key = k.child_by_field_name("key").map(|key| property_key(key, src)).unwrap_or_default();
                        if let Some(v) = k.child_by_field_name("value") {
                            let mut p = path.clone();
                            p.push(key);
                            bind(v, src, position, p, default, ty.clone(), out);
                        }
                    }
                    "comment" => {}
                    _ => bind(k, src, position, path.clone(), default, ty.clone(), out),
                }
            }
        }
        "array_pattern" => {
            let mut c = n.walk();
            let mut index = 0;
            // Holes (`[, b]`) are bare commas, so count every element slot.
            let kids: Vec<Node> = n.children(&mut c).collect();
            let mut slot_filled = false;
            for k in kids {
                if !k.is_named() {
                    if k.kind() == "," {
                        index += 1;
                        slot_filled = false;
                    }
                    continue;
                }
                if k.kind() == "comment" || slot_filled {
                    continue;
                }
                slot_filled = true;
                let mut p = path.clone();
                if k.kind() != "rest_pattern" {
                    p.push(index.to_string());
                }
                bind(k, src, position, p, default, ty.clone(), out);
            }
        }
        _ => {}
    }
}

/// The property name a pattern key reads: the bare name, or a string or
/// number key without quotes.
fn property_key(key: Node, src: &[u8]) -> String {
    let t = text(key, src);
    match key.kind() {
        "string" => t.trim_matches(['"', '\'']).to_string(),
        _ => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lang: &str, src: &str) -> tree_sitter::Tree {
        let language: tree_sitter::Language = match lang {
            "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_javascript::LANGUAGE.into(),
        };
        let mut p = tree_sitter::Parser::new();
        p.set_language(&language).unwrap();
        p.parse(src, None).unwrap()
    }

    /// (name, position, path, rest, default) per parameter of the first function.
    fn sig(lang: &str, src: &str) -> Vec<(String, usize, Vec<String>, bool, bool)> {
        let tree = parse(lang, src);
        let sigs = ts_signatures(tree.root_node(), src.as_bytes());
        let s = sigs.first().unwrap_or_else(|| panic!("no function in {}", src));
        s.params.iter().map(|p| (p.name.clone(), p.position, p.path.clone(), p.rest, p.default)).collect()
    }

    fn row(name: &str, pos: usize, path: &[&str], rest: bool, default: bool) -> (String, usize, Vec<String>, bool, bool) {
        (name.to_string(), pos, path.iter().map(|s| s.to_string()).collect(), rest, default)
    }

    fn abc() -> Vec<(String, usize, Vec<String>, bool, bool)> {
        vec![row("a", 0, &[], false, false), row("b", 1, &["b"], false, false), row("c", 1, &["c"], false, false), row("rest", 2, &[], true, false)]
    }

    #[test]
    fn destructured_params_in_ts_tsx_and_js() {
        assert_eq!(sig("ts", "function f(a, {b, c}: P, ...rest) {}"), abc());
        assert_eq!(sig("tsx", "function f(a, {b, c}: P, ...rest: string[]) {}"), abc());
        assert_eq!(sig("js", "function f(a, {b, c}, ...rest) {}"), abc());
    }

    #[test]
    fn ts_types_are_kept() {
        let tree = parse("ts", "function f(a: string, {b}: P) {}");
        let s = &ts_signatures(tree.root_node(), b"function f(a: string, {b}: P) {}")[0];
        assert_eq!(s.name, "f");
        assert_eq!(s.params[0].ty, "string");
        assert_eq!(s.params[1].ty, "P");
    }

    #[test]
    fn arrow_functions() {
        for lang in ["ts", "tsx", "js"] {
            assert_eq!(sig(lang, "const g = (props) => props.x;"), vec![row("props", 0, &[], false, false)], "{}", lang);
            assert_eq!(sig(lang, "const g = props => props.x;"), vec![row("props", 0, &[], false, false)], "{}", lang);
            assert_eq!(sig(lang, "const g = (a, {b, c}, ...rest) => a;"), abc(), "{}", lang);
        }
        let src = "const g = (props) => props.x;";
        let tree = parse("tsx", src);
        assert_eq!(ts_signatures(tree.root_node(), src.as_bytes())[0].name, "g");
    }

    #[test]
    fn default_values() {
        for lang in ["ts", "tsx", "js"] {
            assert_eq!(sig(lang, "function f(a = 1) {}"), vec![row("a", 0, &[], false, true)], "{}", lang);
            assert_eq!(sig(lang, "const f = (a = 1) => a;"), vec![row("a", 0, &[], false, true)], "{}", lang);
            assert_eq!(sig(lang, "function f({b = 2}) {}"), vec![row("b", 0, &["b"], false, true)], "{}", lang);
        }
        assert_eq!(sig("ts", "function f(a?: number) {}"), vec![row("a", 0, &[], false, true)]);
    }

    #[test]
    fn renamed_nested_and_array_patterns() {
        for lang in ["ts", "js"] {
            assert_eq!(
                sig(lang, "function f({b: x, d: {e}, ...others}, [p, , q]) {}"),
                vec![
                    row("x", 0, &["b"], false, false),
                    row("e", 0, &["d", "e"], false, false),
                    row("others", 0, &[], true, false),
                    row("p", 1, &["0"], false, false),
                    row("q", 1, &["2"], false, false),
                ],
                "{}",
                lang
            );
        }
    }

    #[test]
    fn methods_and_callbacks_are_listed() {
        let src = "class A { m(x) { return [1].map((y, i) => y); } }";
        let tree = parse("ts", src);
        let sigs = ts_signatures(tree.root_node(), src.as_bytes());
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0].name, "m");
        assert_eq!(sigs[1].name, "");
        assert_eq!(sigs[1].params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["y", "i"]);
    }

    #[test]
    fn no_params_and_non_functions() {
        assert!(sig("js", "function f() {}").is_empty());
        let tree = parse("js", "let x = 1;");
        assert!(ts_signatures(tree.root_node(), b"let x = 1;").is_empty());
    }
}
