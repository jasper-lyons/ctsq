use super::*;
use crate::config::Mechanics;

fn mech() -> Mechanics {
    toml::from_str(include_str!("../../config/languages/go.toml")).unwrap()
}

fn parse(src: &str) -> tree_sitter::Tree {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_go::LANGUAGE.into()).unwrap();
    p.parse(src, None).unwrap()
}

fn is_sink(n: Node, src: &[u8]) -> bool {
    n.child_by_field_name("function").is_some_and(|f| f.utf8_text(src).unwrap() == "sink")
}

/// Run from `ident` at its first occurrence after `anchor`, inside the function `func`.
fn run_with(src: &str, anchor: &str, ident: &str, func: &str, outside: &dyn Outside) -> FlowResult {
    let m = mech();
    let tree = parse(src);
    let md = Module::build(&[("a.go", tree.root_node(), src.as_bytes())], &m);
    let cx = FlowCtx { file: "a.go", src: src.as_bytes(), root: tree.root_node(), mech: &m, module: &md };
    let a = src.find(anchor).expect("anchor");
    let sb = a + src[a..].find(ident).expect("ident");
    let start = start_at(&cx, sb, sb + ident.len()).expect("start");
    let fb = src.find(&format!("func {}(", func)).expect("func") + 1;
    let f = enclosing_function(cx.root, &m, fb).expect("function");
    analyse_function(&cx, f, &start, &is_sink, outside)
}

fn run(src: &str, anchor: &str, ident: &str) -> FlowResult {
    run_with(src, anchor, ident, "F", &no_models)
}

/// The hop whose source line contains `needle`.
fn hop<'r>(r: &'r FlowResult, needle: &str) -> Option<&'r Hop> {
    r.root.walk().into_iter().skip(1).find(|h| h.text.contains(needle))
}

fn tags(r: &FlowResult, needle: &str) -> Vec<String> {
    hop(r, needle).unwrap_or_else(|| panic!("no hop for {needle}\n{}", report(&[r.clone()]))).tags.clone()
}

#[test]
fn exact_copy_reaches_sink() {
    let src = "package a\nfunc F(t string) {\n\ta := t\n\tsink(a)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(r.root.tags, vec!["source"]);
    assert_eq!(tags(&r, "a := t"), vec!["exact"]);
    assert_eq!(tags(&r, "sink(a)"), vec!["sink"]);
    // the sink hangs under the copy, the copy under the source
    let copy = &r.root.children[0];
    assert_eq!(copy.line, 3);
    assert_eq!(copy.children[0].line, 4);
    assert_eq!(copy.func, "F");
    assert_eq!(r.sinks(), 1);
}

#[test]
fn concat_is_derived() {
    let src = "package a\nfunc F(t string) {\n\tb := \"x\" + t\n\tsink(b)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "b := "), vec!["derived"]);
    assert_eq!(tags(&r, "sink(b)"), vec!["sink"]);
}

#[test]
fn overwrite_removes() {
    let src = "package a\nfunc F(t string) {\n\ta := t\n\ta = \"c\"\n\tsink(a)\n}\n";
    let r = run(src, "F(", "t");
    assert!(hop(&r, "a := t").is_some());
    assert!(hop(&r, "sink(a)").is_none());
    assert_eq!(r.sinks(), 0);
}

#[test]
fn if_else_both_branches_is_not_conditional() {
    let src = "package a\nfunc F(t string, c bool) {\n\tvar r string\n\tif c {\n\t\tr = t\n\t} else {\n\t\tr = t + \"!\"\n\t}\n\tsink(r)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(r)"), vec!["sink"]);
}

#[test]
fn if_one_branch_is_conditional_on_the_condition_line() {
    let src = "package a\nfunc F(t string, c bool) {\n\tvar r string\n\tif c {\n\t\tr = t\n\t}\n\tsink(r)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(r)"), vec!["conditional@4", "sink"]);
}

#[test]
fn else_overwrite_makes_conditional() {
    let src = "package a\nfunc F(t string, c bool) {\n\tr := t\n\tif c {\n\t\tr = \"\"\n\t}\n\tsink(r)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(r)"), vec!["conditional@4", "sink"]);
}

#[test]
fn for_loop_reaches_fixed_point() {
    // c only holds t after the second pass of the body
    let src = "package a\nfunc F(t string) {\n\ta := t\n\tvar b, c string\n\tfor i := 0; i < 3; i++ {\n\t\tc = b\n\t\tb = a\n\t}\n\tsink(c)\n}\n";
    let r = run(src, "F(", "t");
    // b only holds t once the body has run, so the copy is conditional on the loop
    assert_eq!(tags(&r, "c = b"), vec!["exact", "conditional@5"]);
    assert_eq!(tags(&r, "sink(c)"), vec!["conditional@5", "sink"]);
}

