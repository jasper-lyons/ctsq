//! Acceptance: `ctsq path` over fixtures/flow/booking/, a synthetic booking
//! app combining the shapes of the earlier fixtures: a generic form decoder,
//! a StorageDriver interface with one constructed implementation, a reflection
//! insert ending in database/sql Exec, a generic JSON helper, a `%v` log line,
//! a Template closure over templates.Render and an inline email template.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/booking", env!("CARGO_MANIFEST_DIR"))
}

fn path_cmd(query: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).arg("path").arg(query).arg(dir()).output().expect("failed to run ctsq")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// The trimmed row whose text contains `needle`.
fn row<'t>(t: &'t str, needle: &str) -> &'t str {
    t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}")).trim_start()
}

#[test]
fn goal_call_route_parse_config_to_validate() {
    let o = path_cmd("function#parseConfig --> function#validate");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "parseConfig").contains("calls validate at :11"), "{t}");
    assert!(t.contains("1 route"), "{t}");
}

#[test]
fn goal_variable_to_printf_sink() {
    let o = path_cmd("function#handle variable#name ==> function#/Printf|Log/");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "fmt.Printf(\"hello %s\", name)").ends_with("sink"), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn goal_field_to_log_printf_finds_the_percent_v_line() {
    let o = path_cmd("field#Booking.CustomerPhone ==> function#log.Printf");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let r = row(&t, "log.Printf(\"%v\", b)");
    assert!(r.starts_with(&format!("{}/handlers/bookings.go:", dir())), "{t}");
    assert!(r.contains("AdminView") && r.ends_with("sink"), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn field_to_encode_finds_the_json_helper_through_embedding() {
    let o = path_cmd("field#Booking.CustomerPhone ==> function#/Encode/");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let r = row(&t, "json.NewEncoder(w).Encode(v)");
    assert!(r.starts_with(&format!("{}/handlers/handler.go:12 JSON", dir())), "{t}");
    assert!(r.ends_with("sink"), "{t}");
    // reached from the plain booking and from the AdminBooking that embeds it
    assert!(t.contains("JSON(w, b)") && t.contains("JSON(w, ab)"), "{t}");
    assert!(t.contains("2 routes to sink"), "{t}");
}

#[test]
fn field_to_template_action_reaches_the_page_and_the_inline_email() {
    let o = path_cmd("field#Booking.CustomerPhone ==> template_action");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let d = dir();
    let page = row(&t, "<p>{{.CustomerPhone}}</p>");
    assert!(page.starts_with(&format!("{d}/templates/bookings.html.tmpl:4 ")), "{t}");
    assert!(page.ends_with("exact, rendered, sink"), "{t}");
    let mail = row(&t, "we will call you on {{ .CustomerPhone }}.");
    assert!(mail.starts_with(&format!("{d}/handlers/email.go:")), "{t}");
    assert!(mail.ends_with("exact, rendered, sink"), "{t}");
    // the layout title is not the phone
    assert!(!t.contains("{{.Title}}"), "{t}");
}

#[test]
fn goal_username_field_reaches_the_user_page() {
    let o = path_cmd("field#User.Username ==> template_action");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let r = row(&t, "<p>{{.Username}}</p>");
    assert!(r.starts_with(&format!("{}/templates/user.html.tmpl:3 ", dir())), "{t}");
    assert!(r.ends_with("exact, rendered, sink"), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn no_end_lists_every_outside_leaf_and_the_trailer() {
    let o = path_cmd("field#Booking.CustomerPhone ==>");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    // the database/sql Exec at the end of the reflection insert, through the
    // interface call r.driver.Insert and its one constructed implementation
    let exec = row(&t, "db.Exec(q, args...)");
    assert!(exec.contains("outside"), "{t}");
    assert!(exec.starts_with(&format!("{}/data/booking.go:", dir())), "{t}");
    assert!(row(&t, "r.driver.Insert(\"bookings\", b)").contains("via r.driver.Insert"), "{t}");
    // the JSON helper and the log line are leaves too
    assert!(row(&t, "json.NewEncoder(w).Encode(v)").ends_with("outside"), "{t}");
    assert!(row(&t, "log.Printf(\"%v\", b)").ends_with("outside"), "{t}");
    // trailers
    assert!(t.contains("routes to outside"), "{t}");
    // reflection helpers are modelled (fixer D), so none of them ends a route
    let trailer: Vec<&str> = t.lines().filter(|l| l.contains("unmodelled call")).collect();
    assert!(trailer.iter().all(|l| !l.contains("v.Field") && !l.contains("v.Type")), "{t}");
}

/// Template(name, page) returns a closure that is handed to http.HandleFunc:
/// the closure body is walked under the call, so the page is reached.
#[test]
fn the_template_closure_reaches_the_page() {
    let o = path_cmd("field#Booking.CustomerPhone ==> template_action");
    let t = out(&o);
    assert!(t.contains("bookings.go:60"), "{t}");
}

/// A call used directly as an argument of another call
/// (`Send(to, ConfirmationEmail(b))`) is walked.
#[test]
fn a_nested_call_argument_is_walked() {
    let o = path_cmd("field#Booking.CustomerPhone ==> template_action");
    let t = out(&o);
    assert!(t.contains("a.mailer.Send(\"customer@example.com\", ConfirmationEmail(b))"), "{t}");
}
