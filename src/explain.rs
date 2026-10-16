//! `explain` answers "what is this symbol and how is it used" in one call.
//!
//! Nothing here is new analysis. It composes the parts that already exist:
//! `def` resolution for the definition sites, the call graph behind `callers`
//! and `callees` for the surrounding calls, the outline behind `tree` when a
//! name is a class or a variable rather than a function, and an `id#Symbol`
//! query for everything left over. Those parts keep their own behaviour.

use crate::calltree::short_path;
use crate::footer::{footer, Hidden};
use crate::compiler::{self, lang};
use crate::files;
use crate::resolve::graph::{self, CallGraph, NodeKey};
use crate::resolve::{resolve_def_query, Confidence, DefKind};
use crate::{query, runner, tree};
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct Options {
    /// How many levels of callers and callees to walk.
    pub depth: usize,
    /// Maximum body lines printed per definition before truncation.
    pub max_lines: usize,
    /// Print only the signature line, never the body.
    pub no_body: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { depth: 1, max_lines: 80, no_body: false }
    }
}

pub fn run(symbol: &str, path: &str, lang_name: Option<&str>, opts: &Options, no_ignore: bool, hide: &files::HideOpts) {
    let jobs = files::resolve_jobs(path, lang_name, no_ignore);
    let mut hider = files::Hider::new(hide);
    hider.add_jobs(path, &jobs);
    print!("{}", report_hiding(symbol, &jobs, opts, &mut hider));
}

// ── Report ───────────────────────────────────────────────────────────────────

