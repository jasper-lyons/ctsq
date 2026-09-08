//! End-to-end checks on what the binary actually prints. The unit tests cover
//! the rendering functions; these cover the argument handling and exit codes
//! around them, which only the process boundary shows.

use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn ctsq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(args)
        .output()
        .expect("failed to run ctsq")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_query_prints_path_line_and_the_whole_source_line() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#NewServer", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        text.contains(&format!("{}:14: func NewServer(", f)),
        "expected path:LINE: source line, got: {}",
        text
    );
    assert!(!text.contains("<match>"), "the node-text form must be off by default: {}", text);
}

#[test]
fn the_node_flag_restores_the_old_form() {
    let f = fixture("example.go");
    let text = stdout(&ctsq(&["-q", "id#NewServer", "--node", &f]));
    assert!(text.contains("<match>:14: \"NewServer\""), "{}", text);
}

#[test]
fn a_query_that_matches_nothing_says_so_on_stdout() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#no_such_name_anywhere", &f]);
    let text = stdout(&out);
    assert!(text.starts_with("no match for id#no_such_name_anywhere in "), "{}", text);
    assert!(text.contains("(1 files scanned)"), "{}", text);
    assert_eq!(out.status.code(), Some(1), "a miss exits 1 like grep");
}

#[test]
fn def_resolves_a_type_and_reports_a_miss_on_stdout() {
    let dir = fixture("");
    let out = ctsq(&["def", "Server", &dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("type Server struct {"), "{}", stdout(&out));

    let miss = ctsq(&["def", "NewServe", &dir]);
    let text = stdout(&miss);
    assert!(text.starts_with("no definition for 'NewServe' in "), "{}", text);
    assert!(text.contains("files scanned)"), "{}", text);
    assert!(text.contains("similar names: NewServer"), "{}", text);
    assert_eq!(miss.status.code(), Some(1));
}

#[test]
fn tree_rejects_body_with_the_query_that_does_it() {
    let out = ctsq(&["tree", &fixture("example.go"), "--body"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("tree has no --body, use ctsq -q '*function#Name' --body FILE"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn tree_prints_a_header_then_signatures() {
    let f = fixture("example.go");
    let text = stdout(&ctsq(&["tree", &f]));
    let mut lines = text.lines();
    let header = lines.next().unwrap();
    assert!(header.starts_with(&f), "{}", header);
    assert!(header.contains("tokens"), "{}", header);
    assert!(text.contains("func NewServer(host string, port int) *Server {"), "{}", text);
}

#[test]
fn files_lists_only_what_matches_every_query() {
    let dir = fixture("");
    let out = ctsq(&["files", "-q", "id#NewServer", "-q", "id#host", &dir]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "only the Go fixture holds both: {}", text);
    assert!(lines[0].starts_with(&format!("{}example.go  ", dir)), "{}", lines[0]);
    assert!(lines[0].ends_with("  2 3"), "trailing per-query counts: {}", lines[0]);
}

#[test]
fn files_reports_a_miss_on_stdout() {
    let out = ctsq(&["files", "-q", "id#NewServer", "-q", "id#nothing_here", &fixture("")]);
    let text = stdout(&out);
    assert!(text.starts_with("no file matches all 2 queries in "), "{}", text);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_go_type_query_with_body_prints_the_declaration() {
    let text = stdout(&ctsq(&["-q", "*type#Server", "--body", &fixture("example.go")]));
    assert!(text.contains("host string"), "expected the struct body: {}", text);
    assert!(text.contains("port int"), "{}", text);
}
