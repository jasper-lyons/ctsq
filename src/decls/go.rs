//! Go declarations from outside the repo: the standard library under GOROOT,
//! modules in the module cache, and a project's `vendor/` directory.
//!
//! `GoRoots::detect` asks the `go` on PATH where those live (once per run, with
//! `GOTOOLCHAIN=local` so nothing is downloaded). `package_dir` maps an import
//! path to a directory, `read_package` reads that directory's declarations
//! (function bodies are never walked), and `GoPackages` holds each package read
//! for the rest of the run. Nothing is written to disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use tree_sitter::{Node, Parser};

use super::TypeRef;

/// Where Go code outside the repo lives, and the host the build constraints follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoRoots {
    pub goroot: PathBuf,
    pub modcache: PathBuf,
    /// The project's `go.mod`. Its `vendor/` directory is the one beside it.
    pub go_mod: Option<PathBuf>,
    pub goos: String,
    pub goarch: String,
    pub cgo: bool,
    /// `go env GOVERSION` without the leading `go`, such as `1.25.5`.
    pub go_version: String,
}

impl GoRoots {
    /// Runs `go env` once, with `GOTOOLCHAIN=local`. None when there is no `go`.
    pub fn detect(project_dir: &Path) -> Option<GoRoots> {
        let mut cmd = Command::new("go");
        cmd.args(["env", "GOROOT", "GOMODCACHE", "GOOS", "GOARCH", "CGO_ENABLED", "GOVERSION"])
            .env("GOTOOLCHAIN", "local");
        if project_dir.is_dir() {
            cmd.current_dir(project_dir);
        }
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        let mut lines = text.lines().map(str::trim);
        let goroot = lines.next().filter(|s| !s.is_empty())?;
        let modcache = lines.next().unwrap_or("");
        let goos = lines.next().unwrap_or("");
        let goarch = lines.next().unwrap_or("");
        let cgo = lines.next() == Some("1");
        let version = lines.next().unwrap_or("");
        Some(GoRoots {
            goroot: PathBuf::from(goroot),
            modcache: PathBuf::from(modcache),
            go_mod: crate::config::gomod::find_go_mod(project_dir),
            goos: goos.to_string(),
            goarch: goarch.to_string(),
            cgo,
            go_version: version.trim_start_matches("go").to_string(),
        })
    }

    /// The `vendor/` directory beside the project's `go.mod`, when it exists.
    pub fn vendor_dir(&self) -> Option<PathBuf> {
        let dir = self.go_mod.as_ref()?.parent()?.join("vendor");
        dir.is_dir().then_some(dir)
    }

