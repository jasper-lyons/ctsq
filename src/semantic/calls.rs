//! Call combinators: `->` and `<-` follow one call edge, `-->` and `<--`
//! follow any number of edges, shortest routes first. The walk runs over
//! `CallGraph` edges that resolved to a definition. Results are route data,
//! the printer lives with the `path` command.

use super::{segment_defs, split_query};
use crate::query::ast::{Combinator, Query};
use crate::resolve::graph::CallGraph;
use crate::resolve::{Confidence, DefKind, TreeStore};
use std::collections::{HashMap, HashSet, VecDeque};

/// Limits on the walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOptions {
    /// Longest route in call edges, counted over the whole route.
    pub max: usize,
    /// Every route instead of the `limit` shortest per start.
    pub all: bool,
    /// Routes kept per start when `all` is false.
    pub limit: usize,
}

impl Default for CallOptions {
    fn default() -> Self {
        CallOptions { max: 12, all: false, limit: 5 }
    }
}

/// One function on a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    /// Index into `CallGraph.index.defs`.
    pub def: usize,
    /// File and 1-based line of the definition.
    pub file: String,
    pub line: usize,
    /// Bare name of the definition.
    pub name: String,
    /// Receiver type of a method, or interface of an interface method.
    pub owner: Option<String>,
    /// Line inside this function where it calls the next hop. None on the
    /// last hop.
    pub call_line: Option<usize>,
    /// Confidence of the call edge that reached this hop. Exact on the first
    /// hop. A Guess prints `?`.
    pub confidence: Confidence,
}

/// One route, hops always in call order: each hop calls the next. For an
/// upward query (`<-`, `<--`) that is the reverse of the reading order, so
/// the query's first selector matches the last hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Def index of the start in reading order: `hops[0]` for a downward
    /// query, the last hop for an upward one.
    pub start: usize,
    pub hops: Vec<Hop>,
}

impl Route {
    /// Whether any edge on the route is a guess.
    pub fn guessed(&self) -> bool {
        self.hops.iter().any(|h| h.confidence == Confidence::Guess)
    }
}

/// What `evaluate_calls` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRoutes {
    /// Grouped by start (file, then line), shortest first within a start.
    pub routes: Vec<Route>,
    /// True when the query reads upward (`<-`, `<--`).
    pub upward: bool,
    /// Definitions each segment matched, in reading order, before `:not()`
    /// exclusions. A 0 names the selector that matched nothing.
    pub segment_matches: Vec<usize>,
    /// Definitions removed by `:not()`.
    pub excluded: usize,
    /// True when the search stopped at its expansion cap, so routes may be
    /// missing.
    pub truncated: bool,
    /// Starts with no route within `max` that do reach the end on a longer
    /// one, so `--max` cut their routes.
    pub cut: usize,
    /// The `max` the walk ran with.
    pub max: usize,
}

/// Paths popped from the search queue before the walk gives up.
const EXPANSION_CAP: usize = 200_000;

