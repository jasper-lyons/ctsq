//! End-to-end checks on what the binary actually prints. The unit tests cover
//! the rendering functions; these cover the argument handling and exit codes
//! around them, which only the process boundary shows.

use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
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

#[test]
fn a_query_prints_path_line_and_the_whole_source_line() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#NewServer", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        text.contains(&format!("{}:14: func NewServer(", f)),
        "expected path:LINE: source line, got: {}",
        text
    );
    assert!(!text.contains("<match>"), "the node-text form must be off by default: {}", text);
}

#[test]
fn the_node_flag_prints_node_text_under_the_files_path() {
    let f = fixture("example.go");
    let text = stdout(&ctsq(&["-q", "id#NewServer", "--node", &f]));
    assert!(text.contains(&format!("{}:14: \"NewServer\"", f)), "{}", text);
    assert!(!text.contains("<match>"), "the placeholder gives no file to open: {}", text);
}

#[test]
fn a_multi_line_body_match_names_the_file_it_is_in() {
    // The node form is the only rendering a multi-line --body match has, and
    // without the path the reader has to go looking for the file.
    let f = fixture("typescript/example.ts");
    let text = stdout(&ctsq(&["-q", "*variable#defaults", "--body", &f]));
    assert!(text.starts_with(&format!("{}:38-41: const defaults = {{", f)), "{}", text);
    assert!(!text.contains("<match>"), "{}", text);
}

#[test]
fn stdin_source_keeps_its_own_name_in_the_node_form() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(["-l", "go", "-q", "*type#Meta", "--body", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run ctsq");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"package main\n\ntype Meta struct {\n\tA string\n}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = stdout(&out);
    assert!(text.starts_with("<stdin>:3-5: "), "{}", text);
}

#[test]
fn a_query_that_matches_nothing_says_so_on_stdout() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#no_such_name_anywhere", &f]);
    let text = stdout(&out);
    assert!(text.starts_with("no identifier matches id#no_such_name_anywhere in "), "{}", text);
    assert!(text.contains("(1 file)"), "{}", text);
    assert_eq!(out.status.code(), Some(1), "a miss exits 1 like grep");
}

#[test]
fn a_query_other_than_a_lone_id_keeps_the_no_match_header() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "function#no_such_name_anywhere variable#x", &f]);
    let text = stdout(&out);
    assert!(text.starts_with("no match for function#no_such_name_anywhere variable#x in "), "{}", text);
    assert!(text.contains("(1 file)"), "{}", text);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn an_id_miss_reports_where_the_name_is_a_string_literal() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#/starting/", &f]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(text.starts_with(&format!("no identifier matches id#/starting/ in {} (1 file)\n", f)), "{}", text);
    assert!(text.contains("found as string literal:\n"), "{}", text);
    assert!(text.contains(&format!("{}:11: fmt.Println(\"starting\")\n", f)), "{}", text);
    let try_line = text.lines().last().unwrap();
    assert_eq!(try_line, format!("try: ctsq -q 'string#/starting/' {}", f));
    // The suggestion really finds the hit.
    let again = ctsq(&["-q", "string#/starting/", &f]);
    assert!(again.status.success(), "{}", stdout(&again));
    assert!(stdout(&again).contains(":11:"), "{}", stdout(&again));
}

#[test]
fn a_definition_miss_says_what_the_name_is_defined_as() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "*function#Server", &f]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(text.starts_with(&format!("no definition matches *function#Server in {} (1 file)\n", f)), "{}", text);
    assert!(text.contains(&format!("Server is defined as a type here: {}:5\n", f)), "{}", text);
    assert!(text.ends_with(&format!("try: ctsq -q '*type#Server' {}\n", f)), "{}", text);
}

