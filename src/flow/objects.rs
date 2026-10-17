//! What the walk knows about the whole module when it follows objects
//! (decision 12): struct fields with their tags and embedded structs, and
//! package-level variables with the functions that read them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tree_sitter::Node;

use super::engine::{function_name, kids, named, text};
use super::models::qualify_type;
use crate::config::Mechanics;
use crate::resolve::go::go_imports;

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
    /// The declared type with its package qualifier replaced by the import
    /// path, as `database/sql.DB` for `*sql.DB`. None when the type has no
    /// package part or is not a plain named type.
    pub path: Option<String>,
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
    /// Names of every type the scanned files declare.
    pub types: HashSet<String>,
    /// First result type of each function (`F`), method and interface
    /// method (`T.M`), as (base name, `path.T` when it has a package part).
    pub results: HashMap<String, (String, Option<String>)>,
    /// Set when the maps above are empty and each answer is read from the
    /// files holding the name asked (`Module::lazy`).
    lazy: Option<Rc<LazyModule>>,
}

/// A module read one name at a time. Every map of `Module` is keyed by a
/// name its declaring file holds: a type, struct, variable or function name,
/// the method name of `T.M`, the name of a field, and a reader of a variable
/// holds the variable. So an answer read from the files holding the name, in
/// store order with the last declaration winning, equals the whole module's.
pub struct LazyModule {
    finder: crate::resolve::NameFinder,
    mech: Mechanics,
    /// Each file's own declarations, by path.
    decls: RefCell<HashMap<String, Rc<Module>>>,
    /// Parses read here, kept so the walk that reads the file next finds it parsed.
    kept: RefCell<Vec<Rc<crate::parsed::ParsedFile>>>,
    readers: RefCell<HashMap<String, Vec<Reader>>>,
    /// The files read for each (name, binding).
    lists: RefCell<HashMap<(String, bool), Vec<(String, &'static str)>>>,
}

impl std::fmt::Debug for LazyModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LazyModule")
    }
}

impl LazyModule {
    /// (path, declarations) of every Go file holding `name`, in store order,
    /// or only of those that can declare it at package level or in a type
    /// when `binding` (an embedded field does not put its name first).
    fn holders(&self, name: &str, binding: bool) -> Vec<(String, Rc<Module>)> {
        let mut out = vec![];
        // No declaration is named by nothing, and every file holds it.
        if name.is_empty() {
            return out;
        }
        let key = (name.to_string(), binding);
        let cached = self.lists.borrow().get(&key).cloned();
        let files = match cached {
            Some(f) => f,
            None => {
                let f = if binding { self.finder.binders(name) } else { self.finder.holders(name) };
                self.lists.borrow_mut().insert(key, f.clone());
                f
            }
        };
        for (path, lang) in files {
            if lang != "go" {
                continue;
            }
            if let Some(d) = self.decls.borrow().get(&path) {
                out.push((path, d.clone()));
                continue;
            }
            let Some(p) = crate::parsed::get(&path, lang) else { continue };
            let Some(tree) = &p.tree else { continue };
            let mut m = Module::default();
            m.add_decls(tree.root_node(), &p.source, &self.mech);
            let m = Rc::new(m);
            self.decls.borrow_mut().insert(path.clone(), m.clone());
            self.kept.borrow_mut().push(p.clone());
            out.push((path, m));
        }
        out
    }

    /// The last declaration of `key` in a file holding `name`.
    fn last<T: Clone>(&self, name: &str, get: impl Fn(&Module) -> Option<T>) -> Option<T> {
        self.holders(name, true).iter().filter_map(|(_, m)| get(m)).last()
    }
}

/// `*pkg.Name[T]` as `Name`.
pub fn base_type(ty: &str) -> String {
    let t = ty.trim().trim_start_matches(['*', '&']);
    let t = t.split('[').next().unwrap_or(t);
    t.rsplit('.').next().unwrap_or(t).to_string()
}

impl Module {
    /// A module whose answers are read from the files `finder` gives for
    /// each name asked.
    pub fn lazy(finder: crate::resolve::NameFinder, mech: &Mechanics) -> Module {
        Module {
            lazy: Some(Rc::new(LazyModule {
                finder,
                mech: mech.clone(),
                decls: RefCell::new(HashMap::new()),
                kept: RefCell::new(vec![]),
                readers: RefCell::new(HashMap::new()),
                lists: RefCell::new(HashMap::new()),
            })),
            ..Module::default()
        }
    }