    /// The minor version: 25 for `1.25.5`.
    fn go_minor(&self) -> u32 {
        let v = self.go_version.trim_start_matches("devel ").trim_start_matches("go");
        v.split('.').nth(1).and_then(|m| {
            let digits: String = m.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        }).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// go.mod

/// The parts of a `go.mod` that locate modules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoMod {
    pub module: Option<String>,
    pub requires: Vec<Require>,
    pub replaces: Vec<Replace>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Require {
    pub path: String,
    pub version: String,
    pub indirect: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replace {
    pub old: String,
    /// Only this version of `old` is replaced. None replaces every version.
    pub old_version: Option<String>,
    pub new: ReplaceTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceTarget {
    /// A local path, already joined onto go.mod's directory.
    Dir(PathBuf),
    Module { path: String, version: String },
}

/// Reads and parses a `go.mod`.
pub fn read_go_mod(path: &Path) -> Option<GoMod> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(parse_go_mod(&text, path.parent().unwrap_or(Path::new("."))))
}

/// Parses `module`, `require` and `replace`, in block and single-line forms.
/// A local `replace` target is resolved against `dir`, go.mod's directory.
pub fn parse_go_mod(text: &str, dir: &Path) -> GoMod {
    let mut out = GoMod::default();
    let mut block: Option<String> = None;
    for raw in text.lines() {
        let (line, comment) = match raw.find("//") {
            Some(i) => (&raw[..i], raw[i + 2..].trim()),
            None => (raw, ""),
        };
        let line = line.trim();
        let indirect = comment == "indirect" || comment.starts_with("indirect;");
        if line.is_empty() {
            continue;
        }
        if let Some(verb) = block.clone() {
            if line == ")" {
                block = None;
            } else {
                go_mod_directive(&mut out, &verb, line, indirect, dir);
            }
            continue;
        }
        let (verb, rest) = match line.split_once(char::is_whitespace) {
            Some((v, r)) => (v, r.trim()),
            None => (line, ""),
        };
        if rest == "(" {
            block = Some(verb.to_string());
        } else {
            go_mod_directive(&mut out, verb, rest, indirect, dir);
        }
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').trim_matches('`').to_string()
}

fn go_mod_directive(out: &mut GoMod, verb: &str, rest: &str, indirect: bool, dir: &Path) {
    match verb {
        "module" => out.module = Some(unquote(rest)),
        "require" => {
            let f: Vec<&str> = rest.split_whitespace().collect();
            if f.len() >= 2 {
                out.requires.push(Require { path: unquote(f[0]), version: unquote(f[1]), indirect });
            }
        }
        "replace" => {
            let Some((left, right)) = rest.split_once("=>") else { return };
            let l: Vec<&str> = left.split_whitespace().collect();
            let r: Vec<&str> = right.split_whitespace().collect();
            if l.is_empty() || r.is_empty() {
                return;
            }
            let target = unquote(r[0]);
            let local = r.len() == 1
                || target.starts_with("./")
                || target.starts_with("../")
                || target.starts_with('/');
            let new = if local {
                ReplaceTarget::Dir(dir.join(&target))
            } else {
                ReplaceTarget::Module { path: target, version: unquote(r[1]) }
            };
            out.replaces.push(Replace { old: unquote(l[0]), old_version: l.get(1).map(|v| unquote(v)), new });
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Locating packages

/// The standard library: an import path whose first segment has no dot.
pub fn is_std(import_path: &str) -> bool {
    !import_path.split('/').next().unwrap_or("").contains('.')
}

/// The module cache's case escaping: `BigCorp` is `!big!corp`.
pub fn escape_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn unescape_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bang = false;
    for c in s.chars() {
        if c == '!' {
            bang = true;
        } else if bang {
            out.push(c.to_ascii_uppercase());
            bang = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn under(import_path: &str, module: &str) -> Option<String> {
    if import_path == module {
        Some(String::new())
    } else {
        import_path.strip_prefix(module)?.strip_prefix('/').map(str::to_string)
    }
}

fn join_rest(base: PathBuf, rest: &str) -> PathBuf {
    if rest.is_empty() { base } else { base.join(rest) }
}

fn cache_dir(roots: &GoRoots, module: &str, version: &str, rest: &str) -> PathBuf {
    let base = roots.modcache.join(format!("{}@{}", escape_path(module), escape_path(version)));
    join_rest(base, rest)
}

/// The directory holding the package at `import_path`, when it exists on disk.
///
/// The standard library is `GOROOT/src/<path>`. Anything else is looked for in
/// the project's `vendor/` first, then by the longest `require` in go.mod that
/// prefixes it (after `replace`), in the module cache. The project's own module
/// maps to go.mod's directory. Last, `GOROOT/src/vendor/<path>` covers the
/// golang.org/x packages the standard library vendors. None means not found,
/// which for a required module means not downloaded.
pub fn package_dir(roots: &GoRoots, import_path: &str) -> Option<PathBuf> {
    let existing = |d: PathBuf| d.is_dir().then_some(d);
    if is_std(import_path) {
        return existing(roots.goroot.join("src").join(import_path));
    }
    if let Some(vendor) = roots.vendor_dir() {
        if let Some(d) = existing(vendor.join(import_path)) {
            return Some(d);
        }
    }
    if let Some(go_mod) = roots.go_mod.as_ref() {
        if let Some(gm) = read_go_mod(go_mod) {
            if let Some(found) = module_package_dir(roots, go_mod, &gm, import_path) {
                return existing(found);
            }
        }
    }
    existing(roots.goroot.join("src/vendor").join(import_path))
}

/// The directory the go.mod says `import_path` is in, existing or not. None
/// when no module in the go.mod prefixes it.
fn module_package_dir(roots: &GoRoots, go_mod: &Path, gm: &GoMod, import_path: &str) -> Option<PathBuf> {
    let best = gm
        .requires
        .iter()
        .filter(|r| under(import_path, &r.path).is_some())
        .max_by_key(|r| r.path.len());
    let own = gm.module.as_deref().filter(|m| under(import_path, m).is_some());
    if let Some(m) = own {
        if best.is_none_or(|r| r.path.len() <= m.len()) {
            let rest = under(import_path, m)?;
            return Some(join_rest(go_mod.parent()?.to_path_buf(), &rest));
        }
    }
    let req = best?;
    let rest = under(import_path, &req.path)?;
    let replace = gm.replaces.iter().find(|r| {
        r.old == req.path && r.old_version.as_ref().is_none_or(|v| *v == req.version)
    });
    Some(match replace.map(|r| &r.new) {
        Some(ReplaceTarget::Dir(d)) => join_rest(d.clone(), &rest),
        Some(ReplaceTarget::Module { path, version }) => cache_dir(roots, path, version, &rest),
        None => cache_dir(roots, &req.path, &req.version, &rest),
    })
}

/// The import path of a package directory, the reverse of `package_dir`.
pub fn import_path_for_dir(roots: &GoRoots, dir: &Path) -> String {
    let slash = |p: &Path| {
        p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
    };
    let src = roots.goroot.join("src");
    if let Ok(rel) = dir.strip_prefix(src.join("vendor")) {
        return slash(rel);
    }
    if let Ok(rel) = dir.strip_prefix(&src) {
        return slash(rel);
    }
    if let Some(vendor) = roots.vendor_dir() {
        if let Ok(rel) = dir.strip_prefix(&vendor) {
            return slash(rel);
        }
    }
    if let Ok(rel) = dir.strip_prefix(&roots.modcache) {
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if let Some(i) = parts.iter().position(|p| p.contains('@')) {
            let mut module: Vec<String> = parts[..i].to_vec();
            module.push(parts[i].split('@').next().unwrap_or("").to_string());
            let mut path = unescape_path(&module.join("/"));
            for p in &parts[i + 1..] {
                path.push('/');
                path.push_str(p);
            }
            return path;
        }
    }
    if let Some(go_mod) = roots.go_mod.as_ref() {
        if let Some(gm) = read_go_mod(go_mod) {
            for r in &gm.replaces {
                if let ReplaceTarget::Dir(d) = &r.new {
                    if let Ok(rel) = dir.strip_prefix(d) {
                        return join_import(&r.old, &slash(rel));
                    }
                }
            }
            if let (Some(m), Some(base)) = (gm.module.as_ref(), go_mod.parent()) {
                if let Ok(rel) = dir.strip_prefix(base) {
                    return join_import(m, &slash(rel));
                }
            }
        }
    }
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn join_import(module: &str, rest: &str) -> String {
    if rest.is_empty() { module.to_string() } else { format!("{module}/{rest}") }
}

/// A declaration's location for printing: `$GOROOT/src/net/http/header.go:42`,
/// `$GOMODCACHE/...`, or the path as given.
pub fn short_location(roots: &GoRoots, file: &Path, line: usize) -> String {
    let shown = if let Ok(rel) = file.strip_prefix(&roots.goroot) {
        format!("$GOROOT/{}", rel.display())
    } else if let Ok(rel) = file.strip_prefix(&roots.modcache) {
        format!("$GOMODCACHE/{}", rel.display())
    } else {
        file.display().to_string()
    };
    format!("{shown}:{line}")
}

// ---------------------------------------------------------------------------
// Build constraints

const KNOWN_OS: &[&str] = &[
    "aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "js", "linux",
    "nacl", "netbsd", "openbsd", "plan9", "solaris", "wasip1", "windows", "zos",
];
const UNIX_OS: &[&str] = &[
    "aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "linux",
    "netbsd", "openbsd", "solaris",
];
const KNOWN_ARCH: &[&str] = &[
    "386", "amd64", "amd64p32", "arm", "armbe", "arm64", "arm64be", "loong64", "mips", "mipsle",
    "mips64", "mips64le", "mips64p32", "mips64p32le", "ppc", "ppc64", "ppc64le", "riscv",
    "riscv64", "s390", "s390x", "sparc", "sparc64", "wasm",
];

/// Whether a build tag is true on the host. GOOS also satisfies the OS it
/// extends, as in Go: android is linux, ios is darwin, illumos is solaris.
fn tag_true(roots: &GoRoots, tag: &str) -> bool {
    let os = roots.goos.as_str();
    if tag == os || tag == roots.goarch {
        return true;
    }
    match tag {
        "unix" => UNIX_OS.contains(&os),
        "gc" => true,
        "cgo" => roots.cgo,
        "linux" => os == "android",
        "darwin" => os == "ios",
        "solaris" => os == "illumos",
        _ => tag
            .strip_prefix("go1.")
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| n >= 1 && n <= roots.go_minor()),
    }
}

/// Whether a file name passes: not `_test.go`, and any `_GOOS`/`_GOARCH`
/// suffix matches the host.
fn file_name_ok(roots: &GoRoots, name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".go") else { return false };
    if stem.ends_with("_test") {
        return false;
    }
    let stem = stem.split('.').next().unwrap_or(stem);
    let Some(i) = stem.find('_') else { return true };
    let parts: Vec<&str> = stem[i..].split('_').collect();
    let n = parts.len();
    if n >= 2 && KNOWN_OS.contains(&parts[n - 2]) && KNOWN_ARCH.contains(&parts[n - 1]) {
        return tag_true(roots, parts[n - 2]) && tag_true(roots, parts[n - 1]);
    }
    let last = parts[n - 1];
    if KNOWN_OS.contains(&last) || KNOWN_ARCH.contains(&last) {
        return tag_true(roots, last);
    }
    true
}

/// The expression of a file's `//go:build` line, read from the comments before
/// the package clause.
fn build_line(src: &str) -> Option<&str> {
    let mut in_block = false;
    for line in src.lines() {
        let t = line.trim();
        if in_block {
            if t.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("//go:build") {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some(rest.trim());
            }
            continue;
        }
        if t.starts_with("//") {
            continue;
        }
        if t.starts_with("/*") {
            in_block = !t[2..].contains("*/");
            continue;
        }
        return None;
    }
    None
}

/// Evaluates a `//go:build` expression with `&&`, `||`, `!` and parentheses.
/// None when it does not parse.
pub fn eval_build(expr: &str, tag: &dyn Fn(&str) -> bool) -> Option<bool> {
    let toks = tokenize(expr)?;
    let mut p = BuildParser { toks: &toks, pos: 0, tag };
    let v = p.or()?;
    (p.pos == toks.len()).then_some(v)
}

#[derive(Debug, PartialEq)]
enum Tok {
    And,
    Or,
    Not,
    Open,
    Close,
    Tag(String),
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' => i += 1,
            b'&' if b.get(i + 1) == Some(&b'&') => { out.push(Tok::And); i += 2; }
            b'|' if b.get(i + 1) == Some(&b'|') => { out.push(Tok::Or); i += 2; }
            b'!' => { out.push(Tok::Not); i += 1; }
            b'(' => { out.push(Tok::Open); i += 1; }
            b')' => { out.push(Tok::Close); i += 1; }
            c if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                    i += 1;
                }
                out.push(Tok::Tag(s[start..i].to_string()));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct BuildParser<'a> {
    toks: &'a [Tok],
    pos: usize,
    tag: &'a dyn Fn(&str) -> bool,
}

impl BuildParser<'_> {
    fn or(&mut self) -> Option<bool> {
        let mut v = self.and()?;
        while self.toks.get(self.pos) == Some(&Tok::Or) {
            self.pos += 1;
            let r = self.and()?;
            v = v || r;
        }
        Some(v)
    }
    fn and(&mut self) -> Option<bool> {
        let mut v = self.unary()?;
        while self.toks.get(self.pos) == Some(&Tok::And) {
            self.pos += 1;
            let r = self.unary()?;
            v = v && r;
        }
        Some(v)
    }
    fn unary(&mut self) -> Option<bool> {
        match self.toks.get(self.pos)? {
            Tok::Not => {
                self.pos += 1;
                Some(!self.unary()?)
            }
            Tok::Open => {
                self.pos += 1;
                let v = self.or()?;
                if self.toks.get(self.pos) != Some(&Tok::Close) {
                    return None;
                }
                self.pos += 1;
                Some(v)
            }
            Tok::Tag(t) => {
                self.pos += 1;
                Some((self.tag)(t))
            }
            _ => None,
        }
    }
}

/// Whether the file's `//go:build` line holds on the host. A line that does
/// not parse keeps the file.
fn build_ok(roots: &GoRoots, src: &str) -> bool {
    match build_line(src) {
        None => true,
        Some(expr) => eval_build(expr, &|t| tag_true(roots, t)).unwrap_or(true),
    }
}

/// The host's source files in a package directory, sorted, with their text,
/// read one at a time as the iterator is driven.
fn package_files<'a>(roots: &'a GoRoots, dir: &Path) -> impl Iterator<Item = (PathBuf, String)> + 'a {
    let mut names: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| file_name_ok(roots, n)))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names.into_iter().filter_map(move |p| {
        let src = std::fs::read_to_string(&p).ok()?;
        build_ok(roots, &src).then_some((p, src))
    })
}