#[test]
fn range_binds_an_element() {
    let src = "package a\nfunc F(ts []string) {\n\tfor _, v := range ts {\n\t\tsink(v)\n\t}\n}\n";
    let r = run(src, "F(", "ts");
    assert_eq!(tags(&r, "range ts"), vec!["derived"]);
    assert_eq!(tags(&r, "sink(v)"), vec!["sink"]);
}

#[test]
fn switch_with_default_on_every_arm() {
    let src = "package a\nfunc F(t string, k int) {\n\tvar r string\n\tswitch k {\n\tcase 1:\n\t\tr = t\n\tdefault:\n\t\tr = t\n\t}\n\tsink(r)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(r)"), vec!["sink"]);
}

#[test]
fn switch_without_default_is_conditional() {
    let src = "package a\nfunc F(t string, k int) {\n\tvar r string\n\tswitch k {\n\tcase 1:\n\t\tr = t\n\tcase 2:\n\t\tr = t\n\t}\n\tsink(r)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(r)"), vec!["conditional@4", "sink"]);
}

#[test]
fn condition_only_use_is_not_tracked() {
    let src = "package a\nfunc F(t string) {\n\trole := \"\"\n\tif t == \"admin\" {\n\t\trole = \"admin\"\n\t}\n\tsink(role)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(r.sinks(), 0);
    assert!(r.root.children.is_empty());
}

#[test]
fn field_start_tracks_every_read() {
    let src = "package a\ntype User struct {\n\tEmail string\n}\nfunc F(u User, o Other) {\n\te := u.Email\n\tsink(o.Email)\n\tsink(e)\n}\n";
    let r = run(src, "struct", "Email");
    assert_eq!(r.root.func, "field");
    assert_eq!(r.root.text, "User.Email");
    assert_eq!(r.root.line, 3);
    assert_eq!(tags(&r, "e := u.Email"), vec!["exact"]);
    assert_eq!(tags(&r, "sink(e)"), vec!["sink"]);
    // o is an Other, not a User
    assert!(hop(&r, "sink(o.Email)").is_none());
}

#[test]
fn field_store_follows_the_object() {
    let src = "package a\nfunc F(t string) {\n\tvar o Out\n\to.Name = t\n\tsink(o)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "o.Name = t"), vec!["exact", "field Name"]);
    assert_eq!(tags(&r, "sink(o)"), vec!["sink"]);
}

#[test]
fn variable_start_inside_the_body() {
    let src = "package a\nimport \"os\"\nfunc F() {\n\tsink(x)\n\tx := os.Getenv(\"A\")\n\ty := x\n\tsink(y)\n}\n";
    let r = run(src, "x :=", "x");
    assert_eq!(r.root.line, 5);
    assert_eq!(r.root.text, "x := os.Getenv(\"A\")");
    // the read above the binding is not tracked
    assert!(hop(&r, "sink(x)").is_none());
    assert_eq!(tags(&r, "sink(y)"), vec!["sink"]);
}

#[test]
fn unmodelled_call_carries_and_tags() {
    let src = "package a\nimport \"strings\"\nfunc F(t string) {\n\tu := strings.ToUpper(t)\n\tsink(u)\n\tsend(t)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "u := strings"), vec!["derived", "unmodelled"]);
    assert_eq!(tags(&r, "send(t)"), vec!["moved", "unmodelled"]);
    assert_eq!(r.unmodelled_ends().len(), 1);
}

#[test]
fn outside_hook_sees_the_import_path_and_can_clear() {
    let src = "package a\nimport \"strings\"\nfunc F(t string) {\n\tn := len(t)\n\tu := strings.ToUpper(t)\n\tsink(n)\n\tsink(u)\n}\n";
    let hook = |c: &CallSite| match (c.package.as_deref(), c.callee.as_str()) {
        (Some("strings"), _) => {
            assert_eq!(c.method.as_deref(), Some("ToUpper"));
            assert_eq!(c.tracked_args, vec![0]);
            CallEffect::Pass
        }
        (_, "len") => CallEffect::Clear,
        _ => CallEffect::Unknown,
    };
    let r = run_with(src, "F(", "t", "F", &hook);
    assert!(hop(&r, "sink(n)").is_none());
    assert_eq!(tags(&r, "u := strings"), vec!["derived"]);
    assert_eq!(tags(&r, "sink(u)"), vec!["sink"]);
}

