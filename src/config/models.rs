//! Model files: how a call into outside code moves values.

use super::ConfigError;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// One end of a `pass` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Argument by index.
    Arg(usize),
    /// Every argument (`*`), valid only as a source.
    AllArgs,
    Receiver,
    Return,
}

impl Slot {
    fn parse(s: &str) -> Option<Slot> {
        match s {
            "*" => Some(Slot::AllArgs),
            "receiver" => Some(Slot::Receiver),
            "return" => Some(Slot::Return),
            n => n.parse().ok().map(Slot::Arg),
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Slot::Arg(n) => write!(f, "{}", n),
            Slot::AllArgs => write!(f, "*"),
            Slot::Receiver => write!(f, "receiver"),
            Slot::Return => write!(f, "return"),
        }
    }
}

/// A `from -> to` entry. `from` is an argument, `*` or `receiver`.
/// `to` is an argument, `receiver` or `return`.
///
/// Two suffixes bind a collection's elements to a callback's parameter, as
/// `receiver[*] -> 0.param0` for `xs.map(f)`: `[*]` on the source moves each
/// element rather than the whole value, and `.paramN` on an argument target
/// moves it into parameter N of the function passed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    pub from: Slot,
    pub to: Slot,
    /// `[*]` on the source: the elements of `from`, not `from` itself.
    pub from_elements: bool,
    /// `.paramN` on the target: parameter N of the callback passed at `to`.
    pub to_param: Option<usize>,
}

impl Pass {
    /// A pass with no suffixes.
    pub fn plain(from: Slot, to: Slot) -> Pass {
        Pass { from, to, from_elements: false, to_param: None }
    }
}

impl Pass {
    pub fn parse(s: &str) -> Result<Pass, String> {
        let bad = || {
            format!(
                "bad pass entry \"{}\": expected \"from -> to\" with an argument index, *, receiver or return",
                s
            )
        };
        let (a, b) = s.split_once("->").ok_or_else(bad)?;
        let (a, from_elements) = match a.trim().strip_suffix("[*]") {
            Some(rest) => (rest.trim_end(), true),
            None => (a.trim(), false),
        };
        let b = b.trim();
        let (b, to_param) = match b.split_once(".param") {
            Some((slot, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => {
                (slot.trim_end(), Some(n.parse::<usize>().map_err(|_| bad())?))
            }
            Some(_) => return Err(bad()),
            None => (b, None),
        };
        let from = Slot::parse(a).ok_or_else(bad)?;
        let to = Slot::parse(b).ok_or_else(bad)?;
        if from == Slot::Return {
            return Err(format!("bad pass entry \"{}\": return cannot be a source", s));
        }
        if to == Slot::AllArgs {
            return Err(format!("bad pass entry \"{}\": * can only be a source", s));
        }
        if from_elements && from == Slot::AllArgs {
            return Err(format!("bad pass entry \"{}\": [*] needs one argument or the receiver", s));
        }
        if to_param.is_some() && !matches!(to, Slot::Arg(_)) {
            return Err(format!("bad pass entry \"{}\": .paramN needs an argument index before it", s));
        }
        Ok(Pass { from, to, from_elements, to_param })
    }
}

/// `template = { data_arg = N }`: the call renders a template with this data.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateUse {
    pub data_arg: usize,
}

/// `template_load = { ... }`: the call loads templates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateLoad {
    pub fs_arg: Option<usize>,
    pub names_from: Option<usize>,
    pub text_arg: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub pass: Vec<Pass>,
    pub clear: bool,
    pub whole: bool,
    pub source: Option<Slot>,
    pub template: Option<TemplateUse>,
    pub template_load: Option<TemplateLoad>,
    /// Names of the named arguments, in index order: a JSX call
    /// `<For each={xs}>{f}</For>` with `args = ["each", "children"]` has
    /// `each` as argument 0 and its children as argument 1, so `pass` and
    /// `source` address them by those indices. Empty for positional calls.
    pub args: Vec<String>,
    /// Where the model came from, for `config dump`.
    pub origin: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    #[serde(default)]
    pass: Vec<String>,
    #[serde(default)]
    clear: bool,
    #[serde(default)]
    whole: bool,
    source: Option<String>,
    template: Option<TemplateUse>,
    template_load: Option<TemplateLoad>,
    #[serde(default)]
    args: Vec<String>,
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].bytes().filter(|b| *b == b'\n').count() + 1
}

