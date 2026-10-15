//! The `Outside` hook answered from model files (decisions 11, 13 and 18).

use std::collections::HashMap;

use super::{CallEffect, CallSite, Module, Outside};
use crate::config::{gomod, LangConfig, Model};

/// Answers outside calls from a language's loaded models.
pub struct ModelOutside<'c> {
    pub cfg: &'c LangConfig,
    /// The scanned files' declarations. With it, a receiver type without a
    /// package part is in the repo only when one of them declares it.
    pub module: Option<&'c Module>,
}

impl<'c> ModelOutside<'c> {
    pub fn new(cfg: &'c LangConfig) -> ModelOutside<'c> {
        ModelOutside { cfg, module: None }
    }

    pub fn with_module(cfg: &'c LangConfig, module: &'c Module) -> ModelOutside<'c> {
        ModelOutside { cfg, module: Some(module) }
    }
}

/// Result types of outside calls, keyed like models, for receivers whose
/// variable is bound from such a call (`v := reflect.ValueOf(x)` then
/// `v.Field(i)`). Only the first result is listed.
const RESULT_TYPES: &[(&str, &str)] = &[
    ("reflect.ValueOf", "reflect.Value"),
    ("reflect.Indirect", "reflect.Value"),
    ("reflect.TypeOf", "reflect.Type"),
    ("(reflect.Value).Elem", "reflect.Value"),
    ("(reflect.Value).Field", "reflect.Value"),
    ("(reflect.Value).FieldByName", "reflect.Value"),
    ("(reflect.Value).Index", "reflect.Value"),
    ("(reflect.Value).Type", "reflect.Type"),
    ("(reflect.Type).Elem", "reflect.Type"),
    ("(reflect.Type).Field", "reflect.StructField"),
    ("(*database/sql.DB).Exec", "database/sql.Result"),
    ("(*database/sql.DB).ExecContext", "database/sql.Result"),
    ("(*database/sql.Tx).Exec", "database/sql.Result"),
    ("(*database/sql.Tx).ExecContext", "database/sql.Result"),
    ("(*database/sql.Stmt).Exec", "database/sql.Result"),
];

/// The type (as `path.T`) a call returns: `path.F` for a package function,
/// or a method `m` on a receiver of type `recv` (as `path.T`).
pub fn result_type(func: Option<&str>, recv: Option<&str>, m: Option<&str>) -> Option<String> {
    let keys: Vec<String> = match (func, recv, m) {
        (Some(f), _, _) => vec![f.to_string()],
        (None, Some(t), Some(m)) => vec![format!("(*{}).{}", t, m), format!("({}).{}", t, m)],
        _ => return None,
    };
    keys.iter().find_map(|k| RESULT_TYPES.iter().find(|(x, _)| x == k).map(|(_, t)| t.to_string()))
}

/// `pkg.T` or `*pkg.T` with `pkg` an import name, as `path.T`. None when the
/// type has no package part or the package is not imported.
pub fn qualify_type(ty: &str, imports: &HashMap<String, String>) -> Option<String> {
    let t = ty.trim_start_matches('*');
    let (pkg, name) = t.split_once('.')?;
    imports.get(pkg).map(|path| format!("{}.{}", path, name))
}

/// Model keys a call may match, most specific first: `path.Func` for a
/// package call, `(*path.Type).Method` then `(path.Type).Method` for a method
/// on a typed receiver, `builtin.name` for a plain name.
pub fn candidate_keys(site: &CallSite) -> Vec<String> {
    if let (Some(pkg), Some(m)) = (&site.package, &site.method) {
        return vec![format!("{}.{}", pkg, m)];
    }
    if let (Some(t), Some(m)) = (&site.recv_path, &site.method) {
        return vec![format!("(*{}).{}", t, m), format!("({}).{}", t, m)];
    }
    if site.method.is_none() && !site.callee.is_empty() && !site.callee.contains(['.', '(', '[']) {
        return vec![format!("builtin.{}", site.callee)];
    }
    vec![]
}