#[test]
fn whole_effect_taints_the_receiver() {
    let src = "package a\nfunc F(t string) {\n\tvar b Builder\n\tb.WriteString(t)\n\tsink(b.String())\n}\n";
    let hook = |c: &CallSite| if c.method.as_deref() == Some("WriteString") { CallEffect::Whole } else { CallEffect::Pass };
    let r = run_with(src, "F(", "t", "F", &hook);
    assert_eq!(tags(&r, "b.WriteString(t)"), vec!["moved"]);
    assert_eq!(tags(&r, "sink(b.String())"), vec!["sink"]);
}

#[test]
fn return_is_moved_and_compound_assign_keeps() {
    let src = "package a\nfunc F(t string) string {\n\ts := \"\"\n\ts += t\n\ts += \"!\"\n\treturn s\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "s += t"), vec!["derived"]);
    assert_eq!(tags(&r, "s += \"!\""), vec!["derived"]);
    assert_eq!(tags(&r, "return s"), vec!["moved"]);
}

#[test]
fn report_rows_start_with_path_line_and_indent_two() {
    let src = "package a\nfunc F(t string) {\n\ta := t\n\tsink(a)\n}\n";
    let r = run(src, "F(", "t");
    let out = report(&[r]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("a.go:2 "), "{out}");
    assert!(lines[1].starts_with("  a.go:3 "), "{out}");
    assert!(lines[1].contains("F") && lines[1].contains("a := t") && lines[1].ends_with("exact"), "{out}");
    assert!(lines[2].starts_with("    a.go:4 ") && lines[2].ends_with("sink"), "{out}");
    assert_eq!(lines[3], "1 route to sink");
}

#[test]
fn report_lists_unmodelled_ends() {
    let src = "package a\nfunc F(t string) {\n\tsend(t)\n}\n";
    let out = report(&[run(src, "F(", "t")]);
    assert!(out.contains("no route to sink\n"), "{out}");
    assert!(out.ends_with("1 route ends at an unmodelled call: a.go:3 send\n"), "{out}");
}

const BOOKING: &str = "type Booking struct {\n\tID int\n\tCustomerPhone string `json:\"phone\"`\n\tVenue string\n}\ntype AdminBooking struct {\n\tBooking\n\tNotes string\n}\n";

#[test]
fn a_stored_field_is_followed_through_the_object() {
    let src = format!("package a\n{BOOKING}func F(t string) {{\n\tvar o Booking\n\to.CustomerPhone = t\n\tsink(o.Venue)\n\tp := o.CustomerPhone\n\tsink(p)\n}}\n");
    let r = run(&src, "F(", "t");
    assert_eq!(tags(&r, "o.CustomerPhone = t"), vec!["exact", "field CustomerPhone", "tag json:\"phone\""]);
    // a read of another field is not tracked
    assert!(hop(&r, "sink(o.Venue)").is_none(), "{}", report(&[r.clone()]));
    assert_eq!(tags(&r, "p := o.CustomerPhone"), vec!["exact", "tag json:\"phone\""]);
    assert_eq!(tags(&r, "sink(p)"), vec!["sink"]);
}

#[test]
fn nested_field_paths_are_kept() {
    let src = "package a\nfunc F(t string) {\n\tvar o Out\n\to.A.B = t\n\tsink(o.A.C)\n\tsink(o.A)\n\tx := o.A.B\n\tsink(x)\n}\n";
    let r = run(src, "F(", "t");
    assert!(hop(&r, "sink(o.A.C)").is_none(), "{}", report(&[r.clone()]));
    // o.A still holds the data in .B
    assert_eq!(tags(&r, "sink(o.A)"), vec!["sink"]);
    assert_eq!(tags(&r, "x := o.A.B"), vec!["exact"]);
}

#[test]
fn composite_literals_store_into_their_fields() {
    let src = format!("package a\n{BOOKING}func F(t string) {{\n\tb := &Booking{{CustomerPhone: t}}\n\tc := Booking{{1, t, \"v\"}}\n\tsink(b.Venue)\n\tsink(c.ID)\n\tsink(b.CustomerPhone)\n\tsink(c.CustomerPhone)\n}}\n");
    let r = run(&src, "F(", "t");
    assert_eq!(tags(&r, "b := &Booking"), vec!["exact", "field CustomerPhone", "tag json:\"phone\""]);
    assert_eq!(tags(&r, "c := Booking"), vec!["exact", "field CustomerPhone", "tag json:\"phone\""]);
    assert!(hop(&r, "sink(b.Venue)").is_none());
    assert!(hop(&r, "sink(c.ID)").is_none());
    assert_eq!(tags(&r, "sink(b.CustomerPhone)"), vec!["tag json:\"phone\"", "sink"]);
    assert_eq!(tags(&r, "sink(c.CustomerPhone)"), vec!["tag json:\"phone\"", "sink"]);
}

