//! The tree printer for `ctsq path`. Every row starts `path:line`, levels
//! indent by two spaces and tags are comma-separated. `row` is the one place
//! a row is laid out, so call routes and flow trees print alike.

use crate::resolve::{Confidence, TreeStore};
use crate::semantic::calls::{CallRoutes, Hop, Route};

/// Width the `path:line` column is padded to.
const LOC_WIDTH: usize = 24;
/// Width the function name column is padded to.
const FUNC_WIDTH: usize = 14;

#[derive(Debug, Clone, Default)]
pub struct PrintOpts {
    /// Print each hop's definition body under its row, one level deeper.
    pub body: bool,
}

/// One output row without indentation: `path:line`, padded, the function
/// name, a trimmed source line or note, then the tags comma-separated.
/// Empty parts are left out and trailing spaces are never printed.
pub fn row(path: &str, line: usize, func: &str, text: &str, tags: &[String]) -> String {
    let mut out = format!("{:<w$}", format!("{}:{}", path, line), w = LOC_WIDTH);
    let tail = !text.is_empty() || !tags.is_empty();
    if tail || !func.is_empty() {
        out.push(' ');
        if tail {
            out.push_str(&format!("{:<w$}", func, w = FUNC_WIDTH));
        } else {
            out.push_str(func);
        }
    }
    if !text.is_empty() {
        out.push(' ');
        out.push_str(text);
    }
    if !tags.is_empty() {
        out.push_str("  ");
        out.push_str(&tags.join(", "));
    }
    out.trim_end().to_string()
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn display_name(h: &Hop) -> String {
    match &h.owner {
        Some(o) => format!("{}.{}", o, h.name),
        None => h.name.clone(),
    }
}

/// A node of the merged tree: hops that share every earlier hop share a row.
struct Node {
    hop: Hop,
    /// For each child, the text naming it from this row.
    links: Vec<String>,
    children: Vec<Node>,
}

fn guess(h: &Hop) -> &'static str {
    if h.confidence == Confidence::Guess { "?" } else { "" }
}

/// Hops in reading order, each paired with the link text that names its
/// neighbour on the next row. Downward: `calls <next> at :<line>`. Upward the
/// next row is the caller, so `called by <caller> at :<line>`.
fn display_hops(route: &Route, upward: bool) -> Vec<(Hop, Option<String>)> {
    let hops = &route.hops;
    let n = hops.len();
    if !upward {
        (0..n)
            .map(|i| {
                let link = hops.get(i + 1).map(|next| {
                    let at = hops[i].call_line.map(|l| format!(" at :{}", l)).unwrap_or_default();
                    format!("calls {}{}{}", display_name(next), guess(next), at)
                });
                (hops[i].clone(), link)
            })
            .collect()
    } else {
        (0..n)
            .rev()
            .map(|i| {
                let link = if i > 0 {
                    let caller = &hops[i - 1];
                    let at = caller.call_line.map(|l| format!(" at :{}", l)).unwrap_or_default();
                    Some(format!("called by {}{}{}", display_name(caller), guess(&hops[i]), at))
                } else {
                    None
                };
                (hops[i].clone(), link)
            })
            .collect()
    }
}

fn insert(nodes: &mut Vec<Node>, path: &[(Hop, Option<String>)]) {
    let Some(((hop, link), rest)) = path.split_first() else { return };
    let at = match nodes.iter().position(|n| n.hop.def == hop.def) {
        Some(i) => i,
        None => {
            nodes.push(Node { hop: hop.clone(), links: vec![], children: vec![] });
            nodes.len() - 1
        }
    };
    if let Some(l) = link {
        if !nodes[at].links.contains(l) {
            nodes[at].links.push(l.clone());
        }
    }
    insert(&mut nodes[at].children, rest);
}

fn body_lines(store: &TreeStore, hop: &Hop) -> Vec<String> {
    let Some(pf) = store.get(&hop.file) else { return vec![] };
    let Some(tree) = pf.tree.as_ref() else { return vec![] };
    let row_ix = hop.line.saturating_sub(1);
    // Outermost node that starts on the definition line and is not the file.
    let mut cur = tree.root_node();
    let mut found = None;
    'walk: loop {
        let mut c = cur.walk();
        for child in cur.children(&mut c) {
            if child.start_position().row == row_ix && child.is_named() {
                found = Some(child);
                break 'walk;
            }
            if child.start_position().row <= row_ix && child.end_position().row >= row_ix {
                cur = child;
                continue 'walk;
            }
        }
        break;
    }
    match found {
        Some(n) => String::from_utf8_lossy(&pf.source[n.start_byte()..n.end_byte()])
            .lines()
            .map(|l| l.trim_end().to_string())
            .collect(),
        None => vec![],
    }
}

fn emit(nodes: &[Node], depth: usize, store: &TreeStore, opts: &PrintOpts, out: &mut String) {
    for n in nodes {
        let text = n.links.join(", ");
        out.push_str(&indent(depth));
        out.push_str(&row(&n.hop.file, n.hop.line, &display_name(&n.hop), &text, &[]));
        out.push('\n');
        if opts.body {
            for l in body_lines(store, &n.hop) {
                if l.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&indent(depth + 1));
                    out.push_str(&l);
                    out.push('\n');
                }
            }
        }
        emit(&n.children, depth + 1, store, opts, out);
    }
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("{} {}", n, word) } else { format!("{} {}s", n, word) }
}

/// Print every route as an indented tree, one tree per route start, then the
/// trailer. Upward queries print in reading order, the first selector's
/// definition first.
pub fn print_call_routes(routes: &CallRoutes, store: &TreeStore, opts: &PrintOpts) -> String {
    let mut out = String::new();
    let mut roots: Vec<(usize, Vec<Node>)> = Vec::new();
    for r in &routes.routes {
        let path = display_hops(r, routes.upward);
        let at = match roots.iter().position(|(s, _)| *s == r.start) {
            Some(i) => i,
            None => {
                roots.push((r.start, vec![]));
                roots.len() - 1
            }
        };
        insert(&mut roots[at].1, &path);
    }
    for (_, nodes) in &roots {
        emit(nodes, 0, store, opts, &mut out);
    }
    out.push_str(&plural(routes.routes.len(), "route"));
    out.push('\n');
    if routes.routes.is_empty() {
        for (i, m) in routes.segment_matches.iter().enumerate() {
            if *m == 0 {
                out.push_str(&format!("note: selector {} matched no definition\n", i + 1));
            }
        }
    }
    if routes.excluded > 0 {
        out.push_str(&format!("note: {} excluded by :not\n", plural(routes.excluded, "definition")));
    }
    if routes.cut > 0 {
        out.push_str(&format!("{} cut at --max-depth {}\n", plural(routes.cut, "route"), routes.max));
    }
    if routes.truncated {
        out.push_str("note: the search stopped at its limit, some routes may be missing\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_pads_the_location_and_function_and_drops_trailing_space() {
        assert_eq!(row("a.go", 3, "f", "", &[]), format!("{:<24} f", "a.go:3"));
        let r = row("a.go", 3, "f", "x := 1", &["sink".to_string(), "exact".to_string()]);
        assert_eq!(r, format!("{:<24} {:<14} x := 1  sink, exact", "a.go:3", "f"));
        assert_eq!(row("a.go", 3, "", "", &[]), format!("a.go:3"));
    }

    #[test]
    fn plural_counts() {
        assert_eq!(plural(1, "route"), "1 route");
        assert_eq!(plural(0, "route"), "0 routes");
    }
}
