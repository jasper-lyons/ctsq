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
use crate::resolve::graph::{CallGraph, NodeKey};
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

pub fn run(names: &[String], where_: &str, jobs: &[(String, &'static str)], mut hider: files::Hider, opts: &Options) {
    let r = report_many(names, where_, jobs, opts, &mut hider);
    print!("{}", r.out);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprint!("{}", r.err);
    if r.code != 0 {
        // Nothing resolved, or everything was hidden: exit 1 like -q, def and refs.
        std::process::exit(r.code);
    }
}

/// `report_hiding` for several names, in order, a blank line between. With one
/// name the text is exactly `report_hiding`'s. A name that resolved to nothing
/// prints no block on stdout and goes on the `not found:` stderr line, and when
/// it is the one name asked for its `SYMBOL X (no definition resolved)` block
/// goes to stderr too. When nothing was shown because everything was hidden,
/// stdout is empty and stderr says so (BUILD.md A20). The footer counts every
/// name, once, at the end. The code is 0 when any name showed a definition or a
/// reference.
pub fn report_many(
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
) -> crate::resolve::NamesOut {
    let mut hidden = Hidden::default();
    let (g, facts) = gather(names, jobs, opts, hider);
    let mut out = String::new();
    let mut miss_text = String::new();
    let mut missing: Vec<String> = Vec::new();
    let mut any = false;
    for symbol in names {
        let before = hidden.tests + hidden.excluded;
        let (text, found) = body(symbol, jobs, &g, &facts[bare_name(symbol)], opts, hider, &mut hidden);
        if found {
            any = true;
        } else if hidden.tests + hidden.excluded == before {
            missing.push(symbol.clone());
            if names.len() == 1 {
                miss_text = text;
            }
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&text);
    }
    crate::footer::settle(&mut hidden);
    let mut err = miss_text;
    if !any && hidden.any() {
        out.clear();
        err.push_str(&crate::footer::nothing_shown(
            &format!("no definition to {}", names.join(", ")),
            where_,
            jobs.len(),
            &hidden,
        ));
    } else if any {
        let shown = out.matches("\nDEFINITION ").count();
        out.push_str(&footer(shown, "definition", &hidden));
    }
    err.push_str(&crate::resolve::not_found_line(&missing, jobs));
    crate::resolve::NamesOut { out, err, code: if any { 0 } else { 1 } }
}

/// What the files holding a name's bare form say about it.
#[derive(Default)]
struct Facts {
    /// (file, lang, kind, line) of each outline entry named the bare name, in
    /// job order then outline order.
    outline: Vec<(String, &'static str, &'static str, usize)>,
    /// Every `id#bare` match as (file, line, source line), sorted.
    refs: Vec<(String, usize, String)>,
}

/// The graph the walks of every name read, built from the files they need
/// (callers and callees `opts.depth` levels from each name's definitions),
/// and the facts of each bare name. One pass reads every file holding a bare
/// name, for its outline and references, and hands the same parse to the
/// graph builder, so no file is parsed twice for them.
fn gather(
    names: &[String],
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
) -> (CallGraph, HashMap<String, Facts>) {
    use crate::resolve::scoped;
    let mut seeds: Vec<String> = Vec::new();
    for s in names {
        for n in [bare_name(s), query_name(s)] {
            if !seeds.iter().any(|x| x == n) {
                seeds.push(n.to_string());
            }
        }
    }
    let seeds = scoped::Seeds { names: seeds, up: opts.depth, down: opts.depth };
    let mut bares: Vec<&str> = Vec::new();
    for s in names {
        if !bares.contains(&bare_name(s)) {
            bares.push(bare_name(s));
        }
    }
    let mut facts: HashMap<String, Facts> = bares.iter().map(|b| (b.to_string(), Facts::default())).collect();
    let mut prep = (!scoped::whole()).then(|| scoped::Prepared::new(jobs));
    let mut queries: HashMap<(&str, &'static str), (Box<dyn lang::Lang>, Option<compiler::CompiledQuery>)> = HashMap::new();
    let mut lines = LineCache::default();
    for (j, (file, lang_name)) in jobs.iter().enumerate() {
        let Some(src) = crate::parsed::source(file) else { continue };
        // An outline entry or an `id#` match of a name holds its bytes.
        let held: Vec<&str> = bares.iter().copied().filter(|b| crate::needle::holds(&src, b)).collect();
        if held.is_empty() {
            continue;
        }
        let Some(p) = crate::parsed::get(file, lang_name) else { continue };
        if let Some(pr) = prep.as_mut() {
            pr.offer(j, &p);
        }
        // The rows of this file are hidden or not by its test scopes, read
        // now from the parse in hand.
        hider.scope_lines_in(file, Some(&p.source), p.tree.as_ref());
        let locs = tree::collect_def_locs(file, lang_name);
        for b in held {
            let f = facts.get_mut(b).expect("a fact per bare name");
            for (kind, name, line) in &locs {
                if name == b {
                    f.outline.push((file.clone(), lang_name, kind, *line));
                }
            }
            let (lang_box, compiled) = queries.entry((b, lang_name)).or_insert_with(|| {
                let l = lang::for_name(lang_name).expect("lang_name always valid here");
                let c = query::parse(&format!("(id#\"{}\" @r)", b)).ok().map(|ast| compiler::compile(&ast, l.as_ref()));
                (l, c)
            });
            let Some(compiled) = compiled.as_ref() else { continue };
            let Ok(matches) = runner::run_rows_cached(compiled, lang_box.as_ref(), file, lang_name, false) else { continue };
            for m in matches {
                if let Some(line) = crate::resolve::fallback::parse_line("r", &m.node_text) {
                    f.refs.push((file.clone(), line, lines.text(file, line)));
                }
            }
        }
    }
    for f in facts.values_mut() {
        f.refs.sort();
        f.refs.dedup();
    }
    let g = match prep {
        Some(pr) => pr.build(&seeds),
        None => crate::resolve::graph::build(jobs),
    };
    (g, facts)
}

/// The name `resolve_def_query` looks `expr` up by.
fn query_name(expr: &str) -> &str {
    if expr.contains("::") {
        expr.split("::")
            .filter(|s| !s.is_empty() && !matches!(*s, "crate" | "self" | "super"))
            .last()
            .unwrap_or(expr)
    } else if let Some((_, name)) = expr.rsplit_once('.') {
        name
    } else {
        expr
    }
}

// ── Report ───────────────────────────────────────────────────────────────────

/// Build the whole report as text, hiding nothing. Split out from `run` so
/// tests can assert on it without capturing stdout.
#[cfg(test)]
pub fn report(symbol: &str, jobs: &[(String, &'static str)], opts: &Options) -> String {
    report_hiding(symbol, jobs, opts, &mut files::Hider::none()).0
}

/// `report` with the definitions, callers and references `hider` hides left
/// out, and the footer counting them at the end. The bool is whether it shows
/// a definition or a reference row. The call graph is still built
/// over every file, so hiding a test never changes how a call resolves.
#[cfg(test)]
pub fn report_hiding(
    symbol: &str,
    jobs: &[(String, &'static str)],
    opts: &Options,
    hider: &mut files::Hider,
) -> (String, bool) {
    let mut hidden = Hidden::default();
    let (g, facts) = gather(&[symbol.to_string()], jobs, opts, hider);
    let (mut out, found) = body(symbol, jobs, &g, &facts[bare_name(symbol)], opts, hider, &mut hidden);
    let shown = out.matches("\nDEFINITION ").count();
    out.push_str(&footer(shown, "definition", &hidden));
    (out, found)
}

fn body(
    symbol: &str,
    jobs: &[(String, &'static str)],
    g: &CallGraph,
    facts: &Facts,
    opts: &Options,
    hider: &mut files::Hider,
    hidden: &mut Hidden,
) -> (String, bool) {
    let mut out = String::new();
    let mut lines = LineCache::default();
    // Every file:line already counted as hidden, so a site that is both a
    // caller and a reference, or a definition and a reference, counts once.
    let mut hidden_seen: HashSet<(String, usize)> = HashSet::new();

    let sites = keep_once(hider, definition_sites(symbol, g, jobs, facts), |s| (s.file.clone(), s.line), hidden, &mut hidden_seen);

    if sites.is_empty() {
        out.push_str(&format!("SYMBOL {} (no definition resolved)\n", symbol));
        let n = push_reference_list(&mut out, facts, opts, hider, hidden, &mut hidden_seen);
        return (out, n > 0);
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
        let mut ctx = WalkCtx { lines: &mut lines, listed: &mut listed, hider: &mut *hider, hidden: &mut *hidden, hidden_seen: &mut hidden_seen };
        push_walk(&mut out, "CALLERS", Direction::Up, &g, &caller_keys, opts, &mut ctx);
        push_walk(&mut out, "CALLEES", Direction::Down, &g, &keys, opts, &mut ctx);
    }

    let remaining: Vec<_> = facts.refs.iter().cloned()
        .filter(|(file, line, _)| !listed.contains(&(file.clone(), *line)))
        .collect();
    let remaining = keep_once(hider, remaining, |(f, l, _)| (f.clone(), *l), hidden, &mut hidden_seen);
    let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for (file, _, _) in remaining {
        *by_file.entry(file).or_default() += 1;
        total += 1;
    }
    out.push_str(&format!("\nREFERENCES ({} more)\n", total));
    for (file, count) in by_file {
        if crate::footer::take(&file) {
            out.push_str(&format!("  {}: {}\n", file, count));
        }
    }

    (out, true)
}

/// `Hider::keep`, except a hidden `file:line` already in `seen` is not counted
/// again, so the hidden count equals the extra rows `--tests` would show.
fn keep_once<T>(
    hider: &mut files::Hider,
    items: Vec<T>,
    at: impl Fn(&T) -> (String, usize),
    hidden: &mut Hidden,
    seen: &mut HashSet<(String, usize)>,
) -> Vec<T> {
    let mut out = Vec::new();
    for it in items {
        let (f, l) = at(&it);
        match hider.site(&f, l) {
            Some(h) => {
                if seen.insert((f, l)) {
                    hidden.count(h, 1);
                }
            }
            None => out.push(it),
        }
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
fn definition_sites(symbol: &str, g: &CallGraph, jobs: &[(String, &'static str)], facts: &Facts) -> Vec<Site> {
    let lang_of: HashMap<&str, &'static str> =
        jobs.iter().map(|(f, l)| (f.as_str(), *l)).collect();

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
    for (file, lang_name, kind, line) in &facts.outline {
        let (file, line) = (file, *line);
        if index_resolved && *kind != "class" {
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
fn definition_text(file: &str, lang_name: &'static str, line: usize) -> Option<String> {
    let p = crate::parsed::get(file, lang_name)?;
    let source = &p.source;
    let root = p.tree.as_ref()?.root_node();

    let row = line.checked_sub(1)?;
    let mut node = root;
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
            return w.utf8_text(source).ok().map(|s| s.to_string());
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
    hidden_seen: &'a mut HashSet<(String, usize)>,
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
    let rows = keep_once(ctx.hider, rows, |r| (r.0.clone(), r.1), ctx.hidden, ctx.hidden_seen);

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

fn push_reference_list(
    out: &mut String,
    facts: &Facts,
    opts: &Options,
    hider: &mut files::Hider,
    hidden: &mut Hidden,
    seen: &mut HashSet<(String, usize)>,
) -> usize {
    let refs = keep_once(hider, facts.refs.clone(), |(f, l, _)| (f.clone(), *l), hidden, seen);
    let refs: Vec<_> = refs.into_iter().filter(|(f, _, _)| crate::footer::take(f)).collect();
    out.push_str(&format!("\nREFERENCES ({})\n", refs.len()));
    for (file, line, text) in refs.iter().take(opts.max_lines) {
        out.push_str(&format!("  {}:{}  {}\n", file, line, text));
    }
    if refs.len() > opts.max_lines {
        out.push_str(&format!("  ... ({} more)\n", refs.len() - opts.max_lines));
    }
    refs.len()
}

// ── Source line lookup ───────────────────────────────────────────────────────

#[derive(Default)]
struct LineCache {
    files: HashMap<String, Vec<String>>,
}

impl LineCache {
    fn text(&mut self, file: &str, line: usize) -> String {
        let entry = self.files.entry(file.to_string()).or_insert_with(|| {
            crate::parsed::source(file)
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
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
