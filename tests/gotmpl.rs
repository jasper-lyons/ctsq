//! Go templates through the binary, against `fixtures/gotmpl/`.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/gotmpl", env!("CARGO_MANIFEST_DIR"))
}

fn ctsq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(args)
        .output()
        .expect("failed to run ctsq")
}

/// Every printed row as `file:line: text`, with the fixture directory stripped.
fn rows(args: &[&str]) -> Vec<String> {
    let out = ctsq(args);
    assert!(out.status.success(), "ctsq {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    let prefix = format!("{}/", dir());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.strip_prefix(&prefix).unwrap_or(l).to_string())
        .collect()
}

#[test]
fn template_action_lists_printing_actions_only() {
    let d = dir();
    let got = rows(&["-q", "template_action", "--node", &d]);
    assert_eq!(
        got,
        vec![
            "page.html.tmpl:3: \"{{.Phone}}\"",
            "page.html.tmpl:4: \"{{.X.Label}}\"",
            "page.html.tmpl:5: \"{{$n}}\"",
            "page.html.tmpl:6: \"{{printf \\\"%s\\\" .Title | html}}\"",
        ]
    );
    let all = got.join("\n");
    for not in ["range", "if", "define", "block", "template \\\"", ":="] {
        assert!(!all.contains(not), "{} printed: {}", not, all);
    }
}

#[test]
fn template_action_rows_keep_the_line_format() {
    let d = dir();
    let got = rows(&["-q", "template_action", &d]);
    assert_eq!(got[0], "page.html.tmpl:3: {{range .Items}}<p>{{.Phone}}</p>{{end}}");
}

#[test]
fn ref_field_finds_phone() {
    let d = dir();
    let got = rows(&["-q", "&field#Phone", "--node", &d]);
    assert_eq!(got, vec!["page.html.tmpl:3: \".Phone\""]);
}

#[test]
fn chained_field_matches_its_last_part() {
    let d = dir();
    let got = rows(&["-q", "&field#Label", "--node", &d]);
    assert_eq!(got, vec!["page.html.tmpl:4: \".X.Label\""]);
}

#[test]
fn define_and_block_are_function_definitions() {
    let d = dir();
    let got = rows(&["-q", "*function#/body/", &d]);
    assert_eq!(got.len(), 2, "{:?}", got);
    assert!(got[0].starts_with("layout.html.tmpl:1:"));
    assert!(got[1].starts_with("page.html.tmpl:2:"));
}

#[test]
fn template_call_and_functions_are_calls() {
    let d = dir();
    let got = rows(&["-q", "call", "--node", &d]);
    assert!(got[0].starts_with("page.html.tmpl:1: \"{{template"), "{:?}", got);
    assert!(got.iter().any(|r| r.contains("printf")), "{:?}", got);
}

#[test]
fn variable_definition_and_use() {
    let d = dir();
    assert_eq!(rows(&["-q", "*variable#n", "--node", &d]), vec!["page.html.tmpl:5: \"$n := .Name\""]);
    let uses = rows(&["-q", "&variable#n", "--node", &d]);
    assert!(uses.iter().all(|r| r == "page.html.tmpl:5: \"$n\""), "{:?}", uses);
}

#[test]
fn tree_runs_on_templates() {
    let d = dir();
    for f in ["page.html.tmpl", "layout.html.tmpl"] {
        let out = ctsq(&["tree", &format!("{}/{}", d, f)]);
        assert!(out.status.success(), "tree {} failed: {}", f, String::from_utf8_lossy(&out.stderr));
    }
    let out = ctsq(&["tree", &d]);
    assert!(out.status.success());
}

#[test]
fn template_action_is_unknown_to_go() {
    let f = format!("{}/fixtures/example.go", env!("CARGO_MANIFEST_DIR"));
    let out = ctsq(&["-q", "template_action", &f]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown kind 'template_action'"));
}
