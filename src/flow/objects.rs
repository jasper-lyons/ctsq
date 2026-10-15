//! What the walk knows about the whole module when it follows objects
//! (decision 12): struct fields with their tags and embedded structs, and
//! package-level variables with the functions that read them.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::engine::{function_name, kids, named, text};
use crate::config::Mechanics;

/// Where a tracked value holds the data: `None` for the value itself, or the
/// field paths of an object that holds it, as `[["CustomerPhone"]]`.
pub type Paths = Option<Vec<Vec<String>>>;

/// Join two path sets. The value itself wins over any field.
pub fn join_paths(a: &Paths, b: &Paths) -> Paths {
    match (a, b) {
        (Some(x), Some(y)) => {
            let mut out = x.clone();
            for p in y {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// `prefix` put in front of every path, `None` becoming `[prefix]`.
pub fn under(prefix: &[String], p: &Paths) -> Paths {
    match p {
        None => Some(vec![prefix.to_vec()]),
        Some(ps) => Some(
            ps.iter()
                .map(|q| {
                    let mut v = prefix.to_vec();
                    v.extend(q.iter().cloned());
                    v
                })
                .collect(),
        ),
    }
}

/// One field of a struct declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    /// The field name. An embedded struct's name is its type name.
    pub name: String,
    /// Base type name, without pointer, package or type arguments.
    pub ty: String,
    pub embedded: bool,
    /// Struct tag without the quotes, as `json:"phone"`.
    pub tag: Option<String>,
}

/// A function that reads a package-level variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub file: String,
    pub line: usize,
    pub func: String,
}

/// Struct declarations and package-level variables of the scanned files.
#[derive(Debug, Default, Clone)]
pub struct Module {
    /// Struct type name to its fields.
    pub structs: HashMap<String, Vec<FieldInfo>>,
    /// Package-level variable name to whether it holds a map.
    pub globals: HashMap<String, bool>,
    /// Package-level variable name to the functions that use it.
    pub readers: HashMap<String, Vec<Reader>>,
}

/// `*pkg.Name[T]` as `Name`.
pub fn base_type(ty: &str) -> String {
    let t = ty.trim().trim_start_matches(['*', '&']);
    let t = t.split('[').next().unwrap_or(t);
    t.rsplit('.').next().unwrap_or(t).to_string()
}

impl Module {
    /// Index every file: (path, root node, source).
    pub fn build(files: &[(&str, Node, &[u8])], m: &Mechanics) -> Module {
        let mut out = Module::default();
        for (_, root, src) in files {
            out.add_decls(*root, src, m);
        }
        for (file, root, src) in files {
            out.add_readers(file, *root, src, m);
        }
        out
    }

    fn add_decls(&mut self, root: Node, src: &[u8], m: &Mechanics) {
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if Mechanics::is(&m.nodes.function, n.kind()) {
                continue;
            }
            if Mechanics::is(&m.nodes.type_decl, n.kind()) {
                let name = kids(n, &m.fields.name).first().map(|x| text(*x, src).to_string());
                if let Some(name) = name {
                    let fields = struct_fields(n, src, m);
                    if !fields.is_empty() {
                        self.structs.insert(name, fields);
                    }
                }
                continue;
            }
            if Mechanics::is(&m.nodes.global, n.kind()) && n.parent().is_some_and(|p| p.id() == root.id()) {
                let mut specs = vec![n];
                while let Some(s) = specs.pop() {
                    if Mechanics::is(&m.nodes.assign, s.kind()) {
                        let is_map = kids(s, &m.fields.ty).first().is_some_and(|t| is_map_expr(*t, m))
                            || kids(s, &m.fields.right)
                                .first()
                                .and_then(|v| named(*v).into_iter().next().or(Some(*v)))
                                .is_some_and(|v| is_map_expr(v, m));
                        for x in kids(s, &m.fields.left) {
                            if Mechanics::is(&m.nodes.name, x.kind()) {
                                self.globals.insert(text(x, src).to_string(), is_map);
                            }
                        }
                    } else {
                        specs.extend(named(s));
                    }
                }
                continue;
            }
            stack.extend(named(n));
        }
    }

    fn add_readers(&mut self, file: &str, root: Node, src: &[u8], m: &Mechanics) {
        if self.globals.is_empty() {
            return;
        }
        for f in named(root).into_iter().filter(|f| Mechanics::is(&m.nodes.function, f.kind())) {
            let func = function_name(f, src, m);
            let mut seen: HashSet<String> = HashSet::new();
            let mut stack = vec![f];
            let mut hits: Vec<(usize, String)> = vec![];
            while let Some(n) = stack.pop() {
                if Mechanics::is(&m.nodes.name, n.kind()) {
                    let t = text(n, src);
                    if self.globals.contains_key(t) && seen.insert(t.to_string()) {
                        hits.push((n.start_position().row + 1, t.to_string()));
                    }
                }
                stack.extend(named(n));
            }
            for (line, g) in hits {
                self.readers.entry(g).or_default().push(Reader { file: file.to_string(), line, func: func.clone() });
            }
        }
    }

    /// The field `name` of struct `ty`, directly or promoted through embedded
    /// structs, with the embedded names passed on the way.
    pub fn field(&self, ty: &str, name: &str) -> Option<(Vec<String>, &FieldInfo)> {
        self.field_at(&base_type(ty), name, 0)
    }

    fn field_at(&self, ty: &str, name: &str, depth: usize) -> Option<(Vec<String>, &FieldInfo)> {
        let fields = self.structs.get(ty)?;
        if let Some(f) = fields.iter().find(|f| f.name == name) {
            return Some((vec![], f));
        }
        if depth > 4 {
            return None;
        }
        for e in fields.iter().filter(|f| f.embedded) {
            if let Some((mut chain, f)) = self.field_at(&e.ty, name, depth + 1) {
                chain.insert(0, e.name.clone());
                return Some((chain, f));
            }
        }
        None
    }

    /// Whether struct `ty` is `owner` or embeds it, at any depth.
    pub fn promotes(&self, ty: &str, owner: &str) -> bool {
        let (t, o) = (base_type(ty), base_type(owner));
        if t == o {
            return true;
        }
        let mut stack = vec![(t, 0)];
        while let Some((t, d)) = stack.pop() {
            for e in self.structs.get(&t).into_iter().flatten().filter(|f| f.embedded) {
                if e.ty == o {
                    return true;
                }
                if d < 4 {
                    stack.push((e.ty.clone(), d + 1));
                }
            }
        }
        false
    }

    /// Whether `name` is an embedded field of some struct.
    pub fn is_embedded(&self, name: &str) -> bool {
        self.structs.values().flatten().any(|f| f.embedded && f.name == name)
    }

    /// The name of the `i`th field of struct `ty`, for a positional literal.
    pub fn positional(&self, ty: &str, i: usize) -> Option<String> {
        self.structs.get(&base_type(ty))?.get(i).map(|f| f.name.clone())
    }

    /// The field called `name` when the owner type is not known: the one
    /// struct that declares it, or the tag every declaring struct agrees on.
    pub fn any_field(&self, name: &str) -> Option<&FieldInfo> {
        let all: Vec<&FieldInfo> = self.structs.values().flatten().filter(|f| f.name == name && !f.embedded).collect();
        let first = *all.first()?;
        all.iter().all(|f| f.tag == first.tag).then_some(first)
    }

    /// The struct tag of field `name` on type `ty` (or any type when unknown),
    /// as a row tag `tag json:"phone"`.
    pub fn tag_of(&self, ty: Option<&str>, name: &str) -> Option<String> {
        let f = match ty {
            Some(t) => self.field(t, name).map(|(_, f)| f),
            None => self.any_field(name),
        }?;
        f.tag.as_ref().map(|t| format!("tag {}", t))
    }
}

/// Whether a type or value node is a map: a map type, a map literal or
/// `make(map...)`.
pub fn is_map_expr(n: Node, m: &Mechanics) -> bool {
    if Mechanics::is(&m.nodes.map_type, n.kind()) {
        return true;
    }
    if Mechanics::is(&m.nodes.composite, n.kind()) {
        return kids(n, &m.fields.ty).first().is_some_and(|t| is_map_expr(*t, m));
    }
    if Mechanics::is(&m.nodes.call, n.kind()) {
        return kids(n, &m.fields.arguments)
            .first()
            .and_then(|a| named(*a).into_iter().next())
            .is_some_and(|a| Mechanics::is(&m.nodes.map_type, a.kind()));
    }
    kids(n, &m.fields.operand).first().is_some_and(|o| is_map_expr(*o, m))
}

/// The fields of a struct type declaration, empty when it is not a struct.
fn struct_fields(decl: Node, src: &[u8], m: &Mechanics) -> Vec<FieldInfo> {
    let mut out = vec![];
    let mut stack: Vec<Node> = kids(decl, &m.fields.ty);
    let mut decls = vec![];
    while let Some(n) = stack.pop() {
        if Mechanics::is(&m.nodes.field_decl, n.kind()) {
            decls.push(n);
            continue;
        }
        if Mechanics::is(&m.nodes.type_decl, n.kind()) || Mechanics::is(&m.nodes.function, n.kind()) {
            continue;
        }
        stack.extend(named(n));
    }
    decls.sort_by_key(|d| d.start_byte());
    for d in decls {
        let ty = kids(d, &m.fields.ty).first().map(|t| base_type(text(*t, src))).unwrap_or_default();
        let tag = kids(d, &m.fields.tag).first().map(|t| text(*t, src).trim_matches('`').to_string());
        let names = kids(d, &m.fields.name);
        if names.is_empty() {
            out.push(FieldInfo { name: ty.clone(), ty: ty.clone(), embedded: true, tag: tag.clone() });
        }
        for nm in names {
            out.push(FieldInfo { name: text(nm, src).to_string(), ty: ty.clone(), embedded: false, tag: tag.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mech() -> Mechanics {
        toml::from_str(include_str!("../../config/languages/go.toml")).unwrap()
    }

    #[test]
    fn structs_tags_embedding_and_globals() {
        let src = "package a\ntype Booking struct {\n\tID int\n\tCustomerPhone string `json:\"phone\"`\n}\ntype AdminBooking struct {\n\t*Booking\n\tNotes string\n}\nvar cache = map[string]Booking{}\nvar last Booking\nfunc get() Booking { return cache[\"a\"] }\n";
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_go::LANGUAGE.into()).unwrap();
        let tree = p.parse(src, None).unwrap();
        let m = mech();
        let md = Module::build(&[("a.go", tree.root_node(), src.as_bytes())], &m);
        assert_eq!(md.tag_of(Some("Booking"), "CustomerPhone").as_deref(), Some("tag json:\"phone\""));
        assert_eq!(md.tag_of(Some("*AdminBooking"), "CustomerPhone").as_deref(), Some("tag json:\"phone\""));
        assert_eq!(md.field("AdminBooking", "CustomerPhone").unwrap().0, vec!["Booking".to_string()]);
        assert!(md.promotes("AdminBooking", "Booking"));
        assert!(!md.promotes("Booking", "AdminBooking"));
        assert!(md.is_embedded("Booking"));
        assert_eq!(md.positional("Booking", 1).as_deref(), Some("CustomerPhone"));
        assert_eq!(md.globals.get("cache"), Some(&true));
        assert_eq!(md.globals.get("last"), Some(&false));
        assert_eq!(md.readers["cache"], vec![Reader { file: "a.go".into(), line: 12, func: "get".into() }]);
    }
}