    /// Whether the scanned files declare a type named `t`.
    pub fn has_type(&self, t: &str) -> bool {
        match &self.lazy {
            None => self.types.contains(t),
            Some(l) => l.holders(t, true).iter().any(|(_, m)| m.types.contains(t)),
        }
    }

    /// Whether package-level variable `name` holds a map, None when there is
    /// no such variable.
    pub fn global(&self, name: &str) -> Option<bool> {
        match &self.lazy {
            None => self.globals.get(name).copied(),
            Some(l) => l.last(name, |m| m.globals.get(name).copied()),
        }
    }

    /// The functions that use package-level variable `name`, in file order.
    pub fn readers_of(&self, name: &str) -> Vec<Reader> {
        let Some(l) = &self.lazy else { return self.readers.get(name).cloned().unwrap_or_default() };
        if let Some(r) = l.readers.borrow().get(name) {
            return r.clone();
        }
        let mut out = vec![];
        if self.global(name).is_some() {
            for (path, _) in l.holders(name, false) {
                let Some(p) = crate::parsed::get(&path, "go") else { continue };
                let Some(tree) = &p.tree else { continue };
                let mut one = Module::default();
                one.globals.insert(name.to_string(), false);
                one.add_readers(&path, tree.root_node(), &p.source, &l.mech);
                out.extend(one.readers.remove(name).unwrap_or_default());
            }
        }
        l.readers.borrow_mut().insert(name.to_string(), out.clone());
        out
    }

    /// The first result type recorded under `key` (`F` or `T.M`).
    pub fn result(&self, key: &str) -> Option<(String, Option<String>)> {
        match &self.lazy {
            None => self.results.get(key).cloned(),
            Some(l) => {
                let name = key.rsplit('.').next().unwrap_or(key);
                if name.is_empty() {
                    return None;
                }
                l.last(name, |m| m.results.get(key).cloned())
            }
        }
    }

    /// The fields of struct `ty`.
    fn struct_fields(&self, ty: &str) -> Option<Vec<FieldInfo>> {
        match &self.lazy {
            None => self.structs.get(ty).cloned(),
            Some(l) => l.last(ty, |m| m.structs.get(ty).cloned()),
        }
    }

