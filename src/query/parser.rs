use super::ast::*;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Star,
    Amp,
    Gt,
    Plus,
    Tilde,
    Dot,
    LParen,
    RParen,
    At,
    Hash,
    LBracket,
    RBracket,
    Colon,
    Eq,
    PrefixEq,
    ContainsEq,
    SuffixEq,
    Calls,
    CallsEventually,
    CalledBy,
    CalledByEventually,
    Flows,
    FlowsEventually,
    FlowsFrom,
    FlowsFromEventually,
    /// A character that starts no token. `parse` turns it into an error.
    Invalid(char),
    /// A quoted name or regex whose closing delimiter never came.
    Unterminated(char),
    Ident(String),
    QuotedStr(String),
    Regex(String),
    Whitespace,
    Eof,
}

/// Byte span of a token in the query string: start inclusive, end exclusive.
type Span = (usize, usize);

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    /// Byte offset of the next unread character.
    at: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Self { chars: s.chars().peekable(), at: 0 }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next();
        if let Some(c) = c {
            self.at += c.len_utf8();
        }
        c
    }

    fn next_token(&mut self) -> Token {
        match self.chars.peek().copied() {
            None => Token::Eof,
            Some(c) if c == ' ' || c == '\t' => {
                while matches!(self.chars.peek(), Some(' ') | Some('\t')) {
                    self.bump();
                }
                Token::Whitespace
            }
            Some('*') => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    Token::ContainsEq
                } else {
                    Token::Star
                }
            }
            Some('^') => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    Token::PrefixEq
                } else {
                    Token::Invalid('^')
                }
            }
            Some('$') => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    Token::SuffixEq
                } else {
                    Token::Invalid('$')
                }
            }
            Some('[') => { self.bump(); Token::LBracket }
            Some(']') => { self.bump(); Token::RBracket }
            Some(':') => { self.bump(); Token::Colon }
            Some('-') => {
                self.bump();
                match self.chars.peek() {
                    Some('>') => { self.bump(); Token::Calls }
                    Some('-') => {
                        self.bump();
                        if self.chars.peek() == Some(&'>') {
                            self.bump();
                            Token::CallsEventually
                        } else {
                            Token::Invalid('-')
                        }
                    }
                    _ => Token::Invalid('-'),
                }
            }
            Some('<') => {
                self.bump();
                match self.chars.peek() {
                    Some('-') => {
                        self.bump();
                        if self.chars.peek() == Some(&'-') {
                            self.bump();
                            Token::CalledByEventually
                        } else {
                            Token::CalledBy
                        }
                    }
                    Some('=') => {
                        self.bump();
                        if self.chars.peek() == Some(&'=') {
                            self.bump();
                            Token::FlowsFromEventually
                        } else {
                            Token::FlowsFrom
                        }
                    }
                    _ => Token::Invalid('<'),
                }
            }
            Some('=') => {
                self.bump();
                match self.chars.peek() {
                    Some('>') => { self.bump(); Token::Flows }
                    Some('=') => {
                        self.bump();
                        if self.chars.peek() == Some(&'>') {
                            self.bump();
                            Token::FlowsEventually
                        } else {
                            Token::Invalid('=')
                        }
                    }
                    _ => Token::Eq,
                }
            }
            Some('&') => { self.bump(); Token::Amp }
            Some('>') => { self.bump(); Token::Gt }
            Some('+') => { self.bump(); Token::Plus }
            Some('~') => { self.bump(); Token::Tilde }
            Some('.') => { self.bump(); Token::Dot }
            Some('(') => { self.bump(); Token::LParen }
            Some(')') => { self.bump(); Token::RParen }
            Some('@') => { self.bump(); Token::At }
            Some('#') => { self.bump(); Token::Hash }
            Some('"') => {
                self.bump();
                let mut s = String::new();
                loop {
                    match self.bump() {
                        Some('"') => return Token::QuotedStr(s),
                        None => return Token::Unterminated('"'),
                        // `\"` is a quote and `\\` a backslash, so a name can hold either.
                        Some('\\') => match self.chars.peek().copied() {
                            Some(c @ ('"' | '\\')) => {
                                self.bump();
                                s.push(c);
                            }
                            _ => s.push('\\'),
                        },
                        Some(c) => s.push(c),
                    }
                }
            }
            Some('/') => {
                self.bump();
                let mut s = String::new();
                loop {
                    match self.bump() {
                        Some('/') => return Token::Regex(s),
                        None => return Token::Unterminated('/'),
                        // `\/` is a literal slash in the regex. Every other escape
                        // (`\d`, `\.`, `\\`) belongs to the regex engine and stays as typed.
                        Some('\\') => match self.bump() {
                            Some('/') => s.push('/'),
                            Some(c) => {
                                s.push('\\');
                                s.push(c);
                            }
                            None => return Token::Unterminated('/'),
                        },
                        Some(c) => s.push(c),
                    }
                }
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                let mut s = String::new();
                while matches!(self.chars.peek(), Some(c) if c.is_alphanumeric() || *c == '_') {
                    s.push(self.bump().unwrap());
                }
                Token::Ident(s)
            }
            Some(c) => {
                self.bump();
                Token::Invalid(c)
            }
        }
    }

    fn tokenize(mut self) -> (Vec<Token>, Vec<Span>) {
        let mut tokens = Vec::new();
        let mut spans = Vec::new();
        loop {
            let start = self.at;
            let t = self.next_token();
            let done = t == Token::Eof;
            tokens.push(t);
            spans.push((start, self.at));
            if done { break; }
        }
        (tokens, spans)
    }
}

