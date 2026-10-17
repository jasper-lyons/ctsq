use crate::compiler::lang::Lang;
use crate::compiler::CompiledQuery;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

/// One match, in both renderings the CLI can print.
///
/// `node_text` is the historical form (`@cap:LINE: "text"`), kept behind
/// `--node` and used verbatim by every internal caller that parses it back.
/// `src_line` carries the whole source line the match sits on, which is what
/// `-q` prints by default. It is None when the source line would lose
/// information: a match with two or more user captures, or a multi-line node
/// printed with --body.
#[derive(Debug, Clone)]
pub struct MatchRow {
    pub line: usize,
    pub src_line: Option<String>,
    pub node_text: String,
    /// Byte range of the primary node: the node `line` points at.
    pub start_byte: usize,
    pub end_byte: usize,
    /// Each user capture of the match (names not starting with `_`), in
    /// pattern order. `find` reads its key and target from here.
    pub captures: Vec<Capture>,
}

/// One user capture of a match: its name, its node's text and position.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub name: String,
    pub text: String,
    /// 1-based line of the node's first byte.
    pub line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

thread_local! {
    /// Compiled tree-sitter queries by grammar and pattern text. Building a
    /// query costs far more than running it on one file, and a run over a
    /// repo asks for the same pattern once per file.
    static QUERIES: std::cell::RefCell<std::collections::HashMap<(tree_sitter::Language, String), std::rc::Rc<Query>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn cached_query(lang: &tree_sitter::Language, text: &str) -> Result<std::rc::Rc<Query>, tree_sitter::QueryError> {
    let key = (lang.clone(), text.to_string());
    if let Some(q) = QUERIES.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(q);
    }
    let q = std::rc::Rc::new(Query::new(lang, text)?);
    QUERIES.with(|c| c.borrow_mut().insert(key, q.clone()));
    Ok(q)
}

/// Backwards-compatible wrapper: the node-text rendering only.
#[cfg(test)]
pub fn run(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    body: bool,
) -> Result<Vec<String>, String> {
    Ok(run_rows(compiled, lang, source, body)?
        .into_iter()
        .map(|r| r.node_text)
        .collect())
}

pub fn run_rows(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    body: bool,
) -> Result<Vec<MatchRow>, String> {
    if compiled.is_empty() {
        return Ok(vec![]);
    }

    let ts_lang = lang.ts_language();

    let mut parser = Parser::new();
    parser.set_language(&ts_lang).map_err(|e| e.to_string())?;

    let tree = parser.parse(source, None).ok_or("failed to parse source")?;
    run_rows_on(compiled, lang, source, &tree, body)
}

/// `run_rows` over `file` as the run's cache holds it (`parsed::get`), so the
/// file is read and parsed once however many queries and consumers ask.
/// `lang_name` is the job's lang name, the cache key. A file that cannot be
/// read is an error. A query with no pattern returns no rows without parsing.
#[allow(dead_code)] // for the graph and consumer packages
pub fn run_rows_cached(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    file: &str,
    lang_name: &'static str,
    body: bool,
) -> Result<Vec<MatchRow>, String> {
    if compiled.is_empty() {
        return Ok(vec![]);
    }
    let p = crate::parsed::get(file, lang_name).ok_or_else(|| format!("failed to read {}", file))?;
    let tree = p.tree.as_ref().ok_or("failed to parse source")?;
    run_rows_on(compiled, lang, &p.source, tree, body)
}

/// Whether a simple query has any match in `tree`. Any other query reports true,
/// so the answer only ever rules a file out.
pub fn has_match(compiled: &CompiledQuery, lang: &dyn Lang, source: &[u8], tree: &tree_sitter::Tree) -> bool {
    let CompiledQuery::Simple(text) = compiled else { return true };
    let Ok(query) = cached_query(&lang.ts_language(), text) else { return true };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source);
    matches.next().is_some()
}

/// `run_rows` over a tree the caller already parsed from `source` with `lang`'s
/// grammar, so several queries can share one parse.
pub fn run_rows_on(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    tree: &tree_sitter::Tree,
    body: bool,
) -> Result<Vec<MatchRow>, String> {
    run_rows_on_at(compiled, lang, source, tree, body, None)
}

