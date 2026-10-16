mod compiler;
mod cooccur;
mod example;
mod explain;
mod files;
mod footer;
mod miss;
mod query;
mod runner;
mod tree;
mod calltree;
mod importgraph;
mod types;
mod resolve;
mod config;
mod flow;
mod semantic;

use clap::{Parser, Subcommand};
use compiler::lang;
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Read};

#[derive(Parser)]
#[command(
    name = "ctsq",
    about = "Cascading TreeSitter Queries",
    after_help = "QUERY SYNTAX
  Abstract types  id        any identifier — replaces grep -n \"Symbol\" (see examples below)
                  function  class  var  variable  const  param  type  import
                  literal  block  if  for  while  switch  call  comment
                  method (alias of function)  string (the string literals)
                  variable (alias const) is a variable declaration: *variable#X
                  reaches the whole `const X = …` statement, &variable#X the uses.
                  call#name matches the last segment of a member call, so
                  call#Println finds fmt.Println(...) and call#toBe finds a.toBe().
                  function includes arrow functions bound to a const, let or var.
                  id includes property, field and JSX attribute names.
                  Unknown identifiers pass through as raw tree-sitter node types;
                  anything else is rejected with the valid kinds for the language.

  Sigils (scope)  *type    definitions only   (function_definition, class declaration…)
                  &type    references only     (call_expression, variable use…)
                  type     both (default)

  Name filter     type#name        exact match
                  type#\"my name\"  exact match (quoted, allows spaces)
                  type#/regex/     regex match (a / inside is written \\/)

  Captures        Wrap in () and append @label to bind the matched node:
                  (&function#malloc @call)

  Field access    selector.body(inner)     match inner inside the node's body
                  selector.params(inner)   match inner inside the node's params
                  selector.body()          the body node itself

  Combinators     A B    descendant  B anywhere inside A
                  A > B  child       B as a direct child of A
                  A + B  adjacent    A and B are adjacent siblings
                  A ~ B  sibling     A and B are siblings

OUTPUT
  Each match prints as path:LINE: <the whole source line, trimmed>, the same
  rendering `explain` uses for its REFERENCES. --node prints the matched
  node's own text instead. --body prints the full text of a multi-line match,
  as path:START-END: <text>. Every form opens with the file the match is in
  (<stdin> for piped source), so nothing needs a second look to be located.
  No match prints a miss report on stdout and exits 1. It names the query, the
  path and the file count. A lone id#name that matches nothing is rerun as text
  over string literals, JSX text and comments, and prints a `found as` block per
  kind plus a `try:` line to run. A lone *kind#name that matches nothing says
  where the name is defined under another kind. A walk that finds no files is
  a miss too. A path that does not exist prints `no such file: X (did you mean
  Y)` on stderr and the other paths still run. A query that does not parse
  prints the query with a caret under the fault and exits 2.

  Test files and test scopes (such as Rust mod tests) are hidden from tree, -q,
  def, refs, callers, callees, explain and files. Output that hid something
  ends with a count of what it showed and a hidden: line naming each count and
  the flag that shows it, such as `hidden: 2 in tests (--tests)`. --exclude GLOB
  hides more paths, judged from the searched path. A file named directly is
  never hidden. When every match was hidden the count reads 0 and the exit is 1.

  -C N (--context N) adds N source lines either side of each match, like
  grep -C: matched lines keep path:LINE:, context lines use path-LINE-, groups
  are separated by --, and lines keep their indentation. Not valid with --node.

EXAMPLES (a question as a comment, the call, then the next call)
  # What is this symbol and how is it used? Start here.
  ctsq explain count_items src/
  # Show definitions by name, several at once with a regex filter.
  ctsq -q '*function#/^(fmt_tokens|count_items)$/' --body src/tree.rs
  # Where is a name used? Then the same hits with context.
  ctsq -q 'id#fmt_tokens' src/
  ctsq -q 'id#fmt_tokens' -C 3 src/tree.rs
  # Where does some text appear? id# only matches identifiers, literal# matches strings.
  ctsq -q 'literal#/valid kinds/' src/compiler
  # Outline a file, then pull one body.
  ctsq tree src/tree.rs
  ctsq -q '*function#count_items' --body src/tree.rs
  # Find calls by the last segment of the callee, comments by text.
  ctsq -q 'call#Println' fixtures/example.go
  ctsq -q 'comment#/shebang/' src/files.rs
  # A wrong path is named on stderr and the other paths still run.
  ctsq -q 'id#fmt_tokens' src/tre.rs src/tree.rs
  # Tests are hidden and counted on a last line. Show them, or hide more paths.
  ctsq refs HiddenLimit fixtures/hidden --tests
  ctsq tree src/ --exclude semantic
  # Who calls it? Which files mention both names?
  ctsq callers count_items src/
  ctsq files -q 'id#DefIndex' -q 'id#CallGraph' src/

STDIN
  Pipe file paths (language detected per file from extension):
    find . -name '*.rs' | ctsq -q 'id#Symbol'
    find . -name '*.py' | ctsq -l python -q '*function'

  Pipe source content directly (requires --lang; use - as the path):
    cat file.ts | ctsq -l typescript -q '*function' -
    echo 'fn foo() {}' | ctsq -l rust -q '*function' -

QUERY BY EXAMPLE
  Paste the code you're looking for; ctsq derives the queries that match it:
    ctsq example 'u.sendEmail(msg)' -l js
  Candidates run most-literal (exact structural match) to most-general, each
  one verified against the snippet — every printed line works with -q.
  $UPPERCASE marks a wildcard; unmarked parts stay literal:
    ctsq example '$X.sendEmail(msg)' -l js   any receiver; method + arg pinned

SUBCOMMANDS
  ctsq explain Symbol src/              what a symbol is and how it is used, in one call
  ctsq explain Symbol src/ --depth 2    more levels of callers and callees
  ctsq tree file.cpp                    structural outline of a file
  ctsq tree src/                        structural outline of all files in a directory
  ctsq callers ProcessMessages src/     who calls this function (up the call chain)
  ctsq callees ProcessMessages src/     what this function calls (down the call chain)
  ctsq callgraph src/                   full call graph in DOT format (pipe to dot -Tpng)
  ctsq callgraph src/ --format edges    full call graph as plain caller -> callee pairs
  ctsq def helper src/                  where is this function defined (file:line)
  ctsq def Foo::method src/             resolve a qualified path or method to its definition
  ctsq files -q 'id#A' -q 'id#B' src/   files where every one of several queries matches
  ctsq imports src/main.rs src/         what files this file imports (down the import tree)
  ctsq importers src/files.rs src/      what files import this file (up the import tree)
  ctsq importgraph src/                 full import graph in DOT format (pipe to dot -Tpng)
  ctsq importgraph src/ --format edges  full import graph as plain source -> target pairs
  ctsq types                            how abstract types map to tree-sitter nodes (all langs)
  ctsq types rust                       the same mapping for a single language
  ctsq example 'u.sendEmail()' -l js    suggest queries that match this code snippet
  ctsq example '$X.send(msg)' -l js     $UPPERCASE = wildcard; unmarked parts stay literal

RESOLUTION
  Rust and Go call sites resolve through imports/aliases, qualified paths,
  bound function values, and method receivers typed via params, annotations,
  literals, constructors, and call return types. Dynamic dispatch expands:
  a call through a Go interface or a Rust dyn Trait / T: Trait / impl Trait
  receiver gets one edge per implementing type. Ambiguous calls resolve to
  the best-ranked candidate (same file > same dir > imported) and are marked
  with '?'. Other languages match by name only.

  Import edges (imports/importers/importgraph) resolve per language: C/C++
  #include, JS/TS relative paths (with extension + index probing, plus
  require/dynamic-import/re-export), Python dotted and relative imports, and
  Rust use paths. Go imports resolve to a package (a directory node) with edges
  annotated by the specific member file whose symbols are used. Externals and
  unresolved specifiers show as leaf '(external)' nodes; ambiguous resolutions
  are marked with '?'."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Target language (c, cpp, javascript, typescript, python, ruby, rust, go); inferred from the
    /// extension, or the shebang line of a file with no extension, if omitted.
    /// ts includes .tsx and js includes .jsx, each parsed with its own grammar
    #[arg(long, short, global = true)]
    lang: Option<String>,

    /// Abstract query string (required when no subcommand given)
    #[arg(long, short)]
    query: Option<String>,

    /// Source files, directories, or '-' to read source from stdin (requires --lang).
    /// Omit entirely to read file paths from stdin (e.g. find . | ctsq -q QUERY).
    #[arg(num_args(0..))]
    source_files: Vec<String>,

    /// Print the compiled tree-sitter S-expression
    #[arg(long)]
    show_query: bool,

    /// Include full body text of captured nodes (default: first line only)
    #[arg(long)]
    body: bool,

    /// Print the matched node's own text instead of the whole source line
    #[arg(long)]
    node: bool,

    /// Print N source lines either side of each match, like grep -C
    #[arg(short = 'C', long = "context", value_name = "N")]
    context: Option<usize>,

    /// Ignore .gitignore and .ignore files (search all files)
    #[arg(long, global = true)]
    no_ignore: bool,

    /// Show test files and test scopes, which are hidden by default and counted in the footer
    #[arg(long, global = true)]
    tests: bool,

    /// Hide paths matching GLOB, judged relative to the search root (repeatable)
    #[arg(long, global = true, value_name = "GLOB")]
    exclude: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Everything about a symbol in one call: definition bodies, callers, callees, remaining references
    #[command(after_help = "Answers \"what is this and how is it used\" without follow-up reads.
