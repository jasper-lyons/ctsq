//! The abstract `field` kind through the binary, against `fixtures/field/`.

use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!("{}/fixtures/field/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn ctsq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(args)
        .output()
        .expect("failed to run ctsq")
}

/// The 1-based line numbers of every printed row.
fn lines(query: &str, file: &str) -> Vec<usize> {
    let f = fixture(file);
    let out = ctsq(&["-q", query, &f]);
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l[f.len() + 1..].split(':').next().unwrap().parse().unwrap())
        .collect()
}

#[test]
fn go_field_matches_attrs_rows() {
    let f = format!("{}/fixtures/attrs/logs.go", env!("CARGO_MANIFEST_DIR"));
    let out = ctsq(&["-q", "field#User.Email", &f]);
    let rows: Vec<usize> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l[f.len() + 1..].split(':').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(rows, vec![10, 18, 19, 20, 24]);
    let out = ctsq(&["-q", "field#Account.Email", &f]);
    assert!(out.status.success());
}

#[test]
fn go_sigils_split_declarations_from_accesses() {
    let f = format!("{}/fixtures/attrs/logs.go", env!("CARGO_MANIFEST_DIR"));
    let run = |q: &str| -> Vec<usize> {
        let out = ctsq(&["-q", q, &f]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l[f.len() + 1..].split(':').next().unwrap().parse().unwrap())
            .collect()
    };
    assert_eq!(run("*field#Email"), vec![10, 14]);
    assert_eq!(run("&field#Email"), vec![18, 19, 20, 24, 25]);
}

#[test]
fn javascript_class_fields_and_member_accesses() {
    assert_eq!(lines("*field#email", "user.js"), vec![2]);
    assert_eq!(lines("&field#email", "user.js"), vec![5, 9]);
}

#[test]
fn typescript_class_and_interface_fields() {
    assert_eq!(lines("*field#email", "user.ts"), vec![2, 5]);
    assert_eq!(lines("&field#email", "user.ts"), vec![7]);
}

#[test]
fn python_self_assignment_and_attribute_access() {
    assert_eq!(lines("*field#email", "user.py"), vec![3]);
    assert_eq!(lines("&field#email", "user.py"), vec![3, 6]);
}

#[test]
fn rust_struct_fields_and_field_expressions() {
    assert_eq!(lines("*field#email", "user.rs"), vec![2]);
    assert_eq!(lines("&field#email", "user.rs"), vec![6]);
}

#[test]
fn c_and_cpp_members() {
    assert_eq!(lines("*field#email", "user.c"), vec![2]);
    assert_eq!(lines("*field#age", "user.c"), vec![3]);
    assert_eq!(lines("&field#age", "user.c"), vec![7]);
    assert_eq!(lines("*field#age", "user.cpp"), vec![2]);
    assert_eq!(lines("&field#age", "user.cpp"), vec![6]);
}

#[test]
fn ruby_instance_variables() {
    assert_eq!(lines("*field#\"@email\"", "user.rb"), vec![3]);
    assert_eq!(lines("field#\"@email\"", "user.rb"), vec![3, 7]);
}
