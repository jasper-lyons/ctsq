//! Review fixes over fixtures/flow/fix-a/: writes through pointer parameters,
//! pointer receivers, local aliases and captured outer variables reach the
//! caller, multi-result calls track only the positions a return carries, and
//! a chained selector `b.Contact.Phone` belongs to `Contact`, not `Booking`.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/fix-a", env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).args(args).arg(dir()).output().expect("failed to run ctsq")
}

fn path_cmd(query: &str) -> (Option<i32>, String) {
    let o = run(&["path", query]);
    (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string())
}

fn one_route(func: &str, sink_row: &str) -> String {
    let (code, t) = path_cmd(&format!("function#{func} param#x ==> function#Println"));
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
    let l = t.lines().find(|l| l.contains(sink_row)).unwrap_or_else(|| panic!("no row {sink_row}\n{t}"));
    assert!(l.ends_with("sink"), "{t}");
    t
}

#[test]
fn a_write_through_a_pointer_parameter_reaches_the_caller() {
    let t = one_route("ParamWrite", "fmt.Println(b.Phone)");
    assert!(t.contains("fill(&b, x)  moved, via fill"), "{t}");
}

#[test]
fn a_write_through_a_pointer_receiver_reaches_the_caller() {
    let t = one_route("ReceiverWrite", "fmt.Println(b.Phone)");
    assert!(t.contains("b.SetPhone(x)  moved, via Booking.SetPhone"), "{t}");
}

#[test]
fn a_write_through_a_copied_pointer_reaches_the_original() {
    let t = one_route("AliasWrite", "fmt.Println(b.Phone)");
    assert!(t.contains("c.Phone = x  exact, field Phone"), "{t}");
}

#[test]
fn a_write_through_a_deref_reaches_the_pointee() {
    let t = one_route("DerefWrite", "fmt.Println(y)");
    assert!(t.contains("*p = x  exact"), "{t}");
    assert!(!t.contains("stopped"), "{t}");
}

#[test]
fn a_write_to_a_captured_variable_reaches_the_enclosing_function() {
    let t = one_route("CaptureWrite", "fmt.Println(out)");
    assert!(t.contains("out = x  exact"), "{t}");
}

#[test]
fn a_deref_write_through_a_pointer_of_unknown_origin_is_tagged() {
    let t = one_route("UnknownPointer", "fmt.Println(*p)");
    assert!(t.contains("*p = x  exact, stopped: pointer write"), "{t}");
}

#[test]
fn only_the_result_position_a_return_carries_is_tracked() {
    let (code, t) = path_cmd("function#SecondResult param#x ==> function#Println");
    assert_eq!(code, Some(1), "{t}");
    assert!(t.contains("\n0 routes to sink\n"), "{t}");
    one_route("FirstResult", "fmt.Println(v)");
}

#[test]
fn a_bare_return_hands_back_the_named_results() {
    let t = one_route("NamedResults", "fmt.Println(v)");
    assert!(!t.contains("fmt.Println(err)"), "{t}");
    assert!(t.contains("return  moved"), "{t}");
}

#[test]
fn a_chained_selector_belongs_to_the_field_type() {
    let o = run(&["-q", "field#Booking.Phone"]);
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(!t.contains("b.Contact.Phone"), "{t}");
    assert!(t.contains("writes.go:83: fmt.Println(b.Phone)"), "{t}");
    let o = run(&["-q", "field#Contact.Phone"]);
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(t.contains("writes.go:82: fmt.Println(b.Contact.Phone)"), "{t}");
}

#[test]
fn a_field_start_is_one_group_without_the_chained_read() {
    let (code, t) = path_cmd("field#Booking.Phone ==> function#Println");
    assert_eq!(code, Some(0), "{t}");
    let roots = t.lines().filter(|l| !l.starts_with(' ') && l.contains(" field ")).count();
    assert_eq!(roots, 1, "{t}");
    assert!(!t.contains("b.Contact.Phone"), "{t}");
    let (_, t) = path_cmd("field#Contact.Phone ==> function#Println");
    assert!(t.contains("fmt.Println(b.Contact.Phone)  sink"), "{t}");
}
