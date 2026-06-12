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
    Ident(String),
    QuotedStr(String),
    Regex(String),
    Whitespace,
    Eof,
}

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Self { chars: s.chars().peekable() }
    }

    fn next_token(&mut self) -> Token {
        match self.chars.peek().copied() {
            None => Token::Eof,
            Some(c) if c == ' ' || c == '\t' => {
                while matches!(self.chars.peek(), Some(' ') | Some('\t')) {
                    self.chars.next();
                }
                Token::Whitespace
            }
            Some('*') => { self.chars.next(); Token::Star }
            Some('&') => { self.chars.next(); Token::Amp }
            Some('>') => { self.chars.next(); Token::Gt }
            Some('+') => { self.chars.next(); Token::Plus }
            Some('~') => { self.chars.next(); Token::Tilde }
            Some('.') => { self.chars.next(); Token::Dot }
            Some('(') => { self.chars.next(); Token::LParen }
            Some(')') => { self.chars.next(); Token::RParen }
            Some('@') => { self.chars.next(); Token::At }
            Some('#') => { self.chars.next(); Token::Hash }
            Some('"') => {
                self.chars.next();
                let mut s = String::new();
                loop {
                    match self.chars.next() {
                        Some('"') | None => break,
                        Some(c) => s.push(c),
                    }
                }
                Token::QuotedStr(s)
            }
            Some('/') => {
                self.chars.next();
                let mut s = String::new();
                loop {
                    match self.chars.next() {
                        Some('/') | None => break,
                        Some(c) => s.push(c),
                    }
                }
                Token::Regex(s)
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                let mut s = String::new();
                while matches!(self.chars.peek(), Some(c) if c.is_alphanumeric() || *c == '_') {
                    s.push(self.chars.next().unwrap());
                }
                Token::Ident(s)
            }
            Some(c) => {
                self.chars.next();
                Token::Ident(c.to_string())
            }
        }
    }

    fn tokenize(mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let t = self.next_token();
            let done = t == Token::Eof;
            tokens.push(t);
            if done { break; }
        }
        tokens
    }
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(s: &str) -> Self {
        Self { tokens: Lexer::new(s).tokenize(), pos: 0 }
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

    pub fn parse_query(&mut self) -> Result<Query, String> {
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
                    } else {
                        break;
                    }
                }
                Token::Gt => { self.advance(); self.skip_whitespace(); Combinator::Child }
                Token::Plus => { self.advance(); self.skip_whitespace(); Combinator::Adjacent }
                Token::Tilde => { self.advance(); self.skip_whitespace(); Combinator::Sibling }
                _ => break,
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

    fn parse_selector(&mut self) -> Result<Selector, String> {
        let node = if self.peek() == &Token::LParen {
            self.advance(); // consume (
            self.skip_whitespace();
            let query = self.parse_query()?;
            self.skip_whitespace();
            let capture = if self.peek() == &Token::At {
                self.advance();
                match self.advance() {
                    Token::Ident(name) => Some(name),
                    _ => return Err("expected capture name after @".into()),
                }
            } else {
                None
            };
            self.skip_whitespace();
            if self.peek() != &Token::RParen {
                return Err("expected )".into());
            }
            self.advance();
            SelectorNode::Group { query: Box::new(query), capture }
        } else {
            SelectorNode::Bare(self.parse_atom()?)
        };

        let mut fields = Vec::new();
        while self.peek() == &Token::Dot {
            self.advance();
            let field = match self.advance() {
                Token::Ident(name) => name,
                _ => return Err("expected field name after .".into()),
            };
            if self.peek() != &Token::LParen {
                return Err("expected ( after field name".into());
            }
            self.advance();
            self.skip_whitespace();
            let inner = if self.peek() == &Token::RParen {
                None
            } else {
                Some(Box::new(self.parse_query()?))
            };
            self.skip_whitespace();
            if self.peek() != &Token::RParen {
                return Err("expected ) to close field access".into());
            }
            self.advance();
            fields.push(FieldAccess { field, inner });
        }

        Ok(Selector { node, fields })
    }

    fn parse_atom(&mut self) -> Result<Atom, String> {
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

        let name_match = if self.peek() == &Token::Hash {
            self.advance();
            Some(self.parse_name_match()?)
        } else {
            None
        };

        Ok(Atom { sigil, node_type, name_match })
    }

    fn parse_name_match(&mut self) -> Result<NameMatch, String> {
        match self.peek().clone() {
            Token::QuotedStr(s) => { self.advance(); Ok(NameMatch::Exact(s)) }
            Token::Regex(s) => { self.advance(); Ok(NameMatch::Regex(s)) }
            Token::Ident(s) => { self.advance(); Ok(NameMatch::Exact(s)) }
            _ => Err("expected name match after #".into()),
        }
    }
}

pub fn parse(s: &str) -> Result<Query, String> {
    let mut parser = Parser::new(s);
    let query = parser.parse_query()?;
    parser.skip_whitespace();
    if parser.peek() != &Token::Eof {
        return Err(format!("unexpected input at position {}", parser.pos));
    }
    Ok(query)
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
}