pub struct Parser {
    src: String,
    tokens: Vec<Token>,
    spans: Vec<Span>,
    pos: usize,
}

impl Parser {
    pub fn new(s: &str) -> Self {
        let (tokens, spans) = Lexer::new(s).tokenize();
        Self { src: s.to_string(), tokens, spans, pos: 0 }
    }

    /// An error at the token under the cursor.
    fn err_here(&self, msg: impl Into<String>) -> QueryError {
        self.err_at(self.pos, msg)
    }

    /// An error at the token `advance` just consumed.
    fn err_prev(&self, msg: impl Into<String>) -> QueryError {
        self.err_at(self.pos.saturating_sub(1), msg)
    }

    fn err_at(&self, idx: usize, msg: impl Into<String>) -> QueryError {
        if let Some(e) = self.slash_error(idx) {
            return e;
        }
        let pos = self.spans.get(idx).map(|s| s.0).unwrap_or(self.src.len());
        QueryError::new(msg, pos)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        t
    }

    fn skip_whitespace(&mut self) {
        while self.peek() == &Token::Whitespace {
            self.advance();
        }
    }

    pub fn parse_query(&mut self) -> Result<Query, QueryError> {
        self.skip_whitespace();
        let head = self.parse_selector()?;
        let mut tail = Vec::new();

        loop {
            let combinator = match self.peek() {
                Token::Whitespace => {
                    // whitespace is descendant combinator only if followed by a selector start
                    if self.is_selector_start_at(self.pos + 1) {
                        self.advance();
                        Combinator::Descendant
                    } else if matches!(
                        self.tokens.get(self.pos + 1),
                        Some(Token::Gt | Token::Plus | Token::Tilde)
                    ) || self.tokens.get(self.pos + 1).and_then(semantic_combinator).is_some() {
                        // `A > B` — the space belongs to the explicit combinator
                        self.advance();
                        continue;
                    } else {
                        break;
                    }
                }
                Token::Gt => { self.advance(); self.skip_whitespace(); Combinator::Child }
                Token::Plus => { self.advance(); self.skip_whitespace(); Combinator::Adjacent }
                Token::Tilde => { self.advance(); self.skip_whitespace(); Combinator::Sibling }
                t => match semantic_combinator(t) {
                    Some(c) => { self.advance(); self.skip_whitespace(); c }
                    None => break,
                },
            };
            let sel = self.parse_selector()?;
            tail.push((combinator, sel));
        }

        Ok(Query { head, tail })
    }

    fn is_selector_start_at(&self, idx: usize) -> bool {
        match self.tokens.get(idx).unwrap_or(&Token::Eof) {
            Token::Star | Token::Amp | Token::Ident(_) | Token::LParen | Token::Hash => true,
            _ => false,
        }
    }

