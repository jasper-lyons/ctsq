//! go.mod reading: module path and go version, found by walking upward.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// A Go language version, major and minor only (`go 1.23.4` is 1.23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GoVersion {
    pub major: u32,
    pub minor: u32,
}

impl GoVersion {
    /// Parse `1.23`, `1.23.4` or `1.23rc1`. Returns None for anything else.
    pub fn parse(s: &str) -> Option<GoVersion> {
        let mut parts = s.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor_raw = parts.next()?;
        let digits: String = minor_raw.chars().take_while(|c| c.is_ascii_digit()).collect();
        let minor = digits.parse().ok()?;
        Some(GoVersion { major, minor })
    }
}

impl Ord for GoVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor).cmp(&(other.major, other.minor))
    }
}
impl PartialOrd for GoVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl std::fmt::Display for GoVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Nearest `go.mod` at or above `path` (a file or a directory).
pub fn find_go_mod(path: &Path) -> Option<PathBuf> {
    let mut dir: PathBuf = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        let candidate = dir.join("go.mod");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The module path from the `module` line of the nearest go.mod.
pub fn module_path(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(find_go_mod(path)?).ok()?;
    parse_module_path(&text)
}

/// The version from the `go` line of the nearest go.mod.
pub fn go_version(path: &Path) -> Option<GoVersion> {
    let text = std::fs::read_to_string(find_go_mod(path)?).ok()?;
    parse_go_version(&text)
}

pub fn parse_module_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = strip_comment(line);
        if let Some(rest) = line.strip_prefix("module") {
            if !rest.starts_with(char::is_whitespace) {
                continue;
            }
            let v = rest.trim();
            let v = v.trim_matches(|c| c == '"' || c == '`');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn parse_go_version(text: &str) -> Option<GoVersion> {
    for line in text.lines() {
        let line = strip_comment(line);
        if let Some(rest) = line.strip_prefix("go") {
            if !rest.starts_with(char::is_whitespace) {
                continue;
            }
            return GoVersion::parse(rest);
        }
    }
    None
}

fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => line[..i].trim(),
        None => line.trim(),
    }
}

/// Decision 16: an import path is outside the codebase when it is neither the
/// module path nor under it.
pub fn is_outside(module: &str, import_path: &str) -> bool {
    import_path != module && !import_path.starts_with(&format!("{}/", module))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_line_is_found() {
        let t = "// c\nmodule github.com/acme/app // trailing\n\ngo 1.23.4\n";
        assert_eq!(parse_module_path(t).as_deref(), Some("github.com/acme/app"));
        assert_eq!(parse_module_path("module \"x/y\"\n").as_deref(), Some("x/y"));
        assert_eq!(parse_module_path("go 1.21\n"), None);
    }

    #[test]
    fn go_line_parses_to_major_minor() {
        assert_eq!(parse_go_version("module m\ngo 1.23.4\n"), GoVersion::parse("1.23"));
        assert_eq!(parse_go_version("go 1.25\ntoolchain go1.25.1\n"), GoVersion::parse("1.25"));
        assert_eq!(parse_go_version("module m\n"), None);
    }

    #[test]
    fn finds_nearest_go_mod_upward() {
        let d = std::env::temp_dir().join(format!("ctsq-gomod-{}", std::process::id()));
        let sub = d.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(d.join("go.mod"), "module example.com/m\ngo 1.22\n").unwrap();
        assert_eq!(module_path(&sub).as_deref(), Some("example.com/m"));
        assert_eq!(module_path(&sub.join("x.go")).as_deref(), Some("example.com/m"));
        assert_eq!(go_version(&sub), GoVersion::parse("1.22"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn outside_means_not_under_module() {
        assert!(is_outside("example.com/m", "fmt"));
        assert!(is_outside("example.com/m", "example.com/mother"));
        assert!(!is_outside("example.com/m", "example.com/m"));
        assert!(!is_outside("example.com/m", "example.com/m/store"));
    }
}
