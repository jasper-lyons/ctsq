mod compiler;
mod files;
mod query;
mod runner;
mod tree;
mod calltree;
mod importgraph;
mod types;
mod resolve;

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
                  function  class  var  param  type  import  literal  block
                  if  for  while  switch
                  Unknown identifiers pass through as raw tree-sitter node types.

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

EXAMPLES
  ctsq -q 'id#Symbol' file.cpp                             grep -n \"Symbol\" file.cpp
  ctsq -q 'id#/A|B|C/' file.cpp                           grep -n \"A\\|B\\|C\" file.cpp
  ctsq -l cpp -q 'id#Symbol' src/                         grep -rn \"Symbol\" src/ --include=*.cpp
  ctsq -q 'id#Symbol' a.cpp b.cpp                         grep -n \"Symbol\" a.cpp b.cpp
  ctsq -q '*function#Foo' file.cpp                        grep -n \"void Foo\" file.cpp
  ctsq -q '(*function @f).body(id#CONST)' file.cpp        functions that reference CONST

STDIN
  Pipe file paths (language detected per file from extension):
    find . -name '*.rs' | ctsq -q 'id#Symbol'
    find . -name '*.py' | ctsq -l python -q '*function'

  Pipe source content directly (requires --lang; use - as the path):
    cat file.ts | ctsq -l typescript -q '*function' -
    echo 'fn foo() {}' | ctsq -l rust -q '*function' -

SUBCOMMANDS
  ctsq tree file.cpp                    structural outline of a file
  ctsq tree src/                        structural outline of all files in a directory
  ctsq callers ProcessMessages src/     who calls this function (up the call chain)
  ctsq callees ProcessMessages src/     what this function calls (down the call chain)
  ctsq callgraph src/                   full call graph in DOT format (pipe to dot -Tpng)
  ctsq callgraph src/ --format edges    full call graph as plain caller -> callee pairs
  ctsq def helper src/                  where is this function defined (file:line)
  ctsq def Foo::method src/             resolve a qualified path or method to its definition
  ctsq imports src/main.rs src/         what files this file imports (down the import tree)
  ctsq importers src/files.rs src/      what files import this file (up the import tree)
  ctsq importgraph src/                 full import graph in DOT format (pipe to dot -Tpng)
  ctsq importgraph src/ --format edges  full import graph as plain source -> target pairs
  ctsq types                            how abstract types map to tree-sitter nodes (all langs)
  ctsq types rust                       the same mapping for a single language

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

    /// Ignore .gitignore and .ignore files (search all files)
    #[arg(long, global = true)]
    no_ignore: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Structural outline of a file or directory: classes, functions, variables
    Tree {
        /// Files or directories to outline
        #[arg(num_args(1..))]
        paths: Vec<String>,
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
    /// Show how abstract node types map to tree-sitter nodes, per language
    Types {
        /// Languages to show (default: all). E.g. `ctsq types rust go`
        #[arg(num_args(0..))]
        langs: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Tree { paths }) => {
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
        Some(Commands::Types { langs }) => {
            types::run(&langs, cli.lang.as_deref());
            return;
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

    // --show-query with explicit --lang: print and maybe exit
    if cli.show_query {
        if let Some(lang_name) = &cli.lang {
            let lang = require_lang(lang_name);
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
                        eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, rust, go", ln);
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
                        eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, rust, go", ln);
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

        match runner::run(compiled, lang_box.as_ref(), &source, cli.body) {
            Ok(matches) if matches.is_empty() => {
                if cli.show_query && !multiple {
                    println!("(no matches)");
                }
            }
            Ok(matches) => {
                for m in &matches {
                    if multiple {
                        println!("{}:{}", display, m);
                    } else {
                        println!("{}", m);
                    }
                }
            }
            Err(e) => {
                eprintln!("Runner error in {}: {}", file, e);
            }
        }
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
    let multiple = jobs.len() > 1 || std::path::Path::new(path).is_dir();

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
                    if multiple {
                        println!("{}:{}", file, m);
                    } else {
                        println!("{}", m);
                    }
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
            eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, rust, go", name);
            std::process::exit(1);
        }
    }
}