    fn parse_selector(&mut self) -> Result<Selector, QueryError> {
        let node = if self.peek() == &Token::LParen {
            self.advance(); // consume (
            self.skip_whitespace();
            let query = self.parse_query()?;
            self.skip_whitespace();
            let capture = if self.peek() == &Token::At {
                self.advance();
                match self.advance() {
                    Token::Ident(name) => Some(name),
                    _ => return Err(self.err_prev("expected capture name after @")),
                }
            } else {
                None
            };
            self.skip_whitespace();
            if self.peek() != &Token::RParen {
                return Err(self.err_here("expected )").with_hint(format!("close the group: {})", self.src)));
            }
            self.advance();
            SelectorNode::Group { query: Box::new(query), capture }
        } else {
            SelectorNode::Bare(self.parse_atom()?)
        };

        let mut fields = Vec::new();
        let mut not = Vec::new();
        loop {
            if self.peek() == &Token::Colon {
                self.advance();
                match self.advance() {
                    Token::Ident(name) if name == "not" => {}
                    Token::Ident(name) => return Err(self.err_prev(format!("unsupported pseudo-class :{}", name)).with_hint("the only pseudo-class is :not(selector)")),
                    _ => return Err(self.err_prev("expected pseudo-class name after :")),
                }
                if self.peek() != &Token::LParen {
                    return Err(self.err_here("expected ( after :not"));
                }
                self.advance();
                self.skip_whitespace();
                not.push(self.parse_selector()?);
                self.skip_whitespace();
                if self.peek() != &Token::RParen {
                    return Err(self.err_here("expected ) to close :not"));
                }
                self.advance();
                continue;
            }
            if self.peek() != &Token::Dot {
                break;
            }
            self.advance();
            let field = match self.advance() {
                Token::Ident(name) => name,
                _ => return Err(self.err_prev("expected field name after .")),
            };
            if self.peek() != &Token::LParen {
                return Err(self.err_here("expected ( after field name"));
            }
            self.advance();
            self.skip_whitespace();
            let inner = if self.peek() == &Token::RParen {
                None
            } else if self.peek() == &Token::At {
                // `.value(@target)`: any node, captured.
                let any = Query {
                    head: Selector {
                        node: SelectorNode::Bare(Atom { sigil: None, node_type: None, name_match: None, attrs: vec![] }),
                        fields: vec![],
                        not: vec![],
                    },
                    tail: vec![],
                };
                let name = self.parse_capture()?;
                Some(Box::new(captured(any, name)))
            } else {
                let q = self.parse_query()?;
                self.skip_whitespace();
                // `.value(literal @key)`: a capture on the field's inner query
                // without a group around it.
                if self.peek() == &Token::At {
                    let name = self.parse_capture()?;
                    Some(Box::new(captured(q, name)))
                } else {
                    Some(Box::new(q))
                }
            };
            self.skip_whitespace();
            if self.peek() != &Token::RParen {
                return Err(self.err_here("expected ) to close field access").with_hint(format!("close the field access: {})", self.src)));
            }
            self.advance();
            fields.push(FieldAccess { field, inner });
        }

        Ok(Selector { node, fields, not })
    }

    /// `@name`, with the cursor on the `@`.
    fn parse_capture(&mut self) -> Result<String, QueryError> {
        self.advance();
        match self.advance() {
            Token::Ident(name) => Ok(name),
            _ => Err(self.err_prev("expected capture name after @")),
        }
    }

    fn parse_atom(&mut self) -> Result<Atom, QueryError> {
        let sigil = match self.peek() {
            Token::Star => { self.advance(); Some(Sigil::Def) }
            Token::Amp => { self.advance(); Some(Sigil::Ref) }
            _ => None,
        };

        let node_type = match self.peek() {
            Token::Ident(_) => {
                if let Token::Ident(name) = self.advance() { Some(name) } else { None }
            }
            _ => None,
        };

        let mut attrs = Vec::new();
        let name_match = if self.peek() == &Token::Hash {
            self.advance();
            let (nm, qual) = self.parse_name_match()?;
            attrs.extend(qual);
            Some(nm)
        } else {
            None
        };

        while self.peek() == &Token::LBracket {
            attrs.push(self.parse_attr()?);
        }

        Ok(Atom { sigil, node_type, name_match, attrs })
    }

