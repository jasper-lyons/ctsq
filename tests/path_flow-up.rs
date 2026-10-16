//! `ctsq path` upward data-flow queries (`<=` and `<==`) through the binary
//! over fixtures/flow/flow-up/: render(w, name) called from three functions
//! with literals and from admin with a concatenation, a value from
//! os.Getenv trimmed into q, and a JS logger made by pino().

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/flow-up", env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).args(args).output().expect("failed to run ctsq")
}

fn path_cmd(query: &str) -> Output {
    run(&["path", query, &dir()])
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

/// The row whose text contains `needle`, with its indent depth.
fn row<'t>(t: &'t str, needle: &str) -> (usize, &'t str) {
    let l = t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}"));
    ((l.len() - l.trim_start().len()) / 2, l.trim_start())
}

#[test]
fn a_variable_reaches_the_getenv_line_tagged_source() {
    let o = path_cmd("variable#q <== function#os.Getenv");
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    let (d0, r0) = row(&t, "q := strings.TrimSpace(dir)");
    assert_eq!(d0, 0, "the queried value prints first\n{t}");
    assert!(r0.starts_with(&format!("{}/render.go:33", dir())), "{t}");
    let (d1, r1) = row(&t, "dir := os.Getenv(\"DATA_DIR\")");
    assert_eq!(d1, 1, "{t}");
    assert!(r1.starts_with(&format!("{}/render.go:32", dir())), "{t}");
    assert!(r1.ends_with("  source"), "{t}");
    assert!(t.contains("1 route from source"), "{t}");
}

#[test]
fn an_open_start_lists_every_origin() {
    let o = path_cmd("function#save variable#q <== ");
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(row(&t, "os.Getenv").1.ends_with("source"), "{t}");
}

#[test]
fn a_parameter_lists_its_three_literal_callers() {
    let o = path_cmd("function#render param#name <== ");
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert_eq!(row(&t, "func render(w io.Writer, name string)").0, 0, "{t}");
    for (needle, line) in [("\"home.html.tmpl\"", 16), ("\"about.html.tmpl\"", 20), ("\"contact.html.tmpl\"", 24)] {
        let (d, r) = row(&t, needle);
        assert_eq!(d, 1, "{t}");
        assert!(r.starts_with(&format!("{}/render.go:{}", dir(), line)), "{t}");
        assert!(r.ends_with("moved, source literal"), "{t}");
    }
    // the concatenating caller goes on to admin's parameter, which nothing calls
    assert_eq!(row(&t, "\"admin-\"+page").0, 1, "{t}");
    let (d, r) = row(&t, "func admin(w io.Writer, page string)");
    assert_eq!(d, 2, "{t}");
    assert!(r.ends_with("source param"), "{t}");
    assert!(t.contains("4 routes from origins"), "{t}");
}

#[test]
fn one_step_stops_at_the_callers() {
    let o = path_cmd("function#render param#name <= ");
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(!t.contains("func admin("), "{t}");
    assert!(t.contains("3 routes from origins"), "{t}");
}

#[test]
fn an_end_that_no_origin_matches_exits_1() {
    let o = path_cmd("variable#q <== function#os.Hostname");
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(out(&o).contains("0 routes from source"));
}

#[test]
fn max_depth_cuts_the_walk_into_callers() {
    let o = run(&["path", "function#render param#name <== ", &dir(), "--max-depth", "0"]);
    let t = out(&o);
    assert!(row(&t, "func render(").1.ends_with("stopped: max depth"), "{t}");
    assert!(t.contains("1 route cut at --max 0"), "{t}");
}

#[test]
fn from_on_a_method_receiver_follows_the_creating_call() {
    let o = run(&["-q", "function#info[from=pino]", &dir()]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("log.js:6: logger.info(x);"), "{t}");
    let none = run(&["-q", "function#info[from=winston]", &dir()]);
    assert_eq!(code(&none), 1);
}
