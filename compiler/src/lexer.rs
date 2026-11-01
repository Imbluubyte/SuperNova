//! Lexer: turns SuperNova source into tokens, including INDENT/DEDENT.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    /// Index of the source file (0 = the entry file).
    pub file: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FPart {
    Text(String),
    /// Source text of an interpolated expression, with the span of its start.
    Expr(String, Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FPart>),
    Ident(String),
    // keywords
    Var,
    Const,
    Fn,
    Return,
    If,
    Orif,
    Else,
    While,
    For,
    In,
    Match,
    Case,
    Default,
    And,
    Or,
    Not,
    True,
    False,
    Null,
    Class,
    Extends,
    Try,
    Catch,
    Finally,
    Throw,
    Async,
    Await,
    Use,
    Has,
    As,
    Private,
    Manages,
    Break,
    Continue,
    Pass,
    // punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Arrow,
    Plus,
    Minus,
    Star,
    Slash,
    SlashSlash,
    Percent,
    StarStar,
    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    DotDot,
    DotDotLt,
    Question,
    /// `??`
    QQ,
    /// postfix `!`
    Bang,
    // layout
    Newline,
    Indent,
    Dedent,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "identifier '{}'", s),
            Tok::Int(n) => write!(f, "{}", n),
            Tok::Float(n) => write!(f, "{}", n),
            Tok::Str(_) | Tok::FStr(_) => write!(f, "string"),
            Tok::Newline => write!(f, "end of line"),
            Tok::Indent => write!(f, "indent"),
            Tok::Dedent => write!(f, "dedent"),
            Tok::Eof => write!(f, "end of file"),
            other => write!(f, "{:?}", other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub span: Span,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, span: Span) -> Self {
        CompileError { msg: msg.into(), span }
    }
}

fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "var" => Tok::Var,
        "const" => Tok::Const,
        "fn" => Tok::Fn,
        "return" => Tok::Return,
        "if" => Tok::If,
        "orif" => Tok::Orif,
        "else" => Tok::Else,
        "while" => Tok::While,
        "for" => Tok::For,
        "in" => Tok::In,
        "has" => Tok::Has,
        "match" => Tok::Match,
        "case" => Tok::Case,
        "default" => Tok::Default,
        "and" => Tok::And,
        "or" => Tok::Or,
        "not" => Tok::Not,
        "true" => Tok::True,
        "false" => Tok::False,
        "null" => Tok::Null,
        "class" => Tok::Class,
        "extends" => Tok::Extends,
        "try" => Tok::Try,
        "catch" => Tok::Catch,
        "finally" => Tok::Finally,
        "throw" => Tok::Throw,
        "async" => Tok::Async,
        "await" => Tok::Await,
        "use" => Tok::Use,
        "as" => Tok::As,
        "private" => Tok::Private,
        "manages" => Tok::Manages,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "pass" => Tok::Pass,
        _ => return None,
    })
}

#[allow(dead_code)]
pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    lex_file(src, 0)
}

pub fn lex_file(src: &str, file: u16) -> Result<Vec<Token>, CompileError> {
    let mut l = Lexer::new(src);
    l.file = file;
    l.run()
}