    /// Returns the name match, plus the `qual` attribute when the name used the
    /// dot shorthand (`A.B` not followed by `(` is name `B` qualified by `A`).
    fn parse_name_match(&mut self) -> Result<(NameMatch, Option<AttrMatch>), QueryError> {
        match self.peek().clone() {
            Token::QuotedStr(s) => { self.advance(); Ok((NameMatch::Exact(s), None)) }
            Token::Regex(s) => { self.advance(); Ok((NameMatch::Regex(s), None)) }
            Token::Ident(s) => {
                self.advance();
                if self.peek() == &Token::Dot {
                    if let (Some(Token::Ident(name)), after) =
                        (self.tokens.get(self.pos + 1).cloned(), self.tokens.get(self.pos + 2))
                    {
                        if after != Some(&Token::LParen) {
                            self.pos += 2;
                            let qual = AttrMatch { key: "qual".into(), op: AttrOp::Eq, value: s };
                            return Ok((NameMatch::Exact(name), Some(qual)));
                        }
                    }
                }
                Ok((NameMatch::Exact(s), None))
            }
            _ => Err(self.err_here("expected a name after #").with_hint("a name is an identifier, \"quoted text\" or /regex/, as in id#foo, id#\"a b\" or id#/a|b/")),
        }
    }

    fn parse_attr(&mut self) -> Result<AttrMatch, QueryError> {
        self.advance(); // [
        self.skip_whitespace();
        let key = match self.advance() {
            Token::Ident(k) => k,
            _ => return Err(self.err_prev("expected attribute name after [")),
        };
        self.skip_whitespace();
        let op = match self.advance() {
            Token::Eq => AttrOp::Eq,
            Token::PrefixEq => AttrOp::Prefix,
            Token::ContainsEq => AttrOp::Contains,
            Token::SuffixEq => AttrOp::Suffix,
            _ => return Err(self.err_prev(format!("expected =, ^=, *= or $= after attribute '{}'", key))),
        };
        self.skip_whitespace();
        let (op, value) = match (self.advance(), op) {
            (Token::Ident(v) | Token::QuotedStr(v), op) => (op, v),
            (Token::Regex(v), AttrOp::Eq) => (AttrOp::Regex, v),
            (Token::Regex(_), _) => return Err(self.err_prev("a /regex/ value needs plain =")),
            _ => return Err(self.err_prev(format!("expected a value for attribute '{}'", key))),
        };
        self.skip_whitespace();
        if self.advance() != Token::RBracket {
            return Err(self.err_prev("expected ] to close attribute"));
        }
        Ok(AttrMatch { key, op, value })
    }
}

/// `q` as a one-selector query whose group carries the capture `name`, the
/// same AST `(q @name)` parses to.
fn captured(q: Query, name: String) -> Query {
    Query {
        head: Selector {
            node: SelectorNode::Group { query: Box::new(q), capture: Some(name) },
            fields: vec![],
            not: vec![],
        },
        tail: vec![],
    }
}

fn semantic_combinator(t: &Token) -> Option<Combinator> {
    Some(match t {
        Token::Calls => Combinator::Calls,
        Token::CallsEventually => Combinator::CallsEventually,
        Token::CalledBy => Combinator::CalledBy,
        Token::CalledByEventually => Combinator::CalledByEventually,
        Token::Flows => Combinator::Flows,
        Token::FlowsEventually => Combinator::FlowsEventually,
        Token::FlowsFrom => Combinator::FlowsFrom,
        Token::FlowsFromEventually => Combinator::FlowsFromEventually,
        _ => return None,
    })
}

pub fn parse(s: &str) -> Result<Query, QueryError> {
    let mut parser = Parser::new(s);
    for (i, t) in parser.tokens.iter().enumerate() {
        match t {
            Token::Invalid(c) => {
                return Err(QueryError::new(format!("unexpected character '{}'", c), parser.spans[i].0));
            }
            Token::Unterminated(d) => {
                // A stray inner slash starts a second regex that never closes. Name the
                // first slash rather than the last, since that is where the fault is.
                for j in 1..i {
                    if let Some(e) = parser.slash_error(j) {
                        return Err(e);
                    }
                }
                let what = if *d == '/' { "regex" } else { "quoted name" };
                let start = parser.spans[i].0;
                let fix = format!("{}{}", s, d);
                return Err(QueryError::new(format!("unterminated {}", what), start)
                    .with_hint(format!("close it with a '{}': {}", d, fix)));
            }
            _ => {}
        }
    }
    let query = parser.parse_query()?;
    parser.skip_whitespace();
    if parser.peek() != &Token::Eof {
        let at = parser.spans[parser.pos];
        let text = &s[at.0..at.1];
        return Err(parser.err_here(format!("unexpected '{}'", text)));
    }
    Ok(query)
}

