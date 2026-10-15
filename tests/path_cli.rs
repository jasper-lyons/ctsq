//! `ctsq path` through the binary: output shape, flags and exit codes for
//! call routes over fixtures/path/ (main -> a -> b -> c, a branch through d,
//! a method, an interface call and one guessed call).

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/path", env!("CARGO_MANIFEST_DIR"))
}

fn path_cmd(query: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .arg("path")
        .arg(query)
        .arg(dir())
        .args(extra)
        .output()
        .expect("failed to run ctsq")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

#[test]
fn a_route_prints_rows_with_call_lines_and_a_trailer() {
    let o = path_cmd("function#a --> function#c", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{}", t);
    let d = dir();
    assert!(t.contains(&format!("{}/main.go:16", d)), "{}", t);
    assert!(t.contains("a              calls b at :17, calls d at :18"), "{}", t);
    assert!(t.contains("\n  ") && t.contains("b              calls c at :22"), "{}", t);
    assert!(t.trim_end().ends_with("2 routes"), "{}", t);
    // two spaces per level, no box drawing
    assert!(!t.contains('├') && !t.contains('└'));
}

#[test]
fn max_limits_route_length_and_none_found_exits_1() {
    let o = path_cmd("function#a --> function#c", &["--max", "1"]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(out(&o).contains("0 routes"));
    let o = path_cmd("function#a --> function#c", &["--max", "2"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).trim_end().ends_with("2 routes"));
}

#[test]
fn not_removes_a_route_and_is_noted() {
    let o = path_cmd("function#a --> function#c:not(function#d)", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{}", t);
    assert!(t.contains("1 route\n"), "{}", t);
    assert!(!t.contains(" d "), "{}", t);
    assert!(t.contains("1 definition excluded by :not"), "{}", t);
}

#[test]
fn body_prints_definition_bodies_one_level_deeper() {
    let o = path_cmd("function#a -> function#b", &["--body"]);
    let t = out(&o);
    assert!(t.contains("\n  func a() {\n  \tb()\n"), "{}", t);
    assert!(t.contains("\n    func b() {\n"), "{}", t);
}

#[test]
fn upward_queries_print_in_reading_order() {
    let o = path_cmd("function#c <-- function#a", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{}", t);
    let first = t.lines().next().unwrap();
    assert!(first.contains("main.go:25") && first.contains("called by"), "{}", t);
    assert!(t.contains("called by b at :22"), "{}", t);
    assert!(t.trim_end().ends_with("2 routes"), "{}", t);
}

#[test]
fn a_guessed_edge_prints_a_question_mark() {
    let o = path_cmd("function#entry --> function#c", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{}", t);
    assert!(t.contains("calls Alpha.Ping? at :17"), "{}", t);
}

#[test]
fn methods_print_with_their_owner() {
    let o = path_cmd("function#main --> function#Save", &[]);
    assert!(out(&o).contains("Store.Save"), "{}", out(&o));
}

#[test]
fn bad_queries_exit_2() {
    for q in ["function#main", "function#main --> (", "function#a --> function#b <- function#c"] {
        let o = path_cmd(q, &[]);
        assert_eq!(code(&o), 2, "{}: {}", q, out(&o));
    }
}

#[test]
fn data_flow_from_a_function_start_reports_no_variable() {
    let o = path_cmd("function#main ==> function#c", &[]);
    assert_eq!(code(&o), 1);
    let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("matched no variable, parameter or field"));
}

#[test]
fn only_is_accepted() {
    let o = path_cmd("function#a -> function#b", &["--only", "sink,exact"]);
    assert_eq!(code(&o), 0);
}
