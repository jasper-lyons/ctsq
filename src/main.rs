mod compiler;
mod cooccur;
mod example;
mod explain;
mod files;
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
                  literal  block  if  for  while  switch
                  variable (alias const) is a variable declaration: *variable#X
                  reaches the whole `const X = …` statement, &variable#X the uses.
                  Unknown identifiers pass through as raw tree-sitter node types;
                  anything else is rejected with the valid kinds for the language.

  Sigils (scope)  *type    definitions only   (function_definition, class declaration…)
                  &type    references only     (call_expression, variable use…)
                  type     both (default)

  Name filter     type#name        exact match
                  type#\"my name\"  exact match (quoted, allows spaces)
                  type#/regex/     regex match

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
  No match prints a one-line miss report on stdout and exits 1.

  -C N (--context N) adds N source lines either side of each match, like
  grep -C: matched lines keep path:LINE:, context lines use path-LINE-, groups
  are separated by --, and lines keep their indentation. Not valid with --node.

EXAMPLES
  ctsq -q 'id#Symbol' file.cpp                             grep -n \"Symbol\" file.cpp
  ctsq -q 'id#/A|B|C/' file.cpp                           grep -n \"A\\|B\\|C\" file.cpp
  ctsq -l cpp -q 'id#Symbol' src/                         grep -rn \"Symbol\" src/ --include=*.cpp
  ctsq -q 'id#Symbol' a.cpp b.cpp                         grep -n \"Symbol\" a.cpp b.cpp
  ctsq -q 'id#Symbol' -C 2 file.ts                        grep -n -C 2 \"Symbol\" file.ts
  ctsq -q '*function#Foo' file.cpp                        grep -n \"void Foo\" file.cpp
  ctsq -q '*type#Config' --body file.go                   the whole type declaration
  ctsq -q '*variable#config' --body file.ts               the whole const declaration
  ctsq -q '(*function @f).body(id#CONST)' file.cpp        functions that reference CONST
  ctsq files -q 'id#A' -q 'id#B' src/                     grep -rl A | xargs grep -l B

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

    /// Target language (c, cpp, javascript, python, rust, go); inferred from extension if omitted
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
    /// Show where config and model files are read from, or the merged result for a language
    #[command(after_help = "Language mechanics and outside-call models are TOML files, looked up in order:
repo .ctsq/, then ~/.config/ctsq, then ~/.cache/ctsq, then built-in defaults.

  ctsq config path
  ctsq config dump go")]
    Config {
        /// `path` or `dump <lang>`
        #[arg(num_args(1..=2), required = true)]
        args: Vec<String>,
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

    match cli.command {
        Some(Commands::Explain { symbol, path, depth, max_lines, no_body }) => {
            let opts = explain::Options { depth, max_lines, no_body };
            explain::run(&symbol, &path, cli.lang.as_deref(), &opts, cli.no_ignore);
            return;
        }
        Some(Commands::Tree { paths, body }) => {
            if body {
                eprintln!("tree has no --body, use ctsq -q '*function#Name' --body FILE");
                std::process::exit(2);
            }
            let jobs: Vec<_> = paths.iter()
                .flat_map(|p| files::resolve_jobs(p, cli.lang.as_deref(), cli.no_ignore))
                .collect();
            let multiple = jobs.len() > 1;
            for (i, (file, lang_name)) in jobs.iter().enumerate() {
                if multiple && i > 0 { println!(); }
                tree::run(file, Some(lang_name));
            }
            return;
        }
        Some(Commands::Callers { function, path, depth }) => {
            calltree::callers(&function, &path, cli.lang.as_deref(), depth, cli.no_ignore);
            return;
        }
        Some(Commands::Callees { function, path, depth }) => {
            calltree::callees(&function, &path, cli.lang.as_deref(), depth, cli.no_ignore);
            return;
        }
        Some(Commands::Callgraph { path, format }) => {
            calltree::callgraph(&path, cli.lang.as_deref(), cli.no_ignore, &format);
            return;
        }
        Some(Commands::Files { query, path }) => {
            cooccur::run(&query, &path, cli.lang.as_deref(), cli.no_ignore);
            return;
        }
        Some(Commands::Def { name_or_expr, path }) => {
            resolve::run_def(&name_or_expr, &path, cli.lang.as_deref(), cli.no_ignore);
            return;
        }
        Some(Commands::Refs { name, path }) => {
            run_refs(&name, &path, cli.lang.as_deref(), cli.no_ignore);
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
        Some(Commands::Config { args }) => {
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
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
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
                std::process::exit(1);
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

        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Failed to access {}: {}", path, e);
                std::process::exit(1);
            }
        };
        if meta.is_dir() && files_from_stdin {
            continue;
        } else if meta.is_dir() {
            let exts = cli.lang.as_deref()
                .map(lang::extensions_for_name)
                .unwrap_or_else(lang::all_known_extensions);
            let dir_jobs = files::collect_files(path, exts, cli.no_ignore)
                .into_iter()
                .filter_map(|f| {
                    let ext = std::path::Path::new(&f)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    let lang_name = if let Some(ln) = &cli.lang {
                        lang::lang_for_name_canonical(ln)
                    } else {
                        lang::lang_for_extension(ext)
                    }?;
                    Some((f, lang_name))
                });
            jobs.extend(dir_jobs);
        } else {
            let lang_name = if let Some(ln) = &cli.lang {
                match lang::lang_for_name_canonical(ln) {
                    Some(n) => n,
                    None => {
                        eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, ruby, rust, go", ln);
                        std::process::exit(1);
                    }
                }
            } else {
                let ext = std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                match lang::lang_for_extension(ext) {
                    Some(n) => n,
                    None => {
                        eprintln!("Cannot detect language from extension .{}; use --lang", ext);
                        std::process::exit(1);
                    }
                }
            };
            jobs.push((path.clone(), lang_name));
        }
    }

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
            std::process::exit(1);
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

        match runner::run_rows(compiled, lang_box.as_ref(), &source, cli.body) {
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

    if total_matches == 0 && !jobs.is_empty() {
        let where_ = if files_from_stdin {
            "the files on stdin".to_string()
        } else {
            effective_files.join(" ")
        };
        println!(
            "no match for {} in {} ({} files scanned)",
            query_str,
            where_,
            jobs.len()
        );
        std::process::exit(1);
    }
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

fn run_refs(name: &str, path: &str, lang_override: Option<&str>, no_ignore: bool) {
    let query_str = format!("id#{}", name);
    let ast = match query::parse(&query_str) {
        Ok(q) => q,
        Err(e) => { eprintln!("Parse error: {}", e); std::process::exit(1); }
    };

    let jobs = files::resolve_jobs(path, lang_override, no_ignore);
    if jobs.is_empty() {
        return;
    }
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
        match runner::run(compiled, lang_box.as_ref(), &source, false) {
            Ok(matches) => {
                for m in &matches {
                    println!("{}", with_path(file, m));
                }
            }
            Err(e) => eprintln!("Runner error in {}: {}", file, e),
        }
    }
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