impl Parser {
    /// A regex name filter that ends early because it holds an unescaped `/`:
    /// the token after it touches its closing slash. Reports the slash and
    /// suggests the query with the inner slashes escaped.
    fn slash_error(&self, idx: usize) -> Option<QueryError> {
        if idx == 0 || !matches!(self.tokens.get(idx - 1), Some(Token::Regex(_))) {
            return None;
        }
        if !matches!(
            self.tokens.get(idx),
            Some(Token::Ident(_) | Token::Regex(_) | Token::QuotedStr(_) | Token::Unterminated(_))
        ) {
            return None;
        }
        let (open, close) = (self.spans[idx - 1].0, self.spans[idx - 1].1);
        if close != self.spans[idx].0 {
            return None;
        }
        let mut err = QueryError::new("unexpected '/'", close - 1);
        // The filter runs to the next space. Its last slash closes it, the ones before are inner.
        let after_open = open + 1;
        let run_end = self.src[after_open..]
            .find(|c: char| c.is_whitespace())
            .map(|n| after_open + n)
            .unwrap_or(self.src.len());
        let run = &self.src[after_open..run_end];
        if let Some(last) = run.rfind('/') {
            let first_close = close - 1 - after_open;
            if last > first_close {
                let mut body = String::new();
                let mut prev = ' ';
                for c in run[..last].chars() {
                    if c == '/' && prev != '\\' {
                        body.push('\\');
                    }
                    body.push(c);
                    prev = c;
                }
                let fixed = format!(
                    "{}/{}/{}{}",
                    &self.src[..open],
                    body,
                    &run[last + 1..],
                    &self.src[run_end..]
                );
                err = err.with_hint(format!(
                    "a '/' inside a regex name filter must be escaped: {}",
                    fixed
                ));
            }
        }
        Some(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_function() {
        let q = parse("function").unwrap();
        assert_eq!(q.head.node, SelectorNode::Bare(Atom {
            sigil: None,
            node_type: Some("function".into()),
            name_match: None,
            attrs: vec![],
        }));
    }

    #[test]
    fn test_def_sigil() {
        let q = parse("*function").unwrap();
        assert!(matches!(q.head.node, SelectorNode::Bare(Atom { sigil: Some(Sigil::Def), .. })));
    }

    #[test]
    fn test_ref_sigil() {
        let q = parse("&function").unwrap();
        assert!(matches!(q.head.node, SelectorNode::Bare(Atom { sigil: Some(Sigil::Ref), .. })));
    }

    #[test]
    fn test_name_match_bare() {
        let q = parse("&function#sizeof").unwrap();
        let atom = match &q.head.node {
            SelectorNode::Bare(a) => a,
            _ => panic!(),
        };
        assert_eq!(atom.name_match, Some(NameMatch::Exact("sizeof".into())));
    }

    #[test]
    fn test_group_with_capture() {
        let q = parse("(&function#sizeof @f)").unwrap();
        match &q.head.node {
            SelectorNode::Group { query, capture } => {
                assert_eq!(capture.as_deref(), Some("f"));
                let inner_atom = match &query.head.node {
                    SelectorNode::Bare(a) => a,
                    _ => panic!(),
                };
                assert_eq!(inner_atom.sigil, Some(Sigil::Ref));
                assert_eq!(inner_atom.name_match, Some(NameMatch::Exact("sizeof".into())));
            }
            _ => panic!("expected Group"),
        }
    }

    #[test]
    fn test_field_access() {
        // Captures require () grouping per grammar; use explicit parens for @v
        let q = parse("(&function#malloc @f).params((var#ARRAY_SIZE @v))").unwrap();
        assert_eq!(q.head.fields.len(), 1);
        assert_eq!(q.head.fields[0].field, "params");
    }

    #[test]
    fn test_descendant_combinator() {
        let q = parse("(*function#main @def).body((&function#malloc @call))").unwrap();
        assert!(q.tail.is_empty()); // all encoded in field access
        assert_eq!(q.head.fields[0].field, "body");
    }

    #[test]
    fn test_passthrough() {
        let q = parse("arrow_function").unwrap();
        let atom = match &q.head.node {
            SelectorNode::Bare(a) => a,
            _ => panic!(),
        };
        assert_eq!(atom.node_type.as_deref(), Some("arrow_function"));
        assert_eq!(atom.sigil, None);
    }

    #[test]
    fn test_regex_match() {
        let q = parse("(&function#/GFILE|gfile|GFile/ @f)").unwrap();
        let inner = match &q.head.node {
            SelectorNode::Group { query, .. } => query,
            _ => panic!(),
        };
        let atom = match &inner.head.node {
            SelectorNode::Bare(a) => a,
            _ => panic!(),
        };
        assert_eq!(atom.name_match, Some(NameMatch::Regex("GFILE|gfile|GFile".into())));
    }

    fn atom_of(q: &Query) -> &Atom {
        match &q.head.node {
            SelectorNode::Bare(a) => a,
            _ => panic!("expected bare atom"),
        }
    }

    fn attr(key: &str, op: AttrOp, value: &str) -> AttrMatch {
        AttrMatch { key: key.into(), op, value: value.into() }
    }

    #[test]
    fn attribute_operators() {
        for (src, op) in [
            ("function[from=log]", AttrOp::Eq),
            ("function[from^=log]", AttrOp::Prefix),
            ("function[from*=log]", AttrOp::Contains),
            ("function[from$=log]", AttrOp::Suffix),
        ] {
            let q = parse(src).unwrap();
            assert_eq!(atom_of(&q).attrs, vec![attr("from", op, "log")], "{}", src);
        }
    }

    #[test]
    fn attribute_values_quoted_and_regex() {
        let q = parse("function[from=\"log/slog\"]").unwrap();
        assert_eq!(atom_of(&q).attrs, vec![attr("from", AttrOp::Eq, "log/slog")]);
        let q = parse("function[from=/^log.*$/]").unwrap();
        assert_eq!(atom_of(&q).attrs, vec![attr("from", AttrOp::Regex, "^log.*$")]);
        assert!(parse("function[from^=/x/]").is_err());
    }

    #[test]
    fn several_attributes_and_whitespace_inside_brackets() {
        let q = parse("*function#x[ from = log ][in=Server]").unwrap();
        let a = atom_of(&q);
        assert_eq!(a.sigil, Some(Sigil::Def));
        assert_eq!(a.attrs, vec![attr("from", AttrOp::Eq, "log"), attr("in", AttrOp::Eq, "Server")]);
    }

    #[test]
    fn contains_eq_does_not_break_the_def_sigil() {
        let q = parse("*function#x").unwrap();
        assert_eq!(atom_of(&q).sigil, Some(Sigil::Def));
        let q = parse("*function[a*=b]").unwrap();
        let a = atom_of(&q);
        assert_eq!(a.sigil, Some(Sigil::Def));
        assert_eq!(a.attrs, vec![attr("a", AttrOp::Contains, "b")]);
    }

    #[test]
    fn malformed_attributes_are_errors() {
        for src in ["function[", "function[from]", "function[from=]", "function[from=x", "function[=x]"] {
            assert!(parse(src).is_err(), "{}", src);
        }
    }

    #[test]
    fn dot_shorthand_becomes_qual_attribute() {
        let q = parse("function#log.Printf").unwrap();
        let a = atom_of(&q);
        assert_eq!(a.name_match, Some(NameMatch::Exact("Printf".into())));
        assert_eq!(a.attrs, vec![attr("qual", AttrOp::Eq, "log")]);
        assert!(q.head.fields.is_empty());
    }

    #[test]
    fn dot_shorthand_leaves_field_access_alone() {
        let q = parse("function#log.body()").unwrap();
        let a = atom_of(&q);
        assert_eq!(a.name_match, Some(NameMatch::Exact("log".into())));
        assert!(a.attrs.is_empty());
        assert_eq!(q.head.fields[0].field, "body");
    }

    #[test]
    fn dot_shorthand_then_field_access() {
        let q = parse("function#log.Printf.body(variable#x)").unwrap();
        assert_eq!(atom_of(&q).name_match, Some(NameMatch::Exact("Printf".into())));
        assert_eq!(atom_of(&q).attrs, vec![attr("qual", AttrOp::Eq, "log")]);
        assert_eq!(q.head.fields[0].field, "body");
    }

    #[test]
    fn dot_shorthand_composes_with_brackets() {
        let q = parse("field#User.Email[in=Account]").unwrap();
        assert_eq!(
            atom_of(&q).attrs,
            vec![attr("qual", AttrOp::Eq, "User"), attr("in", AttrOp::Eq, "Account")]
        );
    }

    #[test]
    fn quoted_name_keeps_its_dot() {
        let q = parse("id#\"a.b\"").unwrap();
        assert_eq!(atom_of(&q).name_match, Some(NameMatch::Exact("a.b".into())));
        assert!(atom_of(&q).attrs.is_empty());
    }

    #[test]
    fn semantic_combinators_lex_and_parse() {
        for (src, c) in [
            ("a -> b", Combinator::Calls),
            ("a --> b", Combinator::CallsEventually),
            ("a <- b", Combinator::CalledBy),
            ("a <-- b", Combinator::CalledByEventually),
            ("a => b", Combinator::Flows),
            ("a ==> b", Combinator::FlowsEventually),
            ("a <= b", Combinator::FlowsFrom),
            ("a <== b", Combinator::FlowsFromEventually),
            ("a-->b", Combinator::CallsEventually),
            ("a==>b", Combinator::FlowsEventually),
        ] {
            let q = parse(src).unwrap_or_else(|e| panic!("{}: {}", src, e));
            assert_eq!(q.tail.len(), 1, "{}", src);
            assert_eq!(q.tail[0].0, c, "{}", src);
        }
    }

    #[test]
    fn semantic_combinators_chain_and_mix_with_ast_ones() {
        let q = parse("function#a variable#x ==> function#b --> function#c").unwrap();
        let cs: Vec<_> = q.tail.iter().map(|(c, _)| c.clone()).collect();
        assert_eq!(cs, vec![Combinator::Descendant, Combinator::FlowsEventually, Combinator::CallsEventually]);
    }

    #[test]
    fn a_semantic_combinator_inside_a_group_parses() {
        let q = parse("(function#a --> function#b)").unwrap();
        match &q.head.node {
            SelectorNode::Group { query, .. } => assert_eq!(query.tail[0].0, Combinator::CallsEventually),
            _ => panic!(),
        }
    }

    #[test]
    fn not_parses_onto_the_selector() {
        let q = parse("function#a:not(function#b)").unwrap();
        assert_eq!(q.head.not.len(), 1);
        match &q.head.not[0].node {
            SelectorNode::Bare(a) => assert_eq!(a.name_match, Some(NameMatch::Exact("b".into()))),
            _ => panic!(),
        }
        let q = parse("function#a:not(function#b):not(function#c)").unwrap();
        assert_eq!(q.head.not.len(), 2);
        let q = parse("function:not(function#b) variable#x").unwrap();
        assert_eq!(q.head.not.len(), 1);
        assert_eq!(q.tail.len(), 1);
    }

    #[test]
    fn unsupported_pseudo_classes_and_bad_not_are_errors() {
        for src in ["function:has(x)", "function:", "function:not", "function:not(", "function:not(a"] {
            assert!(parse(src).is_err(), "{}", src);
        }
    }

    fn name_of(src: &str) -> Option<NameMatch> {
        atom_of(&parse(src).unwrap_or_else(|e| panic!("{}: {}", src, e))).name_match.clone()
    }

    #[test]
    fn a_regex_filter_holds_commas_pipes_quotes_and_escaped_slashes() {
        assert_eq!(name_of("id#/a,b|c/"), Some(NameMatch::Regex("a,b|c".into())));
        assert_eq!(name_of("literal#/call\"/"), Some(NameMatch::Regex("call\"".into())));
        assert_eq!(name_of("id#/a\\/b/"), Some(NameMatch::Regex("a/b".into())));
        assert_eq!(name_of("id#/\\/ui\\/page/"), Some(NameMatch::Regex("/ui/page".into())));
    }

    #[test]
    fn other_regex_escapes_reach_the_regex_untouched() {
        assert_eq!(name_of("id#/\\d+\\.x/"), Some(NameMatch::Regex("\\d+\\.x".into())));
        assert_eq!(name_of("id#/a\\\\/"), Some(NameMatch::Regex("a\\\\".into())));
    }

    #[test]
    fn a_quoted_filter_holds_slashes_commas_pipes_and_escaped_quotes() {
        assert_eq!(name_of("id#\"a/b\""), Some(NameMatch::Exact("a/b".into())));
        assert_eq!(name_of("id#\"a,b|c\""), Some(NameMatch::Exact("a,b|c".into())));
        assert_eq!(name_of("id#\"say \\\"hi\\\"\""), Some(NameMatch::Exact("say \"hi\"".into())));
        assert_eq!(name_of("id#\"a\\\\b\""), Some(NameMatch::Exact("a\\b".into())));
    }

    #[test]
    fn filters_with_odd_characters_still_close_around_the_rest_of_the_query() {
        let q = parse("(*function#/a,b|c\\/d/ @f).body(id#\"x/y\")").unwrap();
        assert_eq!(q.head.fields[0].field, "body");
        let q = parse("id#/a\"b/ > id#\"c,d\"").unwrap();
        assert_eq!(q.tail.len(), 1);
    }

    #[test]
    fn errors_carry_the_byte_position_of_the_fault() {
        let e = parse("function#(").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (9, "expected a name after #"));
        let e = parse("function ! x").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (9, "unexpected character '!'"));
        let e = parse("(function#a").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (11, "expected )"));
        assert_eq!(e.hint.as_deref(), Some("close the group: (function#a)"));
        let e = parse("function[from").unwrap_err();
        assert_eq!(e.pos, 13);
        let e = parse("function:has(x)").unwrap_err();
        assert_eq!(e.pos, 9);
        assert!(e.hint.is_some());
    }

    #[test]
    fn positions_are_bytes_so_they_follow_multibyte_names() {
        let e = parse("id#\"é\" !").unwrap_err();
        assert_eq!(e.pos, 8);
    }

    #[test]
    fn an_unterminated_filter_says_so_and_offers_the_closing_delimiter() {
        let e = parse("id#/abc").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (3, "unterminated regex"));
        assert_eq!(e.hint.as_deref(), Some("close it with a '/': id#/abc/"));
        let e = parse("id#\"abc").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (3, "unterminated quoted name"));
        let e = parse("id#/abc\\/").unwrap_err();
        assert_eq!(e.msg, "unterminated regex", "an escaped slash does not close it");
    }

    #[test]
    fn an_unescaped_inner_slash_points_at_the_slash_and_suggests_the_escape() {
        let e = parse("id#/ui/page/").unwrap_err();
        assert_eq!((e.pos, e.msg.as_str()), (6, "unexpected '/'"));
        assert_eq!(
            e.hint.as_deref(),
            Some("a '/' inside a regex name filter must be escaped: id#/ui\\/page/")
        );
        let e = parse("(id#/a/b/c/ @x)").unwrap_err();
        assert_eq!(e.pos, 6, "{:?}", e);
        assert_eq!(
            e.hint.as_deref(),
            Some("a '/' inside a regex name filter must be escaped: (id#/a\\/b\\/c/ @x)")
        );
    }

    fn group_capture(q: &Query) -> (Option<&str>, &Query) {
        match &q.head.node {
            SelectorNode::Group { query, capture } => (capture.as_deref(), query),
            other => panic!("expected a group, got {:?}", other),
        }
    }

    #[test]
    fn a_bare_capture_is_a_field_argument_meaning_any_node() {
        let q = parse("pair.value(@target)").unwrap();
        let inner = q.head.fields[0].inner.as_ref().unwrap();
        let (cap, any) = group_capture(inner);
        assert_eq!(cap, Some("target"));
        assert_eq!(atom_of(any).node_type, None, "a wildcard: {:?}", any);
        assert_eq!(atom_of(any).name_match, None);
    }

    #[test]
    fn a_capture_after_a_field_query_needs_no_parentheses() {
        let bare = parse("pair.value(literal @key)").unwrap();
        let grouped = parse("pair.value((literal @key))").unwrap();
        assert_eq!(bare, grouped, "the same AST as the grouped form");
        let (cap, lit) = group_capture(bare.head.fields[0].inner.as_ref().unwrap());
        assert_eq!(cap, Some("key"));
        assert_eq!(atom_of(lit).node_type.as_deref(), Some("literal"));
        assert!(parse("pair.value(literal @)").is_err());
        assert!(parse("pair.value(@)").is_err());
    }

    #[test]
    fn captures_sit_on_both_sides_of_sibling_combinators() {
        for (src, c) in [
            ("(pair.key(id#path).value(literal @key)) ~ (pair.value(@target))", Combinator::Sibling),
            ("(id#a @x) + (id#b @y)", Combinator::Adjacent),
            ("(id#a @x)~(id#b @y)", Combinator::Sibling),
        ] {
            let q = parse(src).unwrap_or_else(|e| panic!("{}: {}", src, e));
            assert_eq!(q.tail.len(), 1, "{}", src);
            assert_eq!(q.tail[0].0, c, "{}", src);
        }
    }

    #[test]
    fn unknown_characters_are_errors() {
        for src in ["function ! x", "function%", "function^x", "function$x", "a - b", "a < b", "a == b", "function;"] {
            assert!(parse(src).is_err(), "{}", src);
        }
    }
}