/// Evaluate a query whose top-level combinators are all call combinators of
/// one direction. Build the inputs with `TreeStore::build(&jobs)` and
/// `graph::build_from_store(&store)`.
///
/// - `A -> B`: B is called directly by A. `A --> B`: B is reached through
///   one or more calls.
/// - `A <- B`, `A <-- B`: A is called by B. Routes still print caller first.
/// - `A --> B --> C`: B is a waypoint, every route passes through it.
/// - `:not(sel)` on the last selector of any step: no route passes through
///   a definition `sel` matches, start and end included.
///
/// Errors: no call combinator, a data-flow combinator, a mix of downward and
/// upward combinators, a misplaced semantic combinator or `:not()`, and a
/// selector every file's language rejects.
pub fn evaluate_calls(
    query: &Query,
    store: &TreeStore,
    graph: &CallGraph,
    opts: &CallOptions,
) -> Result<CallRoutes, String> {
    let split = split_query(query)?;
    if split.joins.is_empty() {
        return Err("no call combinator in the query, use -> --> <- or <--".into());
    }
    let down = |c: &Combinator| matches!(c, Combinator::Calls | Combinator::CallsEventually);
    let up = |c: &Combinator| matches!(c, Combinator::CalledBy | Combinator::CalledByEventually);
    if split.joins.iter().any(|c| !down(c) && !up(c)) {
        return Err("data-flow combinators cannot be mixed with call combinators".into());
    }
    let upward = split.joins.iter().all(up);
    if !upward && !split.joins.iter().all(down) {
        return Err("a query reads one way: use -> and --> or <- and <--, not both".into());
    }

    let index = &graph.index;
    let mut excluded: HashSet<usize> = HashSet::new();
    for sel in split.nots.iter().flatten() {
        let q = Query { head: sel.clone(), tail: vec![] };
        excluded.extend(segment_defs(&q, store, index)?);
    }
    let mut sets: Vec<Vec<usize>> = Vec::new();
    let mut segment_matches = Vec::new();
    for seg in &split.segments {
        let defs = segment_defs(seg, store, index)?;
        segment_matches.push(defs.len());
        sets.push(defs.into_iter().filter(|d| !excluded.contains(d)).collect());
    }

    // call order: caller first
    let mut direct: Vec<bool> = split
        .joins
        .iter()
        .map(|c| matches!(c, Combinator::Calls | Combinator::CalledBy))
        .collect();
    if upward {
        sets.reverse();
        direct.reverse();
    }
    let start_pos = if upward { sets.len() - 1 } else { 0 };

    let walk = Walk::new(graph, &excluded, opts);
    // The same walk without a length bound, to tell a start with no route at
    // all from one whose routes `max` cut.
    let wide_opts = CallOptions { max: usize::MAX / 2, all: false, limit: 1 };
    let wide = Walk::new(graph, &excluded, &wide_opts);
    let mut cut = 0;
    let mut starts = sets[start_pos].clone();
    starts.sort_by(|&a, &b| index.defs[a].id.cmp(&index.defs[b].id));
    let mut routes = Vec::new();
    for s in starts {
        let mut chain: Vec<HashSet<usize>> = sets.iter().map(|v| v.iter().copied().collect()).collect();
        chain[start_pos] = HashSet::from([s]);
        let mut found = walk.chain_routes(&chain, &direct);
        if found.is_empty() && !wide.chain_routes(&chain, &direct).is_empty() {
            cut += 1;
        }
        found.sort_by(|a, b| {
            a.len().cmp(&b.len()).then_with(|| {
                let ids = |r: &Vec<Step>| r.iter().map(|st| &index.defs[st.def].id).collect::<Vec<_>>();
                ids(a).cmp(&ids(b))
            })
        });
        found.dedup();
        if !opts.all {
            found.truncate(opts.limit);
        }
        routes.extend(found.into_iter().map(|steps| to_route(s, &steps, graph)));
    }

    Ok(CallRoutes {
        routes,
        upward,
        segment_matches,
        excluded: excluded.len(),
        truncated: walk.truncated.get(),
        cut,
        max: opts.max,
    })
}

/// A route step: the def reached, and the call edge that reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    def: usize,
    /// Line of the call in the previous step. 0 on the first step.
    via_line: usize,
    conf: Confidence,
}

fn to_route(start: usize, steps: &[Step], graph: &CallGraph) -> Route {
    let hops = steps
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let d = &graph.index.defs[st.def];
            let owner = match &d.kind {
                DefKind::Function => None,
                DefKind::Method { receiver, .. } => Some(receiver.clone()),
                DefKind::InterfaceMethod { interface } => Some(interface.clone()),
            };
            Hop {
                def: st.def,
                file: d.id.file.clone(),
                line: d.id.line,
                name: d.name.clone(),
                owner,
                call_line: steps.get(i + 1).map(|n| n.via_line),
                confidence: st.conf,
            }
        })
        .collect();
    Route { start, hops }
}

struct Walk<'a> {
    /// def -> (callee def, call line, confidence), call order, one per callee.
    adj: Vec<Vec<(usize, usize, Confidence)>>,
    /// def -> caller defs.
    radj: Vec<Vec<usize>>,
    excluded: &'a HashSet<usize>,
    opts: &'a CallOptions,
    expansions: std::cell::Cell<usize>,
    truncated: std::cell::Cell<bool>,
}

