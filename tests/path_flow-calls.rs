//! `ctsq path` data-flow queries through the binary over fixtures/flow/flow-calls/:
//! a request body passed through first, second and third, normalised by a
//! helper in another file of the package, returned, then handed to
//! log.Printf and json.Marshal. A Booking field is read in two functions.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/flow-calls", env!("CARGO_MANIFEST_DIR"))
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

/// The row whose text contains `needle`, with its indent depth.
fn row<'t>(t: &'t str, needle: &str) -> (usize, &'t str) {
    let l = t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}"));
    ((l.len() - l.trim_start().len()) / 2, l.trim_start())
}

fn through_three_functions(t: &str) {
    let d = dir();
    let (d0, r0) = row(t, "body, _ := io.ReadAll");
    assert_eq!(d0, 0, "{t}");
    assert!(r0.starts_with(&format!("{}/handler.go:16", d)), "{t}");
    let (d1, r1) = row(t, "out := first(string(body))");
    assert_eq!(d1, 1, "{t}");
    assert!(r1.ends_with("derived, via first"), "{t}");
    // the walk continues in each callee from its parameter, tagged moved
    for (needle, depth) in [
        ("func first(s string)", 2),
        ("t := second(s)", 3),
        ("func second(s string)", 4),
        ("return third(s)", 5),
        ("func third(s string)", 6),
        ("n := normalise(s)", 7),
        ("func normalise(s string)", 8),
    ] {
        assert_eq!(row(t, needle).0, depth, "{needle}\n{t}");
    }
    assert!(row(t, "func first(s string)").1.ends_with("moved"), "{t}");
    assert!(row(t, "n := normalise(s)").1.ends_with("via normalise"), "{t}");
    // the helper in the other file resolves through the graph
    assert!(row(t, "func normalise(s string)").1.starts_with(&format!("{}/norm.go:5", d)), "{t}");
    // the value comes back through each return
    assert!(t.contains("first          return t  moved\n"), "{t}");
    assert!(row(t, "return n").1.ends_with("moved"), "{t}");
}

#[test]
fn printf_is_reached_through_three_functions() {
    let o = path_cmd("function#handle variable#body ==> function#log.Printf", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    through_three_functions(&t);
    let (d, r) = row(&t, "log.Printf(\"got %s\", out)");
    assert_eq!(d, 2, "{t}");
    assert!(r.ends_with("sink"), "{t}");
    // json.Marshal is not the end, so its branch is pruned
    assert!(!t.contains("json.Marshal"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
    assert!(t.contains("1 route ends at an unmodelled call:"), "{t}");
}

#[test]
fn json_marshal_is_reached_through_three_functions() {
    let o = path_cmd("function#handle variable#body ==> function#json.Marshal", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    through_three_functions(&t);
    let (d, r) = row(&t, "data, _ := json.Marshal(out)");
    assert_eq!(d, 2, "{t}");
    assert!(r.ends_with("derived, sink"), "{t}");
    assert!(!t.contains("log.Printf"), "{t}");
}

#[test]
fn no_end_lists_outside_leaves() {
    let o = path_cmd("function#handle variable#body ==>", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(row(&t, "log.Printf(\"got %s\", out)").1.ends_with("outside"), "{t}");
    assert!(row(&t, "json.Marshal(out)").1.ends_with("derived, outside"), "{t}");
    assert!(row(&t, "w.Write(data)").1.ends_with("outside"), "{t}");
    assert!(row(&t, "strings.TrimSpace(s)").1.ends_with("moved, outside"), "{t}");
    assert!(!t.contains("sink"), "{t}");
    assert!(t.contains("4 routes to outside\n"), "{t}");
}

#[test]
fn only_keeps_tagged_hops_and_their_ancestors() {
    let o = path_cmd("function#handle variable#body ==> function#log.Printf", &["--only", "via"]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(row(&t, "out := first(string(body))").1.contains("via first"), "{t}");
    assert!(row(&t, "n := normalise(s)").1.contains("via normalise"), "{t}");
    // ancestors of a via hop stay for context
    assert!(t.contains("func third(s string)"), "{t}");
    // hops without a via tag and no via below are dropped
    assert!(!t.contains("log.Printf"), "{t}");
    assert!(!t.contains("first          return t"), "{t}");
    assert!(!t.contains("func normalise"), "{t}");
}

#[test]
fn a_field_start_groups_by_source() {
    let o = path_cmd("field#Booking.CustomerPhone ==>", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    let first = t.lines().next().unwrap();
    assert!(first.starts_with(&format!("{}/handler.go:11", dir())), "{t}");
    assert!(first.contains("field") && first.ends_with("Booking.CustomerPhone"), "{t}");
    // one group, each function's tree under the declaration
    assert_eq!(t.lines().filter(|l| !l.starts_with(' ') && l.contains("handler.go")).count(), 1, "{t}");
    let (d, r) = row(&t, "phone := b.CustomerPhone");
    assert_eq!(d, 1, "{t}");
    assert!(r.contains("remind") && r.ends_with("exact"), "{t}");
    assert_eq!(row(&t, "log.Printf(\"text %s\", phone)").0, 2, "{t}");
    let (d, r) = row(&t, "json.Marshal(b.CustomerPhone)");
    assert_eq!(d, 1, "{t}");
    assert!(r.contains("archive") && r.ends_with("outside"), "{t}");
    assert!(t.contains("2 routes to outside\n"), "{t}");
    // with an end only the matching function's tree stays
    let o = path_cmd("field#Booking.CustomerPhone ==> function#log.Printf", &[]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(!t.contains("archive"), "{t}");
    assert!(t.contains("1 route to sink\n"), "{t}");
}

#[test]
fn one_hop_and_no_route_exit_1() {
    // `=>` does not follow the value through other statements or calls
    let o = path_cmd("function#handle variable#body => function#log.Printf", &[]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(out(&o).contains("0 routes to sink"));
    let o = path_cmd("function#handle variable#body ==> function#fmt.Println", &[]);
    assert_eq!(code(&o), 1, "{}", out(&o));
}

#[test]
fn backward_with_no_matching_origin_exits_1_and_mixed_queries_exit_2() {
    let o = path_cmd("function#handle variable#body <== function#log.Printf", &[]);
    assert_eq!(code(&o), 1);
    let o = path_cmd("function#handle variable#body ==> function#a --> function#b", &[]);
    assert_eq!(code(&o), 2);
}

#[test]
fn max_bounds_the_depth_of_calls_followed() {
    let o = path_cmd("function#handle variable#body ==> function#log.Printf", &["--max", "1"]);
    let t = out(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("func first(s string)"), "{t}");
    assert!(!t.contains("func second(s string)"), "{t}");
    assert!(row(&t, "t := second(s)").1.ends_with("derived, via second"), "{t}");
}
