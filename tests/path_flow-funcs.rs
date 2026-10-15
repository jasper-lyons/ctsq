//! `ctsq path` through function values and closures, over
//! fixtures/flow/flow-funcs/: a package-level `var sendConfirmation =
//! sendEmail`, a local `f := helper`, variables assigned twice (package level
//! and local) that stop, a `Template(name, data)` that returns a closure
//! passing data to `fmt.Fprintf`, a literal called in place, and a call
//! through an interface with no implementation.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/flow-funcs", env!("CARGO_MANIFEST_DIR"))
}

fn path_cmd(query: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).arg("path").arg(query).arg(dir()).output().expect("failed to run ctsq")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// The row whose text contains `needle`, with its indent depth.
fn row<'t>(t: &'t str, needle: &str) -> (usize, &'t str) {
    let l = t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}"));
    ((l.len() - l.trim_start().len()) / 2, l.trim_start())
}

#[test]
fn a_package_level_function_value_with_one_assignment_is_walked() {
    let o = path_cmd("function#Signup param#email ==> function#Printf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    // the test file's reassignment does not count
    let (depth, r) = row(&t, "sendConfirmation(email)");
    assert_eq!(depth, 1, "{t}");
    assert!(r.ends_with("moved, via sendEmail"), "{t}");
    let (depth, r) = row(&t, "func sendEmail(to string)");
    assert_eq!(depth, 2, "{t}");
    assert!(r.starts_with(&format!("{}/values.go:11", dir())) && r.ends_with("moved"), "{t}");
    let (depth, r) = row(&t, "log.Printf(\"mail %s\", to)");
    assert_eq!(depth, 3, "{t}");
    assert!(r.ends_with("sink"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
}

#[test]
fn a_local_function_value_is_walked() {
    let o = path_cmd("function#Local param#name ==> function#Printf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "f(name)").1.ends_with("moved, via helper"), "{t}");
    assert!(row(&t, "func helper(s string)").1.ends_with("moved"), "{t}");
    assert!(row(&t, "log.Printf(\"helper %s\", s)").1.ends_with("sink"), "{t}");
}

#[test]
fn a_package_level_function_value_assigned_twice_stops() {
    let o = path_cmd("function#Alert param#phone ==> function#Printf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    let (depth, r) = row(&t, "notify(phone)");
    assert_eq!(depth, 1, "{t}");
    assert!(r.ends_with("moved, stopped: function value"), "{t}");
    // both assignments are listed under the stop, in file order
    let lines: Vec<&str> = t.lines().collect();
    let i = lines.iter().position(|l| l.contains("notify(phone)")).unwrap();
    assert!(lines[i + 1].trim_start().starts_with(&format!("{}/values.go:9", dir())), "{t}");
    assert!(lines[i + 1].ends_with("var notify = sendEmail"), "{t}");
    assert!(lines[i + 2].trim_start().starts_with(&format!("{}/values.go:24", dir())), "{t}");
    assert!(lines[i + 2].contains("useSMS") && lines[i + 2].ends_with("notify = sendSMS"), "{t}");
    assert!(!t.contains("func sendEmail"), "{t}");
    assert!(t.contains("\n0 routes to sink\n"), "{t}");
}

#[test]
fn a_local_function_value_assigned_twice_stops() {
    let o = path_cmd("function#Twice param#name ==> function#Printf");
    let t = out(&o);
    assert!(row(&t, "g(name)").1.ends_with("moved, stopped: function value"), "{t}");
    assert_eq!(row(&t, "g := helper").0, 2, "{t}");
    assert_eq!(row(&t, "g = sendSMS").0, 2, "{t}");
}

#[test]
fn a_returned_closure_carries_the_captured_parameter() {
    let o = path_cmd("function#Template param#data ==> function#Fprintf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let (depth, r) = row(&t, "return func(w http.ResponseWriter, r *http.Request) {");
    assert_eq!(depth, 1, "{t}");
    assert!(r.starts_with(&format!("{}/closures.go:12", dir())) && r.ends_with("moved, closure"), "{t}");
    let (depth, r) = row(&t, "fmt.Fprintf(w, \"%s %v\", name, data)");
    assert_eq!(depth, 2, "{t}");
    assert!(r.ends_with("sink"), "{t}");
}

#[test]
fn a_caller_of_the_template_reaches_the_closure_body() {
    let o = path_cmd("function#Page param#user ==> function#Fprintf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "h := Template(\"page\", user)").1.ends_with("via Template"), "{t}");
    assert_eq!(row(&t, "return func(w http.ResponseWriter").0, 3, "{t}");
    assert_eq!(row(&t, "fmt.Fprintf(").0, 4, "{t}");
}

#[test]
fn a_literal_called_in_place_is_walked() {
    let o = path_cmd("function#Direct param#token ==> function#Printf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let (depth, r) = row(&t, "func() {");
    assert_eq!(depth, 1, "{t}");
    assert!(r.ends_with("moved, closure"), "{t}");
    let (depth, r) = row(&t, "log.Printf(\"token %s\", token)");
    assert_eq!(depth, 2, "{t}");
    assert!(r.ends_with("sink"), "{t}");
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn an_interface_call_with_no_implementation_stops() {
    let o = path_cmd("function#Broadcast param#msg ==>");
    let t = out(&o);
    assert!(row(&t, "n.Notify(msg)").1.ends_with("moved, stopped: interface"), "{t}");
}
