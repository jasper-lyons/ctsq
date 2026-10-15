//! Compiles the vendored Go template grammar (vendor/tree-sitter-go-template).
//! No crate on crates.io ships it for tree-sitter 0.26, see DESIGN.md.

fn main() {
    let dir = std::path::Path::new("vendor/tree-sitter-go-template/src");
    cc::Build::new()
        .include(dir)
        .std("c11")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-trigraphs")
        .file(dir.join("parser.c"))
        .compile("tree-sitter-gotmpl");
    println!("cargo:rerun-if-changed={}", dir.join("parser.c").display());
}