struct Lexer<'a> {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
    paren_depth: i32,
    indents: Vec<usize>,
    out: Vec<Token>,
    at_line_start: bool,
    file: u16,
    indent_char: Option<char>,
    _src: &'a str,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            paren_depth: 0,
            indents: vec![0],
            out: Vec::new(),
            at_line_start: true,
            file: 0,
            indent_char: None,
            _src: src,
        }
    }

    fn span(&self) -> Span {
        Span { line: self.line, col: self.col, file: self.file }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn push(&mut self, tok: Tok, span: Span) {
        self.out.push(Token { tok, span });
    }

    fn run(mut self) -> Result<Vec<Token>, CompileError> {
        loop {
            if self.at_line_start && self.paren_depth == 0 {
                if !self.handle_indent()? {
                    break;
                }
                self.at_line_start = false;
            }
            let Some(c) = self.peek() else { break };
            let sp = self.span();
            match c {
                ' ' | '\t' | '\r' => {
                    self.bump();
                }
                '#' => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                '\\' if self.peek_at(1) == Some('\n') => {
                    self.bump();
                    self.bump();
                }
                '\n' => {
                    self.bump();
                    if self.paren_depth == 0 {
                        if !matches!(
                            self.out.last().map(|t| &t.tok),
                            Some(Tok::Newline) | Some(Tok::Indent) | Some(Tok::Dedent) | None
                        ) {
                            self.push(Tok::Newline, sp);
                        }
                        self.at_line_start = true;
                    }
                }
                '0'..='9' => self.number()?,
                'a'..='z' | 'A'..='Z' | '_' => {
                    if (c == 'f') && matches!(self.peek_at(1), Some('"')) {
                        self.bump();
                        self.string(true, sp)?;
                    } else {
                        self.ident();
                    }
                }
                '"' => self.string(false, sp)?,
                _ => self.punct()?,
            }
        }
        let sp = self.span();
        if !matches!(self.out.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
            self.push(Tok::Newline, sp);
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push(Tok::Dedent, sp);
        }
        self.push(Tok::Eof, sp);
        Ok(self.out)
    }

    /// Measure indentation at the start of a line. Returns false at EOF.
    fn handle_indent(&mut self) -> Result<bool, CompileError> {
        loop {
            let mut width = 0usize;
            let save = (self.pos, self.line, self.col);
            let mut seen: Option<char> = None;
            while let Some(c) = self.peek() {
                match c {
                    ' ' | '\t' => {
                        if seen.map_or(false, |k| k != c) {
                            return Err(CompileError::new(
                                "this line mixes tabs and spaces in its indentation; use one or the other",
                                self.span(),
                            ));
                        }
                        seen = Some(c);
                        width += 1;
                    }
                    _ => break,
                }
                self.bump();
            }
            match self.peek() {
                None => return Ok(false),
                Some('\n') | Some('\r') => {
                    self.bump();
                    continue;
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                    continue;
                }
                _ => {}
            }
            let _ = save;
            let sp = self.span();
            if let Some(k) = seen {
                // A file indents with tabs or with spaces, never both.
                match self.indent_char {
                    None => self.indent_char = Some(k),
                    Some(first) if first != k => {
                        return Err(CompileError::new(
                            if first == '\t' {
                                "this file indents with tabs, but this line is indented with spaces"
                            } else {
                                "this file indents with spaces, but this line is indented with a tab"
                            },
                            sp,
                        ))
                    }
                    _ => {}
                }
            }
            let cur = *self.indents.last().unwrap();
            if width > cur {
                self.indents.push(width);
                self.push(Tok::Indent, sp);
            } else if width < cur {
                while *self.indents.last().unwrap() > width {
                    self.indents.pop();
                    self.push(Tok::Dedent, sp);
                }
                if *self.indents.last().unwrap() != width {
                    return Err(CompileError::new("inconsistent indentation", sp));
                }
            }
            return Ok(true);
        }
    }

    fn number(&mut self) -> Result<(), CompileError> {
        let sp = self.span();
        let mut s = String::new();
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                if c != '_' {
                    s.push(c);
                }
                self.bump();
            } else if c == '.'
                && !is_float
                && self.peek_at(1).map_or(false, |d| d.is_ascii_digit())
            {
                is_float = true;
                s.push(c);
                self.bump();
            } else if (c == 'e' || c == 'E')
                && self
                    .peek_at(1)
                    .map_or(false, |d| d.is_ascii_digit() || d == '-' || d == '+')
            {
                is_float = true;
                s.push(c);
                self.bump();
                s.push(self.bump().unwrap());
            } else {
                break;
            }
        }
        if is_float {
            let v = s
                .parse::<f64>()
                .map_err(|_| CompileError::new("invalid float literal", sp))?;
            self.push(Tok::Float(v), sp);
        } else {
            let v = s
                .parse::<i64>()
                .map_err(|_| CompileError::new("integer literal out of range for Int", sp))?;
            self.push(Tok::Int(v), sp);
        }
        Ok(())
    }

    fn ident(&mut self) {
        let sp = self.span();
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let tok = keyword(&s).unwrap_or(Tok::Ident(s));
        self.push(tok, sp);
    }

    fn string(&mut self, fstring: bool, sp: Span) -> Result<(), CompileError> {
        self.bump(); // opening quote
        let mut parts: Vec<FPart> = Vec::new();
        let mut cur = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(CompileError::new("unterminated string literal", sp));
            };
            match c {
                '"' => break,
                '\n' => return Err(CompileError::new("unterminated string literal", sp)),
                '\\' => {
                    let Some(e) = self.bump() else {
                        return Err(CompileError::new("unterminated string literal", sp));
                    };
                    cur.push(match e {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        '\\' => '\\',
                        '"' => '"',
                        '{' => '{',
                        '}' => '}',
                        other => {
                            return Err(CompileError::new(
                                format!("unknown escape sequence '\\{}'", other),
                                self.span(),
                            ))
                        }
                    });
                }
                '{' if fstring => {
                    if self.peek() == Some('{') {
                        self.bump();
                        cur.push('{');
                        continue;
                    }
                    if !cur.is_empty() {
                        parts.push(FPart::Text(std::mem::take(&mut cur)));
                    }
                    let esp = self.span();
                    let mut depth = 1;
                    let mut expr = String::new();
                    loop {
                        let Some(c) = self.bump() else {
                            return Err(CompileError::new("unterminated f-string", sp));
                        };
                        match c {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            '"' => {
                                return Err(CompileError::new(
                                    "string quotes are not allowed inside f-string expressions in V1",
                                    self.span(),
                                ))
                            }
                            _ => {}
                        }
                        expr.push(c);
                    }
                    if expr.trim().is_empty() {
                        return Err(CompileError::new("empty expression in f-string", esp));
                    }
                    parts.push(FPart::Expr(expr, esp));
                }
                '}' if fstring => {
                    if self.peek() == Some('}') {
                        self.bump();
                    }
                    cur.push('}');
                }
                c => cur.push(c),
            }
        }
        if fstring {
            if !cur.is_empty() {
                parts.push(FPart::Text(cur));
            }
            self.push(Tok::FStr(parts), sp);
        } else {
            self.push(Tok::Str(cur), sp);
        }
        Ok(())
    }

    fn punct(&mut self) -> Result<(), CompileError> {
        let sp = self.span();
        let c = self.bump().unwrap();
        let next = self.peek();
        let mut two = |s: &mut Self, t: Tok| {
            s.bump();
            t
        };
        let tok = match (c, next) {
            ('(', _) => {
                self.paren_depth += 1;
                Tok::LParen
            }
            (')', _) => {
                self.paren_depth -= 1;
                Tok::RParen
            }
            ('[', _) => {
                self.paren_depth += 1;
                Tok::LBracket
            }
            (']', _) => {
                self.paren_depth -= 1;
                Tok::RBracket
            }
            ('{', _) => {
                self.paren_depth += 1;
                Tok::LBrace
            }
            ('}', _) => {
                self.paren_depth -= 1;
                Tok::RBrace
            }
            (',', _) => Tok::Comma,
            (':', _) => Tok::Colon,
            ('?', Some('?')) => two(self, Tok::QQ),
            ('?', _) => Tok::Question,
            ('.', Some('.')) => {
                self.bump();
                if self.peek() == Some('<') {
                    self.bump();
                    Tok::DotDotLt
                } else {
                    Tok::DotDot
                }
            }
            ('.', _) => Tok::Dot,
            ('-', Some('>')) => two(self, Tok::Arrow),
            ('+', Some('=')) => two(self, Tok::PlusEq),
            ('-', Some('=')) => two(self, Tok::MinusEq),
            ('*', Some('*')) => two(self, Tok::StarStar),
            ('*', Some('=')) => two(self, Tok::StarEq),
            ('/', Some('/')) => two(self, Tok::SlashSlash),
            ('/', Some('=')) => two(self, Tok::SlashEq),
            ('%', Some('=')) => two(self, Tok::PercentEq),
            ('=', Some('=')) => two(self, Tok::EqEq),
            ('!', Some('=')) => two(self, Tok::NotEq),
            ('<', Some('=')) => two(self, Tok::LtEq),
            ('>', Some('=')) => two(self, Tok::GtEq),
            ('!', _) => Tok::Bang,
            ('+', _) => Tok::Plus,
            ('-', _) => Tok::Minus,
            ('*', _) => Tok::Star,
            ('/', _) => Tok::Slash,
            ('%', _) => Tok::Percent,
            ('=', _) => Tok::Assign,
            ('<', _) => Tok::Lt,
            ('>', _) => Tok::Gt,
            (';', _) => {
                return Err(CompileError::new(
                    "semicolons are not part of SuperNova; use a newline",
                    sp,
                ))
            }
            (other, _) => {
                return Err(CompileError::new(
                    format!("unexpected character '{}'", other),
                    sp,
                ))
            }
        };
        self.push(tok, sp);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn indentation() {
        let t = toks("if x:\n    y\nz\n");
        assert_eq!(
            t,
            vec![
                Tok::If,
                Tok::Ident("x".into()),
                Tok::Colon,
                Tok::Newline,
                Tok::Indent,
                Tok::Ident("y".into()),
                Tok::Newline,
                Tok::Dedent,
                Tok::Ident("z".into()),
                Tok::Newline,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn parens_suppress_newlines() {
        let t = toks("f(1,\n  2)\n");
        assert!(!t[..t.len() - 2].contains(&Tok::Newline));
    }

    #[test]
    fn fstring_parts() {
        let t = toks("f\"Hi {name}!\"");
        match &t[0] {
            Tok::FStr(p) => assert_eq!(p.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn semicolon_rejected() {
        assert!(lex("var x = 1;\n").is_err());
    }

    #[test]
    fn orif_is_keyword_elif_is_not() {
        assert_eq!(toks("orif")[0], Tok::Orif);
        assert_eq!(toks("elif")[0], Tok::Ident("elif".into()));
    }
}
