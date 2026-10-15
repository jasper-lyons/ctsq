//! `ctsq path` through Go templates over fixtures/flow/templates/: handlers
//! pass an `Item` phone to `templates.Render`, which parses the page named by
//! its parameter plus `layout.html.tmpl` from an `embed.FS` and executes it.
//! The layout calls `{{block "body" .}}`, the page defines `body` and ranges
//! over `.Items` printing `{{.Phone}}`. An inline email template parsed from
//! a Go string constant prints `{{ .Phone }}` too.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/templates", env!("CARGO_MANIFEST_DIR"))
}

fn path_cmd(query: &str, at: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).arg("path").arg(query).arg(at).output().expect("failed to run ctsq")
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
fn a_field_reaches_the_page_line_through_layout_and_body() {
    let o = path_cmd("field#Item.Phone ==> template_action", &dir());
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let d = dir();
    let (exec, r) = row(&t, "return t.Execute(w, ctx)");
    assert!(r.ends_with("via t.Execute"), "{t}");
    let (call, r) = row(&t, "{{template \"layout.html.tmpl\" .}}  exact, via layout.html.tmpl");
    assert!(r.starts_with(&format!("{}/templates/page.html.tmpl:1 ", d)), "{t}");
    let (block, r) = row(&t, "{{block \"body\" .}}");
    assert!(r.starts_with(&format!("{}/templates/layout.html.tmpl:4 ", d)), "{t}");
    assert!(r.ends_with("exact, via body"), "{t}");
    let (define, r) = row(&t, "{{define \"body\"}}");
    assert!(r.ends_with("moved"), "{t}");
    let (range, r) = row(&t, "{{range .Items}}");
    assert!(r.ends_with("exact"), "{t}");
    let (print, r) = row(&t, "<p>{{.Phone}}</p>");
    assert!(r.starts_with(&format!("{}/templates/page.html.tmpl:4 body", d)), "{t}");
    assert!(r.ends_with("exact, rendered, sink"), "{t}");
    assert!(exec < call && call < block && block < define && define < range && range < print, "{t}");
    // the admin page only prints the name, which holds no phone
    assert!(!t.contains("admin.html.tmpl"), "{t}");
    assert!(!t.contains("{{.Title}}"), "{t}");
}

#[test]
fn an_inline_template_points_at_the_go_string() {
    let o = path_cmd("field#Item.Phone ==> template_action", &dir());
    let t = out(&o);
    let d = dir();
    let (_, r) = row(&t, "we will call you on {{ .Phone }}.");
    assert!(r.starts_with(&format!("{}/handlers/email.go:9 email", d)), "{t}");
    assert!(r.ends_with("exact, rendered, sink"), "{t}");
    assert!(!t.contains("Hello {{ .Name }},  exact"), "{t}");
    assert!(t.contains("2 routes to sink"), "{t}");
}

#[test]
fn a_named_end_and_a_scan_without_template_files() {
    let o = path_cmd("field#Item.Phone ==> template_action#Name", &dir());
    assert_eq!(o.status.code(), Some(1), "{}", out(&o));
    // only Go files scanned: the end still parses and the inline template is reached
    let o = path_cmd("field#Item.Phone ==> template_action", &format!("{}/handlers", dir()));
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("1 route to sink"), "{t}");
}

#[test]
fn open_end_mode_makes_rendered_prints_leaves() {
    let o = path_cmd("field#Item.Phone ==>", &dir());
    let t = out(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let (_, r) = row(&t, "<p>{{.Phone}}</p>");
    assert!(r.ends_with("exact, rendered, outside"), "{t}");
}