/// The literal a simple query's single `#eq?` predicate compares a capture
/// with, when the query has one pattern and no other predicate. Every match of
/// such a query holds those bytes, so `run_rows_on_at` can search only around
/// where they occur, and a file without them has no match at all.
pub fn literal_anchor(compiled: &CompiledQuery, lang: &dyn Lang) -> Option<String> {
    let CompiledQuery::Simple(text) = compiled else { return None };
    if text.matches("(#").count() != 1 || text.matches("(#eq? @").count() != 1 {
        return None;
    }
    let q = cached_query(&lang.ts_language(), text).ok()?;
    if q.pattern_count() != 1 {
        return None;
    }
    let rest = &text[text.find("(#eq? @")? + 7..];
    let lit = rest.split_once(" \"")?.1.split_once("\")")?.0;
    if lit.is_empty() || lit.contains(['\\', '"']) {
        return None;
    }
    Some(lit.to_string())
}

/// `run_rows_on` limited to matches that overlap one of `ranges` (byte ranges
/// of the source), each searched with its own cursor so the walk skips every
/// subtree outside it. None searches the whole tree. Scoped queries ignore
/// the ranges and search the whole tree.
pub fn run_rows_on_at(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    tree: &tree_sitter::Tree,
    body: bool,
    ranges: Option<&[std::ops::Range<usize>]>,
) -> Result<Vec<MatchRow>, String> {
    Ok(run_core(compiled, lang, source, tree, body, ranges, false)?.0)
}

/// Rows of a scoped query whose outer pattern filters a name (`_n` capture),
/// each with the text of that name capture. Rows are deduplicated per name,
/// so filtering them by a name gives exactly what the query with that name in
/// an `#eq?` predicate gives. The fallback resolver runs one query per file
/// with a match-anything name instead of one per function.
pub fn run_rows_named(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    tree: &tree_sitter::Tree,
) -> Result<Vec<(MatchRow, String)>, String> {
    let (rows, names) = run_core(compiled, lang, source, tree, false, None, true)?;
    Ok(rows.into_iter().zip(names).collect())
}

