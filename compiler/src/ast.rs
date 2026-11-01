//! Abstract syntax tree for SuperNova.

use crate::lexer::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// A named type: Int, Float, Bool, String, a class name, ...
    Named(String),
    /// `List[T]`
    List(Box<TypeExpr>),
    /// `Dict[K, V]`
    Dict(Box<TypeExpr>, Box<TypeExpr>),
    /// `Fn(A, B) -> R` (return type optional: none means nothing is returned)
    Fn(Vec<TypeExpr>, Option<Box<TypeExpr>>),
    /// `Task[T]`: the handle returned by calling an `async fn`.
    Task(Box<TypeExpr>),
    /// `Tuple[A, B]`
    Tuple(Vec<TypeExpr>),
    /// `T?`
    Optional(Box<TypeExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStrPart {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FStrPart>),
    Bool(bool),
    Null,
    Ident(String),
    List(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    Tuple(Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Index(Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String),
    /// `a..b` (inclusive) or `a..<b` (exclusive end).
    Range { start: Box<Expr>, end: Box<Expr>, inclusive: bool },
    /// A comparison predicate such as `< 4`; only valid as a call argument.
    Predicate(BinOp, Box<Expr>),
    Await(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    pub is_private: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub name: String,
    pub parent: Option<String>,
    pub fields: Vec<Stmt>,
    pub methods: Vec<FnDecl>,
    pub is_private: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    VarDecl {
        name: String,
        is_const: bool,
        is_private: bool,
        ty: Option<TypeExpr>,
        init: Expr,
    },
    Assign { target: Expr, op: Option<BinOp>, value: Expr },
    Expr(Expr),
    If { branches: Vec<(Expr, Vec<Stmt>)>, else_body: Option<Vec<Stmt>> },
    While { cond: Expr, body: Vec<Stmt> },
    For { var: String, iter: Expr, body: Vec<Stmt> },
    Match { subject: Expr, cases: Vec<(Expr, Vec<Stmt>)>, default: Option<Vec<Stmt>> },
    Return(Option<Expr>),
    Break,
    Continue,
    Pass,
    Fn(FnDecl),
    Class(ClassDecl),
    Try {
        body: Vec<Stmt>,
        catch: Option<(String, Vec<Stmt>)>,
        finally: Option<Vec<Stmt>>,
    },
    Throw(Expr),
    Use { path: Vec<String>, alias: Option<String> },
    Manages(String),
}

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Stmt>,
}