Composes def, callers, callees and an id#Symbol scan into one report.

  ctsq explain NewServer fixtures/
  ctsq explain Server.Start src/ --depth 2
  ctsq explain BigFunction src/ --max-lines 20
  ctsq explain Config src/ --no-body

Sections, in order: DEFINITION (file:line plus the full body text),
CALLERS, CALLEES (call site, the function on the other end, where it is
defined, and the call line), then REFERENCES (a per-file count of the
id#Symbol matches not already shown). With --no-body the signature prints on
its own `sig:` line, and a truncated body keeps the signature as the first
line it prints. When the symbol resolves to nothing, the plain id#Symbol
matches are listed instead.")]
    Explain {
        /// Symbol name, qualified path (a::b::f, Foo::method) or method expr (x.method)
        symbol: String,
        /// File or directory to search (default: current directory)
        #[arg(default_value = ".")]
        path: String,
        /// Levels of callers and callees to walk
        #[arg(long, default_value = "1")]
        depth: usize,
        /// Maximum body lines printed per definition
        #[arg(long = "max-lines", default_value = "80")]
        max_lines: usize,
        /// Print signatures only, never definition bodies
        #[arg(long = "no-body")]
        no_body: bool,
    },
    /// Structural outline of a file or directory: classes, functions, variables
    #[command(after_help = "Each file gets a header line — path, line count, rough token count,
number of entries — then one line per entry: its declaration line, the line it
starts on, and a (N lines) span for anything over 10 lines.

  ctsq tree file.cpp
  ctsq tree src/

There is no --body: `tree` is the outline. For one entry's body use
  ctsq -q '*function#Name' --body FILE")]
    Tree {
        /// Files or directories to outline
        #[arg(num_args(1..))]
        paths: Vec<String>,
        /// Rejected with a pointer to the query that does this.
        #[arg(long, hide = true)]
        body: bool,
    },
    /// Show what calls a given function (walk up the call chain)
    Callers {
        /// Function name to trace
        function: String,
        /// File or directory to search
        path: String,
        /// Maximum depth to traverse
        #[arg(long, default_value = "3")]
        depth: usize,
    },
    /// Show what a function calls (walk down the call chain)
    Callees {
        /// Function name to trace
        function: String,
        /// File or directory to search
        path: String,
        /// Maximum depth to traverse
        #[arg(long, default_value = "3")]
        depth: usize,
    },
    /// Build a full call graph for all functions in a path
    Callgraph {
        /// File or directory to analyze
        path: String,
        /// Output format: dot (default) or edges
        #[arg(long, default_value = "dot")]
        format: String,
    },
    /// Files where every one of several queries matches
    #[command(after_help = "Replaces `grep -rl A | xargs grep -l B`. Repeat -q once per query; a file is
listed only when every query matched in it, with the per-query match counts
in query order as the trailing column.

  ctsq files -q 'id#Modal' -q 'id#useAuth' src/
  ctsq files -q '*function#render' -q 'id#Props' src/ --lang tsx")]
    Files {
        /// Query that must match; repeat -q for each one
        #[arg(long, short, num_args(1), required = true)]
        query: Vec<String>,
        /// File or directory to search
        #[arg(default_value = ".")]
        path: String,
    },
    /// Resolve a name to its definition location(s)
    Def {
        /// Name, qualified path (a::b::f, Foo::method) or method expr (x.method)
        name_or_expr: String,
        /// File or directory to search
        path: String,
    },
    /// Find all usage sites of a name (complement of def)
    Refs {
        /// Identifier name to find references to
        name: String,
        /// File or directory to search
        path: String,
    },
    /// Show what files/packages a file imports (forward dependency tree)
    Imports {
        /// File whose imports to trace
        file: String,
        /// File or directory that scopes resolution
        path: String,
        /// Maximum depth to traverse
        #[arg(long, default_value = "3")]
        depth: usize,
    },
    /// Show what files import a file (reverse dependents tree)
    Importers {
        /// File whose importers to trace
        file: String,
        /// File or directory that scopes resolution
        path: String,
        /// Maximum depth to traverse
        #[arg(long, default_value = "3")]
        depth: usize,
    },
    /// Build a full import graph for a path
    Importgraph {
        /// File or directory to analyze
        path: String,
        /// Output format: dot (default) or edges
        #[arg(long, default_value = "dot")]
        format: String,
    },
    /// Derive candidate queries from an example code snippet (query-by-example)
    #[command(after_help = "Prints candidate queries from most literal (an exact structural match) to most
general, each self-verified against the snippet — every line works with -q.

  ctsq example 'u.sendEmail(msg)' -l js

Mark the parts you don't care about with $UPPERCASE placeholders; unmarked
parts stay literal. Placeholders inside string literals are left alone.

  ctsq example '$X.sendEmail(msg)' -l js    any receiver; method + arg pinned
  ctsq example 'function $F() {}' -l js     any function definition

Bare expressions are auto-wrapped in a scratch function for C, C++, and Go.
Constructs with no abstract mapping fall back to raw tree-sitter node kinds.")]
    Example {
        /// Code snippet, or '-' to read it from stdin (requires --lang)
        snippet: String,
        /// Also show rejected candidates and focus-node diagnostics
        #[arg(long)]
        debug: bool,
    },
    /// Show how abstract node types map to tree-sitter nodes, per language
    Types {
        /// Languages to show (default: all). E.g. `ctsq types rust go`
        #[arg(num_args(0..))]
        langs: Vec<String>,
    },
    /// Walk call or data-flow routes between definitions: ctsq path 'function#main --> function#save'
    #[command(after_help = "Joins two or more selectors with a call or data-flow arrow and prints each
route as an indented tree, every row starting path:line.

Call arrows, between functions:
  ->    calls directly
  -->   calls eventually, through any number of functions
  <-    called by directly
  <--   called by eventually

Data-flow arrows, between values:
  =>    flows in one step to
  ==>   flows eventually to, through called functions
  <=    comes in one step from
  <==   comes eventually from, back through callers
A query ending in => or ==> has no end selector and lists every call that
hands the data to code outside the module. One ending in <= or <== lists
every origin.

Tags on each data-flow hop:
  source              where the walk started, or where data enters from outside
  exact               the same value, moved or copied unchanged
  derived             built from a tracked value
  moved               passed into a call or returned
  field F             stored into field F, the walk now follows the object
  conditional@L       only tracked on some branches, L is the line of the condition
  widened             reached through an interface, one of several implementations
  unmodelled          passed through an outside call with no model
  via F               passed through the in-repo function F
  sink                matched the end selector
  rendered            printed by a template action
  outside             handed to code outside the module, the end of a route
  stopped: reason     the walk would have to guess, so it stops and lists the choices
A guessed hop prints ? after its tag or callee.

Examples:
  ctsq path 'field#Booking.CustomerPhone ==> function#log.Printf' .
  ctsq path 'function#main --> function#save' .

A query reads one way, downward or upward. Upward queries print in the order
written, the first selector first.

Exit codes: 0 routes found, 1 no routes or an unknown kind, 2 usage or query error.")]
    Path {
        /// Query with at least one call or data-flow combinator
        query: String,
        /// Files or directories to search (default: current directory)
        #[arg(num_args(0..))]
        paths: Vec<String>,
        /// Longest call route in hops, or how many functions deep a data-flow walk follows
        #[arg(long, default_value = "12")]
        max: usize,
        /// Print every call route, not just the 5 shortest per start (call routes only)
        #[arg(long)]
        all: bool,
        /// Keep only hops carrying these tags, comma-separated (data-flow queries)
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Print each hop's definition body under its row
        #[arg(long)]
        body: bool,
        /// List the outside implementations a widened interface call groups (data-flow queries)
        #[arg(long)]
        expand: bool,
    },
    /// Show where config and model files are read from, or the merged result for a language
    #[command(after_help = "Language mechanics and outside-call models are TOML files, looked up in order:
repo .ctsq/, then $XDG_CONFIG_HOME/ctsq (default ~/.config/ctsq), then
$XDG_CACHE_HOME/ctsq (default ~/.cache/ctsq), then built-in defaults.

  ctsq config path
  ctsq config dump go")]
    Config {
        /// `path` or `dump <lang>`
        #[arg(num_args(1..=2), required = true)]
        args: Vec<String>,
        // The global --lang and --no-ignore do nothing here, so they are hidden.
        #[arg(long, short, hide = true)]
        lang: Option<String>,
        #[arg(long, hide = true)]
        no_ignore: bool,
    },
}