/// The first candidate key that has a model in `cfg`.
pub fn model_key(cfg: &LangConfig, site: &CallSite) -> Option<String> {
    candidate_keys(site).into_iter().find(|k| cfg.model(k).is_some())
}

/// The name for `via` when the callee is a function in this repo: a plain
/// name declared in the file, a package inside the module, or a method on a
/// type without a package part that the module declares. A receiver whose
/// type is unknown is not in the repo.
fn in_repo(cfg: &LangConfig, module: Option<&Module>, site: &CallSite) -> Option<String> {
    if site.local {
        return Some(site.callee.clone());
    }
    if let (Some(pkg), Some(module)) = (&site.package, &cfg.module_path) {
        if !gomod::is_outside(module, pkg) {
            return Some(site.callee.clone());
        }
    }
    if let (Some(t), None, Some(m)) = (&site.recv_type, &site.recv_path, &site.method) {
        let t = t.trim_start_matches('*');
        let declared = module.is_none_or(|md| md.types.contains(t));
        if !t.is_empty() && !t.contains('.') && declared {
            return Some(format!("{}.{}", t, m));
        }
    }
    None
}

/// A model as an effect. `clear` wins, otherwise the `pass` entries and the
/// `whole` flag go to the engine as they are. A model with neither moves
/// nothing, as a source or a template model does here.
pub fn effect_of(model: &Model) -> CallEffect {
    if model.clear {
        return CallEffect::Clear;
    }
    CallEffect::Model { pass: model.pass.clone(), whole: model.whole }
}