/// Parse one model file. `file` is the label used in errors and `origin`.
pub fn parse_file(file: &str, text: &str) -> Result<Vec<(String, Model)>, ConfigError> {
    let raw: BTreeMap<String, toml::Spanned<RawModel>> = toml::from_str(text).map_err(|e| {
        let line = e.span().map(|s| line_of(text, s.start));
        ConfigError::new(file, line, None, e.message().trim())
    })?;
    let mut out = Vec::new();
    for (key, spanned) in raw {
        let line = line_of(text, spanned.span().start);
        let err = |msg: String| ConfigError::new(file, Some(line), Some(key.clone()), &msg);
        let r = spanned.into_inner();
        let mut pass = Vec::new();
        for p in &r.pass {
            pass.push(Pass::parse(p).map_err(&err)?);
        }
        let source = match &r.source {
            None => None,
            Some(s) => match Slot::parse(s) {
                Some(slot) if slot != Slot::AllArgs => Some(slot),
                _ => return Err(err(format!("bad source \"{}\": expected an argument index, receiver or return", s))),
            },
        };
        out.push((
            key,
            Model {
                pass,
                clear: r.clear,
                whole: r.whole,
                source,
                template: r.template,
                template_load: r.template_load,
                args: r.args,
                origin: file.to_string(),
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pass_form_parses() {
        assert_eq!(Pass::parse("0 -> return").unwrap(), Pass::plain(Slot::Arg(0), Slot::Return));
        assert_eq!(Pass::parse("* -> return").unwrap(), Pass::plain(Slot::AllArgs, Slot::Return));
        assert_eq!(Pass::parse("0 -> 1").unwrap(), Pass::plain(Slot::Arg(0), Slot::Arg(1)));
        assert_eq!(Pass::parse("receiver -> return").unwrap(), Pass::plain(Slot::Receiver, Slot::Return));
        assert_eq!(Pass::parse("1->receiver").unwrap(), Pass::plain(Slot::Arg(1), Slot::Receiver));
    }

    #[test]
    fn element_and_param_suffixes_parse() {
        let p = Pass::parse("receiver[*] -> 0.param0").unwrap();
        assert_eq!(p, Pass { from: Slot::Receiver, to: Slot::Arg(0), from_elements: true, to_param: Some(0) });
        let p = Pass::parse("0[*] -> 1.param2").unwrap();
        assert_eq!(p, Pass { from: Slot::Arg(0), to: Slot::Arg(1), from_elements: true, to_param: Some(2) });
        let p = Pass::parse("receiver -> 0.param1").unwrap();
        assert_eq!(p, Pass { from: Slot::Receiver, to: Slot::Arg(0), from_elements: false, to_param: Some(1) });
        let p = Pass::parse("1 [*] -> return").unwrap();
        assert_eq!(p, Pass { from: Slot::Arg(1), to: Slot::Return, from_elements: true, to_param: None });
    }

    #[test]
    fn plain_forms_keep_the_new_fields_at_their_defaults() {
        let p = Pass::parse("0 -> 1").unwrap();
        assert!(!p.from_elements);
        assert_eq!(p.to_param, None);
        assert_eq!(p, Pass::plain(Slot::Arg(0), Slot::Arg(1)));
    }

    #[test]
    fn bad_suffix_forms_are_rejected() {
        for bad in ["*[*] -> return", "0 -> return.param0", "0 -> receiver.param0", "0 -> 1.param", "0 -> 1.param+1", "0 -> 1.paramx", "0[*] -> *", "0[] -> 1", "return[*] -> 0"] {
            assert!(Pass::parse(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn named_args_parse() {
        let t = "[\"solid-js.For\"]\nargs = [\"each\", \"children\"]\npass = [\"0[*] -> 1.param0\"]\n";
        let m = parse_file("f.toml", t).unwrap();
        assert_eq!(m[0].1.args, vec!["each", "children"]);
        assert!(parse_file("f.toml", "[\"a.B\"]\npass = [\"0 -> return\"]\n").unwrap()[0].1.args.is_empty());
    }

    #[test]
    fn bad_pass_forms_are_rejected() {
        for bad in ["", "0", "0 -> ", "x -> return", "return -> 0", "0 -> *", "0 => 1", "-1 -> 0"] {
            assert!(Pass::parse(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn quoted_keys_with_punctuation_parse() {
        let t = "[\"(*html/template.Template).Execute\"]\ntemplate = { data_arg = 1 }\n\n[\"os.Getenv\"]\nsource = \"return\"\n";
        let m = parse_file("f.toml", t).unwrap();
        let get = |k: &str| m.iter().find(|(n, _)| n == k).map(|(_, m)| m.clone()).unwrap();
        assert_eq!(get("(*html/template.Template).Execute").template, Some(TemplateUse { data_arg: 1 }));
        assert_eq!(get("os.Getenv").source, Some(Slot::Return));
    }

    #[test]
    fn error_names_file_line_and_key() {
        let t = "[\"a.B\"]\npass = [\"0 -> return\"]\n\n[\"c.D\"]\npass = [\"nonsense\"]\n";
        let e = parse_file("m.toml", t).unwrap_err();
        assert_eq!(e.file, "m.toml");
        assert_eq!(e.key.as_deref(), Some("c.D"));
        assert!(e.line.is_some());
        assert!(e.to_string().starts_with("m.toml:"));
        assert!(e.to_string().contains("c.D"));
        assert!(e.to_string().contains("nonsense"));
    }

    #[test]
    fn unknown_field_and_syntax_errors_carry_a_line() {
        let e = parse_file("m.toml", "[\"a.B\"]\nbogus = 1\n").unwrap_err();
        assert!(e.line.is_some());
        let e = parse_file("m.toml", "[\"a.B\"\n").unwrap_err();
        assert_eq!(e.line, Some(1));
    }
}
