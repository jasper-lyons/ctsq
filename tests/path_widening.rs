//! `ctsq path` through interface calls, over fixtures/flow/widening/
//! (module example.com/wd): a `Store` interface with three implementations of
//! which `dbStore` and `MemStore` are constructed and `ghostStore` is not, a
//! type assertion and a type switch that narrow, a `Sink` with outside
//! implementations, and a `Notifier` nothing constructs.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/widening", env!("CARGO_MANIFEST_DIR"))
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

#[test]
fn a_tracked_value_widens_to_the_constructed_implementations_only() {
    let o = path_cmd("function#run param#x ==> function#Println", &[]);
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("func (d *dbStore) Save(v string) error {  moved, widened"), "{t}");
    assert!(t.contains("func (m *MemStore) Save(v string) error {  moved, widened"), "{t}");
    assert!(!t.contains("ghostStore"), "{t}");
    // each reaches its own sink
    assert!(t.contains("fmt.Println(\"db\", v)  sink"), "{t}");
    assert!(t.contains("log.Println(\"mem\", v)  sink"), "{t}");
    assert!(t.contains("2 routes to sink"), "{t}");
}

#[test]
fn a_type_assertion_narrows_to_one_implementation() {
    let o = path_cmd("function#narrow param#x ==> function#Println", &[]);
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("m.Save(x)  moved, via MemStore.Save"), "{t}");
    assert!(!t.contains("dbStore"), "{t}");
    assert!(!t.contains("widened"), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn a_type_switch_case_narrows_and_default_widens() {
    let o = path_cmd("function#pick param#x ==> function#Println", &[]);
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("t.Save(x)  moved, via dbStore.Save"), "{t}");
    assert!(t.contains("s.Save(x)  moved, via Store.Save"), "{t}");
    assert!(t.contains("3 routes to sink"), "{t}");
}

#[test]
fn call_routes_list_only_constructed_implementations() {
    let o = path_cmd("function#main --> function#Save", &[]);
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("dbStore.Save") && t.contains("MemStore.Save"), "{t}");
    assert!(!t.contains("ghostStore"), "{t}");
    // run reaches both, narrow reaches one exactly
    assert!(t.contains("calls dbStore.Save? at :11, calls MemStore.Save? at :11"), "{t}");
    assert!(t.contains("calls MemStore.Save at :16"), "{t}");
}

#[test]
fn outside_implementations_are_counted_and_listed_only_with_expand() {
    let q = "function#emit param#data ==> function#Println";
    let t = out(&path_cmd(q, &[]));
    assert!(t.contains("widened: 1 in repo, 2 outside"), "{t}");
    assert!(!t.contains("outside implementation"), "{t}");
    let o = path_cmd(q, &["--expand"]);
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("&bytes.Buffer{}  outside implementation of Sink"), "{t}");
    assert!(t.contains("&strings.Builder{}  outside implementation of Sink"), "{t}");
}

#[test]
fn widened_rows_match_only_widened() {
    let t = out(&path_cmd("function#emit param#data ==> function#Println", &["--only", "widened"]));
    assert!(t.contains("widened: 1 in repo, 2 outside"), "{t}");
}

#[test]
fn an_interface_nothing_constructs_still_stops() {
    let o = path_cmd("function#tell param#x ==> function#Println", &[]);
    let t = out(&o);
    assert!(t.contains("n.Notify(x)  moved, stopped: interface"), "{t}");
    assert!(!t.contains("quietNotifier"), "{t}");
}