    /// Struct names declared in the files holding `name` with a field that
    /// `keep` accepts, whose winning declaration has such a field too.
    fn structs_with(&self, name: &str, keep: impl Fn(&FieldInfo) -> bool) -> Vec<Vec<FieldInfo>> {
        let Some(l) = &self.lazy else {
            return self.structs.values().filter(|fs| fs.iter().any(&keep)).cloned().collect();
        };
        let mut names: Vec<String> = vec![];
        for (_, m) in l.holders(name, false) {
            for (s, fs) in &m.structs {
                if fs.iter().any(&keep) && !names.contains(s) {
                    names.push(s.clone());
                }
            }
        }
        names.sort();
        names.into_iter().filter_map(|s| self.struct_fields(&s)).filter(|fs| fs.iter().any(&keep)).collect()
    }

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
        let imports = go_imports(root, src);
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if Mechanics::is(&m.nodes.function, n.kind()) {
                self.add_result(n, None, src, &imports);
                continue;
            }
            if Mechanics::is(&m.nodes.type_decl, n.kind()) {
                let name = kids(n, &m.fields.name).first().map(|x| text(*x, src).to_string());
                if let Some(name) = name {
                    self.types.insert(name.clone());
                    // Interface methods.
                    if let Some(t) = n.child_by_field_name("type").filter(|t| t.kind() == "interface_type") {
                        for e in named(t) {
                            if matches!(e.kind(), "method_elem" | "method_spec") {
                                self.add_result(e, Some(&name), src, &imports);
                            }
                        }
                    }
                    let fields = struct_fields(n, src, m, &imports);
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

    /// Record the first result type of function or method `f`, keyed `F`,
    /// or `T.M` with `T` its receiver or `owner` (an interface).
    fn add_result(&mut self, f: Node, owner: Option<&str>, src: &[u8], imports: &HashMap<String, String>) {
        let Some(name) = f.child_by_field_name("name").map(|x| text(x, src).to_string()) else { return };
        let recv = f
            .child_by_field_name("receiver")
            .and_then(|r| named(r).into_iter().next())
            .and_then(|p| p.child_by_field_name("type"))
            .map(|t| base_type(text(t, src)));
        let key = match (owner, recv) {
            (Some(o), _) => format!("{}.{}", o, name),
            (None, Some(r)) => format!("{}.{}", r, name),
            (None, None) => name,
        };
        let Some(res) = f.child_by_field_name("result") else { return };
        let ty = if res.kind() == "parameter_list" {
            match named(res).into_iter().next().and_then(|p| p.child_by_field_name("type")) {
                Some(t) => t,
                None => return,
            }
        } else {
            res
        };
        let full = text(ty, src).trim().to_string();
        let plain = full.trim_start_matches('*');
        let path = if plain.contains(['[', ' ', '(', '{']) { None } else { qualify_type(plain, imports) };
        self.results.insert(key, (base_type(&full), path));
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
    pub fn field(&self, ty: &str, name: &str) -> Option<(Vec<String>, FieldInfo)> {
        self.field_at(&base_type(ty), name, 0)
    }

    fn field_at(&self, ty: &str, name: &str, depth: usize) -> Option<(Vec<String>, FieldInfo)> {
        let fields = self.struct_fields(ty)?;
        if let Some(f) = fields.iter().find(|f| f.name == name) {
            return Some((vec![], f.clone()));
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
            for e in self.struct_fields(&t).into_iter().flatten().filter(|f| f.embedded) {
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
        !self.structs_with(name, |f| f.embedded && f.name == name).is_empty()
    }

    /// The name of the `i`th field of struct `ty`, for a positional literal.
    pub fn positional(&self, ty: &str, i: usize) -> Option<String> {
        self.struct_fields(&base_type(ty))?.get(i).map(|f| f.name.clone())
    }

    /// The field called `name` when the owner type is not known: the one
    /// struct that declares it, or the tag every declaring struct agrees on.
    pub fn any_field(&self, name: &str) -> Option<FieldInfo> {
        let keep = |f: &FieldInfo| f.name == name && !f.embedded;
        let all: Vec<FieldInfo> = self.structs_with(name, keep).into_iter().flatten().filter(|f| keep(f)).collect();
        let first = all.first()?;
        all.iter().all(|f| f.tag == first.tag).then(|| first.clone())
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
fn struct_fields(decl: Node, src: &[u8], m: &Mechanics, imports: &HashMap<String, String>) -> Vec<FieldInfo> {
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
    // Type parameters with their constraints: a field of type `D` in
    // `Repo[D Driver]` has the methods of `Driver`.
    let mut tparams: HashMap<String, String> = HashMap::new();
    if let Some(list) = decl.child_by_field_name("type_parameters") {
        for p in named(list) {
            let Some(c) = p.child_by_field_name("type") else { continue };
            let mut cur = p.walk();
            for nm in p.children_by_field_name("name", &mut cur) {
                tparams.insert(text(nm, src).to_string(), text(c, src).trim().to_string());
            }
        }
    }
    for d in decls {
        let mut full = kids(d, &m.fields.ty).first().map(|t| text(*t, src).trim().to_string()).unwrap_or_default();
        if let Some(c) = tparams.get(full.trim_start_matches('*')) {
            full = c.clone();
        }
        let ty = base_type(&full);
        let plain = full.trim_start_matches('*');
        let path = if plain.contains(['[', ' ', '(', '{']) { None } else { qualify_type(plain, imports) };
        let tag = kids(d, &m.fields.tag).first().map(|t| text(*t, src).trim_matches('`').to_string());
        let names = kids(d, &m.fields.name);
        if names.is_empty() {
            out.push(FieldInfo { name: ty.clone(), ty: ty.clone(), embedded: true, tag: tag.clone(), path: path.clone() });
        }
        for nm in names {
            out.push(FieldInfo {
                name: text(nm, src).to_string(),
                ty: ty.clone(),
                embedded: false,
                tag: tag.clone(),
                path: path.clone(),
            });
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
