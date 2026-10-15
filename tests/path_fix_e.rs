//! Final review fixes over fixtures/flow/fix-e/: comparisons clear taint, a
//! mid-route outside call is tagged by its effect and is not a route, an
//! unknown kind exits 1, and the path and config help texts.

use std::process::{Command, Output};

fn dir() -> String {
    format!("{}/fixtures/flow/fix-e", env!("CARGO_MANIFEST_DIR"))
}

fn ctsq(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctsq")).args(args).output().expect("failed to run ctsq")
}

fn path(q: &str) -> (Option<i32>, String) {
    let d = dir();
    let o = ctsq(&["path", q, &d]);
    (o.status.code(), String::from_utf8_lossy(&o.stdout).to_string())
}

fn row<'t>(t: &'t str, needle: &str) -> &'t str {
    t.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row for {needle}\n{t}")).trim_start()
}

#[test]
fn a_comparison_result_does_not_carry_the_data() {
    let (code, t) = path("function#compare param#secret ==> function#fmt.Println");
    assert_eq!(code, Some(0), "{t}");
    assert!(!t.contains("fmt.Println(ok)"), "{t}");
    assert!(!t.contains("fmt.Println(less)"), "{t}");
    assert!(row(&t, "fmt.Println(secret + \"!\")").ends_with("sink"), "{t}");
    assert!(t.contains("\n1 route to sink\n"), "{t}");
}

#[test]
fn a_mid_route_outside_call_is_tagged_by_its_effect() {
    let (code, t) = path("function#chain param#secret ==>");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "up := strings.ToUpper(secret)").ends_with("derived"), "{t}");
    assert!(row(&t, "fmt.Println(up)").ends_with("outside"), "{t}");
    assert!(t.contains("\n1 route to outside\n"), "{t}");

    let (code, t) = path("function#unknown param#secret ==>");
    assert_eq!(code, Some(0), "{t}");
    assert!(row(&t, "v := y.Do(secret)").ends_with("derived, unmodelled"), "{t}");
    assert!(row(&t, "fmt.Println(v)").ends_with("outside"), "{t}");
    assert!(t.contains("\n1 route to outside\n"), "{t}");
    assert!(t.contains("\n1 route passed through unmodelled calls\n"), "{t}");
}

#[test]
fn an_unknown_kind_exits_1_as_with_q() {
    for q in ["bogus#x --> function#y", "bogus#x ==> function#y", "bogus#x ==>"] {
        let (code, _) = path(q);
        assert_eq!(code, Some(1), "{q}");
    }
    let (code, _) = path("function#compare");
    assert_eq!(code, Some(2), "no arrow is a usage error");
}

#[test]
fn no_arrow_names_all_eight() {
    let d = dir();
    let o = ctsq(&["path", "function#compare", &d]);
    let e = String::from_utf8_lossy(&o.stderr);
    for a in ["->", "-->", "<-", "<--", "=>", "==>", "<=", "<=="] {
        assert!(e.split_whitespace().any(|w| w == a), "{a}: {e}");
    }
}

#[test]
fn path_help_lists_arrows_tags_examples_and_exit_codes() {
    let o = ctsq(&["path", "--help"]);
    let h = String::from_utf8_lossy(&o.stdout);
    for a in ["->", "-->", "<-", "<--", "=>", "==>", "<=", "<=="] {
        assert!(h.lines().any(|l| l.trim_start().starts_with(&format!("{a} "))), "{a}: {h}");
    }
    for t in [
        "source", "exact", "derived", "moved", "field F", "conditional@L", "widened", "unmodelled", "via F", "sink",
        "rendered", "outside", "stopped: reason",
    ] {
        assert!(h.lines().any(|l| l.trim_start().starts_with(t)), "{t}: {h}");
    }
    assert!(h.contains("ctsq path 'field#Booking.CustomerPhone ==> function#log.Printf' ."), "{h}");
    assert!(h.contains("ctsq path 'function#main --> function#save' ."), "{h}");
    assert!(h.contains("Exit codes: 0 routes found"), "{h}");
    assert!(h.contains("how many functions deep a data-flow walk follows"), "{h}");
    assert!(h.contains("call routes only"), "{h}");
}

#[test]
fn config_help_hides_scan_flags_and_names_xdg() {
    let o = ctsq(&["config", "--help"]);
    let h = String::from_utf8_lossy(&o.stdout);
    assert!(!h.contains("--lang"), "{h}");
    assert!(!h.contains("--no-ignore"), "{h}");
    assert!(h.contains("$XDG_CONFIG_HOME"), "{h}");
    assert!(h.contains("$XDG_CACHE_HOME"), "{h}");
    // still accepted, and still global for the other subcommands
    assert_eq!(ctsq(&["config", "path", "-l", "go"]).status.code(), Some(0));
    assert!(String::from_utf8_lossy(&ctsq(&["tree", "--help"]).stdout).contains("--lang"));
}
