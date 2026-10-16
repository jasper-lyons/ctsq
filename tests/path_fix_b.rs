//! Review fixes over fixtures/flow/fix-b/: block scopes shadow outer names,
//! `--max` cuts are tagged and counted, loops run to their fixed point,
//! routes through unmodelled calls are counted, `--only` counts what it
//! prints, the copy and append builtins have models and an untracked write
//! of a field removes it.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/fix-b", env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).args(args).arg(dir()).output().expect("failed to run ctsq")
}

fn path_cmd(query: &str, flags: &[&str]) -> (Option<i32>, String) {
    let mut args = vec!["path", query];
    args.extend_from_slice(flags);
    let o = run(&args);
    (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string())
}

fn flow(func: &str, flags: &[&str]) -> (Option<i32>, String) {
    path_cmd(&format!("function#{func} param#x ==> function#Println"), flags)
}

fn row<'t>(t: &'t str, needle: &str) -> &'t str {
    t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row {needle}\n{t}"))
}

#[test]
fn an_inner_block_declaration_does_not_hide_the_outer_name() {
    let (code, t) = flow("BlockShadow", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "fmt.Println(x)").ends_with("  sink"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
}

#[test]
fn an_if_init_declaration_keeps_the_outer_name_tracked() {
    let (code, t) = flow("IfInit", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "fmt.Println(x)").ends_with("  sink"), "{t}");
}

#[test]
fn an_inner_declaration_does_not_reach_the_outer_name() {
    let (code, t) = flow("InnerDecl", &[]);
    assert_eq!(code, Some(1), "{t}");
    assert!(t.contains("\n0 routes to sink\n"), "{t}");
    assert!(!t.contains("fmt.Println(y)"), "{t}");
}

#[test]
fn a_plain_assignment_in_a_block_reaches_the_outer_name() {
    let (code, t) = flow("PlainAssign", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "fmt.Println(y)").ends_with("conditional@30, sink"), "{t}");
}

#[test]
fn a_range_variable_does_not_make_the_outer_name_conditional() {
    let (code, t) = flow("RangeShadow", &[]);
    assert_eq!(code, Some(0), "{t}");
    let r = row(&t, "fmt.Println(x)");
    assert!(r.ends_with("  sink"), "{t}");
    assert!(!r.contains("conditional"), "{t}");
}

#[test]
fn a_flow_cut_by_max_is_tagged_and_counted() {
    let (code, t) = path_cmd("function#start param#s ==> function#Println", &[]);
    assert_eq!(code, Some(1), "{t}");
    assert!(row(&t, "f13(s)").contains("stopped: max depth"), "{t}");
    assert!(t.contains("\n0 routes to sink\n1 route cut at --max-depth 12\n"), "{t}");
    let (code, t) = path_cmd("function#start param#s ==> function#Println", &["--max-depth", "25"]);
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
    assert!(!t.contains("cut at --max-depth"), "{t}");
}

#[test]
fn a_call_route_cut_by_max_is_reported() {
    let (code, t) = path_cmd("function#start --> function#f20", &[]);
    assert_eq!(code, Some(1), "{t}");
    assert!(t.contains("0 routes\n1 route cut at --max-depth 12\n"), "{t}");
    let (code, t) = path_cmd("function#start --> function#f20", &["--max-depth", "20"]);
    assert_eq!(code, Some(0), "{t}");
    assert!(!t.contains("cut at --max-depth"), "{t}");
}

#[test]
fn a_loop_runs_until_its_tracked_set_stops_changing() {
    let (code, t) = flow("Rotate", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "fmt.Println(v32)").ends_with("sink"), "{t}");
    assert!(!t.contains("loop cap"), "{t}");
}

#[test]
fn a_loop_that_never_settles_is_tagged() {
    let (_, t) = flow("Grow", &[]);
    assert!(row(&t, "for i := 0; i < n; i++ {").ends_with("stopped: loop cap"), "{t}");
}

#[test]
fn routes_through_unmodelled_calls_are_counted() {
    let (code, t) = flow("Through", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains("\n1 route to sink\n1 route passed through unmodelled calls\n"), "{t}");
    // an unmodelled call at the end of a route is not passed through
    let (_, t) = flow("BlockShadow", &[]);
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn only_counts_the_routes_it_keeps() {
    for tag in ["exact", "bogus"] {
        let (code, t) = flow("Through", &["--only", tag]);
        assert_eq!(code, Some(1), "{t}");
        assert_eq!(t, format!("0 routes match --only {tag}\n"), "{t}");
    }
    let (code, t) = flow("Through", &["--only", "unmodelled"]);
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains("u := lib.Do(x)"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
}

#[test]
fn copy_moves_its_source_into_its_destination() {
    let (code, t) = flow("Copy", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "copy(dst, x)").ends_with("moved"), "{t}");
    assert!(row(&t, "fmt.Println(dst)").ends_with("sink"), "{t}");
}

#[test]
fn append_returns_its_arguments() {
    let (code, t) = flow("Append", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "out = append(out, x)").ends_with("derived"), "{t}");
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn an_untracked_write_of_a_field_removes_it() {
    let (code, t) = flow("Overwrite", &[]);
    assert_eq!(code, Some(1), "{t}");
    assert!(t.contains("\n0 routes to sink\n"), "{t}");
    // a write of another field keeps it
    let (code, t) = flow("OtherField", &[]);
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "fmt.Println(b.Phone)").ends_with("sink"), "{t}");
}