impl Outside for ModelOutside<'_> {
    fn effect(&self, site: &CallSite) -> CallEffect {
        if let Some(model) = model_key(self.cfg, site).and_then(|k| self.cfg.model(&k)) {
            return effect_of(model);
        }
        match in_repo(self.cfg, self.module, site) {
            Some(name) => CallEffect::Via(name),
            None => CallEffect::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::config::{load_with, Roots};
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctsq-flow-models-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cfg_in(d: &Path) -> LangConfig {
        let home = d.join("home");
        let roots = Roots::from_vars(d, home.to_str(), None, None);
        load_with(&roots, d, "go").unwrap()
    }

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_go::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    fn ends_at(name: &'static str) -> impl Fn(Node, &[u8]) -> bool {
        move |n: Node, src: &[u8]| {
            n.child_by_field_name("function").is_some_and(|f| {
                let t = f.utf8_text(src).unwrap();
                t == name || t.ends_with(&format!(".{}", name))
            })
        }
    }

    /// Run from `ident` after `anchor` through `F` with models from `cfg`.
    fn run_cfg(cfg: &LangConfig, src: &str, anchor: &str, ident: &str, end: &EndFn) -> FlowResult {
        let tree = parse(src);
        let md = Module::default();
        let cx = FlowCtx { file: "a.go", src: src.as_bytes(), root: tree.root_node(), mech: &cfg.mechanics, module: &md };
        let a = src.find(anchor).expect("anchor");
        let sb = a + src[a..].find(ident).expect("ident");
        let start = start_at(&cx, sb, sb + ident.len()).expect("start");
        let fb = src.find("func F(").expect("func") + 1;
        let f = enclosing_function(cx.root, &cfg.mechanics, fb).expect("function");
        analyse_function(&cx, f, &start, end, &ModelOutside::new(cfg))
    }

    fn run(src: &str, anchor: &str, ident: &str) -> FlowResult {
        let d = tmp("run");
        let cfg = cfg_in(&d);
        run_cfg(&cfg, src, anchor, ident, &ends_at("sink"))
    }

    fn tags(r: &FlowResult, needle: &str) -> Vec<String> {
        r.root
            .walk()
            .into_iter()
            .skip(1)
            .find(|h| h.text.contains(needle))
            .unwrap_or_else(|| panic!("no hop for {needle}\n{}", report(&[r.clone()])))
            .tags
            .clone()
    }

    fn has_hop(r: &FlowResult, needle: &str) -> bool {
        r.root.walk().into_iter().skip(1).any(|h| h.text.contains(needle))
    }

    #[test]
    fn to_upper_passes_as_derived() {
        let src = "package a\nimport \"strings\"\nfunc F(t string) {\n\tu := strings.ToUpper(t)\n\tsink(u)\n}\n";
        let r = run(src, "F(", "t");
        assert_eq!(tags(&r, "u := strings"), vec!["derived"]);
        assert_eq!(tags(&r, "sink(u)"), vec!["sink"]);
        assert!(r.unmodelled_ends().is_empty());
    }

    #[test]
    fn len_clears() {
        let src = "package a\nfunc F(t string) {\n\tn := len(t)\n\tsink(n)\n}\n";
        let r = run(src, "F(", "t");
        assert!(!has_hop(&r, "sink(n)"));
        assert_eq!(r.sinks(), 0);
    }

    #[test]
    fn comparison_clears() {
        let src = "package a\nfunc F(t string) {\n\tok := t == \"a\"\n\tsink(ok)\n\tlt := t < \"b\"\n\tsink(lt)\n\tc := t + \"x\"\n\tsink(c)\n}\n";
        let r = run(src, "F(", "t");
        assert!(!has_hop(&r, "sink(ok)"));
        assert!(!has_hop(&r, "sink(lt)"));
        assert!(has_hop(&r, "sink(c)"), "a concatenation still carries the data");
        assert_eq!(r.sinks(), 1);
    }

    #[test]
    fn unknown_outside_call_continues_unmodelled() {
        let src = "package a\nimport \"example.com/x/y\"\nfunc F(t string) {\n\tu := y.Do(t)\n\tsink(u)\n}\n";
        let r = run(src, "F(", "t");
        assert_eq!(tags(&r, "u := y.Do"), vec!["derived", "unmodelled"]);
        assert_eq!(tags(&r, "sink(u)"), vec!["sink"]);
    }

    #[test]
    fn repo_model_override_takes_effect() {
        let d = tmp("override");
        let dir = d.join(".ctsq/models/go");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.toml"), "[\"strings.ToUpper\"]\nclear = true\n").unwrap();
        let cfg = cfg_in(&d);
        let src = "package a\nimport \"strings\"\nfunc F(t string) {\n\tu := strings.ToUpper(t)\n\tsink(u)\n}\n";
        let r = run_cfg(&cfg, src, "F(", "t", &ends_at("sink"));
        assert!(!has_hop(&r, "sink(u)"), "{}", report(&[r.clone()]));
    }

    #[test]
    fn printf_whole_carries_every_field() {
        let src = "package a\nimport \"fmt\"\ntype User struct {\n\tEmail string\n}\nfunc F(u User) {\n\tfmt.Printf(\"%v\", u)\n}\n";
        let d = tmp("whole");
        let cfg = cfg_in(&d);
        let r = run_cfg(&cfg, src, "struct", "Email", &ends_at("Printf"));
        assert_eq!(tags(&r, "fmt.Printf"), vec!["sink"]);
        // a call with no whole model does not carry the field
        let src2 = src.replace("\"fmt\"", "\"example.com/fmt\"");
        let r2 = run_cfg(&cfg, &src2, "struct", "Email", &ends_at("Printf"));
        assert_eq!(r2.sinks(), 0);
    }

    #[test]
    fn unmarshal_carries_body_into_form() {
        let src = "package a\nimport \"encoding/json\"\nfunc F(body []byte) {\n\tvar form Form\n\terr := json.Unmarshal(body, &form)\n\t_ = err\n\tsink(form)\n}\n";
        let r = run(src, "F(", "body");
        assert_eq!(tags(&r, "json.Unmarshal"), vec!["moved"]);
        assert_eq!(tags(&r, "sink(form)"), vec!["sink"]);
        // the same call as a statement
        let src2 = src.replace("err := json.Unmarshal(body, &form)\n\t_ = err", "json.Unmarshal(body, &form)");
        let r2 = run(&src2, "F(", "body");
        assert_eq!(tags(&r2, "sink(form)"), vec!["sink"]);
    }

    #[test]
    fn method_on_typed_receiver_resolves_its_key() {
        let src = "package a\nimport \"strings\"\nfunc F(t string) {\n\tvar b strings.Builder\n\tb.WriteString(t)\n\tsink(b.String())\n}\n";
        let tree = parse(src);
        let d = tmp("method");
        let cfg = cfg_in(&d);
        let md = Module::default();
        let cx = FlowCtx { file: "a.go", src: src.as_bytes(), root: tree.root_node(), mech: &cfg.mechanics, module: &md };
        let sb = src.find("F(t").unwrap() + 2;
        let start = start_at(&cx, sb, sb + 1).unwrap();
        let f = enclosing_function(cx.root, &cfg.mechanics, sb).unwrap();
        let seen = std::cell::RefCell::new(vec![]);
        let spy = |c: &CallSite| {
            seen.borrow_mut().push(model_key(&cfg, c));
            ModelOutside::new(&cfg).effect(c)
        };
        let r = analyse_function(&cx, f, &start, &ends_at("sink"), &spy);
        assert!(seen.borrow().contains(&Some("(*strings.Builder).WriteString".to_string())), "{:?}", seen.borrow());
        assert_eq!(tags(&r, "b.WriteString(t)"), vec!["moved"]);
        assert_eq!(tags(&r, "sink(b.String())"), vec!["sink"]);
    }

    #[test]
    fn local_function_is_via() {
        let src = "package a\nfunc clean(s string) string { return s }\nfunc F(t string) {\n\tu := clean(t)\n\tsink(u)\n}\n";
        let r = run(src, "F(", "t");
        assert_eq!(tags(&r, "u := clean"), vec!["derived", "via clean"]);
        assert!(r.unmodelled_ends().is_empty());
    }

    #[test]
    fn a_receiver_type_is_in_repo_only_when_the_module_declares_it() {
        let d = tmp("inrepo");
        let cfg = cfg_in(&d);
        let src = "package a\nfunc F() { x.M() }\n";
        let tree = parse(src);
        let site = |t: &str| CallSite {
            node: tree.root_node(),
            callee: "x.M".into(),
            package: None,
            method: Some("M".into()),
            recv_type: Some(t.into()),
            recv_path: None,
            tracked_args: vec![0],
            arg_paths: vec![None],
            recv_tracked: false,
            recv_paths: None,
            whole_args: vec![],
            local: false,
        };
        let mut md = Module::default();
        md.types.insert("Local".into());
        let mo = ModelOutside::with_module(&cfg, &md);
        assert_eq!(mo.effect(&site("*Local")), CallEffect::Via("Local.M".into()));
        assert_eq!(mo.effect(&site("Elsewhere")), CallEffect::Unknown);
        assert_eq!(mo.effect(&site("")), CallEffect::Unknown);
    }

    #[test]
    fn keys_for_each_call_shape() {
        let src = "package a\nfunc F() { f() }\n";
        let tree = parse(src);
        let node = tree.root_node();
        let site = |callee: &str, package: Option<&str>, method: Option<&str>, recv_path: Option<&str>| CallSite {
            node,
            callee: callee.into(),
            package: package.map(Into::into),
            method: method.map(Into::into),
            recv_type: None,
            recv_path: recv_path.map(Into::into),
            tracked_args: vec![0],
            arg_paths: vec![None],
            recv_tracked: false,
            recv_paths: None,
            whole_args: vec![],
            local: false,
        };
        assert_eq!(candidate_keys(&site("strings.ToUpper", Some("strings"), Some("ToUpper"), None)), vec!["strings.ToUpper"]);
        assert_eq!(
            candidate_keys(&site("t.Execute", None, Some("Execute"), Some("html/template.Template"))),
            vec!["(*html/template.Template).Execute", "(html/template.Template).Execute"]
        );
        assert_eq!(candidate_keys(&site("len", None, None, None)), vec!["builtin.len"]);
        let imports: HashMap<String, String> = [("tpl".to_string(), "html/template".to_string())].into();
        assert_eq!(qualify_type("*tpl.Template", &imports).as_deref(), Some("html/template.Template"));
        assert_eq!(qualify_type("Local", &imports), None);
    }
}
