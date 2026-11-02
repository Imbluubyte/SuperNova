//! Recursive-descent parser with precedence climbing for expressions.

use crate::ast::*;
use crate::lexer::{lex_file, CompileError, FPart, Span, Tok, Token};

type PResult<T> = Result<T, CompileError>;

#[allow(dead_code)]
pub fn parse(src: &str) -> PResult<Module> {
    parse_file(src, 0)
}

pub fn parse_file(src: &str, file: u16) -> PResult<Module> {
    let toks = lex_file(src, file)?;
    let mut p = Parser { toks, pos: 0 };
    let mut items = Vec::new();
    p.skip_newlines();
    while !p.at(&Tok::Eof) {
        items.push(p.statement()?);
        p.skip_newlines();
    }
    Ok(Module { items })
}

fn parse_expr_str(src: &str, span: Span) -> PResult<Expr> {
    let toks = lex_file(src, span.file).map_err(|e| CompileError::new(e.msg, span))?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.expr().map_err(|e| CompileError::new(e.msg, span))?;
    p.skip_newlines();
    if !p.at(&Tok::Eof) {
        return Err(CompileError::new("unexpected token in f-string expression", span));
    }
    Ok(e)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos.min(self.toks.len() - 1)].span
    }
    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }
    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok, what: &str) -> PResult<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(CompileError::new(
                format!("expected {}, found {}", what, self.peek()),
                self.span(),
            ))
        }
    }
    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }
    /// `repeat(` ... `):` at the start of a statement (`repeat` stays usable as a method name).
    fn is_repeat_header(&self) -> bool {
        if self.peek_at(1) != &Tok::LParen {
            return false;
        }
        let mut depth = 0i32;
        let mut i = 1;
        loop {
            match self.peek_at(i) {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return self.peek_at(i + 1) == &Tok::Colon;
                    }
                }
                Tok::Eof | Tok::Newline => return false,
                _ => {}
            }
            i += 1;
        }
    }

    fn ident(&mut self, what: &str) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(CompileError::new(
                format!("expected {}, found {}", what, other),
                self.span(),
            )),
        }
    }

    /// `:` NEWLINE INDENT stmt+ DEDENT
    fn block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect(&Tok::Colon, "':'")?;
        if !self.at(&Tok::Newline) {
            // single-line block: `if x: return 1`
            let s = self.statement()?;
            return Ok(vec![s]);
        }
        self.skip_newlines();
        self.expect(&Tok::Indent, "an indented block")?;
        let mut body = Vec::new();
        self.skip_newlines();
        while !self.at(&Tok::Dedent) && !self.at(&Tok::Eof) {
            body.push(self.statement()?);
            self.skip_newlines();
        }
        self.expect(&Tok::Dedent, "end of block")?;
        Ok(body)
    }

    fn end_of_stmt(&mut self) -> PResult<()> {
        if self.at(&Tok::Newline) {
            self.bump();
            Ok(())
        } else if matches!(self.peek(), Tok::Eof | Tok::Dedent) {
            Ok(())
        } else {
            Err(CompileError::new(
                format!("expected end of line, found {}", self.peek()),
                self.span(),
            ))
        }
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let span = self.span();
        let kind = match self.peek().clone() {
            Tok::Private => {
                self.bump();
                match self.peek() {
                    Tok::Fn | Tok::Async => {
                        let mut f = self.fn_decl()?;
                        f.is_private = true;
                        StmtKind::Fn(f)
                    }
                    Tok::Var | Tok::Const => self.var_decl(true)?,
                    Tok::Class => {
                        let mut c = self.class_decl()?;
                        c.is_private = true;
                        StmtKind::Class(c)
                    }
                    Tok::Enum => {
                        let mut e = self.enum_decl()?;
                        e.is_private = true;
                        StmtKind::Enum(e)
                    }
                    _ => {
                        return Err(CompileError::new(
                            "'private' must be followed by fn, var, const or class",
                            self.span(),
                        ))
                    }
                }
            }
            Tok::Ident(ref n) if (n == "extern" || n == "export") && self.peek_at(1) == &Tok::Fn => {
                let is_extern = n == "extern";
                self.bump();
                let mut f = self.fn_decl_with(is_extern)?;
                f.is_extern = is_extern;
                f.is_export = !is_extern;
                StmtKind::Fn(f)
            }
            Tok::Ident(ref n) if n == "export" && self.peek_at(1) == &Tok::Class => {
                self.bump();
                let mut c = self.class_decl()?;
                c.is_export = true;
                StmtKind::Class(c)
            }
            Tok::Ident(ref n) if n == "elif" => {
                return Err(CompileError::new(
                    "'elif' is not part of SuperNova; use 'orif'",
                    span,
                ))
            }
            Tok::Ident(ref n) if n == "repeat" && self.is_repeat_header() => {
                self.bump();
                self.expect(&Tok::LParen, "'('")?;
                let count = self.expr()?;
                self.expect(&Tok::RParen, "')'")?;
                let body = self.block()?;
                StmtKind::Repeat { count, body }
            }
            Tok::Var | Tok::Const => self.var_decl(false)?,
            Tok::Fn | Tok::Async => StmtKind::Fn(self.fn_decl()?),
            Tok::Class => StmtKind::Class(self.class_decl()?),
            Tok::Enum => StmtKind::Enum(self.enum_decl()?),
            Tok::If => self.if_stmt()?,
            Tok::While => {
                self.bump();
                let cond = self.expr()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            Tok::For => {
                self.bump();
                let var = self.ident("loop variable")?;
                self.expect(&Tok::In, "'in'")?;
                let iter = self.expr()?;
                let body = self.block()?;
                StmtKind::For { var, iter, body }
            }
            Tok::Match => self.match_stmt()?,
            Tok::Return => {
                self.bump();
                let v = if matches!(self.peek(), Tok::Newline | Tok::Eof | Tok::Dedent) {
                    None
                } else {
                    Some(self.expr_or_tuple()?)
                };
                self.end_of_stmt()?;
                StmtKind::Return(v)
            }
            Tok::Break => {
                self.bump();
                self.end_of_stmt()?;
                StmtKind::Break
            }
            Tok::Continue => {
                self.bump();
                self.end_of_stmt()?;
                StmtKind::Continue
            }
            Tok::Pass => {
                self.bump();
                self.end_of_stmt()?;
                StmtKind::Pass
            }
            Tok::Try => self.try_stmt()?,
            Tok::Throw => {
                self.bump();
                let e = self.expr()?;
                self.end_of_stmt()?;
                StmtKind::Throw(e)
            }
            Tok::Use => self.use_stmt()?,
            Tok::Manages => {
                self.bump();
                let n = self.ident("a name after 'manages'")?;
                self.end_of_stmt()?;
                StmtKind::Manages(n)
            }
            _ => self.simple_stmt()?,
        };
        Ok(Stmt { kind, span })
    }

    fn simple_stmt(&mut self) -> PResult<StmtKind> {
        let target = self.expr_or_tuple()?;
        let op = match self.peek() {
            Tok::Assign => Some(None),
            Tok::PlusEq => Some(Some(BinOp::Add)),
            Tok::MinusEq => Some(Some(BinOp::Sub)),
            Tok::StarEq => Some(Some(BinOp::Mul)),
            Tok::SlashEq => Some(Some(BinOp::Div)),
            Tok::PercentEq => Some(Some(BinOp::Mod)),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            match target.kind {
                ExprKind::Ident(_) | ExprKind::Index(..) | ExprKind::Member(..) => {}
                _ => return Err(CompileError::new("invalid assignment target", target.span)),
            }
            let value = self.expr_or_tuple()?;
            self.end_of_stmt()?;
            Ok(StmtKind::Assign { target, op, value })
        } else {
            self.end_of_stmt()?;
            Ok(StmtKind::Expr(target))
        }
    }

    fn var_decl(&mut self, is_private: bool) -> PResult<StmtKind> {
        let is_const = self.bump() == Tok::Const;
        let name = self.ident("a variable name")?;
        let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
        self.expect(&Tok::Assign, "'=' (variables must be initialised)")?;
        let init = self.expr_or_tuple()?;
        self.end_of_stmt()?;
        Ok(StmtKind::VarDecl { name, is_const, is_private, ty, init })
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let name = match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                s
            }
            Tok::Null => {
                self.bump();
                "null".to_string()
            }
            other => {
                return Err(CompileError::new(
                    format!("expected a type, found {}", other),
                    self.span(),
                ))
            }
        };
        let mut t = if name == "List" && self.eat(&Tok::LBracket) {
            let inner = self.type_expr()?;
            self.expect(&Tok::RBracket, "']'")?;
            TypeExpr::List(Box::new(inner))
        } else if name == "Fn" && self.eat(&Tok::LParen) {
            let mut ps = Vec::new();
            while !self.at(&Tok::RParen) {
                ps.push(self.type_expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen, "')'")?;
            let ret = if self.eat(&Tok::Arrow) { Some(Box::new(self.type_expr()?)) } else { None };
            TypeExpr::Fn(ps, ret)
        } else if name == "Task" && self.eat(&Tok::LBracket) {
            let inner = self.type_expr()?;
            self.expect(&Tok::RBracket, "']'")?;
            TypeExpr::Task(Box::new(inner))
        } else if name == "Dict" && self.eat(&Tok::LBracket) {
            let k = self.type_expr()?;
            self.expect(&Tok::Comma, "',' between the key and value types")?;
            let v = self.type_expr()?;
            self.expect(&Tok::RBracket, "']'")?;
            TypeExpr::Dict(Box::new(k), Box::new(v))
        } else if name == "Tuple" && self.eat(&Tok::LBracket) {
            let mut items = vec![self.type_expr()?];
            while self.eat(&Tok::Comma) {
                items.push(self.type_expr()?);
            }
            self.expect(&Tok::RBracket, "']'")?;
            TypeExpr::Tuple(items)
        } else {
            let mut name = name;
            while self.at(&Tok::Dot) {
                self.bump();
                name.push('.');
                name.push_str(&self.ident("a type name after '.'")?);
            }
            TypeExpr::Named(name)
        };
        if self.eat(&Tok::Question) {
            t = TypeExpr::Optional(Box::new(t));
        }
        Ok(t)
    }

    /// `name` or `module.name`.
    fn dotted_name(&mut self, what: &str) -> PResult<String> {
        let mut n = self.ident(what)?;
        while self.at(&Tok::Dot) {
            self.bump();
            n.push('.');
            n.push_str(&self.ident("a name after '.'")?);
        }
        Ok(n)
    }

    fn fn_decl(&mut self) -> PResult<FnDecl> {
        self.fn_decl_with(false)
    }

    fn fn_decl_with(&mut self, is_extern: bool) -> PResult<FnDecl> {
        let span = self.span();
        let is_async = self.eat(&Tok::Async);
        self.expect(&Tok::Fn, "'fn'")?;
        let name = self.ident("a function name")?;
        self.expect(&Tok::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.at(&Tok::RParen) {
            let pname = match self.peek() {
                Tok::Ident(_) => self.ident("a parameter name")?,
                other => {
                    return Err(CompileError::new(
                        format!(
                            "expected a parameter name, found {}; \
                             literal parameters such as onTimer(1.5) are an engine \
                             convention and are not part of the core language",
                            other
                        ),
                        self.span(),
                    ))
                }
            };
            let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
            let default = if self.eat(&Tok::Assign) { Some(self.expr()?) } else { None };
            params.push(Param { name: pname, ty, default });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        let body = if is_extern { Vec::new() } else { self.block()? };
        Ok(FnDecl { name, params, ret, body, is_async, is_private: false, is_export: false, is_extern, span })
    }

    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let span = self.span();
        self.expect(&Tok::Enum, "'enum'")?;
        let name = self.ident("an enum name")?;
        let body = self.block()?;
        let mut variants: Vec<(String, i64)> = Vec::new();
        let mut next = 0i64;
        for s in body {
            let (vname, value) = match s.kind {
                StmtKind::Expr(Expr { kind: ExprKind::Ident(n), .. }) => (n, next),
                StmtKind::Assign { target: Expr { kind: ExprKind::Ident(n), .. }, op: None, value } => match value.kind {
                    ExprKind::Int(v) => (n, v),
                    ExprKind::Unary(UnOp::Neg, ref x) if matches!(x.kind, ExprKind::Int(_)) => {
                        let ExprKind::Int(v) = x.kind else { unreachable!() };
                        (n, -v)
                    }
                    _ => return Err(CompileError::new("an enum value must be an integer literal", value.span)),
                },
                StmtKind::Pass => continue,
                _ => {
                    return Err(CompileError::new(
                        "an enum body lists variant names, one per line (optionally `Name = 5`)",
                        s.span,
                    ))
                }
            };
            if variants.iter().any(|(n, _)| *n == vname) {
                return Err(CompileError::new(format!("enum variant '{}' is declared twice", vname), s.span));
            }
            if variants.iter().any(|(_, v)| *v == value) {
                return Err(CompileError::new(format!("two variants of enum '{}' have the value {}", name, value), s.span));
            }
            next = value + 1;
            variants.push((vname, value));
        }
        if variants.is_empty() {
            return Err(CompileError::new("an enum needs at least one variant", span));
        }
        Ok(EnumDecl { name, variants, is_private: false, span })
    }

    fn class_decl(&mut self) -> PResult<ClassDecl> {
        let span = self.span();
        self.expect(&Tok::Class, "'class'")?;
        let name = self.ident("a class name")?;
        let parent = if self.eat(&Tok::Extends) { Some(self.dotted_name("a parent class")?) } else { None };
        if self.at(&Tok::Comma) {
            return Err(CompileError::new(
                "classes support single inheritance only",
                self.span(),
            ));
        }
        let body = self.block()?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        for s in body {
            match s.kind {
                StmtKind::VarDecl { .. } => fields.push(s),
                StmtKind::Fn(f) => methods.push(f),
                StmtKind::Pass => {}
                _ => {
                    return Err(CompileError::new(
                        "only fields (var/const) and methods (fn) are allowed in a class body",
                        s.span,
                    ))
                }
            }
        }
        Ok(ClassDecl { name, parent, fields, methods, is_private: false, is_export: false, span })
    }

    fn if_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(&Tok::If, "'if'")?;
        let mut branches = Vec::new();
        let cond = self.expr()?;
        branches.push((cond, self.block()?));
        self.skip_newlines();
        while self.at(&Tok::Orif) {
            self.bump();
            let c = self.expr()?;
            branches.push((c, self.block()?));
            self.skip_newlines();
        }
        let else_body = if self.at(&Tok::Else) {
            self.bump();
            Some(self.block()?)
        } else {
            None
        };
        Ok(StmtKind::If { branches, else_body })
    }

    fn match_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(&Tok::Match, "'match'")?;
        let subject = self.expr()?;
        self.expect(&Tok::Colon, "':'")?;
        self.skip_newlines();
        self.expect(&Tok::Indent, "an indented block")?;
        let mut cases = Vec::new();
        let mut default = None;
        self.skip_newlines();
        while !self.at(&Tok::Dedent) && !self.at(&Tok::Eof) {
            if self.eat(&Tok::Case) {
                let pat = self.expr()?;
                cases.push((pat, self.block()?));
            } else if self.eat(&Tok::Default) {
                if default.is_some() {
                    return Err(CompileError::new("duplicate 'default' in match", self.span()));
                }
                default = Some(self.block()?);
            } else {
                return Err(CompileError::new(
                    format!("expected 'case' or 'default', found {}", self.peek()),
                    self.span(),
                ));
            }
            self.skip_newlines();
        }
        self.expect(&Tok::Dedent, "end of match")?;
        Ok(StmtKind::Match { subject, cases, default })
    }

    fn try_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(&Tok::Try, "'try'")?;
        let body = self.block()?;
        self.skip_newlines();
        let catch = if self.eat(&Tok::Catch) {
            let name = self.ident("an error variable name")?;
            Some((name, self.block()?))
        } else {
            None
        };
        self.skip_newlines();
        let finally = if self.eat(&Tok::Finally) { Some(self.block()?) } else { None };
        if catch.is_none() && finally.is_none() {
            return Err(CompileError::new("'try' needs a 'catch' or 'finally'", self.span()));
        }
        Ok(StmtKind::Try { body, catch, finally })
    }

    fn use_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(&Tok::Use, "'use'")?;
        let mut path = vec![self.ident("a module name")?];
        while self.eat(&Tok::Dot) {
            path.push(self.ident("a module member name")?);
        }
        let alias = if self.eat(&Tok::As) { Some(self.ident("an alias")?) } else { None };
        self.end_of_stmt()?;
        Ok(StmtKind::Use { path, alias })
    }

    // ---------------- expressions ----------------

    /// An expression, or a comma-separated tuple without parentheses.
    fn expr_or_tuple(&mut self) -> PResult<Expr> {
        let first = self.expr()?;
        if self.at(&Tok::Comma) {
            let span = first.span;
            let mut items = vec![first];
            while self.eat(&Tok::Comma) {
                items.push(self.expr()?);
            }
            Ok(Expr { kind: ExprKind::Tuple(items), span })
        } else {
            Ok(first)
        }
    }

    fn expr(&mut self) -> PResult<Expr> {
        let l = self.or_expr()?;
        if self.eat(&Tok::QQ) {
            // right associative: a ?? b ?? c  is  a ?? (b ?? c)
            let r = self.expr()?;
            return Ok(self.bin(BinOp::Coalesce, l, r));
        }
        Ok(l)
    }

    fn bin(&self, op: BinOp, l: Expr, r: Expr) -> Expr {
        let span = l.span;
        Expr { kind: ExprKind::Binary(op, Box::new(l), Box::new(r)), span }
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let mut l = self.and_expr()?;
        while self.eat(&Tok::Or) {
            let r = self.and_expr()?;
            l = self.bin(BinOp::Or, l, r);
        }
        Ok(l)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        let mut l = self.not_expr()?;
        while self.eat(&Tok::And) {
            let r = self.not_expr()?;
            l = self.bin(BinOp::And, l, r);
        }
        Ok(l)
    }

    fn not_expr(&mut self) -> PResult<Expr> {
        if self.at(&Tok::Not) {
            let span = self.span();
            self.bump();
            let e = self.not_expr()?;
            return Ok(Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(e)), span });
        }
        self.cmp_expr()
    }

    fn cmp_op(&self) -> Option<BinOp> {
        Some(match self.peek() {
            Tok::EqEq => BinOp::Eq,
            Tok::NotEq => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::LtEq => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::GtEq => BinOp::Ge,
            _ => return None,
        })
    }

    fn cmp_expr(&mut self) -> PResult<Expr> {
        let l = self.range_expr()?;
        if self.at(&Tok::Has) {
            self.bump();
            let r = self.range_expr()?;
            return Ok(self.bin(BinOp::Has, l, r));
        }
        if let Some(op) = self.cmp_op() {
            self.bump();
            let r = self.range_expr()?;
            if self.cmp_op().is_some() {
                return Err(CompileError::new(
                    "comparison chaining is not supported; use 'and'",
                    self.span(),
                ));
            }
            return Ok(self.bin(op, l, r));
        }
        Ok(l)
    }

    fn range_expr(&mut self) -> PResult<Expr> {
        let l = self.add_expr()?;
        let inclusive = match self.peek() {
            Tok::DotDot => true,
            Tok::DotDotLt => false,
            _ => return Ok(l),
        };
        self.bump();
        let r = self.add_expr()?;
        let span = l.span;
        Ok(Expr {
            kind: ExprKind::Range { start: Box::new(l), end: Box::new(r), inclusive },
            span,
        })
    }

    fn add_expr(&mut self) -> PResult<Expr> {
        let mut l = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let r = self.mul_expr()?;
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

    fn mul_expr(&mut self) -> PResult<Expr> {
        let mut l = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::SlashSlash => BinOp::FloorDiv,
                Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.bump();
            let r = self.unary_expr()?;
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

    fn unary_expr(&mut self) -> PResult<Expr> {
        if self.at(&Tok::Minus) {
            let span = self.span();
            self.bump();
            let e = self.unary_expr()?;
            return Ok(Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(e)), span });
        }
        self.pow_expr()
    }

    fn pow_expr(&mut self) -> PResult<Expr> {
        let base = self.postfix_expr()?;
        if self.eat(&Tok::StarStar) {
            // right associative; exponent may be a unary minus
            let exp = self.unary_expr()?;
            return Ok(self.bin(BinOp::Pow, base, exp));
        }
        Ok(base)
    }

    fn postfix_expr(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            let sp = e.span;
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let args = self.call_args()?;
                    e = Expr { kind: ExprKind::Call(Box::new(e), args), span: sp };
                }
                Tok::LBracket => {
                    self.bump();
                    let idx = self.expr()?;
                    self.expect(&Tok::RBracket, "']'")?;
                    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), span: sp };
                }
                Tok::Bang => {
                    self.bump();
                    e = Expr { kind: ExprKind::Force(Box::new(e)), span: sp };
                }
                Tok::Dot => {
                    self.bump();
                    let name = self.ident("a member name")?;
                    e = Expr { kind: ExprKind::Member(Box::new(e), name), span: sp };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        while !self.at(&Tok::RParen) {
            // predicate expression: `< 4`, `>= x`, ...
            if let Some(op) = self.cmp_op() {
                let span = self.span();
                self.bump();
                let operand = self.add_expr()?;
                args.push(Arg {
                    name: None,
                    value: Expr { kind: ExprKind::Predicate(op, Box::new(operand)), span },
                });
            } else if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Assign {
                let name = self.ident("an argument name")?;
                self.bump();
                let value = self.expr()?;
                args.push(Arg { name: Some(name), value });
            } else {
                let value = self.expr()?;
                args.push(Arg { name: None, value });
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok(args)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        let kind = match self.bump() {
            Tok::Int(n) => ExprKind::Int(n),
            Tok::Float(n) => ExprKind::Float(n),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::FStr(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        FPart::Text(t) => out.push(FStrPart::Text(t)),
                        FPart::Expr(src, sp) => out.push(FStrPart::Expr(parse_expr_str(&src, sp)?)),
                    }
                }
                ExprKind::FStr(out)
            }
            Tok::True => ExprKind::Bool(true),
            Tok::False => ExprKind::Bool(false),
            Tok::Null => ExprKind::Null,
            Tok::Ident(s) => ExprKind::Ident(s),
            Tok::Await => {
                let e = self.unary_expr()?;
                ExprKind::Await(Box::new(e))
            }
            Tok::Fn => {
                self.expect(&Tok::LParen, "'(' after 'fn' in a lambda")?;
                let mut params = Vec::new();
                while !self.at(&Tok::RParen) {
                    let name = self.ident("a parameter name")?;
                    let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                    params.push(Param { name, ty, default: None });
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "')'")?;
                let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
                self.expect(&Tok::FatArrow, "'=>' (a lambda looks like fn(x) => x * 2)")?;
                let body = self.expr()?;
                ExprKind::Lambda(Box::new(Lambda { params, ret, body }))
            }
            Tok::LParen => {
                let first = self.expr()?;
                if self.at(&Tok::Comma) {
                    let mut items = vec![first];
                    while self.eat(&Tok::Comma) {
                        if self.at(&Tok::RParen) {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                    self.expect(&Tok::RParen, "')'")?;
                    ExprKind::Tuple(items)
                } else {
                    self.expect(&Tok::RParen, "')'")?;
                    return Ok(first);
                }
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                while !self.at(&Tok::RBracket) {
                    items.push(self.expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBracket, "']'")?;
                ExprKind::List(items)
            }
            Tok::LBrace => {
                let mut items = Vec::new();
                while !self.at(&Tok::RBrace) {
                    let k = self.expr()?;
                    self.expect(&Tok::Colon, "':' after dictionary key")?;
                    let v = self.expr()?;
                    items.push((k, v));
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBrace, "'}'")?;
                ExprKind::Dict(items)
            }
            other => {
                return Err(CompileError::new(
                    format!("expected an expression, found {}", other),
                    span,
                ))
            }
        };
        Ok(Expr { kind, span })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_function_and_if() {
        let m = parse(
            "fn f(a, b: Int) -> Int:\n    if a > b:\n        return a\n    orif a == b:\n        return 0\n    else:\n        return b\n",
        )
        .unwrap();
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn parses_filter_predicate() {
        let m = parse("var x = numbers.filter(< 4)\n").unwrap();
        match &m.items[0].kind {
            StmtKind::VarDecl { init, .. } => match &init.kind {
                ExprKind::Call(_, args) => {
                    assert!(matches!(args[0].value.kind, ExprKind::Predicate(BinOp::Lt, _)))
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn tuple_return() {
        let m = parse("fn p():\n    return 10, 20\n").unwrap();
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn elif_is_rejected() {
        assert!(parse("if x:\n    pass\nelif y:\n    pass\n").is_err());
    }

    #[test]
    fn multiple_inheritance_rejected() {
        assert!(parse("class A extends B, C:\n    pass\n").is_err());
    }

    #[test]
    fn range_inclusive_and_exclusive() {
        parse("for i in 1..10:\n    pass\nfor j in 1..<10:\n    pass\n").unwrap();
    }
}
