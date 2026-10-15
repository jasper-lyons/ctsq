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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    pub from: Slot,
    pub to: Slot,
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
        let from = Slot::parse(a.trim()).ok_or_else(bad)?;
        let to = Slot::parse(b.trim()).ok_or_else(bad)?;
        if from == Slot::Return {
            return Err(format!("bad pass entry \"{}\": return cannot be a source", s));
        }
        if to == Slot::AllArgs {
            return Err(format!("bad pass entry \"{}\": * can only be a source", s));
        }
        Ok(Pass { from, to })
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
        assert_eq!(Pass::parse("0 -> return").unwrap(), Pass { from: Slot::Arg(0), to: Slot::Return });
        assert_eq!(Pass::parse("* -> return").unwrap(), Pass { from: Slot::AllArgs, to: Slot::Return });
        assert_eq!(Pass::parse("0 -> 1").unwrap(), Pass { from: Slot::Arg(0), to: Slot::Arg(1) });
        assert_eq!(Pass::parse("receiver -> return").unwrap(), Pass { from: Slot::Receiver, to: Slot::Return });
        assert_eq!(Pass::parse("1->receiver").unwrap(), Pass { from: Slot::Arg(1), to: Slot::Receiver });
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
