//! Data flow rows in the plan's Output format shape.

use super::{FlowResult, Hop};

struct Row {
    depth: usize,
    loc: String,
    func: String,
    text: String,
    tags: String,
}

fn rows(h: &Hop, depth: usize, out: &mut Vec<Row>) {
    out.push(Row {
        depth,
        loc: format!("{}:{}", h.file, h.line),
        func: h.func.clone(),
        text: h.text.clone(),
        tags: h.tags.iter().filter(|t| !(depth == 0 && *t == "source")).cloned().collect::<Vec<_>>().join(", "),
    });
    for c in &h.children {
        rows(c, depth + 1, out);
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// One row per hop: `path:line`, padded, function, trimmed source line, tags.
/// Children are indented two spaces under their parent. Each result is one
/// source group. A summary of sink and unmodelled endings follows.
pub fn report(results: &[FlowResult]) -> String {
    let mut all = vec![];
    for r in results {
        rows(&r.root, 0, &mut all);
    }
    let wl = all.iter().map(|r| r.loc.len()).max().unwrap_or(0) + 2;
    let wf = all.iter().map(|r| r.func.len()).max().unwrap_or(0) + 2;
    let wt = all.iter().map(|r| r.text.len()).max().unwrap_or(0) + 2;
    let mut s = String::new();
    for r in &all {
        let line = format!("{}{:<wl$}{:<wf$}{:<wt$}{}", "  ".repeat(r.depth), r.loc, r.func, r.text, r.tags);
        s.push_str(line.trim_end());
        s.push('\n');
    }
    let sinks: usize = results.iter().map(|r| r.sinks()).sum();
    if sinks == 0 {
        s.push_str("no route to sink\n");
    } else {
        s.push_str(&format!("{} to sink\n", plural(sinks, "route", "routes")));
    }
    let ends: Vec<&Hop> = results.iter().flat_map(|r| r.unmodelled_ends()).collect();
    if !ends.is_empty() {
        let list: Vec<String> = ends
            .iter()
            .map(|h| format!("{}:{} {}", h.file, h.line, h.unmodelled.as_deref().unwrap_or("")))
            .collect();
        let verb = if ends.len() == 1 { "route ends" } else { "routes end" };
        s.push_str(&format!("{} {} at an unmodelled call: {}\n", ends.len(), verb, list.join(", ")));
    }
    s
}
