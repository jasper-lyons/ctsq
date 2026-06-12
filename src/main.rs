mod compiler;
mod files;
mod query;
mod runner;
mod tree;
mod calltree;
mod resolve;

use clap::{Parser, Subcommand};
use compiler::lang;
use std::collections::HashMap;

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

SUBCOMMANDS
  ctsq tree file.cpp                    structural outline of a file
  ctsq tree src/                        structural outline of all files in a directory
  ctsq callers ProcessMessages src/     who calls this function (up the call chain)
  ctsq callees ProcessMessages src/     what this function calls (down the call chain)
  ctsq callgraph src/                   full call graph in DOT format (pipe to dot -Tpng)
  ctsq callgraph src/ --format edges    full call graph as plain caller -> callee pairs
  ctsq def helper src/                  where is this function defined (file:line)
  ctsq def Foo::method src/             resolve a qualified path or method to its definition

RESOLUTION
  Rust and Go call sites resolve through imports/aliases, qualified paths,
  bound function values, and method receivers typed via params, annotations,
  literals, constructors, and call return types. Dynamic dispatch expands:
  a call through a Go interface or a Rust dyn Trait / T: Trait / impl Trait
  receiver gets one edge per implementing type. Ambiguous calls resolve to
  the best-ranked candidate (same file > same dir > imported) and are marked
  with '?'. Other languages match by name only."
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

    /// Source files or directories to search
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

    if cli.source_files.is_empty() {
        if cli.show_query && cli.lang.is_none() {
            eprintln!("--show-query requires --lang when no source file is given");
            std::process::exit(1);
        }
        return;
    }

    let mut jobs: Vec<(String, &'static str)> = Vec::new();
    for path in &cli.source_files {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Failed to access {}: {}", path, e);
                std::process::exit(1);
            }
        };
        if meta.is_dir() {
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

        let source = match std::fs::read(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to read {}: {}", file, e);
                continue;
            }
        };

        match runner::run(compiled, lang_box.as_ref(), &source, cli.body) {
            Ok(matches) if matches.is_empty() => {
                if cli.show_query && !multiple {
                    println!("(no matches)");
                }
            }
            Ok(matches) => {
                for m in &matches {
                    if multiple {
                        println!("{}:{}", file, m);
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

fn require_lang(name: &str) -> Box<dyn lang::Lang> {
    match lang::for_name(name) {
        Some(l) => l,
        None => {
            eprintln!("Unknown language: {}. Supported: c, cpp, javascript, python, rust, go", name);
            std::process::exit(1);
        }
    }
}