#[test]
fn a_walk_with_no_files_is_a_miss() {
    let d = std::env::temp_dir().join(format!("ctsq-cli-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("notes.txt"), "id").unwrap();
    let dir = d.to_str().unwrap();
    let out = ctsq(&["-q", "id#x", dir]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{}{}", text, stderr(&out));
    assert!(text.starts_with(&format!("no identifier matches id#x in {} (0 files)\n", dir)), "{}", text);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_query_error_prints_the_query_with_a_caret_and_exits_2() {
    let out = ctsq(&["-q", "function#(", &fixture("example.go")]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "{}", err);
    assert!(err.starts_with("query error: expected a name after # at position 9\n  function#(\n"), "{}", err);
    assert!(err.contains("\n           ^\n"), "caret under the paren: {}", err);
    assert!(!err.contains("Invalid"), "no raw tree-sitter text: {}", err);
}

#[test]
fn an_unescaped_slash_in_a_regex_filter_suggests_the_escape() {
    let out = ctsq(&["-q", "id#/ui/page/", &fixture("example.go")]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2));
    assert!(err.starts_with("query error: unexpected '/' at position 6\n  id#/ui/page/\n        ^\n"), "{}", err);
    assert!(err.contains("must be escaped: id#/ui\\/page/"), "{}", err);
}

#[test]
fn a_regex_that_does_not_compile_is_reported_against_the_query_not_tree_sitter() {
    let out = ctsq(&["-q", "id#/a(/", &fixture("example.go")]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "{}", err);
    assert!(err.starts_with("query error: the regex /a(/ does not compile"), "{}", err);
    assert!(err.contains("\n  id#/a(/\n     ^\n"), "caret under the opening slash: {}", err);
    assert!(!err.contains("Invalid predicate"), "{}", err);
    assert!(!err.contains("Runner error"), "{}", err);
}

#[test]
fn an_escaped_slash_and_a_quote_inside_a_filter_compile_and_run() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#/a\\/b/", &f]);
    assert_eq!(out.status.code(), Some(1), "a miss, not a parse error: {}", stderr(&out));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    let out = ctsq(&["-q", "literal#/call\"/", &f]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).is_empty(), "no per-file tree-sitter error: {}", stderr(&out));
}

#[test]
fn a_missing_path_is_named_on_stderr_and_the_others_still_run() {
    let f = fixture("example.go");
    let missing = fixture("nope.go");
    let out = ctsq(&["-q", "id#NewServer", &missing, &f]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains(&format!("no such file: {}", missing)), "{}", stderr(&out));
    assert!(stdout(&out).contains(&format!("{}:14: func NewServer(", f)), "{}", stdout(&out));
    // Nothing matched overall: exit 1.
    let none = ctsq(&["-q", "id#NewServer", &missing]);
    assert_eq!(none.status.code(), Some(1));
    assert!(stderr(&none).contains("no such file:"), "{}", stderr(&none));
}

#[test]
fn a_missing_path_suggests_a_close_sibling() {
    let typo = fixture("exampl.go");
    let out = ctsq(&["-q", "id#NewServer", &typo, &fixture("example.go")]);
    assert!(
        stderr(&out).contains(&format!("no such file: {} (did you mean {})", typo, fixture("example.go"))),
        "{}",
        stderr(&out)
    );
}

#[test]
fn def_resolves_a_type_and_reports_a_miss_on_stdout() {
    let dir = fixture("");
    let out = ctsq(&["def", "Server", &dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("type Server struct {"), "{}", stdout(&out));

    let miss = ctsq(&["def", "NewServe", &dir]);
    let text = stdout(&miss);
    assert!(text.starts_with("no definition for 'NewServe' in "), "{}", text);
    assert!(text.contains("files scanned)"), "{}", text);
    assert!(text.contains("similar names: NewServer"), "{}", text);
    assert_eq!(miss.status.code(), Some(1));
}

#[test]
fn tree_rejects_body_with_the_query_that_does_it() {
    let out = ctsq(&["tree", &fixture("example.go"), "--body"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("tree has no --body, use ctsq -q '*function#Name' --body FILE"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn tree_prints_a_header_then_signatures() {
    let f = fixture("example.go");
    let text = stdout(&ctsq(&["tree", &f]));
    let mut lines = text.lines();
    let header = lines.next().unwrap();
    assert!(header.starts_with(&f), "{}", header);
    assert!(header.contains("tokens"), "{}", header);
    assert!(text.contains("func NewServer(host string, port int) *Server {"), "{}", text);
}

#[test]
fn files_lists_only_what_matches_every_query() {
    let dir = fixture("");
    let out = ctsq(&["files", "-q", "id#NewServer", "-q", "id#host", &dir]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "only the Go fixture holds both: {}", text);
    assert!(lines[0].starts_with(&format!("{}example.go  ", dir)), "{}", lines[0]);
    assert!(lines[0].ends_with("  2 3"), "trailing per-query counts: {}", lines[0]);
}

#[test]
fn files_reports_a_miss_on_stdout() {
    let out = ctsq(&["files", "-q", "id#NewServer", "-q", "id#nothing_here", &fixture("")]);
    let text = stdout(&out);
    assert!(text.starts_with("no file matches all 2 queries in "), "{}", text);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_go_type_query_with_body_prints_the_declaration() {
    let text = stdout(&ctsq(&["-q", "*type#Server", "--body", &fixture("example.go")]));
    assert!(text.contains("host string"), "expected the struct body: {}", text);
    assert!(text.contains("port int"), "{}", text);
}

#[test]
fn context_prints_the_lines_either_side_of_each_match() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "id#NewServer", "-C", "1", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    // Matched lines keep the colon form, context lines take the dash form.
    assert!(text.contains(&format!("{}:14: func NewServer(", f)), "{}", text);
    assert!(text.contains(&format!("{}-13-", f)), "context before: {}", text);
    assert!(text.contains(&format!("{}-15- \treturn &Server", f)), "context after: {}", text);
    assert!(text.contains(&format!("{}:19: \ts := NewServer(", f)), "{}", text);
    // Two matches five lines apart with one line of context stay two groups.
    assert!(text.lines().any(|l| l == "--"), "expected a group separator: {}", text);
}

#[test]
fn context_windows_that_touch_merge_into_one_group() {
    let f = fixture("example.go");
    let text = stdout(&ctsq(&["-q", "id#NewServer", "-C", "3", &f]));
    assert!(!text.lines().any(|l| l == "--"), "11-17 and 16-22 overlap: {}", text);
    assert!(text.contains(&format!("{}-17-", f)), "the gap is filled in: {}", text);
}

#[test]
fn context_is_rejected_with_the_node_form() {
    let out = ctsq(&["-q", "id#NewServer", "-C", "2", "--node", &fixture("example.go")]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("-C/--context does not apply to --node"), "{}", stderr(&out));
}

#[test]
fn a_ts_variable_query_with_body_prints_the_whole_declaration() {
    let f = fixture("typescript/example.ts");
    let out = ctsq(&["-q", "*variable#defaults", "--body", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.contains("const defaults = {"), "{}", text);
    assert!(text.contains("retries: 3"), "expected the object body: {}", text);
}

#[test]
fn a_js_const_query_finds_the_declaration() {
    let f = fixture("example.js");
    let text = stdout(&ctsq(&["-q", "*const#helper", &f]));
    assert!(text.contains(&format!("{}:5: const helper = () => {{", f)), "{}", text);
}

#[test]
fn a_ruby_file_is_detected_by_its_extension() {
    // No --lang: the .rb suffix has to pick the language on its own.
    let f = fixture("example.rb");
    let out = ctsq(&["-q", "*function#main", "--body", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.starts_with(&format!("{}:6-9: def main", f)), "{}", text);
    assert!(text.contains("allocate_buffer(MAX_SIZE)"), "expected the body: {}", text);
}

#[test]
fn an_unknown_kind_names_the_valid_ones() {
    let out = ctsq(&["-q", "*bogus#X", &fixture("typescript/example.ts")]);
    let err = stderr(&out);
    assert!(
        err.starts_with("unknown kind 'bogus' for typescript, valid kinds: "),
        "{}",
        err
    );
    assert!(err.contains("variable"), "{}", err);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_const_arrow_component_in_a_tsx_file_is_a_function() {
    // Regression: the name lives in the binding, and the .tsx suffix alone picks the grammar.
    let f = fixture("tsx/Topbar.tsx");
    let out = ctsq(&["-q", "*function#Topbar", "--body", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.starts_with(&format!("{}:3-6: ", f)), "{}", text);
    assert!(text.contains("const label = title;"), "expected the whole body: {}", text);
}

#[test]
fn lang_ts_walks_tsx_files_and_parses_each_with_its_own_grammar() {
    let dir = fixture("tsx");
    let out = ctsq(&["-l", "ts", "-q", "*function", &dir]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}{}", text, stderr(&out));
    assert!(text.contains("Topbar.tsx:3:"),"the TSX file is walked and parsed as TSX: {}", text);
    assert!(text.contains("util.ts:1:"), "the plain .ts file still matches: {}", text);
    let named = ctsq(&["-l", "typescript", "-q", "*function#Topbar", &fixture("tsx/Topbar.tsx")]);
    assert!(named.status.success(), "{}", stderr(&named));
}

#[test]
fn a_script_with_no_extension_is_read_by_its_shebang() {
    // No --lang: the `#!/usr/bin/env ruby` line has to pick the language.
    let f = fixture("shebang/brief");
    let out = ctsq(&["tree", &f]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.contains("def run_brief(name)"), "{}", text);
    let q = ctsq(&["-q", "*function#run_brief", &f]);
    assert!(q.status.success(), "{}", stderr(&q));
    assert!(stdout(&q).contains(&format!("{}:4: def run_brief(name)", f)), "{}", stdout(&q));
}

#[test]
fn a_directory_walk_includes_shebang_scripts_in_each_language() {
    let dir = fixture("shebang");
    let out = ctsq(&["-q", "*function", &dir]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.contains("brief:4: def run_brief(name)"), "ruby via env: {}", text);
    assert!(text.contains("serve:4: def serve_forever(port):"), "python3 by direct path: {}", text);
    assert!(text.contains("tool:4: function runTool(arg) {"), "node via env: {}", text);
    assert!(!text.contains("deploy"), "a bash script has no grammar, so it is skipped: {}", text);
    // --lang narrows the walk to the scripts of that language.
    let ruby = stdout(&ctsq(&["-l", "ruby", "-q", "*function", &dir]));
    assert!(ruby.contains("brief:") && !ruby.contains("serve:") && !ruby.contains("tool:"), "{}", ruby);
}

#[test]
fn naming_a_shell_script_says_why_there_is_no_language() {
    let out = ctsq(&["-q", "*function", &fixture("shebang/deploy")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("shell scripts have no grammar"), "{}", stderr(&out));
}

#[test]
fn naming_a_file_with_no_extension_and_no_shebang_asks_for_lang() {
    let out = ctsq(&["-q", "*function", &fixture("../Makefile")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("use --lang"), "{}", stderr(&out));
}

#[test]
fn an_unknown_language_is_rejected_for_a_directory_too() {
    let out = ctsq(&["-l", "cobol", "-q", "id#x", &fixture("shebang")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("Unknown language: cobol"), "{}", stderr(&out));
}

#[test]
fn call_matches_member_calls_by_their_last_segment() {
    let out = ctsq(&["-q", "call#Println", &fixture("example.go")]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("fmt.Println(\"starting\")"), "{}", stdout(&out));
}

#[test]
fn the_types_table_lists_the_new_aliases() {
    let text = stdout(&ctsq(&["types", "go"]));
    for kind in ["method", "string", "comment", "call"] {
        assert!(text.lines().any(|l| l.trim_start().starts_with(kind)), "{} missing: {}", kind, text);
    }
    assert!(text.contains("method_declaration"), "method maps to function nodes: {}", text);
    assert!(text.contains("interpreted_string_literal"), "{}", text);
    let call = text.lines().find(|l| l.trim_start().starts_with("call ")).unwrap();
    assert!(call.contains("call_expression"), "call is mapped, not empty: {}", call);
}

// ── Hidden tests and the footer ───────────────────────────────────────────────

#[test]
fn refs_hides_test_callers_and_counts_them_in_the_footer() {
    let dir = fixture("hidden");
    let out = ctsq(&["refs", "HiddenLimit", &dir]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!text.contains("limit_test.go"), "{}", text);
    assert!(text.ends_with("2 references\nhidden: 2 in tests (--tests)\n"), "{}", text);

    let all = stdout(&ctsq(&["refs", "HiddenLimit", &dir, "--tests"]));
    assert!(all.contains("limit_test.go:3:"), "{}", all);
    assert!(!all.contains("hidden:"), "nothing hidden, no footer: {}", all);
}

#[test]
fn a_query_hides_rust_test_scopes() {
    let out = ctsq(&["-q", "id#HIDDEN_MAX", &fixture("hidden")]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.contains("lib.rs:4: HIDDEN_MAX"), "{}", text);
    assert!(!text.contains("lib.rs:8:") && !text.contains("lib.rs:15:"), "{}", text);
    assert!(text.ends_with("2 matches\nhidden: 3 in tests (--tests)\n"), "{}", text);
}

#[test]
fn a_query_whose_only_matches_are_hidden_prints_the_footer_and_exits_1() {
    let out = ctsq(&["-q", "id#limitSeen", &fixture("hidden")]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "0 matches\nhidden: 1 in tests (--tests)\n");
    let shown = ctsq(&["-q", "id#limitSeen", &fixture("hidden"), "--tests"]);
    assert!(shown.status.success(), "{}", stdout(&shown));
}

#[test]
fn a_test_file_named_directly_is_never_hidden() {
    let f = fixture("hidden/limit_test.go");
    let out = ctsq(&["-q", "id#limitSeen", &f]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(stdout(&out), format!("{}:3: var limitSeen = HiddenLimit\n", f));
}

#[test]
fn output_that_hides_nothing_has_no_footer() {
    let text = stdout(&ctsq(&["-q", "id#NewServer", &fixture("example.go")]));
    assert!(!text.contains("hidden:") && !text.contains("matches"), "{}", text);
    let tree = stdout(&ctsq(&["tree", &fixture("example.go")]));
    assert!(!tree.contains("hidden:"), "{}", tree);
}

#[test]
fn exclude_hides_paths_and_names_its_own_reason() {
    let out = ctsq(&["-q", "id#HiddenLimit", &fixture("hidden"), "--exclude", "limit.go"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "0 matches\nhidden: 2 in tests (--tests), 2 in excluded paths (drop --exclude)\n"
    );
}

#[test]
fn tree_leaves_out_test_scopes_and_sums_the_count_once() {
    let out = ctsq(&["tree", &fixture("hidden")]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!text.contains("mod tests") && !text.contains("HIDDEN_DOUBLE"), "{}", text);
    assert!(!text.contains("limit_test.go"), "{}", text);
    assert!(text.ends_with("2 entries\nhidden: 3 in tests (--tests)\n"), "{}", text);
    assert_eq!(text.matches("hidden:").count(), 1, "{}", text);
    let all = stdout(&ctsq(&["tree", &fixture("hidden"), "--tests"]));
    assert!(all.contains("mod tests {") && all.contains("limit_test.go"), "{}", all);
}

#[test]
fn def_and_files_hide_tests_too() {
    let dir = fixture("hidden");
    let def = ctsq(&["def", "max_is_three", &dir]);
    assert_eq!(def.status.code(), Some(1));
    assert_eq!(stdout(&def), "0 definitions\nhidden: 1 in tests (--tests)\n");
    assert!(stdout(&ctsq(&["def", "max_is_three", &dir, "--tests"])).contains("lib.rs:14:"));

    let files = ctsq(&["files", "-q", "id#HiddenLimit", "-q", "id#limitSeen", &dir]);
    assert_eq!(files.status.code(), Some(1));
    assert_eq!(stdout(&files), "0 files\nhidden: 1 in tests (--tests)\n");
}

#[test]
fn callers_and_explain_hide_a_caller_written_in_a_test() {
    let d = std::env::temp_dir().join(format!("ctsq-cli-callers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("calc.go"),
        "package calc\n\nfunc Add(a, b int) int { return a + b }\n\nfunc Twice(a int) int { return Add(a, a) }\n",
    )
    .unwrap();
    std::fs::write(d.join("calc_test.go"), "package calc\n\nfunc TestAdd() { Add(1, 2) }\n").unwrap();
    let dir = d.to_str().unwrap();
    let text = stdout(&ctsq(&["callers", "Add", dir]));
    assert!(text.contains("Twice()") && !text.contains("TestAdd"), "{}", text);
    assert!(text.ends_with("1 caller\nhidden: 1 in tests (--tests)\n"), "{}", text);
    let all = stdout(&ctsq(&["callers", "Add", dir, "--tests"]));
    assert!(all.contains("TestAdd()") && !all.contains("hidden:"), "{}", all);
    let explain = stdout(&ctsq(&["explain", "Add", dir]));
    assert!(!explain.contains("TestAdd"), "{}", explain);
    assert!(explain.ends_with("1 definition\nhidden: 1 in tests (--tests)\n"), "{}", explain);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn config_dump_takes_the_lang_aliases_and_shows_tests() {
    let out = ctsq(&["config", "dump", "ts"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("language: typescript\n"), "{}", text);
    assert!(text.contains("[tests]\nfiles = [\"*.test.*\", \"*.spec.*\"]\n"), "{}", text);
}

// ── Review fixes ──────────────────────────────────────────────────────────────

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ctsq-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn every_try_line_runs_on_the_same_paths_in_a_mixed_language_dir() {
    let d = scratch_dir("trymixed");
    std::fs::write(d.join("Chat.tsx"), "export const Chat = () => <p>Still reading your files</p>;\n").unwrap();
    std::fs::write(d.join("a.c"), "int main(void) { return 0; }\n").unwrap();
    std::fs::write(d.join("b.go"), "package main\n\nfunc main() {}\n").unwrap();
    let dir = d.to_str().unwrap();
    let out = ctsq(&["-q", "id#/Still reading/", dir]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{}", text);
    assert!(text.contains("found as JSX text:"), "{}", text);
    let tries: Vec<&str> = text.lines().filter(|l| l.starts_with("try: ctsq ")).collect();
    assert!(!tries.is_empty(), "{}", text);
    for t in tries {
        let cmd = t.replacen("try: ctsq", &format!("'{}'", env!("CARGO_BIN_EXE_ctsq")), 1);
        let run = Command::new("sh").args(["-c", &cmd]).output().unwrap();
        assert!(run.status.success(), "{} failed: {}{}", t, stdout(&run), stderr(&run));
        assert!(stdout(&run).contains("Chat.tsx:1:"), "{} found nothing: {}", t, stdout(&run));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_hidden_test_caller_is_counted_once_and_matches_the_extra_rows_of_tests() {
    let d = scratch_dir("hiddenonce");
    std::fs::write(d.join("contest.go"), "package p\n\nfunc Helper() int { return 1 }\n").unwrap();
    std::fs::write(d.join("x_test.go"), "package p\n\nfunc TestX() { Helper() }\n").unwrap();
    let dir = d.to_str().unwrap();
    let text = stdout(&ctsq(&["explain", "Helper", dir]));
    assert!(text.ends_with("1 definition\nhidden: 1 in tests (--tests)\n"), "{}", text);
    let rows = |t: &str| t.lines().filter(|l| l.contains("x_test.go")).count();
    assert_eq!(rows(&text), 0, "{}", text);
    let all = stdout(&ctsq(&["explain", "Helper", dir, "--tests"]));
    assert_eq!(rows(&all), 1, "--tests adds exactly one row: {}", all);
    assert!(!all.contains("hidden:"), "{}", all);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn explain_exits_1_when_nothing_resolves_or_everything_is_hidden() {
    let miss = ctsq(&["explain", "no_such_symbol_anywhere", &fixture("example.c")]);
    assert_eq!(miss.status.code(), Some(1), "{}", stdout(&miss));
    assert!(stdout(&miss).contains("no definition resolved"), "{}", stdout(&miss));

    let d = scratch_dir("explainhidden");
    std::fs::write(d.join("a.go"), "package p\n\nfunc Other() {}\n").unwrap();
    std::fs::write(d.join("a_test.go"), "package p\n\nfunc OnlyInTests() {}\n").unwrap();
    let dir = d.to_str().unwrap();
    let hidden = ctsq(&["explain", "OnlyInTests", dir]);
    assert_eq!(hidden.status.code(), Some(1), "{}", stdout(&hidden));
    assert!(stdout(&hidden).contains("hidden: "), "the footer still prints: {}", stdout(&hidden));
    let shown = ctsq(&["explain", "OnlyInTests", dir, "--tests"]);
    assert!(shown.status.success(), "{}", stdout(&shown));
    let _ = std::fs::remove_dir_all(&d);

    assert!(ctsq(&["explain", "NewServer", &fixture("example.go")]).status.success());
}

#[test]
fn an_unknown_field_is_a_query_error_with_the_caret_under_it() {
    let out = ctsq(&["-q", "*function.nope(id)", &fixture("example.go")]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "{}{}", stdout(&out), err);
    assert!(err.starts_with("query error: unknown field 'nope' at position 10\n  *function.nope(id)\n"), "{}", err);
    assert!(err.contains("\n            ^\n"), "caret under the n of nope: {}", err);
    assert!(err.contains("valid fields include: body, params, "), "{}", err);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn real_fields_still_work_and_a_field_of_another_language_in_the_walk_is_valid() {
    let f = fixture("example.go");
    let out = ctsq(&["-q", "*function.name(id#NewServer)", &f]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let out = ctsq(&["-q", "*function.body(id#NewServer)", &f]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let d = scratch_dir("fieldwalk");
    std::fs::write(d.join("a.js"), "const o = { k: 1 };\n").unwrap();
    std::fs::write(d.join("b.c"), "int main(void) { return 0; }\n").unwrap();
    let out = ctsq(&["-l", "javascript", "-q", "pair.key(id)", d.to_str().unwrap()]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("a.js:1:"), "{}", stdout(&out));
    // A walk over several languages: the field is valid if one of them defines it.
    let out = ctsq(&["-q", "*function.name(id#main)", d.to_str().unwrap(), &f]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("b.c:1:") && stdout(&out).contains("example.go:18:"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&d);
}

// ── --scope ──────────────────────────────────────────────────────────────────

fn scope_fixture() -> String {
    fixture("scope")
}

#[test]
fn def_scope_prints_both_components_a_local_name_is_defined_in() {
    let dir = scope_fixture();
    let out = ctsq(&["def", "saveAssetNow", &dir, "--scope"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let want = format!(
        "{d}/Asset.tsx:4: const saveAssetNow = async () => {{  ?\n  in {d}/Asset.tsx:3-8  export const AssetEditor = (props: Props) => {{\n\
{d}/Asset.tsx:11: const saveAssetNow = () => props.asset  ?\n  in {d}/Asset.tsx:10-14  export function BrandAssetPanel(props: Props) {{\n2 definitions\n",
        d = dir
    );
    assert_eq!(stdout(&out), want);
}

#[test]
fn def_scope_body_indents_the_scope_body_four_spaces_under_its_row() {
    let dir = scope_fixture();
    let text = stdout(&ctsq(&["def", "saveAssetNow", &dir, "--scope", "--body"]));
    assert!(text.contains("3-8  export const AssetEditor = (props: Props) => {\n    export const AssetEditor = (props: Props) => {\n      const saveAssetNow = async () => {\n"), "{}", text);
    assert!(text.contains("\n    }\n"), "{}", text);
    assert!(text.ends_with("2 definitions\n"), "{}", text);
}

#[test]
fn a_body_with_no_scope_is_a_usage_error_on_the_commands_that_gained_scope() {
    let dir = scope_fixture();
    for args in [
        vec!["def", "saveAssetNow", dir.as_str(), "--body"],
        vec!["refs", "saveAssetNow", dir.as_str(), "--body"],
        vec!["callers", "saveAssetNow", dir.as_str(), "--body"],
        vec!["callees", "saveAssetNow", dir.as_str(), "--body"],
        vec!["files", "-q", "id#Props", dir.as_str(), "--body"],
    ] {
        let out = ctsq(&args);
        assert_eq!(out.status.code(), Some(2), "{:?}: {}{}", args, stdout(&out), stderr(&out));
        assert!(stderr(&out).contains("needs --scope"), "{:?}: {}", args, stderr(&out));
    }
    // the --body each other command already had is untouched
    let tree = ctsq(&["tree", &dir, "--body"]);
    assert_eq!(tree.status.code(), Some(2));
    assert!(stderr(&tree).contains("tree has no --body"), "{}", stderr(&tree));
    let q = ctsq(&["-q", "*function#BrandAssetPanel", "--body", &dir]);
    assert!(q.status.success(), "{}", stderr(&q));
    assert!(stdout(&q).contains("Asset.tsx:10-14:"), "{}", stdout(&q));
}

#[test]
fn query_scope_prints_a_row_under_each_hit_and_none_at_the_top_level() {
    let dir = scope_fixture();
    let out = ctsq(&["-q", "id#/^(saveAssetNow|standalone)$/", &dir, "--scope"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}{}", text, stderr(&out));
    let rows: Vec<&str> = text.lines().filter(|l| l.starts_with("  in ")).collect();
    // four saveAssetNow hits, each under its component, and `standalone` has no scope
    assert_eq!(rows.len(), 4, "{}", text);
    assert!(text.contains("standalone = 1\n5 matches\n"), "{}", text);
}

#[test]
fn query_scope_with_context_is_rejected() {
    let out = ctsq(&["-q", "id#Props", &scope_fixture(), "--scope", "-C", "1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--scope"), "{}", stderr(&out));
}

#[test]
fn refs_scope_counts_references_and_shows_the_holder() {
    let out = ctsq(&["refs", "saveAssetNow", &scope_fixture(), "--scope"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", text);
    assert_eq!(text.lines().filter(|l| l.starts_with("  in ")).count(), 4, "{}", text);
    assert!(text.ends_with("4 references\n"), "{}", text);
}

#[test]
fn scope_rows_follow_test_hiding_and_the_count_line_prints_once() {
    let d = scratch_dir("scopehide");
    std::fs::write(d.join("a.go"), "package p\n\nfunc Helper() int { return 1 }\n\nfunc Use() int {\n\treturn Helper()\n}\n").unwrap();
    std::fs::write(d.join("a_test.go"), "package p\n\nfunc TestX() { Helper() }\n").unwrap();
    let dir = d.to_str().unwrap();
    let text = stdout(&ctsq(&["refs", "Helper", dir, "--scope"]));
    assert!(!text.contains("a_test.go") && !text.contains("TestX"), "{}", text);
    assert!(text.contains("a.go:5-7  func Use() int {\n"), "{}", text);
    assert!(text.ends_with("2 references\nhidden: 1 in tests (--tests)\n"), "{}", text);
    assert_eq!(text.matches("references").count(), 1, "{}", text);
    let all = stdout(&ctsq(&["refs", "Helper", dir, "--scope", "--tests"]));
    assert!(all.contains("a_test.go:3-3  func TestX() { Helper() }\n"), "{}", all);
    assert!(all.ends_with("3 references\n"), "{}", all);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn callers_and_callees_scope_go_under_each_call_row() {
    let d = scratch_dir("scopecalls");
    std::fs::write(d.join("c.go"), "package p\n\nfunc Add(a, b int) int { return a + b }\n\nfunc Twice(a int) int {\n\treturn Add(a, a)\n}\n").unwrap();
    let dir = d.to_str().unwrap();
    let up = stdout(&ctsq(&["callers", "Add", dir, "--scope"]));
    assert_eq!(
        up,
        format!("Add()\n└── Twice()  [c.go:5]\n      in {dir}/c.go:5-7  func Twice(a int) int {{\n1 caller\n"),
    );
    let down = stdout(&ctsq(&["callees", "Twice", dir, "--scope", "--depth", "1"]));
    assert_eq!(
        down,
        format!("Twice()\n└── Add()  [c.go:3]\n      in {dir}/c.go:5-7  func Twice(a int) int {{\n1 callee\n"),
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn files_scope_lists_each_distinct_scope_holding_a_match() {
    let d = scratch_dir("scopefiles");
    std::fs::write(d.join("a.go"), "package p\n\ntype T struct{}\n\nfunc One() {\n\tvar x T\n\t_ = x\n}\n\nfunc Two() {\n\tvar y T\n\tvar z T\n}\n").unwrap();
    let dir = d.to_str().unwrap();
    let text = stdout(&ctsq(&["files", "-q", "id#/^[xyz]$/", "-q", "id#One", dir, "--scope"]));
    let f = format!("{}/a.go", dir);
    assert_eq!(
        text,
        format!("{f}  4 1\n  in {f}:5-8  func One() {{\n  in {f}:10-13  func Two() {{\n1 file\n"),
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn scopeof_prints_the_heading_and_body_and_exits_zero() {
    let f = fixture("example.go");
    let out = ctsq(&["scopeof", &format!("{}:15", f)]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(text.starts_with(&format!("{}:15  in  {}:", f, f)), "{}", text);
}

#[test]
fn scopeof_rejects_a_non_coordinate_with_exit_2() {
    let out = ctsq(&["scopeof", "src/footer.rs"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("expected path:LINE or path:LINE:COL"), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn scopeof_missing_file_is_named_on_stderr_and_exits_one() {
    let out = ctsq(&["scopeof", "no/such/file.rs:3"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no such file: no/such/file.rs"), "{}", stderr(&out));
}

// ── find ─────────────────────────────────────────────────────────────────────

/// ctsq run in `dir` with no user config or cache layer, so only the repo
/// `.ctsq/` and the built-in queries count.
fn ctsq_in(dir: &str, args: &[&str]) -> Output {
    let empty = std::env::temp_dir().join(format!("ctsq-cli-xdg-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    Command::new(env!("CARGO_BIN_EXE_ctsq"))
        .args(args)
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", &empty)
        .env("XDG_CACHE_HOME", &empty)
        .output()
        .expect("failed to run ctsq")
}

#[test]
fn find_route_with_a_key_prints_columns_and_the_count_line() {
    let out = ctsq_in(&fixture("queries/react-router"), &["find", "route", "/about"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        stdout(&out),
        "src/main.tsx:6  /about  About  src/pages.tsx:5\n1 route (react-router, from built-in queries/react-router.toml)\n"
    );
}

#[test]
fn find_with_no_arguments_lists_what_applies_and_exits_0() {
    let out = ctsq_in(&fixture("queries/solid"), &["find"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out),
        "built-in queries/jsx-component.toml:8: component  jsx-component  (always)\n\
         built-in queries/error-js.toml:7: error           js-error       (always)\n\
         built-in queries/jsx-label.toml:8: label          jsx-label      (always)\n\
         built-in queries/solid-routes.toml:9: route       solid-routes   (import @solidjs/router in src/App.tsx)\n\
         built-in queries/jsx-text.toml:7: text            jsx-text       (always)\n\
         not detected here: error (go-error), error (ruby-error), route (chi), route (net-http), route (next-app), route (rails-routes), route (react-router)\n"
    );
}

#[test]
fn find_text_and_error_name_the_function_that_renders_or_emits_them() {
    let out = ctsq_in(&fixture("queries/jsx"), &["find", "text", "welcome"]);
    assert_eq!(stdout(&out), "src/Card.tsx:6  Welcome back  Card  src/Card.tsx:3\n1 text (jsx-text, from built-in queries/jsx-text.toml)\n");
    let out = ctsq_in(&fixture("queries/errors"), &["find", "error", "failed to load pipeline 12: open x: no such file"]);
    assert_eq!(stdout(&out), "go/load.go:14  failed to load pipeline %s: %w  Load  go/load.go:10\n1 error (go-error, from built-in queries/error-go.toml)\n");
}

#[test]
fn the_glossary_is_used_by_find_def_and_a_miss_and_each_says_so_last() {
    let dir = fixture("glossary");
    let note = ".ctsq/glossary.toml:2: \"pipeline page\" also searched as Pipeline\n";
    let out = ctsq_in(&dir, &["find", "error", "pipeline page"]);
    assert!(out.status.success() && stdout(&out).ends_with(note), "{}", stdout(&out));
    let out = ctsq_in(&dir, &["def", "pipeline page", "src"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out), format!("src/pipeline.go:5: func Pipeline() error {{\n{}", note));
    let out = ctsq_in(&dir, &["-q", "id#\"pipeline page\"", "src"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).starts_with("no identifier matches id#\"pipeline page\" in src (1 file)\nfound as Pipeline:\nsrc/pipeline.go:5: func Pipeline() error {\n"), "{}", stdout(&out));
    assert!(stdout(&out).ends_with(note), "{}", stdout(&out));
    let out = ctsq_in(&dir, &["def", "supplies", "src"]);
    assert!(stdout(&out).ends_with(".ctsq/glossary.toml:4: \"supplies\" also searched as /resource|stock/\n"), "{}", stdout(&out));
    assert!(stdout(&out).starts_with("src/pipeline.go:13: func stockLevel() error {"), "{}", stdout(&out));
}

#[test]
fn find_uses_a_repo_override_and_names_it() {
    let out = ctsq_in(&fixture("queries/solid-mavexa"), &["find", "route", "/library"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).ends_with("2 routes (solid-routes, from .ctsq/queries/solid-routes.toml)\n"), "{}", stdout(&out));
}

#[test]
fn find_misses_exit_1_and_an_unknown_query_exits_2() {
    let out = ctsq_in(&fixture("queries/solid"), &["find", "route", "/nowhere"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).starts_with("no route matches /nowhere here (tried solid-routes from built-in queries/solid-routes.toml)\n"), "{}", stdout(&out));
    let out = ctsq_in(&fixture("queries/solid"), &["find", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("no named query 'nope'"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

#[test]
fn find_hides_tests_and_tests_shows_them() {
    let dir = fixture("queries/solid");
    let out = ctsq_in(&dir, &["find", "route", "test-only"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "0 routes\nhidden: 1 in tests (--tests)\n");
    let out = ctsq_in(&dir, &["find", "route", "test-only", "--tests"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).starts_with("src/routes.test.ts:3  /about/test-only"), "{}", stdout(&out));
}

#[test]
fn a_broken_repo_query_file_is_an_error_naming_the_file() {
    let d = scratch_dir("findbroken");
    std::fs::create_dir_all(d.join(".ctsq/queries")).unwrap();
    std::fs::write(d.join(".ctsq/queries/x.toml"), "framework = \"x\"\nlangs = [\"ts\"]\n[route]\nmatch = \"id\"\ntarget = \"def\"\nrewrite = [\"a\"]\n").unwrap();
    let out = ctsq_in(d.to_str().unwrap(), &["find"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains(".ctsq/queries/x.toml:3: [route] `rewrite` is [\"<regex>\", \"<replacement>\"]"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn config_path_lists_a_queries_directory_per_layer() {
    let out = ctsq_in(&fixture("queries/solid-mavexa"), &["config", "path"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("\nqueries:\n1. repo      "), "{}", text);
    assert!(text.contains("solid-mavexa/.ctsq/queries\n"), "{}", text);
    assert!(
        text.contains("4. built-in  error-go.toml, error-js.toml, error-ruby.toml, go-chi.toml, go-http.toml, jsx-component.toml, jsx-label.toml, jsx-text.toml, next-app.toml, rails-routes.toml, react-router.toml, solid-routes.toml\n"),
        "{}",
        text
    );
}

/// Every `match` of every built-in pack also runs as a plain -q query and
/// finds something in the pack fixtures. A match using JSX node kinds runs
/// under -l tsx, since the plain typescript grammar has none.
#[test]
fn every_built_in_match_runs_as_a_plain_query() {
    let dir = format!("{}/config/queries", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let table: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        for (name, v) in &table {
            let Some(m) = v.as_table().and_then(|q| q.get("match")) else { continue };
            let matches: Vec<String> = match m {
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(a) => a.iter().map(|x| x.as_str().unwrap().to_string()).collect(),
                other => panic!("{:?}", other),
            };
            // A match using JSX node kinds runs under tsx, since the plain
            // typescript grammar has none. Other packs run in their first language.
            let first = table["langs"].as_array().unwrap()[0].as_str().unwrap();
            let first = if first == "tsx" || first == "ts" { "ts" } else { first };
            for m in matches {
                n += 1;
                let ts = ctsq(&["-l", first, "-q", &m, &fixture("queries")]);
                let ok = ts.status.success()
                    || (stderr(&ts).starts_with("unknown kind")
                        && ctsq(&["-l", "tsx", "-q", &m, &fixture("queries")]).status.success());
                assert!(ok, "{} [{}] {}: {}{}", path.display(), name, m, stdout(&ts), stderr(&ts));
            }
        }
    }
    assert!(n >= 7, "every pack match was checked: {}", n);
}