fn run_core(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    tree: &tree_sitter::Tree,
    body: bool,
    ranges: Option<&[std::ops::Range<usize>]>,
    by_name: bool,
) -> Result<(Vec<MatchRow>, Vec<String>), String> {
    if compiled.is_empty() {
        return Ok((vec![], vec![]));
    }
    // Per row, the outer name capture text of a scoped query (empty otherwise).
    let mut names: Vec<String> = Vec::new();
    let ts_lang = lang.ts_language();

    let source_bytes = source;
    let mut results: Vec<MatchRow> = Vec::new();

    let source_text = String::from_utf8_lossy(source);
    let src_lines: Vec<&str> = source_text.lines().collect();

    let make_row = |node_text: String, primary: tree_sitter::Node, caps: &[(String, tree_sitter::Node)]| -> MatchRow {
        let start = primary.start_position().row + 1;
        let multiline = primary.end_position().row != primary.start_position().row;
        let captures = caps
            .iter()
            .map(|(name, n)| Capture {
                name: name.clone(),
                text: n.utf8_text(source_bytes).unwrap_or("").to_string(),
                line: n.start_position().row + 1,
                start_byte: n.start_byte(),
                end_byte: n.end_byte(),
            })
            .collect();
        let src_line = if caps.len() >= 2 || (body && multiline) {
            None
        } else {
            Some(
                src_lines
                    .get(start.saturating_sub(1))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_default(),
            )
        };
        MatchRow {
            line: start,
            src_line,
            node_text,
            start_byte: primary.start_byte(),
            end_byte: primary.end_byte(),
            captures,
        }
    };

    let loc = |node: tree_sitter::Node| -> String {
        let start = node.start_position().row + 1;
        let end = node.end_position().row + 1;
        if body && end != start {
            format!("{}-{}", start, end)
        } else {
            format!("{}", start)
        }
    };

    let fmt_cap = |name: &str, node: tree_sitter::Node, body: bool| -> String {
        let text = node.utf8_text(source_bytes).unwrap_or("<invalid utf8>");
        let display = if body {
            text.to_string()
        } else {
            text.lines().next().unwrap_or("").to_string()
        };
        if body {
            format!("@{}:{}: {}", name, loc(node), display)
        } else {
            format!("@{}:{}: {:?}", name, loc(node), display)
        }
    };

    match compiled {
        CompiledQuery::Simple(ts_query) => {
            let query = cached_query(&ts_lang, ts_query).map_err(|e| format!("query error: {e}"))?;
            let capture_names = query.capture_names();
            let spans: Vec<Option<std::ops::Range<usize>>> = match ranges {
                Some(r) => r.iter().cloned().map(Some).collect(),
                None => vec![None],
            };
            for span in spans {
            let mut cursor = QueryCursor::new();
            if let Some(r) = span {
                cursor.set_byte_range(r);
            }
            let mut matches = cursor.matches(&query, tree.root_node(), source_bytes);

            while let Some(m) = matches.next() {
                let mut parts: Vec<String> = Vec::new();
                let mut user_nodes: Vec<(String, tree_sitter::Node)> = Vec::new();
                for capture in m.captures {
                    let name = &capture_names[capture.index as usize];
                    if name.starts_with('_') {
                        continue;
                    }
                    parts.push(fmt_cap(name, capture.node, body));
                    user_nodes.push((name.to_string(), capture.node));
                }
                if parts.is_empty() {
                    let root = m
                        .captures
                        .iter()
                        .find(|cap| capture_names[cap.index as usize].starts_with("_root"))
                        .or_else(|| m.captures.first());
                    if let Some(cap) = root {
                        let text =
                            cap.node.utf8_text(source_bytes).unwrap_or("<invalid utf8>");
                        let display = if body {
                            text.to_string()
                        } else {
                            text.lines().next().unwrap_or("").to_string()
                        };
                        let node_text = if body {
                            format!("<match>:{}: {}", loc(cap.node), display)
                        } else {
                            format!("<match>:{}: {:?}", loc(cap.node), display)
                        };
                        results.push(make_row(node_text, cap.node, &[]));
                    } else {
                        results.push(MatchRow {
                            line: 0,
                            src_line: None,
                            node_text: "<match>".into(),
                            start_byte: 0,
                            end_byte: 0,
                            captures: vec![],
                        });
                    }
                } else {
                    let primary = user_nodes[0].1;
                    results.push(make_row(parts.join(", "), primary, &user_nodes));
                }
            }
            }
        }

        CompiledQuery::Scoped { outer, scope_field, inner } => {
            let outer_q = cached_query(&ts_lang, outer).map_err(|e| format!("outer query error: {e}"))?;
            let inner_q = cached_query(&ts_lang, inner).map_err(|e| format!("inner query error: {e}"))?;

            let outer_cap_names = outer_q.capture_names();
            let inner_cap_names = inner_q.capture_names();

            // Phase 1: collect outer match nodes and their user-visible captures.
            // We store (root_node, named_caps) for each outer match.
            // root_node is the container we'll search within — the first non-internal capture,
            // falling back to the first _root capture, falling back to the first capture overall.
            type CapList<'t> = Vec<(String, tree_sitter::Node<'t>)>;
            let mut outer_data: Vec<(tree_sitter::Node, CapList, String)> = Vec::new();
            {
                let mut cursor = QueryCursor::new();
                let mut matches =
                    cursor.matches(&outer_q, tree.root_node(), source_bytes);
                while let Some(m) = matches.next() {
                    let named: CapList = m
                        .captures
                        .iter()
                        .filter_map(|cap| {
                            let name = &outer_cap_names[cap.index as usize];
                            if !name.starts_with('_') {
                                Some((name.to_string(), cap.node))
                            } else {
                                None
                            }
                        })
                        .collect();

                    // An explicit `@_root` marks the head node and wins over user captures.
                    let root = m
                        .captures
                        .iter()
                        .find(|cap| outer_cap_names[cap.index as usize] == "_root")
                        .or_else(|| m.captures.iter().find(|cap| {
                            !outer_cap_names[cap.index as usize].starts_with('_')
                        }))
                        .or_else(|| {
                            m.captures.iter().find(|cap| {
                                outer_cap_names[cap.index as usize].starts_with("_root")
                            })
                        })
                        .or_else(|| m.captures.first())
                        .map(|cap| cap.node);

                    let name: Vec<&str> = m
                        .captures
                        .iter()
                        .filter(|cap| outer_cap_names[cap.index as usize].starts_with("_n"))
                        .map(|cap| cap.node.utf8_text(source_bytes).unwrap_or(""))
                        .collect();
                    if let Some(root_node) = root {
                        outer_data.push((root_node, named, name.join("\n")));
                    }
                }
            }

            // Phase 2: for each outer node, navigate to scope_field then run inner query.
            // tree-sitter searches all descendants of the scope node automatically.
            for (outer_root, outer_named, outer_name) in &outer_data {
                // An empty scope field means a top-level descendant: search the whole node.
                let descendant = scope_field.is_empty();
                let scope_node = if descendant {
                    *outer_root
                } else {
                    outer_root.child_by_field_name(scope_field).unwrap_or(*outer_root)
                };
                let mut cursor = QueryCursor::new();
                let mut matches =
                    cursor.matches(&inner_q, scope_node, source_bytes);

                while let Some(m) = matches.next() {
                    // A descendant is strictly below the head: the head matching its own
                    // inner pattern does not count.
                    if descendant && m.captures.iter().any(|c| c.node.id() == outer_root.id()) {
                        continue;
                    }
                    let inner_named: CapList = m
                        .captures
                        .iter()
                        .filter_map(|cap| {
                            let name = &inner_cap_names[cap.index as usize];
                            if !name.starts_with('_') {
                                Some((name.to_string(), cap.node))
                            } else {
                                None
                            }
                        })
                        .collect();

                    let mut parts: Vec<String> = Vec::new();
                    let mut user_nodes: Vec<(String, tree_sitter::Node)> = Vec::new();
                    for (name, node) in outer_named.iter().chain(inner_named.iter()) {
                        parts.push(fmt_cap(name, *node, body));
                        user_nodes.push((name.clone(), *node));
                    }

                    if parts.is_empty() {
                        if let Some(cap) = m.captures.first() {
                            let text = cap
                                .node
                                .utf8_text(source_bytes)
                                .unwrap_or("<invalid utf8>");
                            let display = if body {
                                text.to_string()
                            } else {
                                text.lines().next().unwrap_or("").to_string()
                            };
                            let node_text = if body {
                                format!("<match>:{}: {}", loc(cap.node), display)
                            } else {
                                format!("<match>:{}: {:?}", loc(cap.node), display)
                            };
                            names.resize(results.len(), String::new());
                            results.push(make_row(node_text, cap.node, &[]));
                            names.push(outer_name.clone());
                        }
                    } else {
                        // A descendant row sits on the inner match, not on the head.
                        let primary = if descendant {
                            inner_named
                                .first()
                                .map(|(_, n)| *n)
                                .or_else(|| m.captures.first().map(|c| c.node))
                                .unwrap_or(user_nodes[0].1)
                        } else {
                            user_nodes[0].1
                        };
                        names.resize(results.len(), String::new());
                        results.push(make_row(parts.join(", "), primary, &user_nodes));
                        names.push(outer_name.clone());
                    }
                }
            }
        }
    }
    names.resize(results.len(), String::new());

    let mut seen = std::collections::HashSet::new();
    let mut kept_names = Vec::new();
    let mut kept = Vec::new();
    for (r, n) in results.into_iter().zip(names) {
        let key = if by_name { (n.clone(), r.node_text.clone()) } else { (String::new(), r.node_text.clone()) };
        if seen.insert(key) {
            kept.push(r);
            kept_names.push(n);
        }
    }

    Ok((kept, kept_names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::lang;

    fn run_str(query: &str, lang_name: &str, source: &str) -> Vec<String> {
        let ast = crate::query::parse(query).unwrap();
        let l = lang::for_name(lang_name).unwrap();
        let compiled = crate::compiler::compile(&ast, l.as_ref());
        run(&compiled, l.as_ref(), source.as_bytes(), false).unwrap()
    }

    fn rows_for(query: &str, lang_name: &str, source: &str, body: bool) -> Vec<MatchRow> {
        let ast = crate::query::parse(query).unwrap();
        let l = lang::for_name(lang_name).unwrap();
        let compiled = crate::compiler::compile(&ast, l.as_ref());
        run_rows(&compiled, l.as_ref(), source.as_bytes(), body).unwrap()
    }

    #[test]
    fn a_match_carries_the_whole_source_line_it_sits_on() {
        let src = "package main\n\nconst itemCols = `id, kind`\n\nfunc f() { _ = itemCols }\n";
        let rows = rows_for("id#itemCols", "go", src, false);
        assert_eq!(rows.len(), 2, "{:?}", rows);
        assert_eq!(rows[0].line, 3);
        assert_eq!(
            rows[0].src_line.as_deref(),
            Some("const itemCols = `id, kind`"),
            "the source line, not the token: {:?}",
            rows[0]
        );
        assert_eq!(rows[1].src_line.as_deref(), Some("func f() { _ = itemCols }"));
    }

    #[test]
    fn the_node_text_form_survives_alongside_it() {
        let src = "package main\n\nconst itemCols = `id`\n";
        let rows = rows_for("id#itemCols", "go", src, false);
        assert_eq!(rows[0].node_text, "<match>:3: \"itemCols\"", "{:?}", rows[0]);
    }

    #[test]
    fn a_multi_line_body_match_keeps_the_node_text_form() {
        let src = "package main\n\ntype Meta struct {\n\tA string\n}\n";
        let rows = rows_for("*type#Meta", "go", src, true);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert!(rows[0].src_line.is_none(), "a body spanning lines has no one line: {:?}", rows[0]);
        assert!(rows[0].node_text.contains("A string"), "{:?}", rows[0]);
    }

    #[test]
    fn a_single_line_match_still_gets_a_source_line_under_body() {
        let src = "package main\n\ntype Meters = float64\n";
        let rows = rows_for("*type#Meters", "go", src, true);
        assert_eq!(rows[0].src_line.as_deref(), Some("type Meters = float64"), "{:?}", rows[0]);
    }

    #[test]
    fn two_captures_keep_the_node_text_form() {
        // Collapsing to one source line would drop the second capture's site.
        let src = "void DoStuff() {\n    Loading();\n}\n";
        let rows = rows_for("(*function @f).body((&function#Loading @c))", "cpp", src, false);
        assert!(!rows.is_empty(), "{:?}", rows);
        assert!(rows[0].src_line.is_none(), "{:?}", rows[0]);
        assert!(rows[0].node_text.contains("@f:"), "{:?}", rows[0]);
        assert!(rows[0].node_text.contains("@c:"), "{:?}", rows[0]);
    }

    #[test]
    fn a_ruby_method_query_with_body_prints_the_whole_definition() {
        let src = "class Job\n  def run\n    perform\n  end\nend\n";
        let rows = rows_for("*function#run", "ruby", src, true);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert_eq!(rows[0].line, 2);
        assert!(rows[0].node_text.contains("def run"), "{:?}", rows[0]);
        assert!(rows[0].node_text.contains("perform"), "the body comes with it: {:?}", rows[0]);
    }

    #[test]
    fn test_body_combinator_finds_call() {
        let src = "void DoStuff() {\n    int x = 1;\n    Loading();\n    x++;\n}\n";
        let results = run_str("(*function @f).body((&function#Loading @c))", "cpp", src);
        assert!(!results.is_empty(), "expected match, got: {:?}", results);
        assert!(results[0].contains("Loading"), "expected Loading in result: {:?}", results);
    }

    #[test]
    fn test_body_combinator_nested_call() {
        let src = "void Outer() {\n    if (x) {\n        Loading();\n    }\n}\n";
        let results = run_str("(*function @f).body((&function#Loading @c))", "cpp", src);
        assert!(!results.is_empty(), "expected match for nested call, got: {:?}", results);
    }

    #[test]
    fn test_body_combinator_condition_call() {
        // Loading() used in an if-condition, matching the actual Cossacks pattern
        let src = "void Init() {\n    if (!Loading()) {\n        return;\n    }\n}\n";
        let results = run_str("(*function @f).body((&function#Loading @c))", "cpp", src);
        assert!(!results.is_empty(), "expected match for condition call, got: {:?}", results);
    }

    #[test]
    fn test_condition_combinator() {
        // Find if statements whose condition contains a call to Check()
        let src = "void f() {\n    if (Check(x)) {\n        DoWork();\n    }\n}\n";
        // .condition() should find Check() in the condition but NOT DoWork() in the body
        let results = run_str("(if @s).condition((&function#Check @c))", "cpp", src);
        assert!(!results.is_empty(), "expected condition match, got: {:?}", results);
        assert!(results[0].contains("Check"), "expected Check: {:?}", results);

        // DoWork is in the body, not the condition — should NOT match with .condition()
        let neg = run_str("(if @s).condition((&function#DoWork @c))", "cpp", src);
        assert!(neg.is_empty(), "DoWork is in body not condition, got: {:?}", neg);
    }

    const DEEP_GO: &str = "package main\n\nfunc handle(a int) {\n\tif a > 0 {\n\t\tfor i := 0; i < a; i++ {\n\t\t\tname := i\n\t\t\t_ = name\n\t\t}\n\t}\n}\n\nfunc other() {\n\tname := 1\n\t_ = name\n}\n";

    #[test]
    fn a_top_level_descendant_finds_a_variable_three_blocks_deep() {
        let rows = rows_for("function#handle variable#name", "go", DEEP_GO, false);
        assert!(!rows.is_empty(), "{:?}", rows);
        assert!(rows.iter().any(|r| r.line == 6), "the declaration line: {:?}", rows);
        assert!(rows.iter().all(|r| r.line <= 7), "nothing from other(): {:?}", rows);
    }

    #[test]
    fn a_descendant_row_sits_on_the_inner_match_with_its_node_fields() {
        let rows = rows_for("function#handle variable#name", "go", DEEP_GO, false);
        let r = rows.iter().find(|r| r.line == 6).expect("row on line 6");
        assert!(r.end_byte > r.start_byte, "{:?}", r);
        let text = &DEEP_GO[r.start_byte..r.end_byte];
        assert!(text.contains("name"), "the byte range is the inner node: {:?} {:?}", text, r);
        assert!(!text.starts_with("func"), "not the function_declaration head: {:?}", r);
    }

    #[test]
    fn root_capture_beats_a_user_capture_elsewhere_in_the_head() {
        // The user capture @c sits on a call inside the head. Scoping must still
        // use the head node, so the identifier below the head is found.
        let src = "void f() { g(); h(); }\nvoid z() { h(); }\n";
        let rows = rows_for("(&function#g @c) id#g", "cpp", src, false);
        assert!(rows.is_empty() || rows.iter().all(|r| r.line == 1), "{:?}", rows);
        let src2 = "void f() { g(h()); }\n";
        let rows2 = rows_for("(&function#g @c) id#h", "cpp", src2, false);
        assert!(!rows2.is_empty(), "scoped to the g call node, which holds h: {:?}", rows2);
        assert!(rows2[0].node_text.contains("@c:"), "{:?}", rows2[0]);
        assert_eq!(&src2[rows2[0].start_byte..rows2[0].end_byte], "h", "the identifier: {:?}", rows2[0]);
    }

    #[test]
    fn the_head_matching_its_own_inner_pattern_is_not_a_descendant() {
        let src = "package main\n\nfunc a() {}\n";
        let rows = rows_for("function#a function#a", "go", src, false);
        assert!(rows.is_empty(), "{:?}", rows);
    }

    const ROUTES_TS: &str = "const routes = [\n  { path: \"/a\", component: A },\n  { component: B },\n  { path: \"/c\", title: \"x\", component: C },\n];\n";

    fn caps(r: &MatchRow) -> Vec<(&str, &str)> {
        r.captures.iter().map(|c| (c.name.as_str(), c.text.as_str())).collect()
    }

    #[test]
    fn sibling_pairs_match_inside_one_object_and_carry_both_captures() {
        let q = "(pair.key(property_identifier#path).value(string @key)) ~ (pair.key(property_identifier#component).value(@target))";
        let rows = rows_for(q, "typescript", ROUTES_TS, false);
        let got: Vec<_> = rows.iter().map(caps).collect();
        assert_eq!(
            got,
            vec![vec![("key", "\"/a\""), ("target", "A")], vec![("key", "\"/c\""), ("target", "C")]],
            "B has no path, so no match crosses objects: {:?}",
            rows
        );
        assert_eq!(rows[1].captures[0].line, 4);
        assert_eq!(&ROUTES_TS[rows[1].captures[1].start_byte..rows[1].captures[1].end_byte], "C");
    }

    #[test]
    fn adjacent_means_the_next_named_sibling() {
        let q = "(pair.key(property_identifier#path) @p) + (pair.key(property_identifier#component) @c)";
        let rows = rows_for(q, "typescript", ROUTES_TS, false);
        assert_eq!(rows.len(), 1, "/c has a title pair between: {:?}", rows);
        assert_eq!(rows[0].line, 2);
    }

    #[test]
    fn an_uncaptured_sibling_query_sits_on_the_later_sibling() {
        let rows = rows_for("pair.key(property_identifier#path) ~ pair.key(property_identifier#title)", "typescript", ROUTES_TS, false);
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert_eq!(&ROUTES_TS[rows[0].start_byte..rows[0].end_byte], "title: \"x\"");
    }

    #[test]
    fn one_capture_keeps_the_source_line_and_lists_its_capture() {
        let rows = rows_for("(id#itemCols @n)", "go", "package main\n\nconst itemCols = 1\n", false);
        assert_eq!(rows[0].src_line.as_deref(), Some("const itemCols = 1"));
        assert_eq!(caps(&rows[0]), vec![("n", "itemCols")]);
    }

    #[test]
    fn a_cached_run_matches_a_run_over_the_text_and_parses_once() {
        let d = std::env::temp_dir().join(format!("ctsq-runner-cached-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.go");
        let src = "package main\n\nconst itemCols = 1\n\nfunc f() { _ = itemCols }\n";
        std::fs::write(&f, src).unwrap();
        let path = f.to_str().unwrap();
        let ast = crate::query::parse("id#itemCols").unwrap();
        let l = lang::for_name("go").unwrap();
        let compiled = crate::compiler::compile(&ast, l.as_ref());
        let lines = |rows: Vec<MatchRow>| rows.into_iter().map(|r| (r.line, r.node_text)).collect::<Vec<_>>();
        let cached = run_rows_cached(&compiled, l.as_ref(), path, "go", false).unwrap();
        let again = run_rows_cached(&compiled, l.as_ref(), path, "go", false).unwrap();
        let plain = run_rows(&compiled, l.as_ref(), src.as_bytes(), false).unwrap();
        assert_eq!(lines(cached), lines(plain.clone()));
        assert_eq!(lines(again), lines(plain));
        let first = crate::parsed::get(path, "go").unwrap();
        assert!(std::rc::Rc::ptr_eq(&first, &crate::parsed::get(path, "go").unwrap()));
        assert!(run_rows_cached(&compiled, l.as_ref(), d.join("gone.go").to_str().unwrap(), "go", false).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn match_rows_carry_byte_range() {
        let src = "package main\n\nconst itemCols = 1\n";
        let rows = rows_for("id#itemCols", "go", src, false);
        assert_eq!(&src[rows[0].start_byte..rows[0].end_byte], "itemCols");
    }
}