/// Restore the kernel default for SIGPIPE. The Rust runtime sets it to SIG_IGN before
/// main, so writes to a closed pipe (ctsq ... | head) return EPIPE and println! panics.
/// With the default disposition the process exits quietly like grep or cat.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
}
#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() {
    reset_sigpipe();
    let cli = Cli::parse();
    let hide = files::HideOpts { tests: cli.tests, exclude: cli.exclude.clone() };

    match cli.command {
        Some(Commands::Explain { symbol, path, depth, max_lines, no_body }) => {
            let opts = explain::Options { depth, max_lines, no_body };
            explain::run(&symbol, &path, cli.lang.as_deref(), &opts, cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Tree { paths, body }) => {
            if body {
                eprintln!("tree has no --body, use ctsq -q '*function#Name' --body FILE");
                std::process::exit(2);
            }
            let mut hider = files::Hider::new(&hide);
            let mut jobs = Vec::new();
            for p in &paths {
                let found = files::resolve_jobs_lenient(p, cli.lang.as_deref(), cli.no_ignore);
                hider.add_jobs(p, &found);
                jobs.extend(found);
            }
            if jobs.is_empty() {
                std::process::exit(1);
            }
            // Each file is outlined on its own. The hidden counts are summed
            // here and printed once, after the last file (BUILD.md A11).
            let mut hidden = footer::Hidden::default();
            let mut entries = 0usize;
            let mut printed = 0usize;
            for (file, lang_name) in &jobs {
                let outline = tree::report(file, Some(lang_name), &mut hider);
                hidden.tests += outline.hidden.tests;
                hidden.excluded += outline.hidden.excluded;
                if outline.text.is_empty() {
                    continue;
                }
                if printed > 0 { println!(); }
                printed += 1;
                entries += outline.entries;
                print!("{}", outline.text);
            }
            print!("{}", footer::footer(entries, "entry", &hidden));
            if printed == 0 {
                std::process::exit(1);
            }
            return;
        }
        Some(Commands::Callers { function, path, depth }) => {
            calltree::callers(&function, &path, cli.lang.as_deref(), depth, cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Callees { function, path, depth }) => {
            calltree::callees(&function, &path, cli.lang.as_deref(), depth, cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Callgraph { path, format }) => {
            calltree::callgraph(&path, cli.lang.as_deref(), cli.no_ignore, &format);
            return;
        }
        Some(Commands::Files { query, path }) => {
            cooccur::run(&query, &path, cli.lang.as_deref(), cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Def { name_or_expr, path }) => {
            resolve::run_def(&name_or_expr, &path, cli.lang.as_deref(), cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Refs { name, path }) => {
            run_refs(&name, &path, cli.lang.as_deref(), cli.no_ignore, &hide);
            return;
        }
        Some(Commands::Imports { file, path, depth }) => {
            importgraph::imports(&file, &path, cli.lang.as_deref(), depth, cli.no_ignore);
            return;
        }
        Some(Commands::Importers { file, path, depth }) => {
            importgraph::importers(&file, &path, cli.lang.as_deref(), depth, cli.no_ignore);
            return;
        }
        Some(Commands::Importgraph { path, format }) => {
            importgraph::importgraph(&path, cli.lang.as_deref(), cli.no_ignore, &format);
            return;
        }
        Some(Commands::Example { snippet, debug }) => {
            std::process::exit(example::run(&snippet, cli.lang.as_deref(), debug));
        }
        Some(Commands::Types { langs }) => {
            types::run(&langs, cli.lang.as_deref());
            return;
        }
        Some(Commands::Path { query, paths, max, all, only, body, expand }) => {
            std::process::exit(run_path(&query, &paths, cli.lang.as_deref(), cli.no_ignore, max, all, &only, body, expand));
        }
        Some(Commands::Config { args, .. }) => {
            std::process::exit(config::run(&args));
        }
        None => {}
    }

    // --- existing search behaviour ---
    let query_str = match cli.query.as_deref() {
        Some(q) => q,
        None => {
            eprintln!("error: --query is required when no subcommand is given");
            eprintln!("Usage: ctsq -q <QUERY> [FILES]  or  ctsq <SUBCOMMAND>");
            eprintln!("Subcommands: tree, callers, callees");
            std::process::exit(1);
        }
    };

    let ast = match query::parse(query_str) {
        Ok(q) => q,
        Err(e) => {
            eprint!("{}", miss::query_error_report(query_str, &e));
            std::process::exit(2);
        }
    };

    // Attributes are filtered after matching, against the last selector's node.
    let attrs = match semantic::attrs::last_selector_attrs(&ast) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    };

    if cli.context.is_some() && cli.node {
        eprintln!("-C/--context does not apply to --node, which already prints the whole node");
        std::process::exit(2);
    }

    // --show-query with explicit --lang: print and maybe exit
    if cli.show_query {
        if let Some(lang_name) = &cli.lang {
            let lang = require_lang(lang_name);
            let canonical = lang::lang_for_name_canonical(lang_name).unwrap_or(lang_name);
            if let Err(msg) = compiler::validate(&ast, lang.as_ref(), canonical) {
                eprintln!("{}", msg);
                std::process::exit(validate_exit_code(&msg));
            }
            if let Err(e) = compiler::check_compiles(&ast, lang.as_ref(), canonical, query_str) {
                eprint!("{}", miss::query_error_report(query_str, &e));
                std::process::exit(2);
            }
            if let Err(e) = compiler::check_fields(&ast, &[lang.as_ref()], query_str) {
                eprint!("{}", miss::query_error_report(query_str, &e));
                std::process::exit(2);
            }
            let compiled = compiler::compile(&ast, lang.as_ref());
            println!("=== Compiled S-expression ({}) ===", lang_name);
            if compiled.is_empty() {
                println!("<no patterns — abstract type has no mapping in this language>");
            } else {
                match &compiled {
                    compiler::CompiledQuery::Simple(s) => println!("{}", s),
                    compiler::CompiledQuery::Scoped { outer, scope_field, inner } => {
                        println!("[scoped two-phase query on field '{}']", scope_field);
                        println!("  outer: {}", outer);
                        println!("  inner: {}", inner);
                    }
                }
            }
            if cli.source_files.is_empty() {
                return;
            }
        }
    }

    let stdin_source_files: Vec<String>;
    let files_from_stdin: bool;
    let effective_files: &Vec<String> = if cli.source_files.is_empty() {
        if !std::io::stdin().is_terminal() {
            stdin_source_files = std::io::stdin()
                .lock()
                .lines()
                .filter_map(|l| l.ok())
                .filter(|l| !l.trim().is_empty())
                .collect();
            files_from_stdin = true;
            &stdin_source_files
        } else {
            if cli.show_query && cli.lang.is_none() {
                eprintln!("--show-query requires --lang when no source file is given");
                std::process::exit(1);
            }
            return;
        }
    } else {
        files_from_stdin = false;
        &cli.source_files
    };

    let stdin_bytes: Option<Vec<u8>> = if effective_files.iter().any(|f| f == "-") {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)
            .unwrap_or_else(|e| { eprintln!("Failed to read stdin: {}", e); std::process::exit(1) });
        Some(buf)
    } else {
        None
    };

    let mut jobs: Vec<(String, &'static str)> = Vec::new();
    let mut hider = files::Hider::new(&hide);
    for path in effective_files {
        if path == "-" {
            let lang_name = match &cli.lang {
                Some(ln) => match lang::lang_for_name_canonical(ln) {
                    Some(n) => n,
                    None => {
                        eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, ruby, rust, go", ln);
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("--lang is required when reading from stdin (-)");
                    std::process::exit(1);
                }
            };
            jobs.push(("-".to_string(), lang_name));
            continue;
        }

        // A directory named among paths piped on stdin is not walked.
        if files_from_stdin && std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
            continue;
        }
        let found = files::resolve_jobs_lenient(path, cli.lang.as_deref(), cli.no_ignore);
        // Paths piped on stdin were not named on the command line, so they are
        // judged from the current directory.
        hider.add_jobs(if files_from_stdin { "." } else { path }, &found);
        jobs.extend(found);
    }
    let mut hidden = footer::Hidden::default();

    let multiple = jobs.len() > 1;
    let mut total_matches = 0usize;
    let mut printed_a_block = false;

    // Reject an abstract kind no language in the scan knows before running
    // anything, so the message names the kind rather than the tree-sitter fallout.
    let mut checked: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
    for (_, lang_name) in &jobs {
        if !checked.insert(lang_name) {
            continue;
        }
        let l = lang::for_name(lang_name).expect("lang_name always valid here");
        if let Err(msg) = compiler::validate(&ast, l.as_ref(), lang_name) {
            eprintln!("{}", msg);
            std::process::exit(validate_exit_code(&msg));
        }
        if let Err(e) = compiler::check_compiles(&ast, l.as_ref(), lang_name, query_str) {
            eprint!("{}", miss::query_error_report(query_str, &e));
            std::process::exit(2);
        }
        if let Err(msg) = semantic::attrs::check_lang(&attrs, lang_name) {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    }
    {
        let boxed: Vec<Box<dyn lang::Lang>> = checked.iter().filter_map(|n| lang::for_name(n)).collect();
        let refs: Vec<&dyn lang::Lang> = boxed.iter().map(|b| b.as_ref()).collect();
        if let Err(e) = compiler::check_fields(&ast, &refs, query_str) {
            eprint!("{}", miss::query_error_report(query_str, &e));
            std::process::exit(2);
        }
    }

    let mut compiled_cache: HashMap<&'static str, (Box<dyn lang::Lang>, compiler::CompiledQuery)> =
        HashMap::new();

    for (file, lang_name) in &jobs {
        let (lang_box, compiled) = compiled_cache.entry(lang_name).or_insert_with(|| {
            let l = lang::for_name(lang_name).expect("lang_name always valid here");
            let c = compiler::compile(&ast, l.as_ref());
            (l, c)
        });

        if cli.show_query && !multiple {
            println!("=== Compiled S-expression ({}) ===", lang_name);
            if compiled.is_empty() {
                println!("<no patterns — abstract type has no mapping in this language>");
            } else {
                match compiled {
                    compiler::CompiledQuery::Simple(s) => println!("{}", s),
                    compiler::CompiledQuery::Scoped { outer, scope_field, inner } => {
                        println!("[scoped two-phase query on field '{}']", scope_field);
                        println!("  outer: {}", outer);
                        println!("  inner: {}", inner);
                    }
                }
            }
            println!("\n=== Matches in {} ===", file);
        }

        let source = if file == "-" {
            stdin_bytes.as_ref().expect("stdin pre-read").clone()
        } else {
            match std::fs::read(file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to read {}: {}", file, e);
                    continue;
                }
            }
        };

        let display = if file == "-" { "<stdin>" } else { file.as_str() };

        let result = runner::run_rows(compiled, lang_box.as_ref(), &source, cli.body).map(|rows| {
            let rows = semantic::attrs::filter_rows(rows, &attrs, lang_name, &lang_box.ts_language(), &source);
            hide_rows(&mut hider, file, &source, rows, true, &mut hidden)
        });
        match result {
            Ok(rows) if rows.is_empty() => {
                if cli.show_query && !multiple {
                    println!("(no matches)");
                }
            }
            Ok(rows) => match cli.context {
                Some(ctx) => {
                    print_with_context(
                        display,
                        &source,
                        &rows,
                        ctx,
                        &mut printed_a_block,
                        &mut total_matches,
                    );
                }
                None => {
                    let mut seen = std::collections::HashSet::new();
                    for r in &rows {
                        let out = match (&r.src_line, cli.node) {
                            // Default: the whole source line, the way explain prints
                            // its REFERENCES, always prefixed with the path.
                            (Some(src), false) => format!("{}:{}: {}", display, r.line, src),
                            // --node, or a match the source line cannot carry.
                            _ => with_path(display, &r.node_text),
                        };
                        if seen.insert(out.clone()) {
                            total_matches += 1;
                            println!("{}", out);
                        }
                    }
                }
            },
            Err(e) => {
                eprintln!("Runner error in {}: {}", file, e);
            }
        }
    }

    // A query whose only matches were hidden prints the footer, not a miss
    // report, so the reader sees what is hidden and how to show it.
    if total_matches == 0 && !hidden.any() {
        let where_ = if files_from_stdin {
            "the files on stdin".to_string()
        } else {
            effective_files.join(" ")
        };
        let shown: Vec<(String, &'static str)> =
            jobs.iter().filter(|(f, _)| hider.file(f).is_none()).cloned().collect();
        print!("{}", miss::report_for(&ast, query_str, &where_, &shown));
        std::process::exit(1);
    }
    print!("{}", footer::footer(total_matches, "match", &hidden));
    if total_matches == 0 {
        std::process::exit(1);
    }
}

/// Drop the rows `hider` hides, counting them in `hidden`. With `per_line` a
/// hidden row counts once per line, the way `-q` prints one line per match.
/// Without it every row counts, the way `refs` prints every row.
fn hide_rows(
    hider: &mut files::Hider,
    file: &str,
    source: &[u8],
    rows: Vec<runner::MatchRow>,
    per_line: bool,
    hidden: &mut footer::Hidden,
) -> Vec<runner::MatchRow> {
    if rows.is_empty() {
        return rows;
    }
    let count = |cut: &[usize]| {
        if per_line {
            cut.iter().collect::<std::collections::HashSet<_>>().len()
        } else {
            cut.len()
        }
    };
    if let Some(why) = hider.file(file) {
        let lines: Vec<usize> = rows.iter().map(|r| r.line).collect();
        hidden.count(why, count(&lines));
        return vec![];
    }
    let scopes = hider.scope_lines(file, Some(source));
    if scopes.is_empty() {
        return rows;
    }
    let mut cut = Vec::new();
    let kept = rows
        .into_iter()
        .filter(|r| {
            let inside = scopes.iter().any(|(a, b)| *a <= r.line && r.line <= *b);
            if inside {
                cut.push(r.line);
            }
            !inside
        })
        .collect();
    hidden.count(files::Hide::Test, count(&cut));
    kept
}

/// `-C N` output, modelled on grep -C: every match prints with the N source
/// lines either side of it. Matched lines keep the `path:LINE: text` form,
/// context lines use `path-LINE- text`, groups are separated by a `--` line, and
/// windows that overlap or touch merge into one group. Lines keep their own
/// indentation here, unlike the one-line form, because the point of asking for
/// context is to read the block as it sits in the file.
///
/// A match the source line cannot carry (two or more captures, or a multi-line
/// --body match) keeps its node form and gets no context.
fn print_with_context(
    display: &str,
    source: &[u8],
    rows: &[runner::MatchRow],
    ctx: usize,
    printed_a_block: &mut bool,
    total_matches: &mut usize,
) {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<&str> = text.lines().collect();

    let mut matched: Vec<usize> = rows
        .iter()
        .filter(|r| r.src_line.is_some() && r.line > 0)
        .map(|r| r.line)
        .collect();
    matched.sort_unstable();
    matched.dedup();
    *total_matches += matched.len();
    let is_match: std::collections::HashSet<usize> = matched.iter().copied().collect();

    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &m in &matched {
        let start = m.saturating_sub(ctx).max(1);
        let end = (m + ctx).min(lines.len());
        match groups.last_mut() {
            // Touching or overlapping windows read as one block.
            Some((_, prev_end)) if start <= *prev_end + 1 => {
                if end > *prev_end {
                    *prev_end = end;
                }
            }
            _ => groups.push((start, end)),
        }
    }

    for (start, end) in groups {
        if *printed_a_block {
            println!("--");
        }
        *printed_a_block = true;
        for ln in start..=end {
            let line = lines.get(ln - 1).map(|l| l.trim_end()).unwrap_or("");
            let marker = if is_match.contains(&ln) { ':' } else { '-' };
            // A blank line keeps no trailing space after the marker.
            if line.is_empty() {
                println!("{}{}{}{}", display, marker, ln, marker);
            } else {
                println!("{}{}{}{} {}", display, marker, ln, marker, line);
            }
        }
    }

    for r in rows.iter().filter(|r| r.src_line.is_none()) {
        if *printed_a_block {
            println!("--");
        }
        *printed_a_block = true;
        *total_matches += 1;
        println!("{}", with_path(display, &r.node_text));
    }
}

/// The node-text form has to say which file it came from, or a multi-line hit
/// sends the reader back to the file to find it. `<match>` is a placeholder for
/// exactly that, so the path replaces it. The capture form (`@f:4: …, @c:6: …`)
/// has no slot of its own and takes the path as a prefix.
fn with_path(display: &str, node_text: &str) -> String {
    match node_text.strip_prefix("<match>") {
        Some(rest) => format!("{}{}", display, rest),
        None => format!("{}:{}", display, node_text),
    }
}

fn run_refs(name: &str, path: &str, lang_override: Option<&str>, no_ignore: bool, hide: &files::HideOpts) {
    let query_str = format!("id#{}", name);
    let ast = match query::parse(&query_str) {
        Ok(q) => q,
        Err(e) => { eprint!("{}", miss::query_error_report(&query_str, &e)); std::process::exit(2); }
    };

    let jobs = files::resolve_jobs(path, lang_override, no_ignore);
    if jobs.is_empty() {
        return;
    }
    let mut hider = files::Hider::new(hide);
    hider.add_jobs(path, &jobs);
    let mut hidden = footer::Hidden::default();
    let mut shown = 0usize;
    let mut compiled_cache: HashMap<&'static str, (Box<dyn lang::Lang>, compiler::CompiledQuery)> =
        HashMap::new();

    for (file, lang_name) in &jobs {
        let (lang_box, compiled) = compiled_cache.entry(lang_name).or_insert_with(|| {
            let l = lang::for_name(lang_name).expect("lang_name always valid here");
            let c = compiler::compile(&ast, l.as_ref());
            (l, c)
        });
        let source = match std::fs::read(file) {
            Ok(s) => s,
            Err(e) => { eprintln!("Failed to read {}: {}", file, e); continue; }
        };
        match runner::run_rows(compiled, lang_box.as_ref(), &source, false) {
            Ok(rows) => {
                for r in &hide_rows(&mut hider, file, &source, rows, false, &mut hidden) {
                    shown += 1;
                    println!("{}", with_path(file, &r.node_text));
                }
            }
            Err(e) => eprintln!("Runner error in {}: {}", file, e),
        }
    }
    print!("{}", footer::footer(shown, "reference", &hidden));
    if shown == 0 && hidden.any() {
        std::process::exit(1);
    }
}

/// `ctsq path`. Returns the exit code: 0 routes found, 1 none or an unknown
/// kind, 2 usage or query error.
#[allow(clippy::too_many_arguments)]
fn run_path(
    query_str: &str,
    paths: &[String],
    lang_override: Option<&str>,
    no_ignore: bool,
    max: usize,
    all: bool,
    only: &[String],
    body: bool,
    expand: bool,
) -> i32 {
    // A query ending in `=>` or `==>` has no end selector.
    let open = semantic::flows::open_end(query_str);
    let parsed = match &open {
        Some((head, _)) => query::parse(head),
        None => query::parse(query_str),
    };
    let ast = match parsed {
        Ok(q) => q,
        Err(e) => {
            eprint!("{}", miss::query_error_report(query_str, &e));
            return 2;
        }
    };
    let flows = open.is_some()
        || ast.tail.iter().any(|(c, _)| {
            c.is_semantic()
                && !matches!(
                    c,
                    query::ast::Combinator::Calls
                        | query::ast::Combinator::CallsEventually
                        | query::ast::Combinator::CalledBy
                        | query::ast::Combinator::CalledByEventually
                )
        });

    let default = [".".to_string()];
    let paths = if paths.is_empty() { &default[..] } else { paths };
    let jobs: Vec<_> = paths
        .iter()
        .flat_map(|p| files::resolve_jobs_lenient(p, lang_override, no_ignore))
        .collect();
    let store = resolve::TreeStore::build(&jobs);
    let graph = resolve::graph::build_from_store(&store);
    if flows {
        let scan = std::path::Path::new(&paths[0]);
        let root = config::find_repo_root(scan);
        let opts = semantic::flows::FlowOptions { max, only: only.to_vec(), expand };
        return match semantic::flows::evaluate_flows(&ast, open.map(|(_, c)| c), &store, &graph, &root, scan, &opts) {
            Ok(r) => {
                print!("{}", r.text);
                if r.found > 0 { 0 } else { 1 }
            }
            Err(e) => {
                eprintln!("{}", e);
                path_error_code(&e)
            }
        };
    }
    let opts = semantic::calls::CallOptions { max, all, ..Default::default() };
    match semantic::calls::evaluate_calls(&ast, &store, &graph, &opts) {
        Ok(routes) => {
            print!(
                "{}",
                semantic::print::print_call_routes(&routes, &store, &semantic::print::PrintOpts { body })
            );
            if routes.routes.is_empty() { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("{}", e);
            path_error_code(&e)
        }
    }
}

/// Exit code for a `ctsq path` error: 1 for an unknown kind, as with `-q`,
/// and 2 for any other usage or query error.
fn path_error_code(msg: &str) -> i32 {
    if msg.starts_with("unknown kind") { 1 } else { 2 }
}

/// A semantic query is a usage error with a pointer to `ctsq path` (exit 2).
/// Any other validation failure is an unknown kind (exit 1).
fn validate_exit_code(msg: &str) -> i32 {
    if compiler::is_semantic_query_error(msg) { 2 } else { 1 }
}

fn require_lang(name: &str) -> Box<dyn lang::Lang> {
    match lang::for_name(name) {
        Some(l) => l,
        None => {
            eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, ruby, rust, go", name);
            std::process::exit(1);
        }
    }
}
