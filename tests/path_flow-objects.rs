//! `ctsq path` object tracking through the binary over
//! fixtures/flow/flow-objects/: a JSON form decoded by a generic
//! `ExtractForm[T]`, the phone stored as `Booking{CustomerPhone: phone}`, the
//! booking handed to a repository and a reflection helper, logged with `%v`,
//! stored in a map, and sent through a generic `JSON[T]` to `Encode`. An
//! `AdminBooking` reads the promoted `.CustomerPhone`.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/flow-objects", env!("CARGO_MANIFEST_DIR"))
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
fn a_field_reaches_the_log_line_and_the_json_encoder() {
    let o = path_cmd("field#Booking.CustomerPhone ==> function#/Printf|Encode/");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let d = dir();
    assert!(t.starts_with(&format!("{}/booking.go:11", d)), "{t}");
    // the composite literal stores the field, with its struct tag
    let (depth, r) = row(&t, "b := Booking{CustomerPhone: phone}");
    assert_eq!(depth, 1, "{t}");
    assert!(r.ends_with("exact, field CustomerPhone, tag json:\"customer_phone\" sql:\"customer_phone\""), "{t}");
    // %v of the object holding the field
    let (depth, r) = row(&t, "log.Printf(\"created %v\", b)");
    assert_eq!(depth, 2, "{t}");
    assert!(r.ends_with("sink"), "{t}");
    // the generic JSON[T] is walked into and reaches Encode
    assert!(row(&t, "JSON[Booking](w, b)").1.ends_with("moved, via JSON"), "{t}");
    assert!(row(&t, "func JSON[T any]").1.starts_with(&format!("{}/form.go:19", d)), "{t}");
    let (depth, r) = row(&t, "json.NewEncoder(w).Encode(v)");
    assert_eq!(depth, 4, "{t}");
    assert!(r.ends_with("sink"), "{t}");
    // the embedded struct's promoted field is read
    let (depth, r) = row(&t, "phone := ab.CustomerPhone");
    assert_eq!(depth, 1, "{t}");
    assert!(r.contains("adminView") && r.ends_with("exact, tag json:\"customer_phone\" sql:\"customer_phone\""), "{t}");
    assert!(row(&t, "log.Printf(\"admin %s\", phone)").1.ends_with("sink"), "{t}");
    assert!(t.contains("\n3 routes to sink\n"), "{t}");
}

#[test]
fn a_map_store_stops_and_lists_its_readers() {
    let o = path_cmd("field#Booking.CustomerPhone ==> function#/Printf|Encode/");
    let t = out(&o);
    let (depth, r) = row(&t, "cache[phone] = b");
    assert_eq!(depth, 2, "{t}");
    assert!(r.ends_with("exact, stopped: shared map, 1 reader"), "{t}");
    // the reader row sits under the stop
    let lines: Vec<&str> = t.lines().collect();
    let i = lines.iter().position(|l| l.contains("cache[phone] = b")).unwrap();
    assert!(lines[i + 1].trim_start().starts_with(&format!("{}/handlers.go:32", dir())), "{t}");
    assert!(lines[i + 1].trim_end().ends_with("lookup"), "{t}");
}

#[test]
fn the_request_flows_through_the_generic_form_decoder() {
    let o = path_cmd("function#createBooking param#r ==> function#/Printf|Encode/");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "form, err := ExtractForm[Form](r)").1.ends_with("derived, via ExtractForm"), "{t}");
    assert!(row(&t, "func ExtractForm[T any]").1.ends_with("moved"), "{t}");
    // Decode moves the body into v, which comes back
    assert!(row(&t, "err := json.NewDecoder(r.Body).Decode(&v)").1.ends_with("moved"), "{t}");
    assert!(row(&t, "return v, err").1.ends_with("moved"), "{t}");
    // the form field read shows its json tag
    assert!(row(&t, "phone := normalise(form.Phone)").1.contains("tag json:\"phone\""), "{t}");
    assert!(row(&t, "log.Printf(\"created %v\", b)").1.ends_with("sink"), "{t}");
    assert!(row(&t, "json.NewEncoder(w).Encode(v)").1.ends_with("sink"), "{t}");
    assert!(t.contains("\n2 routes to sink\n"), "{t}");
}

#[test]
fn with_no_end_the_repository_and_reflection_helper_are_walked() {
    let o = path_cmd("field#Booking.CustomerPhone ==>");
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(row(&t, "repo.Create(b)").1.ends_with("moved, via repo.Create"), "{t}");
    assert!(row(&t, "return insert(r.db, \"bookings\", b)").1.ends_with("via insert"), "{t}");
    assert!(row(&t, "rv := reflect.ValueOf(v)").1.ends_with("outside"), "{t}");
    assert!(row(&t, "db.Exec(").1.ends_with("outside"), "{t}");
    assert!(row(&t, "json.NewEncoder(w).Encode(v)").1.ends_with("outside"), "{t}");
}