/// Build the whole report as text, hiding nothing. Split out from `run` so
/// tests can assert on it without capturing stdout.
#[cfg(test)]
pub fn report(symbol: &str, jobs: &[(String, &'static str)], opts: &Options) -> String {
    report_hiding(symbol, jobs, opts, &mut files::Hider::none())
}

/// `report` with the definitions, callers and references `hider` hides left
/// out, and the footer counting them at the end. The call graph is still built
/// over every file, so hiding a test never changes how a call resolves.
pub fn report_hiding(
    symbol: &str,
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
) -> String {
    let mut hidden = Hidden::default();
    let mut out = body(symbol, jobs, opts, hider, &mut hidden);
    let shown = out.matches("\nDEFINITION ").count();
    out.push_str(&footer(shown, "definition", &hidden));
    out
}

fn body(
    symbol: &str,
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
    hidden: &mut Hidden,
) -> String {
    let mut out = String::new();
    let mut lines = LineCache::default();

    let g = graph::build(jobs);
    let sites = hider.keep(definition_sites(symbol, &g, jobs), |s| (s.file.clone(), s.line), hidden);

    if sites.is_empty() {
        out.push_str(&format!("SYMBOL {} (no definition resolved)\n", symbol));
        push_reference_list(&mut out, symbol, jobs, opts, hider, hidden);
        return out;
    }

    let plural = if sites.len() == 1 { "definition" } else { "definitions" };
    out.push_str(&format!("SYMBOL {} ({} {})\n", symbol, sites.len(), plural));

    // Every file:line the report has already shown, so REFERENCES can report
    // only what is left.
    let mut listed: HashSet<(String, usize)> = HashSet::new();

    for site in &sites {
        listed.insert((site.file.clone(), site.line));
        out.push('\n');
        let mark = if site.guess { "  ?" } else { "" };
        out.push_str(&format!(
            "DEFINITION {}:{}  {}{}\n",
            site.file, site.line, site.kind, mark
        ));

        let body = definition_text(&site.file, site.lang, site.line);
        match body {
            Some(text) if !opts.no_body => {
                let body_lines: Vec<&str> = text.lines().collect();
                if body_lines.len() > opts.max_lines {
                    // The kept lines start at the signature, so the interface
                    // stays visible without repeating it. A body cut to
                    // nothing gets an explicit signature line instead.
                    if opts.max_lines == 0 {
                        out.push_str(&format!("sig: {}\n", body_lines[0].trim_end()));
                    }
                    for l in &body_lines[..opts.max_lines] {
                        out.push_str(l);
                        out.push('\n');
                    }
                    out.push_str(&format!(
                        "... ({} more lines)\n",
                        body_lines.len() - opts.max_lines
                    ));
                } else {
                    out.push_str(&text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            Some(text) => {
                out.push_str(&format!(
                    "sig: {}\n",
                    text.lines().next().unwrap_or("").trim_end()
                ));
            }
            None => {
                out.push_str(&format!("sig: {}\n", lines.text(&site.file, site.line)));
            }
        }
    }

    let keys: Vec<NodeKey> = sites
        .iter()
        .filter_map(|s| s.key.clone())
        .collect();

    // Names the resolver could not pin to one definition sit in the graph
    // under their bare name, so callers of an ambiguous symbol are still
    // reachable. Only used when the definition-keyed lookup found nothing.
    let mut caller_keys = keys.clone();
    caller_keys.push(NodeKey::External(bare_name(symbol).to_string()));

    {
        let mut ctx = WalkCtx { lines: &mut lines, listed: &mut listed, hider: &mut *hider, hidden: &mut *hidden };
        push_walk(&mut out, "CALLERS", Direction::Up, &g, &caller_keys, opts, &mut ctx);
        push_walk(&mut out, "CALLEES", Direction::Down, &g, &keys, opts, &mut ctx);
    }

    let remaining: Vec<_> = references(symbol, jobs)
        .into_iter()
        .filter(|(file, line, _)| !listed.contains(&(file.clone(), *line)))
        .collect();
    let remaining = hider.keep(remaining, |(f, l, _)| (f.clone(), *l), hidden);
    let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for (file, _, _) in remaining {
        *by_file.entry(file).or_default() += 1;
        total += 1;
    }
    out.push_str(&format!("\nREFERENCES ({} more)\n", total));
    for (file, count) in by_file {
        out.push_str(&format!("  {}: {}\n", file, count));
    }

    out
}

// ── Definition sites ─────────────────────────────────────────────────────────

struct Site {
    file: String,
    line: usize,
    lang: &'static str,
    kind: String,
    guess: bool,
    /// Call graph identity, when the symbol is a function the graph knows.
    key: Option<NodeKey>,
}

/// Resolve the symbol to definition sites.
///
/// The call-graph index that `def` uses comes first. It only holds callable
/// definitions, so a class of the same name is merged in from the `tree`
/// outline, and when the index finds nothing at all the outline supplies
/// every kind of definition it knows.
fn definition_sites(symbol: &str, g: &CallGraph, jobs: &[(String, &'static str)]) -> Vec<Site> {
    let lang_of: HashMap<&str, &'static str> =
        jobs.iter().map(|(f, l)| (f.as_str(), *l)).collect();
    let bare = bare_name(symbol);

    let mut sites: Vec<Site> = resolve_def_query(symbol, &g.index)
        .into_iter()
        .map(|(i, conf)| {
            let d = &g.index.defs[i];
            Site {
                file: d.id.file.clone(),
                line: d.id.line,
                lang: lang_of.get(d.id.file.as_str()).copied().unwrap_or("c"),
                kind: kind_label(&d.kind),
                guess: conf == Confidence::Guess,
                key: Some(NodeKey::Def(d.id.clone())),
            }
        })
        .collect();

    let index_resolved = !sites.is_empty();
    // A class or struct is invisible to the call-graph index, so take it from
    // the outline. When the index resolved nothing, take any kind.
    for (file, lang_name) in jobs {
        for (kind, name, line) in tree::collect_def_locs(file, lang_name) {
            if name != bare {
                continue;
            }
            if index_resolved && kind != "class" {
                continue;
            }
            if sites.iter().any(|s| s.file == *file && s.line == line) {
                continue;
            }
            sites.push(Site {
                file: file.clone(),
                line,
                lang: lang_name,
                kind: kind.to_string(),
                guess: false,
                key: None,
            });
        }
    }

    sites.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    if !index_resolved && sites.len() > 1 {
        for s in sites.iter_mut() {
            s.guess = true;
        }
    }
    sites
}

fn kind_label(kind: &DefKind) -> String {
    match kind {
        DefKind::Function => "function".to_string(),
        DefKind::Method { receiver, .. } => format!("method on {}", receiver),
        DefKind::InterfaceMethod { interface } => format!("interface method on {}", interface),
    }
}

/// Last segment of a qualified name: `Foo::bar` and `pkg.Fn` both reduce to
/// the identifier an `id#` query can match.
pub fn bare_name(symbol: &str) -> &str {
    let after_path = symbol.rsplit("::").next().unwrap_or(symbol);
    after_path.rsplit('.').next().unwrap_or(after_path)
}

/// Full source text of the definition node at `file:line`.
///
/// The node wanted is the outermost one that starts on that line, which is the
/// whole `fn`, `class` or declaration rather than its name. Descend from the
/// root until a child starts on the line, preferring the widest such child.
fn definition_text(file: &str, lang_name: &str, line: usize) -> Option<String> {
    let source = std::fs::read(file).ok()?;
    let lang_box = lang::for_name(lang_name)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang_box.ts_language()).ok()?;
    let parsed = parser.parse(&source, None)?;

    let row = line.checked_sub(1)?;
    let mut node = parsed.root_node();
    loop {
        let mut widest: Option<tree_sitter::Node> = None;
        let mut containing: Option<tree_sitter::Node> = None;
        {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.start_position().row == row {
                    if widest.map_or(true, |w| child.end_byte() > w.end_byte()) {
                        widest = Some(child);
                    }
                } else if child.start_position().row < row && child.end_position().row >= row {
                    containing = Some(child);
                }
            }
        }
        if let Some(w) = widest {
            return w.utf8_text(&source).ok().map(|s| s.to_string());
        }
        node = containing?;
    }
}

// ── Callers and callees ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Up,
    Down,
}

/// What a walk carries down: the source line cache, the file:line pairs shown
/// so far, and what is hidden.
struct WalkCtx<'a> {
    lines: &'a mut LineCache,
    listed: &'a mut HashSet<(String, usize)>,
    hider: &'a mut files::Hider,
    hidden: &'a mut Hidden,
}

fn push_walk(
    out: &mut String,
    heading: &str,
    dir: Direction,
    g: &CallGraph,
    roots: &[NodeKey],
    opts: &Options,
    ctx: &mut WalkCtx,
) {
    let levels = if opts.depth == 1 { "1 level".to_string() } else { format!("{} levels", opts.depth) };
    out.push_str(&format!("\n{} ({})\n", heading, levels));

    let before = out.len();
    for root in roots {
        if matches!(root, NodeKey::External(_)) && out.len() != before {
            // The bare-name node is a last resort, not an addition.
            continue;
        }
        let mut visited: HashSet<NodeKey> = HashSet::new();
        visited.insert(root.clone());
        walk(out, dir, g, root, 0, opts.depth, 1, &mut visited, ctx);
    }
    if out.len() == before {
        out.push_str("  (none)\n");
    }
}

/// One row per edge: where the call is written, the function on the other end
/// of it, where that function is defined, and the source line of the call.
#[allow(clippy::too_many_arguments)]
fn walk(
    out: &mut String,
    dir: Direction,
    g: &CallGraph,
    key: &NodeKey,
    depth: usize,
    max_depth: usize,
    indent: usize,
    visited: &mut HashSet<NodeKey>,
    ctx: &mut WalkCtx,
) {
    if depth >= max_depth {
        return;
    }
    let edges = match dir {
        Direction::Up => g.rev.get(key),
        Direction::Down => g.fwd.get(key),
    };
    let Some(edges) = edges else { return };

    // (site_file, site_line, other_name, other_loc, guess, next_key)
    type Row = (String, usize, String, Option<(String, usize)>, bool, Option<NodeKey>);
    let mut rows: Vec<Row> = Vec::new();
    for &e in edges {
        let rc = &g.calls[e];
        let caller = &g.index.defs[rc.caller];
        let guess = matches!(rc.target, Some((_, Confidence::Guess)));
        match dir {
            Direction::Up => rows.push((
                caller.id.file.clone(),
                rc.line,
                caller.name.clone(),
                Some((caller.id.file.clone(), caller.id.line)),
                guess,
                Some(NodeKey::Def(caller.id.clone())),
            )),
            Direction::Down => {
                let target = rc.target.map(|(t, _)| &g.index.defs[t]);
                rows.push((
                    caller.id.file.clone(),
                    rc.line,
                    rc.callee_display.clone(),
                    target.map(|d| (d.id.file.clone(), d.id.line)),
                    guess,
                    target.map(|d| NodeKey::Def(d.id.clone())),
                ));
            }
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    rows.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
    // A call written in a test is hidden with everything above it.
    let rows = ctx.hider.keep(rows, |r| (r.0.clone(), r.1), ctx.hidden);

    for (site_file, site_line, name, loc, guess, next) in rows {
        ctx.listed.insert((site_file.clone(), site_line));
        let where_defined = match &loc {
            Some((f, l)) => {
                ctx.listed.insert((f.clone(), *l));
                format!("[{}:{}]", short_path(f), l)
            }
            None => "[external]".to_string(),
        };
        let cycle = next.as_ref().map_or(false, |k| visited.contains(k));
        let text = ctx.lines.text(&site_file, site_line);
        out.push_str(&format!(
            "{}{}:{}  {}()  {}{}{}  {}\n",
            "  ".repeat(indent),
            site_file,
            site_line,
            name,
            where_defined,
            if guess { " ?" } else { "" },
            if cycle { " (cycle)" } else { "" },
            text
        ));
        if let Some(next_key) = next {
            if !cycle {
                visited.insert(next_key.clone());
                walk(out, dir, g, &next_key, depth + 1, max_depth, indent + 1, visited, ctx);
                visited.remove(&next_key);
            }
        }
    }
}

// ── References ───────────────────────────────────────────────────────────────

/// Every `id#Symbol` match in the search set, as (file, line, source line).
fn references(symbol: &str, jobs: &[(String, &'static str)]) -> Vec<(String, usize, String)> {
    let query_str = format!("(id#\"{}\" @r)", bare_name(symbol));
    let Ok(ast) = query::parse(&query_str) else { return vec![] };

    let mut cache: HashMap<&'static str, (Box<dyn lang::Lang>, compiler::CompiledQuery)> =
        HashMap::new();
    let mut lines = LineCache::default();
    let mut out = Vec::new();

    for (file, lang_name) in jobs {
        let (lang_box, compiled) = cache.entry(lang_name).or_insert_with(|| {
            let l = lang::for_name(lang_name).expect("lang_name always valid here");
            let c = compiler::compile(&ast, l.as_ref());
            (l, c)
        });
        let Ok(source) = std::fs::read(file) else { continue };
        let Ok(matches) = runner::run(compiled, lang_box.as_ref(), &source, false) else { continue };
        for m in matches {
            if let Some(line) = crate::resolve::fallback::parse_line("r", &m) {
                out.push((file.clone(), line, lines.text(file, line)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn push_reference_list(
    out: &mut String,
    symbol: &str,
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
    hidden: &mut Hidden,
) {
    let refs = hider.keep(references(symbol, jobs), |(f, l, _)| (f.clone(), *l), hidden);
    out.push_str(&format!("\nREFERENCES ({})\n", refs.len()));
    for (file, line, text) in refs.iter().take(opts.max_lines) {
        out.push_str(&format!("  {}:{}  {}\n", file, line, text));
    }
    if refs.len() > opts.max_lines {
        out.push_str(&format!("  ... ({} more)\n", refs.len() - opts.max_lines));
    }
}

// ── Source line lookup ───────────────────────────────────────────────────────

#[derive(Default)]
struct LineCache {
    files: HashMap<String, Vec<String>>,
}

impl LineCache {
    fn text(&mut self, file: &str, line: usize) -> String {
        let entry = self.files.entry(file.to_string()).or_insert_with(|| {
            std::fs::read_to_string(file)
                .map(|s| s.lines().map(|l| l.to_string()).collect())
                .unwrap_or_default()
        });
        entry
            .get(line.saturating_sub(1))
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures(sub: &str) -> String {
        format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), sub)
    }

    fn jobs_for(sub: &str) -> Vec<(String, &'static str)> {
        files::resolve_jobs(&fixtures(sub), None, false)
    }

    fn explain(symbol: &str, sub: &str) -> String {
        report(symbol, &jobs_for(sub), &Options::default())
    }

    #[test]
    fn definition_body_is_printed_in_full() {
        let r = explain("NewServer", "example.go");
        assert!(r.contains("DEFINITION"), "expected a definition section: {}", r);
        assert!(r.contains("func NewServer(host string, port int) *Server {"), "{}", r);
        assert!(r.contains("return &Server{host: host, port: port}"), "body line missing: {}", r);
    }

    #[test]
    fn callers_show_site_enclosing_function_and_call_line() {
        let r = explain("NewServer", "example.go");
        let callers = r.split("CALLERS").nth(1).unwrap_or("");
        assert!(callers.contains("example.go:19"), "call site line missing: {}", r);
        assert!(callers.contains("main()"), "enclosing function missing: {}", r);
        assert!(callers.contains("NewServer(\"localhost\", 8080)"), "call line missing: {}", r);
    }

    #[test]
    fn callees_are_listed_with_their_definition_site() {
        let r = explain("main", "example.go");
        let callees = r.split("CALLEES").nth(1).unwrap_or("");
        assert!(callees.contains("NewServer()"), "expected NewServer callee: {}", r);
        assert!(callees.contains("[example.go:14]"), "expected callee def site: {}", r);
    }

    #[test]
    fn body_truncates_at_max_lines_and_keeps_the_signature() {
        let opts = Options { depth: 1, max_lines: 1, no_body: false };
        let r = report("NewServer", &jobs_for("example.go"), &opts);
        assert!(
            r.contains("func NewServer(host string, port int) *Server {"),
            "signature must survive truncation: {}",
            r
        );
        assert!(r.contains("... (2 more lines)"), "expected truncation marker: {}", r);
        assert!(!r.contains("return &Server"), "truncated body must stop: {}", r);
    }

    #[test]
    fn ambiguous_name_still_finds_its_callers() {
        // process is defined in two fixture files, so the resolver leaves the
        // call sites unresolved. explain still reports them.
        let r = report("process", &jobs_for("."), &Options::default());
        let callers = r.split("CALLERS").nth(1).unwrap_or("");
        assert!(callers.contains("example.c:11"), "expected the C call site: {}", r);
        assert!(callers.contains("process(64);"), "expected the call line: {}", r);
    }

    #[test]
    fn no_body_prints_only_the_signature() {
        let opts = Options { depth: 1, max_lines: 80, no_body: true };
        let r = report("NewServer", &jobs_for("example.go"), &opts);
        assert!(r.contains("sig: func NewServer("), "{}", r);
        assert!(!r.contains("return &Server"), "body must be suppressed: {}", r);
    }

    #[test]
    fn arrow_function_definition_resolves_through_its_declaration() {
        let r = explain("helper", "example.js");
        assert!(r.contains("DEFINITION"), "{}", r);
        assert!(r.contains("const helper = () => {"), "{}", r);
        assert!(r.contains("return 42;"), "arrow body missing: {}", r);
    }

    #[test]
    fn class_resolves_through_the_tree_outline() {
        // Buffer is a class, so the call-graph index does not hold it. The
        // outline fallback should still find it and print the class body.
        let r = explain("Buffer", "example.cpp");
        assert!(r.contains("DEFINITION"), "{}", r);
        assert!(r.contains("class"), "expected the class kind label: {}", r);
        assert!(r.contains("void process(int ARRAY_SIZE) {"), "class body missing: {}", r);
    }

    #[test]
    fn unresolved_symbol_falls_back_to_id_matches() {
        let r = explain("malloc", "example.c");
        assert!(r.contains("no definition resolved"), "{}", r);
        assert!(r.contains("REFERENCES (1)"), "{}", r);
        assert!(r.contains("example.c:6"), "expected the malloc call site: {}", r);
    }

    #[test]
    fn missing_symbol_reports_nothing_rather_than_failing() {
        let r = explain("no_such_symbol_anywhere", "example.c");
        assert!(r.contains("no definition resolved"), "{}", r);
        assert!(r.contains("REFERENCES (0)"), "{}", r);
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        let a = explain("main", ".");
        let b = explain("main", ".");
        assert_eq!(a, b);
    }

    #[test]
    fn remaining_references_are_counted_per_file() {
        let r = explain("main", ".");
        let refs = r.split("REFERENCES").nth(1).unwrap_or("");
        assert!(refs.starts_with(" ("), "expected a count in the heading: {}", r);
    }
}