/// The `package` clause of the first host file in `dir`, read as text.
fn package_clause(roots: &GoRoots, dir: &Path) -> Option<String> {
    for (_, src) in package_files(roots, dir) {
        let mut in_block = false;
        for line in src.lines() {
            let t = line.trim();
            if in_block {
                in_block = !t.contains("*/");
                continue;
            }
            if t.starts_with("/*") {
                in_block = !t[2..].contains("*/");
                continue;
            }
            if let Some(rest) = t.strip_prefix("package ") {
                let name: String = rest.trim().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// The package name guessed from an import path: the last segment, with a
/// `/vN` or `.vN` major-version suffix stripped.
fn guess_name(import_path: &str) -> String {
    let segs: Vec<&str> = import_path.split('/').collect();
    let mut last = *segs.last().unwrap_or(&"");
    let is_major = |s: &str| s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit());
    if is_major(last) && segs.len() > 1 {
        last = segs[segs.len() - 2];
    }
    if let Some((base, v)) = last.rsplit_once('.') {
        if is_major(v) {
            last = base;
        }
    }
    last.to_string()
}

// ---------------------------------------------------------------------------
// Declarations

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoKind {
    Func,
    Method,
    Field,
    Type,
    Var,
    Const,
}

/// One declaration.
///
/// - `Method` and `Field` set `recv` to the bare type name (`Client`, not `*Client`).
///   Interface methods are `Method` with the interface as `recv`.
/// - `results` is a function's result types; for a var or const, its type;
///   for a field, its type; for a type declaration, the underlying type,
///   except struct and interface types, which leave it empty.
/// - An embedded field is a `Field` named by its type's bare name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoDecl {
    pub import_path: String,
    pub recv: Option<String>,
    pub name: String,
    pub kind: GoKind,
    pub sig: String,
    pub params: Vec<(String, TypeRef)>,
    pub results: Vec<TypeRef>,
    pub file: PathBuf,
    pub line: usize,
}

/// One package's declarations, from the host's files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PkgDecls {
    pub import_path: String,
    /// The `package` clause.
    pub name: String,
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub decls: Vec<GoDecl>,
    /// Embedded field and interface types, by the type that embeds them.
    pub embedded: HashMap<String, Vec<TypeRef>>,
    index: HashMap<String, Vec<usize>>,
}

fn key(recv: Option<&str>, name: &str) -> String {
    match recv {
        Some(r) => format!("{r}.{name}"),
        None => name.to_string(),
    }
}

impl PkgDecls {
    fn find(&self, recv: Option<&str>, name: &str, kinds: &[GoKind]) -> Option<&GoDecl> {
        self.index
            .get(&key(recv, name))?
            .iter()
            .map(|&i| &self.decls[i])
            .find(|d| kinds.contains(&d.kind))
    }

    pub fn func(&self, name: &str) -> Option<&GoDecl> {
        self.find(None, name, &[GoKind::Func])
    }

    /// A method on `ty`, or one promoted one level through a type `ty` embeds
    /// from this package. Types embedded from other packages are not followed.
    pub fn method(&self, ty: &str, name: &str) -> Option<&GoDecl> {
        self.member(ty, name, GoKind::Method)
    }

    /// A field of `ty`, or one promoted one level as `method` does. An embedded
    /// field is found by its type's bare name.
    pub fn field(&self, ty: &str, name: &str) -> Option<&GoDecl> {
        self.member(ty, name, GoKind::Field)
    }

    fn member(&self, ty: &str, name: &str, kind: GoKind) -> Option<&GoDecl> {
        if let Some(d) = self.find(Some(ty), name, &[kind]) {
            return Some(d);
        }
        self.embedded
            .get(ty)?
            .iter()
            .filter(|e| e.pkg.as_deref() == Some(self.import_path.as_str()))
            .find_map(|e| self.find(Some(&e.name), name, &[kind]))
    }

    /// The `type` declaration itself.
    pub fn type_decl(&self, name: &str) -> Option<&GoDecl> {
        self.find(None, name, &[GoKind::Type])
    }

    /// The type of a package-level var or const, when it is written or can be
    /// read off the value (`&T{}`, `T{}`, a call to a function of this package,
    /// a literal, `errors.New`).
    pub fn type_of(&self, name: &str) -> Option<&TypeRef> {
        self.find(None, name, &[GoKind::Var, GoKind::Const])?.results.first()
    }

    fn push(&mut self, d: GoDecl) {
        self.index.entry(key(d.recv.as_deref(), &d.name)).or_default().push(self.decls.len());
        self.decls.push(d);
    }
}

/// Reads the declarations of the package in `dir`: files that are not tests
/// and whose name suffix and `//go:build` line match the host. Bodies are skipped.
pub fn read_package(dir: &Path, roots: &GoRoots) -> PkgDecls {
    let mut pkg = PkgDecls {
        import_path: import_path_for_dir(roots, dir),
        dir: dir.to_path_buf(),
        ..Default::default()
    };
    let names = Names { roots, cache: RefCell::new(HashMap::new()) };
    let mut parser = Parser::new();
    if parser.set_language(&tree_sitter_go::LANGUAGE.into()).is_err() {
        return pkg;
    }
    let mut pending: Vec<(usize, Pending)> = Vec::new();
    for (path, src) in package_files(roots, dir) {
        let Some(tree) = parser.parse(&src, None) else { continue };
        let mut file = FileReader {
            src: src.as_bytes(),
            path: &path,
            import_path: pkg.import_path.clone(),
            imports: Vec::new(),
            names: &names,
        };
        let root = tree.root_node();
        file.collect_imports(root);
        if pkg.name.is_empty() {
            if let Some(n) = file.package_name(root) {
                pkg.name = n;
            }
        }
        file.walk(root, &mut pkg, &mut pending);
        pkg.files.push(path);
    }
    // Vars whose type comes from another declaration of this package.
    for (i, p) in pending {
        let t = match &p {
            Pending::Call(f) => pkg
                .func(f)
                .and_then(|d| d.results.first().cloned())
                .or_else(|| pkg.type_decl(f).map(|_| named(Some(pkg.import_path.clone()), f))),
            Pending::Ident(n) => pkg.type_of(n).cloned(),
        };
        if let Some(t) = t {
            pkg.decls[i].results = vec![t];
        }
    }
    pkg
}

enum Pending {
    Call(String),
    Ident(String),
}

/// Package names of imported paths, read lazily from the target's `package`
/// clause, cached for one `read_package`.
struct Names<'r> {
    roots: &'r GoRoots,
    cache: RefCell<HashMap<String, String>>,
}

