//! Attribute filters (`[from=...]`, `[in=...]`, dot shorthand) through the
//! binary, against the fixtures in `fixtures/attrs/`.

use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!("{}/fixtures/attrs/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn ctsq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(args)
        .output()
        .expect("failed to run ctsq")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// The 1-based line numbers of every printed row, in output order.
fn lines(query: &str, file: &str) -> Vec<usize> {
    let f = fixture(file);
    let out = ctsq(&["-q", query, &f]);
    if !out.status.success() {
        return vec![];
    }
    stdout(&out)
        .lines()
        .map(|l| {
            let rest = l.strip_prefix(&f).expect("row starts with the path");
            rest[1..].split(':').next().unwrap().parse().unwrap()
        })
        .collect()
}

#[test]
fn dot_shorthand_picks_the_import_qualifier() {
    assert_eq!(lines("&function#log.Printf", "logs.go"), vec![18]);
    assert_eq!(lines("&function#fmt.Printf", "logs.go"), vec![19]);
}

#[test]
fn output_format_is_unchanged() {
    let f = fixture("logs.go");
    let out = ctsq(&["-q", "&function#log.Printf", &f]);
    assert_eq!(stdout(&out), format!("{}:18: log.Printf(\"notify %s\", u.Email)\n", f));
}

#[test]
fn from_operators_behave_as_named() {
    let q = |a: &str| lines(&format!("&function#/^(Printf|Info)$/{}", a), "logs.go");
    assert_eq!(q("[from^=lo]"), vec![18, 20]);
    assert_eq!(q("[from*=og]"), vec![18, 20]);
    assert_eq!(q("[from$=og]"), vec![18, 20]);
    assert_eq!(q("[from=/^log$/]"), vec![18]);
    assert_eq!(q("[from=fmt]"), vec![19]);
}

#[test]
fn slog_matches_by_local_name_and_by_full_path() {
    assert_eq!(lines("&function#slog.Info", "logs.go"), vec![20]);
    assert_eq!(lines("&function#Info[from=\"log/slog\"]", "logs.go"), vec![20]);
}

#[test]
fn a_miss_after_filtering_exits_1() {
    let out = ctsq(&["-q", "&function#Printf[from=nope]", &fixture("logs.go")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).starts_with("no match for"));
}

#[test]
fn field_dot_shorthand_matches_the_owning_struct() {
    // `field` is not an abstract kind yet, so the raw Go node kind stands in.
    let user = lines("field_identifier#User.Email", "logs.go");
    assert_eq!(user, vec![10, 18, 19, 20, 24]);
    let account = lines("field_identifier#Account.Email", "logs.go");
    assert_eq!(account, vec![14, 25]);
}

#[test]
fn in_and_shorthand_name_a_go_method_receiver() {
    assert_eq!(lines("*function#Notify[in=User]", "logs.go"), vec![17]);
    assert_eq!(lines("*function#User.Notify", "logs.go"), vec![17]);
    assert_eq!(lines("*function#Account.Notify", "logs.go"), Vec::<usize>::new());
}

#[test]
fn js_from_follows_a_default_import_binding() {
    assert_eq!(lines("function#info[from=pino]", "logger.js"), vec![5]);
    assert_eq!(lines("function#info[from=winston]", "logger.js"), vec![6]);
    assert_eq!(lines("&function#logger.info", "logger.js"), vec![5]);
}

#[test]
fn python_from_and_in() {
    assert_eq!(lines("&function#join[from=\"os.path\"]", "paths.py"), vec![7]);
    assert_eq!(lines("&function#info[from=logging]", "paths.py"), vec![8]);
    assert_eq!(lines("&function#lg.info", "paths.py"), vec![8]);
    assert_eq!(lines("*function#Store.save", "paths.py"), vec![6]);
}

#[test]
fn semantic_combinator_exits_2_with_the_path_hint() {
    let out = ctsq(&["-q", "function#a --> function#b", &fixture("logs.go")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("use: ctsq path '<query>'"), "{}", stderr(&out));
}

#[test]
fn attributes_off_the_last_selector_exit_2() {
    let out = ctsq(&["-q", "*function#Notify[in=User] &function#Printf", &fixture("logs.go")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("only supported on the last selector"));
}

#[test]
fn from_on_ruby_exits_2() {
    let f = format!("{}/fixtures/example.rb", env!("CARGO_MANIFEST_DIR"));
    let out = ctsq(&["-q", "function#x[from=y]", &f]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr(&out).trim(), "attribute from is not supported for ruby");
}

#[test]
fn unknown_attribute_exits_2() {
    let out = ctsq(&["-q", "function#x[colour=red]", &fixture("logs.go")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("unknown attribute 'colour'"));
}
