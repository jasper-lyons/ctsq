//! Acceptance fixes over fixtures/flow/fix-c/: error results of calls carry
//! no tracked data, calls nested in another call's arguments are walked, and
//! a loop that writes through a slice element reaches its fixed point.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/fix-c", env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).args(args).arg(dir()).output().expect("failed to run ctsq")
}

fn flow(func: &str, param: &str) -> (Option<i32>, String) {
    let q = format!("function#{func} param#{param} ==> function#Printf");
    let o = run(&["path", &q]);
    (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string())
}

fn sinks(t: &str) -> Vec<&str> {
    t.lines().filter(|l| l.ends_with("  sink") || l.ends_with(", sink")).collect()
}

#[test]
fn an_error_result_of_an_in_repo_call_is_not_tracked() {
    let (code, t) = flow("Handle", "b");
    assert_eq!(code, Some(0), "{t}");
    let s = sinks(&t);
    assert_eq!(s.len(), 1, "{t}");
    assert!(s[0].contains(r#"log.Printf("%v", created)"#), "{t}");
    assert!(!t.contains("fail %v"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
}

#[test]
fn an_error_result_of_an_outside_call_is_not_tracked() {
    let (code, t) = flow("Direct", "b");
    assert_eq!(code, Some(0), "{t}");
    let s = sinks(&t);
    assert_eq!(s.len(), 1, "{t}");
    assert!(s[0].contains(r#"log.Printf("%v", res)"#), "{t}");
}

#[test]
fn errorf_of_tracked_data_is_still_derived() {
    let (code, t) = flow("Wrap", "b");
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains(r#"err := fmt.Errorf("bad %v", b.Phone)  derived"#), "{t}");
    assert_eq!(sinks(&t).len(), 1, "{t}");
}

#[test]
fn a_call_nested_in_an_argument_is_walked() {
    let (code, t) = flow("Confirm", "b");
    assert_eq!(code, Some(0), "{t}");
    assert!(t.contains("func ConfirmationEmail(b Booking) string {  moved"), "{t}");
    assert!(t.contains(r#"return "Hi " + b.Phone  moved"#), "{t}");
    assert!(t.contains("func (m *mailer) Send(subj, body string) {  moved"), "{t}");
    assert_eq!(sinks(&t).len(), 1, "{t}");
}

#[test]
fn calls_nested_two_deep_are_walked_innermost_first() {
    let (code, t) = flow("UpdateAll", "bs");
    assert_eq!(code, Some(0), "{t}");
    let pos = |needle: &str| t.find(needle).unwrap_or_else(|| panic!("no {needle}\n{t}"));
    let inner = pos("func ToInterfaceSlice(");
    let mid = pos("func ObjectsToUpdateParams(");
    let outer = pos("func (d *driver) Update(");
    assert!(inner < mid && mid < outer, "{t}");
    assert_eq!(sinks(&t).len(), 1, "{t}");
}

#[test]
fn a_loop_writing_slice_elements_converges() {
    for f in ["Create", "CreateIndexed"] {
        let (code, t) = flow(f, "phone");
        assert_eq!(code, Some(0), "{f}\n{t}");
        assert!(!t.contains("loop cap"), "{f}\n{t}");
        assert!(t.contains(r#"a.driver.Insert("insert", bookings...)"#), "{f}\n{t}");
        assert!(t.contains("\n1 route to sink\n"), "{f}\n{t}");
    }
}