#[test]
fn embedded_struct_promotes_its_fields() {
    let src = format!("package a\n{BOOKING}func F(t string) {{\n\tab := AdminBooking{{Booking: Booking{{CustomerPhone: t}}}}\n\tsink(ab.Notes)\n\tsink(ab.CustomerPhone)\n\tsink(ab.Booking.CustomerPhone)\n}}\n");
    let r = run(&src, "F(", "t");
    assert!(hop(&r, "sink(ab.Notes)").is_none(), "{}", report(&[r.clone()]));
    assert!(hop(&r, "sink(ab.CustomerPhone)").is_some(), "{}", report(&[r.clone()]));
    assert!(hop(&r, "sink(ab.Booking.CustomerPhone)").is_some(), "{}", report(&[r.clone()]));
}

#[test]
fn a_field_start_reads_through_embedding_and_follows_composite_writes() {
    let src = format!("package a\n{BOOKING}func F(ab AdminBooking, n Booking, x string) {{\n\tp := ab.CustomerPhone\n\tsink(p)\n\tb := Booking{{CustomerPhone: x}}\n\tsink(b)\n}}\n");
    let r = run(&src, "struct", "CustomerPhone");
    assert_eq!(tags(&r, "p := ab.CustomerPhone"), vec!["exact", "tag json:\"phone\""]);
    assert_eq!(tags(&r, "b := Booking"), vec!["exact", "field CustomerPhone", "tag json:\"phone\""]);
    assert_eq!(tags(&r, "sink(b)"), vec!["sink"]);
}

#[test]
fn a_map_store_stops_with_its_readers() {
    let src = "package a\nvar cache = map[string]string{}\nfunc F(t string) {\n\tcache[\"k\"] = t\n\tsink(cache)\n}\nfunc get() string { return cache[\"k\"] }\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "cache[\"k\"] = t"), vec!["exact", "stopped: shared map", "1 reader"]);
    let stop = hop(&r, "cache[\"k\"] = t").unwrap();
    assert_eq!((stop.children[0].func.as_str(), stop.children[0].line), ("get", 7));
    // the map is not tracked past the stop
    assert!(hop(&r, "sink(cache)").is_none());
    // a local map stops too, with no readers
    let src = "package a\nfunc F(t string) {\n\tm := make(map[string]string)\n\tm[\"k\"] = t\n\tsink(m)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "m[\"k\"] = t"), vec!["exact", "stopped: shared map"]);
    assert!(hop(&r, "sink(m)").is_none());
}

#[test]
fn a_global_store_and_a_channel_send_stop() {
    let src = "package a\nvar last string\nvar ch = make(chan string)\nfunc F(t string) {\n\tlast = t\n\tch <- t\n\tsink(t)\n}\nfunc read() { sink(<-ch) }\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "last = t"), vec!["exact", "stopped: shared global"]);
    assert_eq!(tags(&r, "ch <- t"), vec!["moved", "stopped: shared channel", "1 reader"]);
    assert_eq!(tags(&r, "sink(t)"), vec!["sink"]);
    // a local of the same name shadows the global
    let src = "package a\nvar last string\nfunc F(t string) {\n\tvar last string\n\tlast = t\n\tsink(last)\n}\n";
    let r = run(src, "F(", "t");
    assert_eq!(tags(&r, "sink(last)"), vec!["sink"]);
}

#[test]
fn whole_models_fire_on_an_object_holding_the_data() {
    let src = format!("package a\n{BOOKING}func F(t string) {{\n\tb := Booking{{CustomerPhone: t}}\n\tsink(fmt.Sprint(b.Venue))\n\tsink(b)\n}}\n");
    let hook = |c: &CallSite| {
        assert_eq!(c.arg_paths, vec![Some(vec![vec!["CustomerPhone".to_string()]])]);
        CallEffect::Pass
    };
    let r = run_with(&src, "F(", "t", "F", &hook);
    assert_eq!(tags(&r, "sink(b)"), vec!["sink"]);
}

#[test]
fn generic_calls_drop_their_type_arguments() {
    let src = "package a\nfunc F(t string) {\n\tv := Extract[string](t)\n\tJSON[string](w, v)\n}\n";
    let seen = std::cell::RefCell::new(vec![]);
    let hook = |c: &CallSite| {
        seen.borrow_mut().push(c.callee.clone());
        CallEffect::Pass
    };
    let r = run_with(src, "F(", "t", "F", &hook);
    assert_eq!(*seen.borrow(), vec!["Extract".to_string(), "JSON".to_string()]);
    assert_eq!(tags(&r, "v := Extract"), vec!["derived"]);
}