impl Names<'_> {
    fn of(&self, import_path: &str) -> String {
        if let Some(n) = self.cache.borrow().get(import_path) {
            return n.clone();
        }
        let n = package_dir(self.roots, import_path)
            .and_then(|d| package_clause(self.roots, &d))
            .unwrap_or_else(|| guess_name(import_path));
        self.cache.borrow_mut().insert(import_path.to_string(), n.clone());
        n
    }
}

struct Import {
    alias: Option<String>,
    path: String,
}

struct FileReader<'a, 'r> {
    src: &'a [u8],
    path: &'a Path,
    import_path: String,
    imports: Vec<Import>,
    names: &'a Names<'r>,
}

const BUILTIN_TYPES: &[&str] = &[
    "any", "bool", "byte", "comparable", "complex64", "complex128", "error", "float32", "float64",
    "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8", "uint16",
    "uint32", "uint64", "uintptr",
];

fn named(pkg: Option<String>, name: &str) -> TypeRef {
    TypeRef { pkg, name: name.to_string(), pointer: false, elems: Vec::new() }
}

fn shape(name: &str, elems: Vec<TypeRef>) -> TypeRef {
    TypeRef { pkg: None, name: name.to_string(), pointer: false, elems }
}

/// Collapses whitespace and the spaces a multi-line list leaves inside brackets.
fn collapse(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(", )", ")")
        .replace("( ", "(")
        .replace(" )", ")")
}

