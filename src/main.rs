mod compiler;
mod cooccur;
mod example;
mod explain;
mod files;
mod find;
mod footer;
mod miss;
mod parsed;
mod query;
mod runner;
mod scope;
mod scopeof;
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
  No match prints a miss report on stderr, nothing on stdout, and exits 1. It
  names the query, the path and the file count. A lone id#name that matches nothing is rerun as text
  over string literals, JSX text and comments, and prints a `found as` block per
  kind plus a `try:` line to run. A lone *kind#name that matches nothing says
  where the name is defined under another kind. A walk that finds no files is
  a miss too. A path that does not exist prints `no such file: X (did you mean
  Y)` on stderr and the other paths still run. A query that does not parse
  prints the query with a caret under the fault and exits 2.

  Test files and test scopes (such as Rust mod tests) are hidden from tree, -q,
  def, refs, callers, callees, explain and files. Output that hid something
  ends with a count of what it showed and one hidden: line per reason, each
  naming its count and the flag that shows it, such as `hidden: 2 in tests
  (--tests)`. --exclude GLOB hides more paths, judged from the searched path. A
  file named directly is never hidden. When every match was hidden, stdout is
  empty, stderr reads `no match for Q outside tests in P (N files)` and the
  hidden: lines, and the exit is 1.

  --max-matches N prints at most N matches, rows or entries and counts the
  rest on a line such as `hidden: 31 over --max-matches 2 in 4 files
  (--max-matches 40)`, where 40 is the N that shows them all.

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
  # Too many hits? Cap the rows, the footer says how many were cut.
  ctsq -q 'id#run' src/ --max-matches 5
  # Who calls it? Which files mention both names?
  ctsq callers count_items src/
  ctsq files -q 'id#DefIndex' -q 'id#CallGraph' src/
  # Several names in one call, names first, then paths. A miss is named on stderr.
  ctsq def fmt_tokens count_items Missing src/
  ctsq explain fmt_tokens -- src/
  # Which function or class holds each hit? --scope adds a row under it, --body the scope's body.
  ctsq -q 'id#hide_rows' --scope src/
  ctsq def saveAssetNow fixtures/scope --scope
  # def goes from a name to a location, scopeof from a location to its definition.
  ctsq def hide_rows src/
  ctsq scopeof src/footer.rs:30
  ctsq scopeof src/footer.rs:30 --up 1 --siblings
  # Which named questions (routes, labels) does this repo answer? Then ask one.
  ctsq find
  ctsq find route /about
  ctsq find route '/users/{id}'
  # Which component shows this text? Which function emits this logged error?
  ctsq find text 'Welcome back'
  ctsq find error 'failed to load pipeline 12: open x: no such file'

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
  ctsq def A B C src/                   def, refs, callers, callees and explain take <NAME>... [PATH]...
                                        a name that is also a file needs --: ctsq def src -- lib/
  ctsq scopeof src/a.rs:40 src/b.rs:9   the reverse of def: the function or class holding each line, with its body
  ctsq files -q 'id#A' -q 'id#B' src/   files where every one of several queries matches
  ctsq find                             the named queries (route, ...) that apply in this directory
  ctsq find route /library              a named query: each route matching the key, and its component
  ctsq find text|label|component|error  UI text, labels, components and error messages, each with the function holding it
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

    /// Under each match print the nearest named scope holding it: its path:START-END and signature.
    /// With --body the scope's body follows the row
    #[arg(long)]
    scope: bool,

    /// Ignore .gitignore and .ignore files (search all files)
    #[arg(long, global = true)]
    no_ignore: bool,

    /// Show test files and test scopes, which are hidden by default and counted in the footer
    #[arg(long, global = true)]
    tests: bool,

    /// Hide paths matching GLOB, judged relative to the search root (repeatable)
    #[arg(long, global = true, value_name = "GLOB")]
    exclude: Vec<String>,

    /// Print at most N matches, rows or entries. The footer counts the rest and names the N that shows them all
    #[arg(long = "max-matches", global = true, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    max_matches: Option<u64>,
}

