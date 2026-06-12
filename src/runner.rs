use crate::compiler::lang::Lang;
use crate::compiler::CompiledQuery;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

pub fn run(
    compiled: &CompiledQuery,
    lang: &dyn Lang,
    source: &[u8],
    body: bool,
) -> Result<Vec<String>, String> {
    if compiled.is_empty() {
        return Ok(vec![]);
    }

    let ts_lang = lang.ts_language();

    let mut parser = Parser::new();
    parser.set_language(&ts_lang).map_err(|e| e.to_string())?;

    let tree = parser.parse(source, None).ok_or("failed to parse source")?;

    let source_bytes = source;
    let mut results = Vec::new();

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
            let query =
                Query::new(&ts_lang, ts_query).map_err(|e| format!("query error: {e}"))?;
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(&query, tree.root_node(), source_bytes);

            let capture_names = query.capture_names();

            while let Some(m) = matches.next() {
                let mut parts: Vec<String> = Vec::new();
                for capture in m.captures {
                    let name = &capture_names[capture.index as usize];
                    if name.starts_with('_') {
                        continue;
                    }
                    parts.push(fmt_cap(name, capture.node, body));
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
                        if body {
                            results.push(format!("<match>:{}: {}", loc(cap.node), display));
                        } else {
                            results.push(format!("<match>:{}: {:?}", loc(cap.node), display));
                        }
                    } else {
                        results.push("<match>".into());
                    }
                } else {
                    results.push(parts.join(", "));
                }
            }
        }

        CompiledQuery::Scoped { outer, scope_field, inner } => {
            let outer_q =
                Query::new(&ts_lang, outer).map_err(|e| format!("outer query error: {e}"))?;
            let inner_q =
                Query::new(&ts_lang, inner).map_err(|e| format!("inner query error: {e}"))?;

            let outer_cap_names = outer_q.capture_names();
            let inner_cap_names = inner_q.capture_names();

            // Phase 1: collect outer match nodes and their user-visible captures.
            // We store (root_node, named_caps) for each outer match.
            // root_node is the container we'll search within — the first non-internal capture,
            // falling back to the first _root capture, falling back to the first capture overall.
            type CapList<'t> = Vec<(String, tree_sitter::Node<'t>)>;
            let mut outer_data: Vec<(tree_sitter::Node, CapList)> = Vec::new();
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

                    let root = m
                        .captures
                        .iter()
                        .find(|cap| {
                            !outer_cap_names[cap.index as usize].starts_with('_')
                        })
                        .or_else(|| {
                            m.captures.iter().find(|cap| {
                                outer_cap_names[cap.index as usize].starts_with("_root")
                            })
                        })
                        .or_else(|| m.captures.first())
                        .map(|cap| cap.node);

                    if let Some(root_node) = root {
                        outer_data.push((root_node, named));
                    }
                }
            }

            // Phase 2: for each outer node, navigate to scope_field then run inner query.
            // tree-sitter searches all descendants of the scope node automatically.
            for (outer_root, outer_named) in &outer_data {
                let scope_node = outer_root
                    .child_by_field_name(scope_field)
                    .unwrap_or(*outer_root);
                let mut cursor = QueryCursor::new();
                let mut matches =
                    cursor.matches(&inner_q, scope_node, source_bytes);

                while let Some(m) = matches.next() {
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
                    for (name, node) in outer_named.iter().chain(inner_named.iter()) {
                        parts.push(fmt_cap(name, *node, body));
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
                            if body {
                                results
                                    .push(format!("<match>:{}: {}", loc(cap.node), display));
                            } else {
                                results.push(format!(
                                    "<match>:{}: {:?}",
                                    loc(cap.node),
                                    display
                                ));
                            }
                        }
                    } else {
                        results.push(parts.join(", "));
                    }
                }
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    results.retain(|r| seen.insert(r.clone()));

    Ok(results)
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
}