fn truncate(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

impl FileReader<'_, '_> {
    fn text(&self, n: Node) -> &str {
        std::str::from_utf8(&self.src[n.byte_range()]).unwrap_or("")
    }

    fn package_name(&self, root: Node) -> Option<String> {
        let mut c = root.walk();
        let clause = root.named_children(&mut c).find(|n| n.kind() == "package_clause")?;
        let mut c2 = clause.walk();
        let id = clause.named_children(&mut c2).find(|n| n.kind() == "package_identifier")?;
        Some(self.text(id).to_string())
    }

    fn collect_imports(&mut self, root: Node) {
        let mut specs = Vec::new();
        let mut c = root.walk();
        for decl in root.named_children(&mut c).filter(|n| n.kind() == "import_declaration") {
            let mut c2 = decl.walk();
            for ch in decl.named_children(&mut c2) {
                if ch.kind() == "import_spec" {
                    specs.push(ch);
                } else if ch.kind() == "import_spec_list" {
                    let mut c3 = ch.walk();
                    specs.extend(ch.named_children(&mut c3).filter(|n| n.kind() == "import_spec"));
                }
            }
        }
        for s in specs {
            let Some(path) = s.child_by_field_name("path") else { continue };
            let path = unquote(self.text(path));
            let alias = s.child_by_field_name("name").map(|n| self.text(n).to_string());
            if matches!(alias.as_deref(), Some("_") | Some(".")) {
                continue;
            }
            self.imports.push(Import { alias, path });
        }
    }

    /// The import path a qualifier names: an alias first, then an import whose
    /// package clause is the qualifier, trying the likely one before the rest.
    fn qualify(&self, q: &str) -> String {
        if let Some(i) = self.imports.iter().find(|i| i.alias.as_deref() == Some(q)) {
            return i.path.clone();
        }
        let plain = || self.imports.iter().filter(|i| i.alias.is_none());
        plain()
            .filter(|i| guess_name(&i.path) == q)
            .chain(plain().filter(|i| guess_name(&i.path) != q))
            .find(|i| self.names.of(&i.path) == q)
            .map(|i| i.path.clone())
            .unwrap_or_else(|| q.to_string())
    }

    fn type_ref(&self, n: Node, tparams: &[String]) -> TypeRef {
        let field = |f: &str| n.child_by_field_name(f);
        let first_named = || {
            let mut c = n.walk();
            let x = n.named_children(&mut c).find(|ch| ch.kind() != "comment");
            x
        };
        match n.kind() {
            "type_identifier" => {
                let t = self.text(n);
                if BUILTIN_TYPES.contains(&t) || tparams.iter().any(|p| p == t) {
                    named(None, t)
                } else {
                    named(Some(self.import_path.clone()), t)
                }
            }
            "qualified_type" => {
                let (Some(p), Some(name)) = (field("package"), field("name")) else {
                    return named(None, &collapse(self.text(n)));
                };
                named(Some(self.qualify(self.text(p))), self.text(name))
            }
            "pointer_type" => match first_named() {
                Some(inner) => {
                    let mut t = self.type_ref(inner, tparams);
                    if t.pointer || (t.pkg.is_none() && t.elems.is_empty() && !is_simple_name(&t.name)) {
                        return named(None, &collapse(self.text(n)));
                    }
                    t.pointer = true;
                    t
                }
                None => named(None, &collapse(self.text(n))),
            },
            "slice_type" | "array_type" | "implicit_length_array_type" => match field("element") {
                Some(e) => shape("[]", vec![self.type_ref(e, tparams)]),
                None => named(None, &collapse(self.text(n))),
            },
            "map_type" => match (field("key"), field("value")) {
                (Some(k), Some(v)) => shape("map", vec![self.type_ref(k, tparams), self.type_ref(v, tparams)]),
                _ => named(None, &collapse(self.text(n))),
            },
            "channel_type" => match field("value") {
                Some(v) => shape("chan", vec![self.type_ref(v, tparams)]),
                None => named(None, &collapse(self.text(n))),
            },
            "generic_type" => {
                let Some(base) = field("type") else { return named(None, &collapse(self.text(n))) };
                let mut t = self.type_ref(base, tparams);
                if let Some(args) = field("type_arguments") {
                    let mut c = args.walk();
                    t.elems = args.named_children(&mut c).map(|a| self.type_ref(a, tparams)).collect();
                }
                t
            }
            "type_elem" | "parenthesized_type" | "type_constraint" => {
                let mut c = n.walk();
                let kids: Vec<Node> = n.named_children(&mut c).filter(|k| k.kind() != "comment").collect();
                if kids.len() == 1 {
                    self.type_ref(kids[0], tparams)
                } else {
                    named(None, &collapse(self.text(n)))
                }
            }
            _ => named(None, &collapse(self.text(n))),
        }
    }

    fn type_params(&self, list: Option<Node>) -> Vec<String> {
        let Some(list) = list else { return Vec::new() };
        let mut out = Vec::new();
        let mut c = list.walk();
        for decl in list.named_children(&mut c) {
            let mut c2 = decl.walk();
            for name in decl.children_by_field_name("name", &mut c2) {
                out.push(self.text(name).to_string());
            }
        }
        out
    }

    fn params(&self, list: Option<Node>, tparams: &[String]) -> Vec<(String, TypeRef)> {
        let Some(list) = list else { return Vec::new() };
        let mut out = Vec::new();
        let mut c = list.walk();
        for p in list.named_children(&mut c) {
            let variadic = p.kind() == "variadic_parameter_declaration";
            if !variadic && p.kind() != "parameter_declaration" {
                continue;
            }
            let Some(ty) = p.child_by_field_name("type") else { continue };
            let mut t = self.type_ref(ty, tparams);
            if variadic {
                t = shape("...", vec![t]);
            }
            let mut c2 = p.walk();
            let names: Vec<String> =
                p.children_by_field_name("name", &mut c2).map(|n| self.text(n).to_string()).collect();
            if names.is_empty() {
                out.push((String::new(), t));
            } else {
                for n in names {
                    out.push((n, t.clone()));
                }
            }
        }
        out
    }

    fn results(&self, result: Option<Node>, tparams: &[String]) -> Vec<TypeRef> {
        match result {
            None => Vec::new(),
            Some(r) if r.kind() == "parameter_list" => {
                self.params(Some(r), tparams).into_iter().map(|(_, t)| t).collect()
            }
            Some(r) => vec![self.type_ref(r, tparams)],
        }
    }

    fn decl(&self, recv: Option<String>, name: &str, kind: GoKind, sig: String, at: Node) -> GoDecl {
        GoDecl {
            import_path: self.import_path.clone(),
            recv,
            name: name.to_string(),
            kind,
            sig,
            params: Vec::new(),
            results: Vec::new(),
            file: self.path.to_path_buf(),
            line: at.start_position().row + 1,
        }
    }

    /// The signature: the declaration's text up to its body.
    fn head(&self, n: Node) -> String {
        let end = n.child_by_field_name("body").map_or(n.end_byte(), |b| b.start_byte());
        collapse(std::str::from_utf8(&self.src[n.start_byte()..end]).unwrap_or(""))
    }

    fn walk(&self, root: Node, pkg: &mut PkgDecls, pending: &mut Vec<(usize, Pending)>) {
        let mut c = root.walk();
        for n in root.named_children(&mut c) {
            match n.kind() {
                "function_declaration" => self.function(n, pkg),
                "method_declaration" => self.method(n, pkg),
                "type_declaration" => {
                    let mut c2 = n.walk();
                    for spec in n.named_children(&mut c2) {
                        if matches!(spec.kind(), "type_spec" | "type_alias") {
                            self.type_spec(spec, pkg);
                        }
                    }
                }
                "var_declaration" | "const_declaration" => self.values(n, pkg, pending),
                "ERROR" => self.walk(n, pkg, pending),
                _ => {}
            }
        }
    }

    fn function(&self, n: Node, pkg: &mut PkgDecls) {
        let Some(name) = n.child_by_field_name("name") else { return };
        let tparams = self.type_params(n.child_by_field_name("type_parameters"));
        let mut d = self.decl(None, self.text(name), GoKind::Func, self.head(n), n);
        d.params = self.params(n.child_by_field_name("parameters"), &tparams);
        d.results = self.results(n.child_by_field_name("result"), &tparams);
        pkg.push(d);
    }

    fn method(&self, n: Node, pkg: &mut PkgDecls) {
        let Some(name) = n.child_by_field_name("name") else { return };
        let Some((recv, tparams)) = n.child_by_field_name("receiver").and_then(|r| self.receiver(r)) else {
            return;
        };
        let mut d = self.decl(Some(recv), self.text(name), GoKind::Method, self.head(n), n);
        d.params = self.params(n.child_by_field_name("parameters"), &tparams);
        d.results = self.results(n.child_by_field_name("result"), &tparams);
        pkg.push(d);
    }

    /// The receiver's bare type name and the type parameters it names (`Set[T]`).
    fn receiver(&self, list: Node) -> Option<(String, Vec<String>)> {
        let mut c = list.walk();
        let p = list.named_children(&mut c).find(|p| p.kind() == "parameter_declaration")?;
        let mut ty = p.child_by_field_name("type")?;
        while matches!(ty.kind(), "pointer_type" | "parenthesized_type") {
            let mut c2 = ty.walk();
            ty = ty.named_children(&mut c2).next()?;
        }
        match ty.kind() {
            "type_identifier" => Some((self.text(ty).to_string(), Vec::new())),
            "generic_type" => {
                let base = ty.child_by_field_name("type")?;
                let mut tps = Vec::new();
                if let Some(args) = ty.child_by_field_name("type_arguments") {
                    let mut c2 = args.walk();
                    for a in args.named_children(&mut c2) {
                        tps.push(self.text(a).trim().to_string());
                    }
                }
                Some((self.text(base).to_string(), tps))
            }
            _ => None,
        }
    }

    fn type_spec(&self, spec: Node, pkg: &mut PkgDecls) {
        let (Some(name), Some(ty)) = (spec.child_by_field_name("name"), spec.child_by_field_name("type")) else {
            return;
        };
        let name = self.text(name).to_string();
        let tparams = self.type_params(spec.child_by_field_name("type_parameters"));
        let lead = collapse(std::str::from_utf8(&self.src[spec.start_byte()..ty.start_byte()]).unwrap_or(""));
        let body = match ty.kind() {
            "struct_type" => "struct{...}".to_string(),
            "interface_type" => "interface{...}".to_string(),
            _ => collapse(self.text(ty)),
        };
        let mut d = self.decl(None, &name, GoKind::Type, format!("type {lead} {body}"), spec);
        if !matches!(ty.kind(), "struct_type" | "interface_type") {
            d.results = vec![self.type_ref(ty, &tparams)];
        }
        pkg.push(d);
        match ty.kind() {
            "struct_type" => self.struct_fields(&name, ty, &tparams, pkg),
            "interface_type" => self.interface_elems(&name, ty, &tparams, pkg),
            _ => {}
        }
    }

    fn struct_fields(&self, owner: &str, st: Node, tparams: &[String], pkg: &mut PkgDecls) {
        let mut c = st.walk();
        let Some(list) = st.named_children(&mut c).find(|n| n.kind() == "field_declaration_list") else { return };
        let mut c2 = list.walk();
        for f in list.named_children(&mut c2).filter(|n| n.kind() == "field_declaration") {
            let Some(ty) = f.child_by_field_name("type") else { continue };
            let mut t = self.type_ref(ty, tparams);
            let sig = collapse(std::str::from_utf8(&self.src[f.start_byte()..ty.end_byte()]).unwrap_or(""));
            let mut c3 = f.walk();
            let names: Vec<String> =
                f.children_by_field_name("name", &mut c3).map(|n| self.text(n).to_string()).collect();
            if names.is_empty() {
                let mut c4 = f.walk();
                if f.children(&mut c4).any(|k| k.kind() == "*") {
                    t.pointer = true;
                }
                let mut d = self.decl(Some(owner.to_string()), &t.name, GoKind::Field, sig, f);
                d.results = vec![t.clone()];
                pkg.push(d);
                pkg.embedded.entry(owner.to_string()).or_default().push(t);
                continue;
            }
            for n in names {
                let mut d = self.decl(Some(owner.to_string()), &n, GoKind::Field, sig.clone(), f);
                d.results = vec![t.clone()];
                pkg.push(d);
            }
        }
    }

    fn interface_elems(&self, owner: &str, it: Node, tparams: &[String], pkg: &mut PkgDecls) {
        let mut c = it.walk();
        for e in it.named_children(&mut c) {
            match e.kind() {
                "method_elem" => {
                    let Some(name) = e.child_by_field_name("name") else { continue };
                    let mut d = self.decl(Some(owner.to_string()), self.text(name), GoKind::Method, collapse(self.text(e)), e);
                    d.params = self.params(e.child_by_field_name("parameters"), tparams);
                    d.results = self.results(e.child_by_field_name("result"), tparams);
                    pkg.push(d);
                }
                "type_elem" => {
                    let t = self.type_ref(e, tparams);
                    // A union or `~T` is a constraint, not an embedded interface.
                    if is_simple_name(&t.name) && (t.pkg.is_some() || !BUILTIN_TYPES.contains(&t.name.as_str())) {
                        pkg.embedded.entry(owner.to_string()).or_default().push(t);
                    }
                }
                _ => {}
            }
        }
    }

    fn values(&self, n: Node, pkg: &mut PkgDecls, pending: &mut Vec<(usize, Pending)>) {
        let is_const = n.kind() == "const_declaration";
        let (kind, word) = if is_const { (GoKind::Const, "const") } else { (GoKind::Var, "var") };
        let mut specs = Vec::new();
        let mut c = n.walk();
        for ch in n.named_children(&mut c) {
            match ch.kind() {
                "var_spec" | "const_spec" => specs.push(ch),
                "var_spec_list" => {
                    let mut c2 = ch.walk();
                    specs.extend(ch.named_children(&mut c2).filter(|s| s.kind() == "var_spec"));
                }
                _ => {}
            }
        }
        // A const spec with no type and no value repeats the previous one's.
        let mut prev_type: Option<Node> = None;
        let mut prev_values: Option<Node> = None;
        for spec in specs {
            let mut ty = spec.child_by_field_name("type");
            let mut values = spec.child_by_field_name("value");
            if is_const && ty.is_none() && values.is_none() {
                ty = prev_type;
                values = prev_values;
            } else {
                prev_type = ty;
                prev_values = values;
            }
            let value_list: Vec<Node> = values
                .map(|v| {
                    let mut c2 = v.walk();
                    v.named_children(&mut c2).filter(|x| x.kind() != "comment").collect()
                })
                .unwrap_or_default();
            let sig = truncate(format!("{word} {}", collapse(self.text(spec))), 120);
            let mut c3 = spec.walk();
            let names: Vec<Node> = spec.children_by_field_name("name", &mut c3).filter(|x| x.kind() == "identifier").collect();
            for (i, name) in names.into_iter().enumerate() {
                let name = self.text(name);
                if name == "_" {
                    continue;
                }
                let mut d = self.decl(None, name, kind, sig.clone(), spec);
                let mut later = None;
                if let Some(t) = ty {
                    d.results = vec![self.type_ref(t, &[])];
                } else if let Some(v) = value_list.get(i) {
                    match self.infer(*v) {
                        Inferred::Known(t) => d.results = vec![t],
                        Inferred::Later(p) => later = Some(p),
                        Inferred::Unknown => {}
                    }
                }
                if let Some(p) = later {
                    pending.push((pkg.decls.len(), p));
                }
                pkg.push(d);
            }
        }
    }

    /// The type of a var or const read off its value.
    fn infer(&self, v: Node) -> Inferred {
        let field = |f: &str| v.child_by_field_name(f);
        match v.kind() {
            "composite_literal" => match field("type") {
                Some(t) => Inferred::Known(self.type_ref(t, &[])),
                None => Inferred::Unknown,
            },
            "unary_expression" => {
                let op = field("operator").map(|o| self.text(o));
                match (op, field("operand")) {
                    (Some("&"), Some(o)) if o.kind() == "composite_literal" => match self.infer(o) {
                        Inferred::Known(mut t) if !t.pointer => {
                            t.pointer = true;
                            Inferred::Known(t)
                        }
                        _ => Inferred::Unknown,
                    },
                    (Some("-" | "+" | "^"), Some(o)) => self.infer(o),
                    _ => Inferred::Unknown,
                }
            }
            "int_literal" | "iota" => Inferred::Known(named(None, "int")),
            "float_literal" => Inferred::Known(named(None, "float64")),
            "imaginary_literal" => Inferred::Known(named(None, "complex128")),
            "rune_literal" => Inferred::Known(named(None, "rune")),
            "interpreted_string_literal" | "raw_string_literal" => Inferred::Known(named(None, "string")),
            "true" | "false" => Inferred::Known(named(None, "bool")),
            "binary_expression" => match field("left") {
                Some(l) => self.infer(l),
                None => Inferred::Unknown,
            },
            "parenthesized_expression" => {
                let mut c = v.walk();
                let inner = v.named_children(&mut c).next();
                inner.map_or(Inferred::Unknown, |i| self.infer(i))
            }
            "identifier" => Inferred::Later(Pending::Ident(self.text(v).to_string())),
            "call_expression" => match field("function") {
                Some(f) if f.kind() == "identifier" => Inferred::Later(Pending::Call(self.text(f).to_string())),
                Some(f) if f.kind() == "selector_expression" => {
                    let text = self.text(f);
                    if text == "errors.New" || text == "fmt.Errorf" {
                        Inferred::Known(named(None, "error"))
                    } else {
                        Inferred::Unknown
                    }
                }
                _ => Inferred::Unknown,
            },
            _ => Inferred::Unknown,
        }
    }
}

enum Inferred {
    Known(TypeRef),
    Later(Pending),
    Unknown,
}

fn is_simple_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Per-run store

/// Packages read so far in this run, by import path. Each is read at most once;
/// a package that cannot be found is remembered as None.
#[derive(Clone, Debug)]
pub struct GoPackages {
    pub roots: GoRoots,
    pkgs: HashMap<String, Option<PkgDecls>>,
}

impl GoPackages {
    pub fn new(roots: GoRoots) -> Self {
        GoPackages { roots, pkgs: HashMap::new() }
    }

    pub fn get(&mut self, import_path: &str) -> Option<&PkgDecls> {
        if !self.pkgs.contains_key(import_path) {
            let pkg = package_dir(&self.roots, import_path).map(|d| read_package(&d, &self.roots));
            self.pkgs.insert(import_path.to_string(), pkg);
        }
        self.pkgs.get(import_path)?.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/decls/go")
    }

    fn roots(project: Option<&str>) -> GoRoots {
        let f = fixtures();
        GoRoots {
            goroot: f.join("goroot"),
            modcache: f.join("modcache"),
            go_mod: project.map(|p| f.join(p).join("go.mod")),
            goos: "darwin".into(),
            goarch: "arm64".into(),
            cgo: true,
            go_version: "1.25.5".into(),
        }
    }

    fn http() -> PkgDecls {
        let r = roots(Some("project"));
        read_package(&package_dir(&r, "net/http").unwrap(), &r)
    }

    fn t(pkg: Option<&str>, name: &str) -> TypeRef {
        named(pkg.map(str::to_string), name)
    }

    fn ptr(mut x: TypeRef) -> TypeRef {
        x.pointer = true;
        x
    }

    #[test]
    fn header_set_signature() {
        let p = http();
        assert_eq!(p.import_path, "net/http");
        assert_eq!(p.name, "http");
        let d = p.method("Header", "Set").unwrap();
        assert_eq!(d.sig, "func (h Header) Set(key, value string)");
        assert_eq!(d.kind, GoKind::Method);
        assert_eq!(d.recv.as_deref(), Some("Header"));
        assert_eq!(d.line, 19);
        assert!(d.file.ends_with("net/http/header.go"));
        assert_eq!(
            d.params,
            vec![("key".to_string(), t(None, "string")), ("value".to_string(), t(None, "string"))]
        );
        assert!(d.results.is_empty());
    }

    #[test]
    fn new_request_results_carry_their_package() {
        let p = http();
        let d = p.func("NewRequest").unwrap();
        assert_eq!(d.results, vec![ptr(t(Some("net/http"), "Request")), t(None, "error")]);
        assert_eq!(d.params[2], ("body".to_string(), t(Some("io"), "Reader")));
    }

    #[test]
    fn composite_and_aliased_types() {
        let p = http();
        let header = p.type_decl("Header").unwrap();
        let strings = shape("[]", vec![t(None, "string")]);
        assert_eq!(header.results, vec![shape("map", vec![t(None, "string"), strings.clone()])]);
        assert_eq!(header.sig, "type Header map[string][]string");
        // `urlpkg "net/url"`: the alias qualifies to the path.
        let url = p.field("Request", "URL").unwrap();
        assert_eq!(url.results, vec![ptr(t(Some("net/url"), "URL"))]);
        assert_eq!(url.sig, "URL *urlpkg.URL");
        let form = p.field("Request", "PostForm").unwrap();
        assert_eq!(form.results, vec![t(Some("net/url"), "Values")]);
        let trailer = p.field("Request", "Trailer").unwrap();
        assert_eq!(trailer.sig, "Trailer map[string][]string");
        let cookies = p.method("Request", "Cookies").unwrap();
        assert_eq!(cookies.params, vec![("names".to_string(), shape("...", vec![t(None, "string")]))]);
        assert_eq!(cookies.results, vec![shape("[]", vec![ptr(t(Some("net/http"), "Cookie"))])]);
        assert_eq!(p.method("Request", "Context").unwrap().results, vec![t(Some("context"), "Context")]);
    }

    #[test]
    fn type_parameters_have_no_package() {
        let p = http();
        let add = p.method("Set", "Add").unwrap();
        assert_eq!(add.params, vec![("v".to_string(), t(None, "T"))]);
        let keys = p.func("Keys").unwrap();
        assert_eq!(
            keys.params,
            vec![("m".to_string(), shape("map", vec![t(None, "K"), t(None, "V")]))]
        );
        assert_eq!(keys.results, vec![shape("[]", vec![t(None, "K")])]);
    }

    #[test]
    fn host_files_only() {
        let p = http();
        for gone in ["FromTestFile", "FromWindowsFile", "FromIgnoreFile", "FromOmitFile", "FromJsFile"] {
            assert!(p.func(gone).is_none(), "{gone} should be skipped");
        }
        assert!(p.func("FromNotJsFile").is_some(), "a !js file is kept");
        assert!(p.func("FromDarwinArm64File").is_some());
    }

    #[test]
    fn promoted_methods_and_fields() {
        let p = http();
        // *conn is embedded in Client, same package.
        let close = p.method("Client", "Close").unwrap();
        assert_eq!(close.recv.as_deref(), Some("conn"));
        assert_eq!(p.field("Client", "idle").unwrap().recv.as_deref(), Some("conn"));
        // io.Reader is from another package: not followed, but it is a field.
        assert!(p.method("Client", "Read").is_none());
        let reader = p.field("Client", "Reader").unwrap();
        assert_eq!(reader.results, vec![t(Some("io"), "Reader")]);
        assert_eq!(p.field("Client", "conn").unwrap().results, vec![ptr(t(Some("net/http"), "conn"))]);
        // An interface embedding another of the same package.
        assert_eq!(p.method("ReadCloser", "Close").unwrap().recv.as_deref(), Some("Closer"));
        assert_eq!(p.method("ReadCloser", "Read").unwrap().sig, "Read(p []byte) (n int, err error)");
    }

    #[test]
    fn declarations_after_a_broken_body_are_read() {
        let p = http();
        assert!(p.func("AfterBroken").is_some());
        assert!(p.type_decl("Response").is_some());
        assert!(p.method("Client", "Do").is_some());
    }

    #[test]
    fn var_and_const_types() {
        let p = http();
        assert_eq!(p.type_of("DefaultClient"), Some(&ptr(t(Some("net/http"), "Client"))));
        assert_eq!(p.type_of("zeroClient"), Some(&t(Some("net/http"), "Client")));
        assert_eq!(p.type_of("defaultTimeout"), Some(&t(Some("time"), "Duration")));
        assert_eq!(p.type_of("MethodGet"), Some(&t(None, "string")));
        assert_eq!(p.type_of("StateIdle"), Some(&t(Some("net/http"), "ConnState")));
        assert_eq!(p.type_of("ErrMissingFile"), Some(&t(None, "error")));
        assert_eq!(p.find(None, "DefaultClient", &[GoKind::Var]).unwrap().sig, "var DefaultClient = &Client{}");
    }

    #[test]
    fn go_mod_forms() {
        let gm = read_go_mod(&fixtures().join("project/go.mod")).unwrap();
        assert_eq!(gm.module.as_deref(), Some("example.com/project"));
        assert_eq!(gm.requires.len(), 5);
        assert!(gm.requires.iter().any(|r| r.path == "github.com/BigCorp/thing" && r.indirect));
        assert!(gm.requires.iter().any(|r| r.path == "example.com/lib" && !r.indirect));
        assert_eq!(gm.replaces.len(), 2);
        assert_eq!(gm.replaces[0].new, ReplaceTarget::Dir(fixtures().join("project/./local")));
        assert_eq!(gm.replaces[1].old_version.as_deref(), Some("v0.9.0"));
    }

    #[test]
    fn module_dirs() {
        let r = roots(Some("project"));
        let f = fixtures();
        assert_eq!(package_dir(&r, "example.com/lib"), Some(f.join("modcache/example.com/lib@v1.2.3")));
        assert_eq!(package_dir(&r, "example.com/lib/sub"), Some(f.join("modcache/example.com/lib@v1.2.3/sub")));
        assert_eq!(
            package_dir(&r, "github.com/BigCorp/thing"),
            Some(f.join("modcache/github.com/!big!corp/thing@v0.4.0"))
        );
        assert_eq!(package_dir(&r, "example.com/local"), Some(f.join("project/./local")));
        assert_eq!(package_dir(&r, "example.com/old/sub"), Some(f.join("modcache/example.com/lib@v1.2.3/sub")));
        assert_eq!(package_dir(&r, "example.com/missing"), None);
        assert_eq!(package_dir(&r, "example.com/unrequired"), None);
        let local = read_package(&package_dir(&r, "example.com/local").unwrap(), &r);
        assert_eq!(local.import_path, "example.com/local");
        assert!(local.func("FromLocal").is_some());
        let thing = read_package(&package_dir(&r, "github.com/BigCorp/thing").unwrap(), &r);
        assert_eq!(thing.import_path, "github.com/BigCorp/thing");
        assert_eq!(thing.name, "widgets");
    }

    #[test]
    fn vendor_wins_over_the_cache() {
        let r = roots(Some("vendored"));
        let dir = package_dir(&r, "example.com/lib").unwrap();
        assert_eq!(dir, fixtures().join("vendored/vendor/example.com/lib"));
        let p = read_package(&dir, &r);
        assert_eq!(p.import_path, "example.com/lib");
        assert!(p.func("FromVendor").is_some());
        assert!(p.func("FromCache").is_none());
    }

    #[test]
    fn qualifiers_follow_package_clauses() {
        let r = roots(Some("project"));
        let p = read_package(&package_dir(&r, "example.com/lib/sub").unwrap(), &r);
        assert_eq!(p.import_path, "example.com/lib/sub");
        let wrap = p.func("Wrap").unwrap();
        assert_eq!(wrap.params[0].1, t(Some("github.com/BigCorp/thing"), "Widget"));
        assert_eq!(wrap.results, vec![t(Some("example.com/lib"), "Result")]);
        // Not found anywhere: the name falls back to the path with /v2 stripped.
        assert_eq!(p.func("Plain").unwrap().results, vec![t(Some("github.com/BigCorp/thing/v2"), "Node")]);
    }

    #[test]
    fn build_expressions() {
        let r = roots(None);
        let on = |e: &str| eval_build(e, &|t| tag_true(&r, t));
        assert_eq!(on("!js"), Some(true));
        assert_eq!(on("ignore"), Some(false));
        assert_eq!(on("unix && cgo"), Some(true));
        assert_eq!(on("go1.25 && !go1.26"), Some(true));
        assert_eq!(on("(linux || windows) && gc"), Some(false));
        assert_eq!(on("linux ||"), None);
        assert!(file_name_ok(&r, "x_darwin.go"));
        assert!(!file_name_ok(&r, "x_linux_amd64.go"));
        assert!(file_name_ok(&r, "windows.go"));
        assert!(!file_name_ok(&r, "x_test.go"));
    }

    #[test]
    fn guessed_names() {
        assert_eq!(guess_name("gopkg.in/yaml.v3"), "yaml");
        assert_eq!(guess_name("github.com/a/b/v2"), "b");
        assert_eq!(guess_name("net/http"), "http");
    }

    #[test]
    fn short_locations() {
        let r = roots(None);
        let file = r.goroot.join("src/net/http/header.go");
        assert_eq!(short_location(&r, &file, 42), "$GOROOT/src/net/http/header.go:42");
        let m = r.modcache.join("example.com/lib@v1.2.3/lib.go");
        assert_eq!(short_location(&r, &m, 3), "$GOMODCACHE/example.com/lib@v1.2.3/lib.go:3");
    }

    #[test]
    fn packages_are_read_once() {
        let mut ps = GoPackages::new(roots(Some("project")));
        assert!(ps.get("net/http").is_some());
        assert!(ps.get("net/http").unwrap().func("NewRequest").is_some());
        assert!(ps.get("example.com/missing").is_none());
    }

    const GOROOT: &str = "/opt/homebrew/Cellar/go/1.25.5/libexec";

    #[test]
    #[ignore]
    fn real_goroot_client_do() {
        let r = GoRoots {
            goroot: PathBuf::from(GOROOT),
            modcache: PathBuf::from("/nonexistent"),
            go_mod: None,
            goos: "darwin".into(),
            goarch: "arm64".into(),
            cgo: true,
            go_version: "1.25.5".into(),
        };
        let dir = package_dir(&r, "net/http").unwrap();
        let start = std::time::Instant::now();
        let p = read_package(&dir, &r);
        let took = start.elapsed();
        eprintln!("read net/http in {took:?}, {} files, {} decls", p.files.len(), p.decls.len());
        let d = p.method("Client", "Do").unwrap();
        assert!(d.file.ends_with("net/http/client.go"));
        assert_eq!(d.line, 586);
        assert_eq!(d.results, vec![ptr(t(Some("net/http"), "Response")), t(None, "error")]);
        assert_eq!(p.type_of("DefaultClient"), Some(&ptr(t(Some("net/http"), "Client"))));
        assert_eq!(p.method("Header", "Set").unwrap().sig, "func (h Header) Set(key, value string)");
        assert_eq!(
            p.func("NewRequest").unwrap().results,
            vec![ptr(t(Some("net/http"), "Request")), t(None, "error")]
        );
        assert_eq!(p.field("Request", "URL").unwrap().results, vec![ptr(t(Some("net/url"), "URL"))]);
        assert_eq!(short_location(&r, &d.file, d.line), "$GOROOT/src/net/http/client.go:586");
        assert!(took.as_millis() < 200, "took {took:?}");
    }

    #[test]
    #[ignore]
    fn detect_runs_go_env() {
        let r = GoRoots::detect(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(r.goroot.join("src/net/http").is_dir());
        assert!(!r.goos.is_empty() && !r.goarch.is_empty());
        assert!(r.go_minor() >= 21);
    }
}