/// `--scope` and its `--body`, shared by the commands that return hits. They
/// are declared per command, since the top-level `--body` is not global and
/// `tree` and `path` each keep a `--body` of their own.
#[derive(clap::Args, Clone, Copy)]
struct ScopeArgs {
    /// Under each hit print the nearest named scope holding it: path:START-END and its signature
    #[arg(long)]
    scope: bool,
    /// With --scope, print each scope's body under its row
    #[arg(long)]
    body: bool,
}

impl ScopeArgs {
    /// None without --scope. A --body with no --scope is a usage error.
    fn opts(self) -> Option<scope::Opts> {
        if self.body && !self.scope {
            eprintln!("--body here needs --scope, it prints the scope's body");
            std::process::exit(2);
        }
        self.scope.then_some(scope::Opts { body: self.body })
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Everything about a symbol in one call: definition bodies, callers, callees, remaining references
    #[command(override_usage = "ctsq explain <NAME>... [PATH]... [OPTIONS]", after_help = "Answers \"what is this and how is it used\" without follow-up reads.
Composes def, callers, callees and an id#Symbol scan into one report.

  ctsq explain NewServer fixtures/
  ctsq explain Server.Start src/ --depth 2
  ctsq explain BigFunction src/ --max-lines 20
  ctsq explain Config src/ --no-body
  ctsq explain NewServer Config Missing src/
  ctsq explain NewServer -- fixtures/

Names come first, then paths (default: the current directory). An argument
that exists on disk is a path. -- makes everything after it a path, and is
needed when a name is also the name of a file or directory here. Each name is
answered in order. Names that did not resolve are listed on stderr as one
`not found:` line, and the exit is 0 when any name resolved.

Sections, in order: DEFINITION (file:line plus the full body text),
CALLERS, CALLEES (call site, the function on the other end, where it is
defined, and the call line), then REFERENCES (a per-file count of the
id#Symbol matches not already shown). With --no-body the signature prints on
its own `sig:` line, and a truncated body keeps the signature as the first
line it prints. When the symbol resolves to nothing, the plain id#Symbol
matches are listed instead.")]
    Explain {
        /// Names (symbol, a::b::f, Foo::method or x.method) then files or directories to search (default: .)
        #[arg(num_args(1..), required = true, value_name = "NAME_OR_PATH")]
        args: Vec<String>,
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
    #[command(override_usage = "ctsq callers <NAME>... [PATH]... [OPTIONS]", after_help = "Several names are answered in order. Names come first, then paths (default:
the current directory), and -- forces what follows to be paths. Names that did
not resolve go on one `not found:` stderr line, and the exit is 0 when any did.

  ctsq callers count_items src/
  ctsq callers report_scoped hide_rows src/ --max-depth 2")]
    Callers {
        /// Function names to trace, then files or directories to search (default: .)
        #[arg(num_args(1..), required = true, value_name = "NAME_OR_PATH")]
        args: Vec<String>,
        /// Maximum depth to traverse
        #[arg(long = "max-depth", alias = "depth", default_value = "3")]
        max_depth: usize,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Show what a function calls (walk down the call chain)
    #[command(override_usage = "ctsq callees <NAME>... [PATH]... [OPTIONS]", after_help = "Several names are answered in order. Names come first, then paths (default:
the current directory), and -- forces what follows to be paths. Names that did
not resolve go on one `not found:` stderr line, and the exit is 0 when any did.

  ctsq callees count_items src/
  ctsq callees report_scoped hide_rows src/")]
    Callees {
        /// Function names to trace, then files or directories to search (default: .)
        #[arg(num_args(1..), required = true, value_name = "NAME_OR_PATH")]
        args: Vec<String>,
        /// Maximum depth to traverse
        #[arg(long = "max-depth", alias = "depth", default_value = "3")]
        max_depth: usize,
        #[command(flatten)]
        scope: ScopeArgs,
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
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Resolve a name to its definition location(s)
    #[command(override_usage = "ctsq def <NAME>... [PATH]... [OPTIONS]", after_help = "With --scope each definition gets a row under it naming the scope that
holds it, so a name defined inside several components tells them apart:

  ctsq def saveAssetNow src/ --scope

Several names are answered in order. Names come first, then paths (default:
the current directory). An argument that exists on disk is a path, -- forces
what follows to be paths, and a name that is also a file or directory here
needs the --. Names that did not resolve go on one `not found:` stderr line,
with the nearest name where there is one, and the exit is 0 when any did:

  ctsq def fmt_tokens count_items Missing src/")]
    Def {
        /// Names (a name, a::b::f, Foo::method or x.method) then files or directories to search (default: .)
        #[arg(num_args(1..), required = true, value_name = "NAME_OR_PATH")]
        args: Vec<String>,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// The function or class holding each path:line, with its body (the reverse of def)
    #[command(after_help = "def goes from a name to a location, scopeof goes from a location to its
definition. It takes the path:line or path:line:col that grep, compilers and
stack traces print, and returns the nearest named scope holding the line:

  ctsq scopeof src/footer.rs:30
  ctsq scopeof src/a.rs:40:12 src/b.rs:9 --up 1
  ctsq scopeof src/a.rs:40 --siblings --callers
  ctsq scopeof src/main.rs:600 --budget 20

Each coordinate prints `path:LINE  in  path:START-END  signature` and then the
scope's lines as they sit in the file. A line outside every named scope prints
`path:LINE  at top level` and a window of the file. A scope longer than --budget
prints its heading, a window centred on the line and a `showing lines` footer.
A line that is a function's own signature returns that function. Closures are
not named scopes, so a line inside one returns its named parent. Test hiding
does not apply. A missing file or a line past the end goes to stderr and the
other coordinates still run. Exit 0 when any coordinate resolved, 1 when none
did, 2 when an argument is not a coordinate.")]
    Scopeof {
        /// path:LINE or path:LINE:COL (COL is a 1-based byte column)
        #[arg(num_args(1..), required = true)]
        coords: Vec<String>,
        /// Climb N enclosing named scopes (method, then class, then module)
        #[arg(long, default_value = "0", conflicts_with = "block")]
        up: usize,
        /// After the body, list the other scopes in the same parent, signature only
        #[arg(long)]
        siblings: bool,
        /// After the body, list the callers of the scope, one level
        #[arg(long)]
        callers: bool,
        /// Return the innermost block (an if or loop body) instead of the named scope
        #[arg(long)]
        block: bool,
        /// A scope longer than N lines prints a window of N lines centred on the line
        #[arg(long, default_value = "150", value_parser = clap::value_parser!(u64).range(1..))]
        budget: u64,
    },
    /// Run a named query (route, ...) for the frameworks detected here, or list them
    #[command(after_help = "A named query answers a question whose shape is fixed per framework, such as
which component serves a route. With no arguments, find lists every named
query that applies in the current directory and why:

  ctsq find
  ctsq find route
  ctsq find route /library
  ctsq find route '/^/w/'

  ctsq find text 'Welcome back'
  ctsq find error 'failed to load pipeline 12: open x: no such file'

An error key is a format string, so %s %v %d %w %q %x %f %T %+v and ${...} match
any text and a message copied from a log finds it. A repo glossary,
.ctsq/glossary.toml at the repo root, maps product words to code names. A key
holding one of its terms also finds rows by those names, and a last line says so.

Each match prints path:LINE, the key, the target and the target's path:LINE
(? when def finds no definition), then one count line per framework naming
the file the query came from. A key matches as a case-insensitive substring,
or as a regex when written /.../. The search root is the current directory,
and tests are hidden as everywhere else (--tests shows them).

Queries are TOML files in queries/ beside languages/, looked up in repo
.ctsq/, then $XDG_CONFIG_HOME/ctsq, then $XDG_CACHE_HOME/ctsq, then built-in
(ctsq config path lists the directories). A repo file with the same framework
and query name replaces the built-in one. Exit 0 when anything matched, 1 when
nothing did, 2 for an unknown query or a query file that does not compile.")]
    Find {
        /// Query name, such as route. Omit to list the queries that apply here
        query: Option<String>,
        /// Key filter: a case-insensitive substring, or /regex/
        key: Option<String>,
    },
    /// Find all usage sites of a name (complement of def)
    #[command(override_usage = "ctsq refs <NAME>... [PATH]... [OPTIONS]", after_help = "Several names are answered in order. Names come first, then paths (default:
the current directory), and -- forces what follows to be paths. Names that did
not resolve go on one `not found:` stderr line, and the exit is 0 when any did.

  ctsq refs fmt_tokens count_items src/")]
    Refs {
        /// Identifier names to find references to, then files or directories to search (default: .)
        #[arg(num_args(1..), required = true, value_name = "NAME_OR_PATH")]
        args: Vec<String>,
        #[command(flatten)]
        scope: ScopeArgs,
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
        #[arg(long = "max-depth", default_value = "12")]
        max_depth: usize,
        /// Renamed to --max-depth. Kept only to point at the new name.
        #[arg(long = "max", hide = true, value_name = "N")]
        old_max: Option<String>,
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
    #[command(after_help = "Language mechanics, outside-call models and named queries are TOML files,
looked up in order: repo .ctsq/, then $XDG_CONFIG_HOME/ctsq (default
~/.config/ctsq), then $XDG_CACHE_HOME/ctsq (default ~/.cache/ctsq), then
built-in defaults. `path` lists each root, then the queries/ directory in each.

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
    parsed::report_stats_at_exit();
    let cli = Cli::parse();
    let hide = files::HideOpts { tests: cli.tests, exclude: cli.exclude.clone() };
    footer::set_max(cli.max_matches.map(|n| n as usize));

    match cli.command {
        Some(Commands::Explain { args, depth, max_lines, no_body }) => {
            let opts = explain::Options { depth, max_lines, no_body };
            let t = targets(&args, "explain", cli.lang.as_deref(), cli.no_ignore, &hide);
            explain::run(&t.names, &t.where_, &t.jobs, t.hider, &opts);
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
            let where_ = paths.join(" ");
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
            footer::settle(&mut hidden);
            if printed == 0 {
                if hidden.any() {
                    eprint!("{}", footer::nothing_shown("no entry to show", &where_, jobs.len(), &hidden));
                }
                std::process::exit(1);
            }
            print!("{}", footer::footer(entries, "entry", &hidden));
            return;
        }
        Some(Commands::Callers { args, max_depth, scope }) => {
            let t = targets(&args, "callers", cli.lang.as_deref(), cli.no_ignore, &hide);
            calltree::callers(&t.names, &t.where_, &t.jobs, t.hider, max_depth, scope.opts());
            return;
        }
        Some(Commands::Callees { args, max_depth, scope }) => {
            let t = targets(&args, "callees", cli.lang.as_deref(), cli.no_ignore, &hide);
            calltree::callees(&t.names, &t.where_, &t.jobs, t.hider, max_depth, scope.opts());
            return;
        }
        Some(Commands::Callgraph { path, format }) => {
            calltree::callgraph(&path, cli.lang.as_deref(), cli.no_ignore, &format);
            return;
        }
        Some(Commands::Files { query, path, scope }) => {
            cooccur::run(&query, &path, cli.lang.as_deref(), cli.no_ignore, &hide, scope.opts());
            return;
        }
        Some(Commands::Def { args, scope }) => {
            let t = targets(&args, "def", cli.lang.as_deref(), cli.no_ignore, &hide);
            resolve::run_def(&t.names, &t.where_, &t.jobs, t.hider, scope.opts());
            return;
        }
        Some(Commands::Scopeof { coords, up, siblings, callers, block, budget }) => {
            let mut parsed = Vec::new();
            for c in &coords {
                match scopeof::parse_coord(c) {
                    Ok(p) => parsed.push(p),
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(2);
                    }
                }
            }
            let opts = scopeof::Options {
                up,
                siblings,
                callers,
                block,
                budget: budget as usize,
                lang: cli.lang.clone(),
                no_ignore: cli.no_ignore,
            };
            let r = scopeof::report(&parsed, &opts, &hide);
            eprint!("{}", r.err);
            print!("{}", r.out);
            std::process::exit(r.code);
        }
        Some(Commands::Find { query, key }) => {
            std::process::exit(find::run(query.as_deref(), key.as_deref(), cli.no_ignore, &hide));
        }
        Some(Commands::Refs { args, scope }) => {
            let t = targets(&args, "refs", cli.lang.as_deref(), cli.no_ignore, &hide);
            run_refs(&t.names, &t.where_, &t.jobs, t.hider, scope.opts());
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
        Some(Commands::Path { query, paths, max_depth, old_max, all, only, body, expand }) => {
            if old_max.is_some() {
                eprintln!("--max is now --max-depth");
                std::process::exit(2);
            }
            std::process::exit(run_path(&query, &paths, cli.lang.as_deref(), cli.no_ignore, max_depth, all, &only, body, expand));
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
    if cli.scope && cli.context.is_some() {
        eprintln!("-C/--context does not apply to --scope, which already adds the scope row under each match");
        std::process::exit(2);
    }
    let scope_opts = cli.scope.then_some(scope::Opts { body: cli.body });
    let mut scopes = scope::Scopes::new();

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

        // A file's text and tree come from the run's cache, so test hiding
        // and anything after reuse this parse. Stdin has no path to key on.
        let fetched = if file == "-" {
            Fetched::Text(std::rc::Rc::new(stdin_bytes.as_ref().expect("stdin pre-read").clone()))
        } else {
            match fetch(file, lang_name, compiled.is_empty()) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to read {}: {}", file, e);
                    continue;
                }
            }
        };
        let source: &[u8] = fetched.source();

        let display = if file == "-" { "<stdin>" } else { file.as_str() };

        let result = fetched.rows(compiled, lang_box.as_ref(), cli.body).map(|rows| {
            let rows = semantic::attrs::filter_rows(rows, &attrs, lang_name, &lang_box.ts_language(), source);
            hide_rows(&mut hider, file, source, fetched.tree(), rows, true, &mut hidden)
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
                        source,
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
                            if !footer::take(display) {
                                continue;
                            }
                            total_matches += 1;
                            println!("{}", out);
                            if let Some(o) = scope_opts {
                                if let Some(s) = scopes.hit(display, lang_name, source, r.start_byte) {
                                    print!("{}", scope::render(&s, source, o.body, ""));
                                }
                            }
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
    footer::settle(&mut hidden);
    let where_ = if files_from_stdin {
        "the files on stdin".to_string()
    } else {
        effective_files.join(" ")
    };
    if total_matches == 0 && hidden.any() {
        eprint!(
            "{}",
            footer::nothing_shown(&format!("no match for {}", query_str), &where_, jobs.len(), &hidden)
        );
        std::process::exit(1);
    }
    if total_matches == 0 {
        let shown: Vec<(String, &'static str)> =
            jobs.iter().filter(|(f, _)| hider.file(f).is_none()).cloned().collect();
        let glossary = match config::glossary::load(&std::env::current_dir().unwrap_or_default()) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(2);
            }
        };
        eprint!("{}", miss::report_for_glossary(&ast, query_str, &where_, &shown, glossary.as_ref()));
        std::process::exit(1);
    }
    print!("{}", footer::tail(total_matches, "match", &hidden, scope_opts.is_some()));
}

/// A file's text and, when the query has patterns, its tree: from the run's
/// cache for a file, unparsed text for stdin.
enum Fetched {
    File(std::rc::Rc<parsed::ParsedFile>),
    /// Read but not parsed: stdin, or a query with no pattern for the language.
    Text(parsed::Source),
}

impl Fetched {
    fn source(&self) -> &[u8] {
        match self {
            Fetched::File(p) => &p.source,
            Fetched::Text(s) => s,
        }
    }

    fn tree(&self) -> Option<&tree_sitter::Tree> {
        match self {
            Fetched::File(p) => p.tree.as_ref(),
            Fetched::Text(_) => None,
        }
    }

    /// `runner::run_rows`, over the cached tree when there is one.
    fn rows(&self, compiled: &compiler::CompiledQuery, l: &dyn lang::Lang, body: bool) -> Result<Vec<runner::MatchRow>, String> {
        match self.tree() {
            Some(t) => runner::run_rows_on(compiled, l, self.source(), t, body),
            None => runner::run_rows(compiled, l, self.source(), body),
        }
    }
}

/// `file` from the cache, parsed with `lang_name`'s grammar unless `text_only`
/// (a query with no pattern matches nothing and needs no tree).
fn fetch(file: &str, lang_name: &'static str, text_only: bool) -> std::io::Result<Fetched> {
    let source = parsed::try_source(file)?;
    if text_only {
        return Ok(Fetched::Text(source));
    }
    Ok(match parsed::get(file, lang_name) {
        Some(p) => Fetched::File(p),
        None => Fetched::Text(source),
    })
}

/// Drop the rows `hider` hides, counting them in `hidden`. With `per_line` a
/// hidden row counts once per line, the way `-q` prints one line per match.
/// Without it every row counts, the way `refs` prints every row. `tree` is
/// the file's parse when the caller holds it, so test scopes reuse it.
fn hide_rows(
    hider: &mut files::Hider,
    file: &str,
    source: &[u8],
    tree: Option<&tree_sitter::Tree>,
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
    let scopes = hider.scope_lines_in(file, Some(source), tree);
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
    matched.retain(|_| footer::take(display));
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
        if !footer::take(display) {
            continue;
        }
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

/// What `def`, `refs`, `callers`, `callees` and `explain` were asked about: the
/// names, the files under the paths given, and a `Hider` that knows which
/// search root each file came from.
struct Targets {
    names: Vec<String>,
    /// The paths as typed, joined by spaces, for messages.
    where_: String,
    jobs: Vec<(String, &'static str)>,
    hider: files::Hider,
}

/// The arguments split into names and paths. `dropped` holds the path-like
/// arguments that do not exist.
#[derive(Debug, PartialEq)]
struct NamesAndPaths {
    names: Vec<String>,
    paths: Vec<String>,
    dropped: Vec<String>,
}

/// A name a person would type as a symbol: segments of letters, digits, `_`
/// and `$`, joined by `::`. A file such as `main.rs` is not one, so it is read
/// as a path without question.
fn identifier_like(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|seg| {
            !seg.is_empty()
                && !seg.starts_with(|c: char| c.is_ascii_digit())
                && seg.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
}

/// Git-style split of `NAME... [PATH]...`. `forced` is how many trailing
/// arguments came after `--`.
///
/// Without `--`, an argument that exists on disk is a path, a missing one with
/// a `/` is a path that is not there (dropped), and any other is a name. An
/// argument that is both an identifier-like name and an existing path is
/// ambiguous when it is the first argument, which is an error that points at
/// `--`. With `--`, everything
/// before it is a name and everything after it is a path, existing or not.
fn split_names_paths(
    cmd: &str,
    args: &[String],
    forced: usize,
    exists: impl Fn(&str) -> bool,
) -> Result<NamesAndPaths, String> {
    let split = args.len().saturating_sub(forced);
    let mut r = NamesAndPaths { names: vec![], paths: vec![], dropped: vec![] };
    for (i, a) in args.iter().enumerate() {
        if i >= split {
            if exists(a) { r.paths.push(a.clone()) } else { r.dropped.push(a.clone()) }
        } else if forced > 0 {
            r.names.push(a.clone());
        } else if exists(a) {
            // The first argument is where a name goes, so one that is also a
            // file or directory here could be either. Later ones are paths.
            if i == 0 && identifier_like(a) {
                return Err(format!(
                    "{a} is both a name and a path here, put -- before the paths: ctsq {cmd} {a} -- PATH"
                ));
            }
            r.paths.push(a.clone());
        } else if a.contains('/') {
            r.dropped.push(a.clone());
        } else {
            r.names.push(a.clone());
        }
    }
    if r.names.is_empty() {
        return Err(format!("{cmd} needs at least one name, usage: ctsq {cmd} <NAME>... [PATH]..."));
    }
    Ok(r)
}

/// How many arguments follow the first `--` on the command line. clap drops
/// the `--` itself, so this is the only place that still sees it.
fn args_after_double_dash() -> usize {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.iter().position(|a| a == "--") {
        Some(i) => argv.len() - i - 1,
        None => 0,
    }
}

/// Split the arguments, report missing paths on stderr, and find the files.
/// A usage error exits 2. When every path given was missing, exits 1.
fn targets(
    args: &[String],
    cmd: &str,
    lang_override: Option<&str>,
    no_ignore: bool,
    hide: &files::HideOpts,
) -> Targets {
    let split = match split_names_paths(cmd, args, args_after_double_dash(), |p| std::path::Path::new(p).exists()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };
    for d in &split.dropped {
        eprintln!("{}", miss::no_such_file(d));
    }
    let paths = if split.paths.is_empty() {
        if !split.dropped.is_empty() {
            std::process::exit(1);
        }
        vec![".".to_string()]
    } else {
        split.paths
    };
    let mut hider = files::Hider::new(hide);
    let mut jobs: Vec<(String, &'static str)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in &paths {
        let found = files::resolve_jobs(p, lang_override, no_ignore);
        hider.add_jobs(p, &found);
        for j in found {
            if seen.insert(j.0.clone()) {
                jobs.push(j);
            }
        }
    }
    Targets { names: split.names, where_: paths.join(" "), jobs, hider }
}

fn run_refs(
    names: &[String],
    where_: &str,
    jobs: &[(String, &'static str)],
    mut hider: files::Hider,
    scope_opts: Option<scope::Opts>,
) {
    let mut asts = Vec::new();
    for name in names {
        let query_str = format!("id#{}", name);
        match query::parse(&query_str) {
            Ok(q) => asts.push(q),
            Err(e) => { eprint!("{}", miss::query_error_report(&query_str, &e)); std::process::exit(2); }
        }
    }
    if jobs.is_empty() {
        eprint!("{}", resolve::not_found_line(names, jobs));
        std::process::exit(1);
    }
    let mut scopes = scope::Scopes::new();
    let mut hidden = footer::Hidden::default();
    let mut shown = 0usize;
    let mut missing: Vec<String> = Vec::new();

    for (name, ast) in names.iter().zip(&asts) {
        let mut name_hidden = footer::Hidden::default();
        let shown_before = shown;
        let mut compiled_cache: HashMap<&'static str, (Box<dyn lang::Lang>, compiler::CompiledQuery)> =
            HashMap::new();
        for (file, lang_name) in jobs {
            let (lang_box, compiled) = compiled_cache.entry(lang_name).or_insert_with(|| {
                let l = lang::for_name(lang_name).expect("lang_name always valid here");
                let c = compiler::compile(ast, l.as_ref());
                (l, c)
            });
            let fetched = match fetch(file, lang_name, compiled.is_empty()) {
                Ok(f) => f,
                Err(e) => { eprintln!("Failed to read {}: {}", file, e); continue; }
            };
            let source = fetched.source();
            match fetched.rows(compiled, lang_box.as_ref(), false) {
                Ok(rows) => {
                    for r in &hide_rows(&mut hider, file, source, fetched.tree(), rows, false, &mut name_hidden) {
                        if !footer::take(file) {
                            continue;
                        }
                        shown += 1;
                        println!("{}", with_path(file, &r.node_text));
                        if let Some(o) = scope_opts {
                            if let Some(s) = scopes.hit(file, lang_name, source, r.start_byte) {
                                print!("{}", scope::render(&s, source, o.body, ""));
                            }
                        }
                    }
                }
                Err(e) => eprintln!("Runner error in {}: {}", file, e),
            }
        }
        if shown == shown_before && !name_hidden.any() {
            missing.push(name.clone());
        }
        resolve::merge_hidden(&mut hidden, &name_hidden);
    }
    footer::settle(&mut hidden);
    if shown == 0 && hidden.any() {
        eprint!(
            "{}",
            footer::nothing_shown(&format!("no reference to {}", names.join(", ")), where_, jobs.len(), &hidden)
        );
    } else if shown > 0 {
        print!("{}", footer::tail(shown, "reference", &hidden, scope_opts.is_some()));
    }
    eprint!("{}", resolve::not_found_line(&missing, jobs));
    if shown == 0 {
        use std::io::Write;
        let _ = std::io::stdout().flush();
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


#[cfg(test)]
mod arg_tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// `src/` and `lib.rs` exist, nothing else does, and `src` is a directory name.
    fn exists(p: &str) -> bool {
        matches!(p, "src/" | "src" | "lib.rs" | "Makefile")
    }

    #[test]
    fn names_then_paths() {
        let r = split_names_paths("def", &v(&["A", "B", "src/"]), 0, exists).unwrap();
        assert_eq!(r.names, v(&["A", "B"]));
        assert_eq!(r.paths, v(&["src/"]));
        assert!(r.dropped.is_empty());
    }

    #[test]
    fn a_file_with_a_dot_is_a_path_not_an_ambiguity() {
        let r = split_names_paths("def", &v(&["A", "lib.rs"]), 0, exists).unwrap();
        assert_eq!(r.paths, v(&["lib.rs"]));
    }

    #[test]
    fn a_name_that_is_also_a_path_is_an_error_naming_the_double_dash() {
        let e = split_names_paths("def", &v(&["src", "x/"]), 0, exists).unwrap_err();
        assert!(e.contains("--"), "{}", e);
        let e = split_names_paths("def", &v(&["Makefile", "src/"]), 0, exists).unwrap_err();
        assert!(e.contains("Makefile") && e.contains("--"), "{}", e);
    }

    #[test]
    fn a_later_argument_that_exists_is_a_path_even_when_it_looks_like_a_name() {
        let r = split_names_paths("callers", &v(&["A", "src"]), 0, exists).unwrap();
        assert_eq!(r.names, v(&["A"]));
        assert_eq!(r.paths, v(&["src"]));
    }

    #[test]
    fn the_double_dash_makes_the_rest_paths_and_the_front_names() {
        let r = split_names_paths("explain", &v(&["src", "lib.rs", "nope/"]), 2, exists).unwrap();
        assert_eq!(r.names, v(&["src"]));
        assert_eq!(r.paths, v(&["lib.rs"]));
        assert_eq!(r.dropped, v(&["nope/"]));
    }

    #[test]
    fn a_missing_argument_with_a_slash_is_dropped_and_one_without_is_a_name() {
        let r = split_names_paths("refs", &v(&["A", "nope/x", "plain"]), 0, exists).unwrap();
        assert_eq!(r.names, v(&["A", "plain"]));
        assert_eq!(r.dropped, v(&["nope/x"]));
    }

    #[test]
    fn no_name_is_a_usage_error() {
        let e = split_names_paths("def", &v(&["src/"]), 0, exists).unwrap_err();
        assert!(e.contains("needs at least one name"), "{}", e);
    }

    #[test]
    fn identifier_like_takes_qualified_names_but_not_files() {
        assert!(identifier_like("Foo"));
        assert!(identifier_like("a::b::f"));
        assert!(!identifier_like("main.rs"));
        assert!(!identifier_like("src/"));
        assert!(!identifier_like("9lives"));
        assert!(!identifier_like(""));
    }
}