impl<'a> Walk<'a> {
    fn new(graph: &CallGraph, excluded: &'a HashSet<usize>, opts: &'a CallOptions) -> Walk<'a> {
        let n = graph.index.defs.len();
        let mut adj: Vec<Vec<(usize, usize, Confidence)>> = vec![vec![]; n];
        let mut radj: Vec<Vec<usize>> = vec![vec![]; n];
        let mut order: Vec<usize> = (0..graph.calls.len()).collect();
        order.sort_by_key(|&i| (graph.calls[i].line, graph.calls[i].callee_display.clone()));
        for i in order {
            let rc = &graph.calls[i];
            let Some((t, conf)) = rc.target else { continue };
            if adj[rc.caller].iter().any(|&(x, _, _)| x == t) {
                continue;
            }
            adj[rc.caller].push((t, rc.line, conf));
            radj[t].push(rc.caller);
        }
        Walk {
            adj,
            radj,
            excluded,
            opts,
            expansions: std::cell::Cell::new(0),
            truncated: std::cell::Cell::new(false),
        }
    }

    fn cap(&self) -> usize {
        if self.opts.all { usize::MAX } else { self.opts.limit.max(1) }
    }

    /// Every route through `chain` (sets in call order), each leg one edge
    /// when `direct[k]`, else any number up to the remaining budget.
    fn chain_routes(&self, chain: &[HashSet<usize>], direct: &[bool]) -> Vec<Vec<Step>> {
        let dists: Vec<HashMap<usize, usize>> = chain[1..].iter().map(|t| self.dist_to(t)).collect();
        let mut out = Vec::new();
        let mut firsts: Vec<usize> = chain[0].iter().copied().collect();
        firsts.sort();
        for s in firsts {
            let route = vec![Step { def: s, via_line: 0, conf: Confidence::Exact }];
            self.extend(route, 0, chain, direct, &dists, &mut out);
        }
        out
    }

    fn extend(
        &self,
        route: Vec<Step>,
        k: usize,
        chain: &[HashSet<usize>],
        direct: &[bool],
        dists: &[HashMap<usize, usize>],
        out: &mut Vec<Vec<Step>>,
    ) {
        if k == direct.len() {
            out.push(route);
            return;
        }
        let used = route.len() - 1;
        let budget = self.opts.max.saturating_sub(used);
        let from = route.last().unwrap().def;
        let on_route: HashSet<usize> = route.iter().map(|s| s.def).collect();
        for leg in self.leg(from, &chain[k + 1], &dists[k], direct[k], budget, &on_route) {
            let mut next = route.clone();
            next.extend(leg);
            self.extend(next, k + 1, chain, direct, dists, out);
        }
    }

    /// Shortest-first simple paths from `from` to any of `targets`, at most
    /// `budget` edges, never through an excluded def or one already on the
    /// route. A path stops at the first target it reaches.
    fn leg(
        &self,
        from: usize,
        targets: &HashSet<usize>,
        dist: &HashMap<usize, usize>,
        direct: bool,
        budget: usize,
        on_route: &HashSet<usize>,
    ) -> Vec<Vec<Step>> {
        let cap = self.cap();
        let mut found = Vec::new();
        if budget == 0 {
            return found;
        }
        let open = |t: usize| !self.excluded.contains(&t) && !on_route.contains(&t);
        if direct {
            for &(t, line, conf) in &self.adj[from] {
                if targets.contains(&t) && open(t) && found.len() < cap {
                    found.push(vec![Step { def: t, via_line: line, conf }]);
                }
            }
            return found;
        }
        let mut queue: VecDeque<Vec<Step>> = VecDeque::from([vec![]]);
        while let Some(path) = queue.pop_front() {
            let n = self.expansions.get() + 1;
            self.expansions.set(n);
            if n > EXPANSION_CAP {
                self.truncated.set(true);
                break;
            }
            let cur = path.last().map(|s| s.def).unwrap_or(from);
            for &(t, line, conf) in &self.adj[cur] {
                if !open(t) || path.iter().any(|s| s.def == t) {
                    continue;
                }
                let len = path.len() + 1;
                let Some(&d) = dist.get(&t) else { continue };
                if len + d > budget {
                    continue;
                }
                let mut next = path.clone();
                next.push(Step { def: t, via_line: line, conf });
                if targets.contains(&t) {
                    found.push(next);
                    if found.len() >= cap {
                        return found;
                    }
                } else {
                    queue.push_back(next);
                }
            }
        }
        found
    }

    /// Fewest edges from each def to any of `targets`, through defs that are
    /// not excluded, up to `max`.
    fn dist_to(&self, targets: &HashSet<usize>) -> HashMap<usize, usize> {
        let mut dist: HashMap<usize, usize> = HashMap::new();
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &t in targets {
            if !self.excluded.contains(&t) {
                dist.insert(t, 0);
                queue.push_back(t);
            }
        }
        while let Some(n) = queue.pop_front() {
            let d = dist[&n];
            if d >= self.opts.max {
                continue;
            }
            for &c in &self.radj[n] {
                if self.excluded.contains(&c) || dist.contains_key(&c) {
                    continue;
                }
                dist.insert(c, d + 1);
                queue.push_back(c);
            }
        }
        dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::parse;
    use crate::resolve::graph::build_from_store;

    // main -> load -> parse -> check, and main -> fast -> check.
    const APP: &str = "package a
func main() {
  load()
  fast()
}
func load() {
  parse()
}
func parse() {
  check()
}
func fast() {
  check()
}
func check() {}
";

    fn eval(files: &[(&str, &str)], q: &str, opts: &CallOptions) -> CallRoutes {
        let mut store = TreeStore::default();
        for (f, src) in files {
            store.insert(f, "go", src.as_bytes().to_vec());
        }
        let g = build_from_store(&store);
        evaluate_calls(&parse(q).unwrap(), &store, &g, opts).unwrap()
    }

    fn names(r: &Route) -> Vec<&str> {
        r.hops.iter().map(|h| h.name.as_str()).collect()
    }

    fn all_names(res: &CallRoutes) -> Vec<Vec<&str>> {
        res.routes.iter().map(names).collect()
    }

    #[test]
    fn one_edge_with_the_call_line() {
        let res = eval(&[("a/a.go", APP)], "function#main -> function#load", &CallOptions::default());
        assert_eq!(all_names(&res), vec![vec!["main", "load"]]);
        let r = &res.routes[0];
        assert_eq!(r.hops[0].call_line, Some(3));
        assert_eq!(r.hops[1].call_line, None);
        assert_eq!((r.hops[1].file.as_str(), r.hops[1].line), ("a/a.go", 6));
        assert!(!r.guessed());
        let none = eval(&[("a/a.go", APP)], "function#main -> function#check", &CallOptions::default());
        assert!(none.routes.is_empty(), "check is two edges away");
    }

    #[test]
    fn eventually_finds_every_route_shortest_first() {
        let res = eval(&[("a/a.go", APP)], "function#main --> function#check", &CallOptions::default());
        assert_eq!(
            all_names(&res),
            vec![vec!["main", "fast", "check"], vec!["main", "load", "parse", "check"]]
        );
        let long = &res.routes[1];
        let lines: Vec<Option<usize>> = long.hops.iter().map(|h| h.call_line).collect();
        assert_eq!(lines, vec![Some(3), Some(7), Some(10), None]);
        assert_eq!(res.segment_matches, vec![1, 1]);
    }

    #[test]
    fn upward_routes_read_caller_first() {
        let res = eval(&[("a/a.go", APP)], "function#check <-- function#main", &CallOptions::default());
        assert!(res.upward);
        assert_eq!(
            all_names(&res),
            vec![vec!["main", "fast", "check"], vec!["main", "load", "parse", "check"]]
        );
        let check = res.routes[0].hops.last().unwrap().def;
        assert!(res.routes.iter().all(|r| r.start == check), "the reading start is check");
        let direct = eval(&[("a/a.go", APP)], "function#check <- function#fast", &CallOptions::default());
        assert_eq!(all_names(&direct), vec![vec!["fast", "check"]]);
    }

    #[test]
    fn a_waypoint_forces_the_route_through_it() {
        let res = eval(
            &[("a/a.go", APP)],
            "function#main --> function#parse --> function#check",
            &CallOptions::default(),
        );
        assert_eq!(all_names(&res), vec![vec!["main", "load", "parse", "check"]]);
    }

    #[test]
    fn not_removes_routes_through_a_def() {
        let res = eval(
            &[("a/a.go", APP)],
            "function#main --> function#check:not(function#fast)",
            &CallOptions::default(),
        );
        assert_eq!(all_names(&res), vec![vec!["main", "load", "parse", "check"]]);
        assert_eq!(res.excluded, 1);
        let gone = eval(
            &[("a/a.go", APP)],
            "function#main --> function#check:not(function#check)",
            &CallOptions::default(),
        );
        assert!(gone.routes.is_empty(), "an excluded end leaves no route");
    }

    #[test]
    fn max_limits_route_length() {
        let opts = CallOptions { max: 2, ..CallOptions::default() };
        let res = eval(&[("a/a.go", APP)], "function#main --> function#check", &opts);
        assert_eq!(all_names(&res), vec![vec!["main", "fast", "check"]]);
        let opts = CallOptions { max: 1, ..CallOptions::default() };
        let res = eval(&[("a/a.go", APP)], "function#main --> function#check", &opts);
        assert!(res.routes.is_empty());
        assert_eq!(res.cut, 1, "a route exists beyond --max 1");
        let res = eval(&[("a/a.go", APP)], "function#check --> function#main", &opts);
        assert_eq!(res.cut, 0, "no route at all is not a cut");
    }

    #[test]
    fn limit_keeps_the_shortest_per_start_and_all_keeps_every_one() {
        let opts = CallOptions { limit: 1, ..CallOptions::default() };
        let res = eval(&[("a/a.go", APP)], "function#main --> function#check", &opts);
        assert_eq!(all_names(&res), vec![vec!["main", "fast", "check"]]);
        let opts = CallOptions { limit: 1, all: true, ..CallOptions::default() };
        let res = eval(&[("a/a.go", APP)], "function#main --> function#check", &opts);
        assert_eq!(res.routes.len(), 2);
    }

    #[test]
    fn routes_group_by_start() {
        let res = eval(&[("a/a.go", APP)], "function#/^(load|fast)$/ --> function#check", &CallOptions::default());
        assert_eq!(
            all_names(&res),
            vec![vec!["load", "parse", "check"], vec!["fast", "check"]],
            "load is declared first"
        );
    }

    #[test]
    fn interface_dispatch_hops_are_guesses() {
        let src = "package a
type W interface {
  Write()
}
type F struct{}
func (f *F) Write() { done() }
type B struct{}
func (b *B) Write() { done() }
func done() {}
func Save(w W) {
  w.Write()
}
func mk() { _, _ = &F{}, &B{} }
";
        let res = eval(&[("a/a.go", src)], "function#Save --> function#done", &CallOptions::default());
        assert_eq!(res.routes.len(), 2, "{:?}", res.routes);
        for r in &res.routes {
            assert_eq!(r.hops[1].confidence, Confidence::Guess);
            assert_eq!(r.hops[2].confidence, Confidence::Exact);
            assert!(r.guessed());
        }
        assert_eq!(res.routes[0].hops[1].owner.as_deref(), Some("F"));
    }

    #[test]
    fn a_start_that_matches_nothing_is_counted() {
        let res = eval(&[("a/a.go", APP)], "function#nope --> function#check", &CallOptions::default());
        assert!(res.routes.is_empty());
        assert_eq!(res.segment_matches, vec![0, 1]);
    }

    #[test]
    fn mixed_directions_and_flow_combinators_are_errors() {
        let mut store = TreeStore::default();
        store.insert("a/a.go", "go", APP.as_bytes().to_vec());
        let g = build_from_store(&store);
        for q in [
            "function#main --> function#parse <-- function#load",
            "function#main ==> function#check",
            "function#main",
        ] {
            let res = evaluate_calls(&parse(q).unwrap(), &store, &g, &CallOptions::default());
            assert!(res.is_err(), "{} should be an error", q);
        }
    }

    #[test]
    fn cross_file_routes_follow_imports() {
        let res = eval(
            &[
                ("proj/util/u.go", "package util\nfunc Helper() {\n  deep()\n}\nfunc deep() {}\n"),
                ("proj/main.go", "package main\nimport \"example.com/proj/util\"\nfunc main() {\n  util.Helper()\n}\n"),
            ],
            "function#main --> function#deep",
            &CallOptions::default(),
        );
        assert_eq!(all_names(&res), vec![vec!["main", "Helper", "deep"]]);
        assert_eq!(res.routes[0].hops[1].file, "proj/util/u.go");
        assert_eq!(res.routes[0].hops[0].call_line, Some(4));
    }
}
