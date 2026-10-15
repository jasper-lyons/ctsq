//! Acceptance fixes over fixtures/flow/fix-d/: a method on a struct field
//! keeps the field's package (`d.db.Exec` on `db *sql.DB` is outside), a
//! template's writer carries what it renders, reflection and sql.Result
//! helpers are modelled, and an unmodelled result nobody reads does not end
//! a route.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/fix-d", env!("CARGO_MANIFEST_DIR"))
}

fn path(q: &str) -> (Option<i32>, String) {
    let o: Output =
        Command::new(env!("CARGO_BIN_EXE_ctsq")).arg("path").arg(q).arg(dir()).output().expect("failed to run ctsq");
    (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string())
}

fn row<'t>(t: &'t str, needle: &str) -> &'t str {
    t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}")).trim_start()
}

#[test]
fn a_method_on_a_struct_field_of_an_outside_type_is_an_outside_leaf() {
    let (code, t) = path("function#Save param#phone ==>");
    assert_eq!(code, Some(0), "{t}");
    let exec = row(&t, "return d.db.Exec(q, args...)");
    assert!(exec.starts_with(&format!("{}/store.go:15 Exec", dir())), "{t}");
    assert!(exec.ends_with("moved, outside"), "{t}");
    assert!(!exec.contains("via"), "{t}");
    assert!(t.contains("\n1 route to outside\n"), "{t}");
    // res.RowsAffected() is modelled as clear, so nothing is unmodelled
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn a_method_on_a_struct_field_is_a_sink_for_its_name() {
    let (code, t) = path("function#Exec param#q ==> function#Exec");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "return d.db.Exec(q, args...)").ends_with("sink"), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn a_rendered_template_writer_carries_the_data() {
    let (code, t) = path("function#Render param#phone ==> function#send");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "mailTmpl.Execute(&buf, Mail{Phone: phone})").contains("via mailTmpl.Execute"), "{t}");
    assert!(row(&t, "send(buf.String())").ends_with("sink"), "{t}");
}

#[test]
fn an_unlinked_template_writer_carries_the_data_through_the_model() {
    let (code, t) = path("function#RenderWith param#phone ==> function#send");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "send(buf.String())").ends_with("sink"), "{t}");
}

#[test]
fn reflection_helpers_are_modelled() {
    let (code, t) = path("function#Fields param#row ==> function#Printf");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "v.Field(i).Interface()").ends_with("sink"), "{t}");
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn an_unmodelled_result_never_read_does_not_end_a_route() {
    let (_, t) = path("function#Forget param#phone ==> function#Printf");
    assert!(!t.contains("unmodelled call"), "{t}");
    // read later, it is a route through the unmodelled call
    let (code, t) = path("function#Keep param#phone ==> function#Printf");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "res, err := ext.Do(phone)").contains("unmodelled"), "{t}");
    assert!(row(&t, "log.Printf(\"%v\", res)").ends_with("sink"), "{t}");
}

#[test]
fn a_generic_field_takes_its_constraint_type() {
    // `driver D` with `D Driver`: result.RowsAffected() on the Exec result
    // is the modelled sql.Result method, not an unmodelled end
    let (code, t) = path("function#Add param#phone ==>");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "return d.db.Exec(q, args...)").ends_with("outside"), "{t}");
    assert!(!t.contains("unmodelled"), "{t}");
}

#[test]
fn a_template_sink_still_writes_its_buffer() {
    // the end matches Execute too, the buffer it renders into still carries on
    let (code, t) = path("function#Notify param#phone ==> function#/SendEmail|Exec/");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "mailTmpl.Execute(&buf").ends_with("sink"), "{t}");
    assert!(row(&t, "c.SendEmail(buf.String())").ends_with("sink"), "{t}");
    assert!(t.contains("2 routes to sink"), "{t}");
}

#[test]
fn a_variable_bound_from_an_outside_call_is_outside() {
    let (code, t) = path("function#Notify param#phone ==>");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "c.SendEmail(buf.String())").ends_with("outside"), "{t}");
    assert!(!t.contains("unmodelled call"), "{t}");
}
