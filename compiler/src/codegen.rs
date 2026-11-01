//! Type checker and C99 emitter.
//!
//! Every expression is typed while it is lowered to C, so there is no separate
//! untyped-to-typed pass. Functions with unannotated parameters are
//! specialised ("monomorphised") per distinct tuple of argument types; each
//! specialisation is type-checked and emitted as its own C function.
//!
//! Evaluation order is left-to-right everywhere. C leaves operand and argument
//! order unspecified, so when two operands may have side effects they are
//! hoisted into temporaries (see `seq2` and the call lowering).

use crate::ast::*;
use crate::lexer::{CompileError, Span};
use std::collections::HashMap;

mod classes;
mod dicts;
mod async_;
mod errors;
mod funcs;
mod optional;

mod stdlib;
use errors::{assigned_in_try, Jump, TryFrame};
mod collections;

thread_local! {
    /// Class names by class index, so `Ty::name()` can print them.
    static CLASS_NAMES: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
}

type R<T> = Result<T, CompileError>;

fn err<T>(msg: impl Into<String>, span: Span) -> R<T> {
    Err(CompileError::new(msg, span))
}

fn nyi<T>(what: &str, span: Span) -> R<T> {
    err(
        format!("{} not implemented yet (see ROADMAP.md)", what),
        span,
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Null,
    Void,
    /// Tagged value: element type of mixed lists.
    Any,
    /// Reference type; compound types are interned by leaking (compiler is short-lived).
    List(&'static Ty),
    /// Insertion-ordered hash map; keys are Int, String or Bool.
    Dict(&'static Ty, &'static Ty),
    /// Handle to a running `async fn` call (or timer / file operation).
    Task(&'static Ty),
    /// Function value: parameter types and return type (a plain C function pointer).
    Func(&'static [Ty], &'static Ty),
    /// `T?`: a value that may be null (a `has` flag plus the value).
    Opt(&'static Ty),
    Tuple(&'static [Ty]),
    /// Instance of a user class (index into `Gen::classes`); a reference to a heap object.
    Class(u32),
}

impl Ty {
    fn name(self) -> String {
        match self {
            Ty::Int => "Int".into(),
            Ty::Float => "Float".into(),
            Ty::Bool => "Bool".into(),
            Ty::Str => "String".into(),
            Ty::Null => "null".into(),
            Ty::Void => "nothing".into(),
            Ty::Any => "Any".into(),
            Ty::Class(c) => CLASS_NAMES.with(|n| n.borrow()[c as usize].clone()),
            Ty::List(e) => format!("List[{}]", e.name()),
            Ty::Dict(k, v) => format!("Dict[{}, {}]", k.name(), v.name()),
            Ty::Task(t) => format!("Task[{}]", t.name()),
            Ty::Opt(t) => format!("{}?", t.name()),
            Ty::Func(ps, r) => {
                let params = ps.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ");
                if *r == Ty::Void { format!("Fn({})", params) } else { format!("Fn({}) -> {}", params, r.name()) }
            }
            Ty::Tuple(items) => format!(
                "Tuple[{}]",
                items.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")
            ),
        }
    }
    fn is_num(self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }
    /// Whether values of this type can contain heap pointers (the collector
    /// never scans storage of pointer-free types).
    fn has_ptrs(self) -> bool {
        match self {
            Ty::Int | Ty::Float | Ty::Bool | Ty::Null | Ty::Void | Ty::Func(..) => false,
            Ty::Str | Ty::Any | Ty::List(_) | Ty::Dict(..) | Ty::Task(_) | Ty::Class(_) => true,
            Ty::Tuple(items) => items.iter().any(|t| t.has_ptrs()),
            Ty::Opt(t) => t.has_ptrs(),
        }
    }
    /// Types an `Any` can hold.
    fn is_scalar(self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Null)
    }
    fn list_of(elem: Ty) -> Ty {
        Ty::List(Box::leak(Box::new(elem)))
    }
    fn func_of(ps: Vec<Ty>, r: Ty) -> Ty {
        Ty::Func(Box::leak(ps.into_boxed_slice()), Box::leak(Box::new(r)))
    }
    /// `T?`; an optional of an optional is the same optional, and `Any` already includes null.
    fn opt_of(t: Ty) -> Ty {
        match t {
            Ty::Opt(_) | Ty::Any => t,
            _ => Ty::Opt(Box::leak(Box::new(t))),
        }
    }
    fn task_of(t: Ty) -> Ty {
        Ty::Task(Box::leak(Box::new(t)))
    }
    fn dict_of(k: Ty, v: Ty) -> Ty {
        Ty::Dict(Box::leak(Box::new(k)), Box::leak(Box::new(v)))
    }
    fn tuple_of(items: Vec<Ty>) -> Ty {
        Ty::Tuple(Box::leak(items.into_boxed_slice()))
    }
}

/// A lowered expression: C text, static type, and whether evaluating it is
/// free of side effects (no calls).
struct E {
    c: String,
    ty: Ty,
    pure: bool,
}

impl E {
    fn new(c: impl Into<String>, ty: Ty, pure: bool) -> E {
        E { c: c.into(), ty, pure }
    }
}

#[derive(Clone)]
struct Var {
    ty: Ty,
    is_const: bool,
    cname: String,
    /// Globals and object fields can change when any function runs, so reading
    /// them is not order-independent (see `E::pure`).
    shared: bool,
}

#[derive(Clone)]
struct FieldInfo {
    name: String,
    ty: Ty,
    owner: usize,
    is_private: bool,
    is_const: bool,
    /// C expression that computes the initial value.
    init_c: String,
}

struct ClassInfo<'a> {
    name: String,
    decl: &'a ClassDecl,
    parent: Option<usize>,
    /// Depth-first numbering: this class's id is `id_lo`; its subclasses use ids up to `id_hi`.
    id_lo: i64,
    id_hi: i64,
    /// 0 = not computed, 1 = in progress, 2 = done.
    layout: u8,
    /// All fields including inherited ones, parents first.
    fields: Vec<FieldInfo>,
    methods: HashMap<String, &'a FnDecl>,
    ctor_emitted: bool,
}

struct Bound {
    ptys: Vec<Ty>,
    cargs: Vec<E>,
    /// Parameter indexes in the order the arguments were written.
    written: Vec<usize>,
}

struct Inst {
    cname: String,
    /// None while an unannotated function is still being checked.
    ret: Option<Ty>,
}

struct Ctx {
    scopes: Vec<HashMap<String, Var>>,
    ret_decl: Option<Ty>,
    rets: Vec<Ty>,
    in_fn: bool,
    loop_depth: usize,
    out: String,
    indent: usize,
    /// Class whose method is being compiled (gives access to `self`, fields, private members).
    cur_class: Option<usize>,
    fn_name: String,
    /// Open try/finally regions, innermost last (see errors.rs).
    try_stack: Vec<TryFrame>,
    /// Compiling the body of an `async fn` (where `await` may suspend).
    is_async: bool,
    /// Locals that must be `volatile` because a try body assigns them.
    volatile: std::collections::HashSet<String>,
    /// Optional variables known not to be null here: C name -> the value type.
    narrowed: HashMap<String, Ty>,
    /// Narrowings added by each open scope (removed when it closes).
    narrow_added: Vec<Vec<String>>,
}

impl Ctx {
    fn new(in_fn: bool) -> Ctx {
        Ctx {
            scopes: vec![HashMap::new()],
            ret_decl: None,
            rets: Vec::new(),
            in_fn,
            loop_depth: 0,
            out: String::new(),
            indent: 1,
            cur_class: None,
            fn_name: String::new(),
            try_stack: Vec::new(),
            is_async: false,
            volatile: std::collections::HashSet::new(),
            narrowed: HashMap::new(),
            narrow_added: vec![Vec::new()],
        }
    }
}

const BUILTINS: [&str; 9] = ["print", "str", "int", "float", "gcCollect", "gcHeapBytes", "sleep", "readFile", "writeFile"];

struct Gen<'a> {
    uses_async: bool,
    files: Vec<String>,
    fns: HashMap<String, &'a FnDecl>,
    fn_order: Vec<&'a FnDecl>,
    globals: HashMap<String, Var>,
    global_decls: String,
    protos: String,
    bodies: String,
    lits: String,
    lit_ids: HashMap<String, String>,
    insts: HashMap<(String, Vec<Ty>), Inst>,
    tmp: usize,
    cx: Ctx,
    /// C type names for lists/tuples, plus their generated declarations.
    types: HashMap<Ty, String>,
    type_decls: String,
    fmt_fns: HashMap<Ty, String>,
    fmt_decls: String,
    /// Globals whose memory the collector must scan.
    global_roots: Vec<String>,
    classes: Vec<ClassInfo<'a>>,
    class_ix: HashMap<String, usize>,
    minsts: HashMap<(usize, String, Vec<Ty>), Inst>,
    fwd_decls: String,
    class_structs: String,
}

pub fn generate<'a>(module: &'a Module, files: &[String]) -> R<String> {
    let mut g = Gen {
        uses_async: false,
        files: files.to_vec(),
        fns: HashMap::new(),
        fn_order: Vec::new(),
        globals: HashMap::new(),
        global_decls: String::new(),
        protos: String::new(),
        bodies: String::new(),
        lits: String::new(),
        lit_ids: HashMap::new(),
        insts: HashMap::new(),
        tmp: 0,
        cx: Ctx::new(false),
        types: HashMap::new(),
        type_decls: String::new(),
        fmt_fns: HashMap::new(),
        fmt_decls: String::new(),
        global_roots: Vec::new(),
        classes: Vec::new(),
        class_ix: HashMap::new(),
        minsts: HashMap::new(),
        fwd_decls: String::new(),
        class_structs: String::new(),
    };
    g.run(module)
}

fn c_escape(s: &str) -> String {
    let mut o = String::new();
    for &b in s.as_bytes() {
        match b {
            b'"' => o.push_str("\\\""),
            b'\\' => o.push_str("\\\\"),
            b'?' => o.push_str("\\?"),
            0x20..=0x7e => o.push(b as char),
            _ => o.push_str(&format!("\\{:03o}", b)),
        }
    }
    o
}

fn fmt_float(f: f64) -> String {
    let s = format!("{:?}", f);
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{}.0", s)
    }
}

fn op_sym(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::FloorDiv => "//",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Has => "has",
        BinOp::Coalesce => "??",
    }
}

/// Wrap a scalar C value into the tagged `Any` representation.
fn box_any(c: &str, from: Ty) -> String {
    let f = match from {
        Ty::Int => "sn_val_int",
        Ty::Float => "sn_val_float",
        Ty::Bool => "sn_val_bool",
        Ty::Str => "sn_val_str",
        _ => "sn_val_null",
    };
    format!("{}({})", f, c)
}

fn to_f(c: &str, from: Ty) -> String {
    if from == Ty::Int {
        format!("((double)({}))", c)
    } else {
        c.to_string()
    }
}

fn always_returns(body: &[Stmt]) -> bool {
    body.iter().any(stmt_returns)
}

fn stmt_returns(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) | StmtKind::Throw(_) => true,
        StmtKind::Try { body, catch, finally } => {
            finally.as_ref().map_or(false, |f| always_returns(f))
                || (always_returns(body) && catch.as_ref().map_or(true, |(_, b)| always_returns(b)))
        }
        StmtKind::If { branches, else_body } => match else_body {
            Some(e) => branches.iter().all(|(_, b)| always_returns(b)) && always_returns(e),
            None => false,
        },
        StmtKind::Match { cases, default, .. } => match default {
            Some(d) => cases.iter().all(|(_, b)| always_returns(b)) && always_returns(d),
            None => false,
        },
        _ => false,
    }
}

impl<'a> Gen<'a> {
    fn run(&mut self, module: &'a Module) -> R<String> {
        // Pass 1: collect functions so they can be used before their declaration.
        for s in &module.items {
            if let StmtKind::Fn(f) = &s.kind {
                if BUILTINS.contains(&f.name.as_str()) {
                    return err(format!("cannot redefine builtin function '{}'", f.name), f.span);
                }
                if matches!(f.name.as_str(), "self" | "super") {
                    return err(format!("'{}' is a reserved name", f.name), f.span);
                }
                if self.fns.insert(f.name.clone(), f).is_some() {
                    return err(format!("function '{}' is already defined", f.name), f.span);
                }
                self.fn_order.push(f);
            }
        }
        self.collect_classes(module)?;
        self.cx.volatile = assigned_in_try(&module.items);
        // Pass 2: top-level statements form the program entry point.
        for s in &module.items {
            if !matches!(s.kind, StmtKind::Fn(_) | StmtKind::Class(_)) {
                self.stmt(s)?;
            }
        }
        // Pass 3: type-check functions whose parameters are all annotated even if never called.
        let order = self.fn_order.clone();
        for f in order {
            if f.params.iter().all(|p| p.ty.is_some()) {
                let mut ptys = Vec::new();
                for p in &f.params {
                    ptys.push(self.resolve_type(p.ty.as_ref().unwrap(), f.span)?);
                }
                self.instance(f, ptys, f.span)?;
            }
        }
        self.check_classes()?;
        let main_body = std::mem::take(&mut self.cx.out);
        let mut out = String::new();
        out.push_str("/* generated by snc — do not edit */\n");
        if self.uses_async {
            out.push_str("#include \"snasync.h\"\n");
        }
        out.push_str(&self.lits);
        out.push_str(&self.fwd_decls);
        out.push_str(&self.type_decls);
        out.push_str(&self.class_structs);
        out.push_str(&self.global_decls);
        out.push_str(&self.protos);
        out.push_str(&self.fmt_decls);
        out.push_str(&self.bodies);
        out.push_str("static void sn_main(void) {\n");
        out.push_str(&main_body);
        out.push_str("}\n\nint main(int argc, char **argv) {\n    sn_argc = argc;\n    sn_argv = argv;\n    sn_gc_init(__builtin_frame_address(0));\n");
        out.push_str("    sn_gc_root(&sn_exc, sizeof(sn_exc));\n");
        for g in &self.global_roots {
            out.push_str(&format!("    sn_gc_root(&{0}, sizeof({0}));\n", g));
        }
        if self.uses_async {
            out.push_str("    sn_async_init();\n    sn_main();\n    sn_async_finish();\n    return 0;\n}\n");
        } else {
            out.push_str("    sn_main();\n    return 0;\n}\n");
        }
        Ok(out)
    }

    // ---------------- helpers ----------------

    fn loc(&self, span: Span) -> String {
        format!("\"{}:{}:{}\"", c_escape(&self.files[span.file as usize]), span.line, span.col)
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.cx.indent {
            self.cx.out.push_str("    ");
        }
        self.cx.out.push_str(s);
        self.cx.out.push('\n');
    }

    fn fresh(&mut self) -> usize {
        self.tmp += 1;
        self.tmp
    }

    fn str_lit(&mut self, s: &str) -> String {
        if let Some(n) = self.lit_ids.get(s) {
            return n.clone();
        }
        let id = format!("slit{}", self.lit_ids.len());
        self.lits.push_str(&format!(
            "static const struct {{ int64_t len; char d[{}]; }} {} = {{ {}, \"{}\" }};\n",
            s.len() + 1,
            id,
            s.len(),
            c_escape(s)
        ));
        let r = format!("((SnStr*)&{})", id);
        self.lit_ids.insert(s.to_string(), r.clone());
        r
    }

    fn resolve_type(&self, t: &TypeExpr, span: Span) -> R<Ty> {
        match t {
            TypeExpr::Named(n) => match n.as_str() {
                "Int" => Ok(Ty::Int),
                "Float" => Ok(Ty::Float),
                "Bool" => Ok(Ty::Bool),
                "String" => Ok(Ty::Str),
                "Any" => Ok(Ty::Any),
                other => match self.class_ix.get(other) {
                    Some(&c) => Ok(Ty::Class(c as u32)),
                    None => err(format!("unknown type '{}'", other), span),
                },
            },
            TypeExpr::List(inner) => Ok(Ty::list_of(self.resolve_type(inner, span)?)),
            TypeExpr::Fn(ps, ret) => {
                let mut v = Vec::new();
                for p in ps {
                    v.push(self.resolve_type(p, span)?);
                }
                let r = match ret {
                    Some(r) => self.resolve_type(r, span)?,
                    None => Ty::Void,
                };
                Ok(Ty::func_of(v, r))
            }
            TypeExpr::Task(t) => Ok(Ty::task_of(self.resolve_type(t, span)?)),
            TypeExpr::Dict(k, v) => {
                let kt = self.resolve_type(k, span)?;
                if !matches!(kt, Ty::Int | Ty::Str | Ty::Bool) {
                    return err(
                        format!("dictionary keys must be Int, String or Bool, not {}", kt.name()),
                        span,
                    );
                }
                Ok(Ty::dict_of(kt, self.resolve_type(v, span)?))
            }
            TypeExpr::Tuple(items) => {
                let mut v = Vec::new();
                for t in items {
                    v.push(self.resolve_type(t, span)?);
                }
                Ok(Ty::tuple_of(v))
            }
            TypeExpr::Optional(t) => {
                let inner = self.resolve_type(t, span)?;
                if matches!(inner, Ty::Void | Ty::Null) {
                    return err("only values can be optional", span);
                }
                Ok(Ty::opt_of(inner))
            }
        }
    }

    /// A variable as the program sees it at this point: an optional that has been
    /// checked against null reads as its value type.
    fn lookup(&mut self, name: &str, span: Span) -> R<Var> {
        let v = self.lookup_raw(name, span)?;
        if !v.cname.starts_with('(') {
            if let Some(&inner) = self.cx.narrowed.get(&v.cname) {
                return Ok(Var { ty: inner, cname: format!("{}.v", v.cname), ..v });
            }
        }
        Ok(v)
    }

    fn lookup_raw(&mut self, name: &str, span: Span) -> R<Var> {
        for sc in self.cx.scopes.iter().rev() {
            if let Some(v) = sc.get(name) {
                return Ok(v.clone());
            }
        }
        if let Some(cc) = self.cx.cur_class {
            self.ensure_layout(cc)?;
            if let Some(f) = self.classes[cc].fields.iter().find(|f| f.name == name).cloned() {
                self.check_access(f.owner, f.is_private, "field", name, span)?;
                return Ok(Var {
                    ty: f.ty,
                    is_const: f.is_const,
                    cname: format!("(v_self->f_{})", name),
                    shared: true,
                });
            }
        }
        if let Some(v) = self.globals.get(name) {
            return Ok(v.clone());
        }
        if name == "self" {
            return err("'self' can only be used inside a class method", span);
        }
        if name == "super" {
            return err("'super' can only be used as super.method(...)", span);
        }
        err(format!("undefined variable '{}'", name), span)
    }

    /// Implicit conversions: Int -> Float, scalars -> Any, subclass -> superclass.
    fn coerce(&mut self, c: String, from: Ty, to: Ty) -> Option<String> {
        if from == to {
            Some(c)
        } else if let Ty::Opt(inner) = to {
            if from == Ty::Null {
                let n = self.type_name(to);
                Some(format!("(({}){{0}})", n))
            } else if matches!(from, Ty::Opt(_)) {
                None
            } else {
                let ic = self.coerce(c, from, *inner)?;
                let n = self.type_name(to);
                Some(format!("(({}){{true, {}}})", n, ic))
            }
        } else if from == Ty::Int && to == Ty::Float {
            Some(format!("((double)({}))", c))
        } else if to == Ty::Any && from.is_scalar() {
            Some(box_any(&c, from))
        } else if let (Ty::Class(a), Ty::Class(b)) = (from, to) {
            if self.is_subclass(a as usize, b as usize) {
                Some(format!("((SnO_{}*)({}))", b, c))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn val(&mut self, e: &Expr) -> R<E> {
        let r = self.expr(e)?;
        if r.ty == Ty::Void {
            return err("this expression produces no value", e.span);
        }
        Ok(r)
    }

    /// Combine two operands preserving left-to-right evaluation.
    fn seq2(&mut self, l: &E, r: &E, f: impl Fn(&str, &str) -> String) -> String {
        if !l.pure && !r.pure {
            let n = self.fresh();
            let (a, b) = (format!("t{}a", n), format!("t{}b", n));
            let (lc, rc) = (self.cty(l.ty), self.cty(r.ty));
            format!(
                "({{ {} {} = {}; {} {} = {}; {}; }})",
                lc,
                a,
                l.c,
                rc,
                b,
                r.c,
                f(&a, &b)
            )
        } else {
            f(&l.c, &r.c)
        }
    }

    // ---------------- functions ----------------

    fn instance(&mut self, decl: &'a FnDecl, ptys: Vec<Ty>, span: Span) -> R<(String, Ty)> {
        let key = (decl.name.clone(), ptys.clone());
        if let Some(inst) = self.insts.get(&key) {
            return match inst.ret {
                Some(r) => Ok((inst.cname.clone(), r)),
                None => err(
                    format!(
                        "recursive function '{}' needs a return type annotation (-> Type)",
                        decl.name
                    ),
                    span,
                ),
            };
        }
        let declared = match &decl.ret {
            Some(t) => Some(self.resolve_type(t, decl.span)?),
            None => None,
        };
        let cname = format!("f{}_{}", self.insts.len(), decl.name);
        if decl.is_async {
            self.uses_async = true;
        }
        let body_name = if decl.is_async { format!("{}_body", cname) } else { cname.clone() };
        let shown_ret = if decl.is_async { declared.map(Ty::task_of) } else { declared };
        self.insts.insert(key.clone(), Inst { cname: cname.clone(), ret: shown_ret });

        let mut cx = Ctx::new(true);
        cx.ret_decl = declared;
        cx.volatile = assigned_in_try(&decl.body);
        cx.is_async = decl.is_async;
        let mut params_c = Vec::new();
        for (p, t) in decl.params.iter().zip(&ptys) {
            let pc = format!("v_{}", p.name);
            if cx.scopes[0]
                .insert(p.name.clone(), Var { ty: *t, is_const: false, cname: pc.clone(), shared: false })
                .is_some()
            {
                return err(format!("duplicate parameter '{}'", p.name), decl.span);
            }
            let tc = self.cty(*t);
            let q = if cx.volatile.contains(&p.name) { " volatile" } else { "" };
            params_c.push(format!("{}{} {}", tc, q, pc));
        }
        let saved = std::mem::replace(&mut self.cx, cx);
        let res = self.fn_body(decl, declared);
        let fcx = std::mem::replace(&mut self.cx, saved);
        let ret = res?;
        let ret_c = self.cty(ret);
        let sig = format!(
            "static {} {}({})",
            ret_c,
            body_name,
            if params_c.is_empty() { "void".to_string() } else { params_c.join(", ") }
        );
        self.protos.push_str(&format!("{};\n", sig));
        self.bodies.push_str(&format!("{} {{\n{}}}\n\n", sig, fcx.out));
        if decl.is_async {
            let ptys_c: Vec<String> = ptys.iter().map(|t| self.cty(*t)).collect();
            self.async_wrapper(&cname, &body_name, ptys_c, ret);
            let tt = Ty::task_of(ret);
            self.insts.get_mut(&key).unwrap().ret = Some(tt);
            return Ok((cname, tt));
        }
        self.insts.get_mut(&key).unwrap().ret = Some(ret);
        Ok((cname, ret))
    }

    fn fn_body(&mut self, decl: &'a FnDecl, declared: Option<Ty>) -> R<Ty> {
        for s in &decl.body {
            self.stmt(s)?;
        }
        let ret = match declared {
            Some(t) => t,
            None => {
                let rets = self.cx.rets.clone();
                match rets.first() {
                    None => Ty::Void,
                    Some(&first) => {
                        if let Some(other) = rets.iter().find(|&&t| t != first) {
                            return err(
                                format!(
                                    "function '{}' returns both {} and {}; annotate the return type or make them consistent",
                                    decl.name,
                                    first.name(),
                                    other.name()
                                ),
                                decl.span,
                            );
                        }
                        first
                    }
                }
            }
        };
        if ret != Ty::Void && !always_returns(&decl.body) {
            return err(
                format!(
                    "function '{}' may reach its end without returning a value",
                    decl.name
                ),
                decl.span,
            );
        }
        Ok(ret)
    }

    fn call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> R<E> {
        let r = self.call_inner(callee, args, span);
        // a called function may reassign a global, so what we knew about globals is stale
        let builtin = matches!(&callee.kind, ExprKind::Ident(n)
            if n.starts_with("__") || matches!(n.as_str(), "print" | "str" | "int" | "float" | "gcCollect" | "gcHeapBytes" | "sleep"));
        if !builtin {
            self.cx.narrowed.retain(|k, _| !k.starts_with("g_"));
        }
        r
    }

    fn call_inner(&mut self, callee: &Expr, args: &[Arg], span: Span) -> R<E> {
        if let ExprKind::Member(obj, method) = &callee.kind {
            if matches!(&obj.kind, ExprKind::Ident(n) if n == "super") {
                return self.super_call(method, args, span);
            }
            return self.method_call(obj, method, args, span);
        }
        let ExprKind::Ident(name) = &callee.kind else {
            // e.g. pick(true)(6) or table["add"](3, 4)
            let f = self.val(callee)?;
            let Ty::Func(ps, r) = f.ty else {
                return err(format!("a value of type {} cannot be called", f.ty.name()), span);
            };
            return self.call_value(f, ps, *r, args, span);
        };
        if BUILTINS.contains(&name.as_str()) {
            return self.builtin(name, args, span);
        }
        if name.starts_with("__") {
            return self.intrinsic_call(name, args, span);
        }
        if self.var_exists(name) {
            let v = self.lookup(name, span)?;
            let Ty::Func(ps, r) = v.ty else {
                return err(format!("'{}' is a {}, not a function", name, v.ty.name()), span);
            };
            return self.call_value(E::new(v.cname, v.ty, !v.shared), ps, *r, args, span);
        }
        if let Some(&c) = self.class_ix.get(name) {
            return self.construct(c, args, span);
        }
        if let Some(cc) = self.cx.cur_class {
            if self.find_method(cc, name).is_some() {
                let recv = E::new("v_self", Ty::Class(cc as u32), true);
                return self.call_method(recv, cc, name, args, span, false);
            }
        }
        let Some(&decl) = self.fns.get(name) else {
            return err(format!("undefined function '{}'", name), span);
        };
        let bound = self.bind_args(decl, name, args, span)?;
        let (cname, ret) = self.instance(decl, bound.ptys.clone(), span)?;
        let n = decl.params.len();
        let impure = bound.written.iter().filter(|&&i| !bound.cargs[i].pure).count();
        let text = if impure >= 2 {
            // Preserve source-order evaluation of arguments.
            let k = self.fresh();
            let mut s = String::from("({ ");
            for &i in &bound.written {
                let tc = self.cty(bound.cargs[i].ty);
                s.push_str(&format!("{} a{}_{} = {}; ", tc, k, i, bound.cargs[i].c));
            }
            let mut final_args = Vec::new();
            for i in 0..n {
                if bound.written.contains(&i) {
                    final_args.push(format!("a{}_{}", k, i));
                } else {
                    final_args.push(bound.cargs[i].c.clone());
                }
            }
            s.push_str(&format!("{}({}); }})", cname, final_args.join(", ")));
            s
        } else {
            format!(
                "{}({})",
                cname,
                bound.cargs.iter().map(|a| a.c.clone()).collect::<Vec<_>>().join(", ")
            )
        };
        Ok(E::new(text, ret, false))
    }

    /// Match call arguments to the parameters of `decl` (positional, named, defaults),
    /// check their types and decide each parameter's type for this call.
    fn bind_args(&mut self, decl: &'a FnDecl, name: &str, args: &[Arg], span: Span) -> R<Bound> {
        let n = decl.params.len();
        let mut slots: Vec<Option<E>> = (0..n).map(|_| None).collect();
        let mut written: Vec<usize> = Vec::new();
        let mut seen_named = false;
        for (i, a) in args.iter().enumerate() {
            let idx = match &a.name {
                None => {
                    if seen_named {
                        return err("positional argument after named argument", a.value.span);
                    }
                    if i >= n {
                        return err(
                            format!("'{}' takes {} argument(s), but {} were given", name, n, args.len()),
                            span,
                        );
                    }
                    i
                }
                Some(pn) => {
                    seen_named = true;
                    match decl.params.iter().position(|p| &p.name == pn) {
                        Some(k) => k,
                        None => {
                            return err(
                                format!("'{}' has no parameter named '{}'", name, pn),
                                a.value.span,
                            )
                        }
                    }
                }
            };
            if slots[idx].is_some() {
                return err(
                    format!("argument '{}' given more than once", decl.params[idx].name),
                    a.value.span,
                );
            }
            let hint = match &decl.params[idx].ty {
                Some(t) => Some(self.resolve_type(t, decl.span)?),
                None => None,
            };
            slots[idx] = Some(self.hinted(&a.value, hint)?);
            written.push(idx);
        }
        // Defaults are evaluated in a context with no caller locals.
        for i in 0..n {
            if slots[i].is_none() {
                match &decl.params[i].default {
                    Some(d) => {
                        let saved = std::mem::replace(&mut self.cx, Ctx::new(true));
                        let r = self.val(d);
                        self.cx = saved;
                        slots[i] = Some(r?);
                    }
                    None => {
                        return err(
                            format!("missing argument '{}' in call to '{}'", decl.params[i].name, name),
                            span,
                        )
                    }
                }
            }
        }
        let mut ptys = Vec::new();
        let mut cargs: Vec<E> = Vec::new();
        for (i, slot) in slots.into_iter().enumerate() {
            let a = slot.unwrap();
            let p = &decl.params[i];
            let pt = match &p.ty {
                Some(t) => self.resolve_type(t, decl.span)?,
                None => {
                    if a.ty == Ty::Null {
                        return err(
                            format!("cannot infer the type of parameter '{}' from null; annotate it", p.name),
                            span,
                        );
                    }
                    a.ty
                }
            };
            let Some(c) = self.coerce(a.c, a.ty, pt) else {
                return err(
                    format!(
                        "argument '{}' of '{}' expects {}, got {}",
                        p.name,
                        name,
                        pt.name(),
                        a.ty.name()
                    ),
                    span,
                );
            };
            ptys.push(pt);
            cargs.push(E::new(c, pt, a.pure));
        }
        Ok(Bound { ptys, cargs, written })
    }

    fn builtin(&mut self, name: &str, args: &[Arg], span: Span) -> R<E> {
        if args.iter().any(|a| a.name.is_some()) {
            return err(format!("'{}' does not accept named arguments", name), span);
        }
        match name {
            "sleep" | "readFile" | "writeFile" => {
                self.uses_async = true;
                let want = if name == "writeFile" { 2 } else { 1 };
                if args.len() != want {
                    return err(format!("'{}' takes {} argument(s)", name, want), span);
                }
                let mut vs = Vec::new();
                for a in args {
                    vs.push(self.val(&a.value)?);
                }
                if name == "sleep" {
                    let c = self.coerce(vs[0].c.clone(), vs[0].ty, Ty::Float);
                    let Some(c) = c else {
                        return err("sleep() takes a number of seconds", args[0].value.span);
                    };
                    return Ok(E::new(format!("sn_sleep({})", c), Ty::task_of(Ty::Void), false));
                }
                for (v, a) in vs.iter().zip(args) {
                    if v.ty != Ty::Str {
                        return err(format!("'{}' needs String arguments, got {}", name, v.ty.name()), a.value.span);
                    }
                }
                if name == "readFile" {
                    let c = format!("sn_read_file({})", vs[0].c);
                    Ok(E::new(c, Ty::task_of(Ty::Str), false))
                } else {
                    let c = self.seq2(&vs[0], &vs[1], |p, d| format!("sn_write_file({}, {})", p, d));
                    Ok(E::new(c, Ty::task_of(Ty::Void), false))
                }
            }
            "gcCollect" | "gcHeapBytes" => {
                if !args.is_empty() {
                    return err(format!("'{}' takes no arguments", name), span);
                }
                if name == "gcCollect" {
                    Ok(E::new("({ sn_gc_collect(); })", Ty::Void, false))
                } else {
                    Ok(E::new("((int64_t)sn_gc_heap_bytes())", Ty::Int, false))
                }
            }
            "print" => {
                let mut s = String::from("({ ");
                for (i, a) in args.iter().enumerate() {
                    let v = self.val(&a.value)?;
                    if i > 0 {
                        s.push_str("sn_print_space(); ");
                    }
                    match v.ty {
                        Ty::Int => s.push_str(&format!("sn_print_int({}); ", v.c)),
                        Ty::Float => s.push_str(&format!("sn_print_float({}); ", v.c)),
                        Ty::Bool => s.push_str(&format!("sn_print_bool({}); ", v.c)),
                        Ty::Str => s.push_str(&format!("sn_print_str({}); ", v.c)),
                        Ty::Null => s.push_str(&format!("sn_print_null({}); ", v.c)),
                        _ => {
                            let t = self.to_str(&v);
                            s.push_str(&format!("sn_print_str({}); ", t));
                        }
                    }
                }
                s.push_str("sn_print_nl(); })");
                Ok(E::new(s, Ty::Void, false))
            }
            "str" | "int" | "float" => {
                if args.len() != 1 {
                    return err(format!("'{}' takes exactly one argument", name), span);
                }
                let v = self.val(&args[0].value)?;
                let loc = self.loc(span);
                let (c, ty) = match (name, v.ty) {
                    ("str", Ty::Str) => (v.c, Ty::Str),
                    ("str", Ty::Int) => (format!("sn_int_to_str({})", v.c), Ty::Str),
                    ("str", Ty::Float) => (format!("sn_float_to_str({})", v.c), Ty::Str),
                    ("str", Ty::Bool) => (format!("sn_bool_to_str({})", v.c), Ty::Str),
                    ("str", Ty::Null) => (format!("sn_null_to_str({})", v.c), Ty::Str),
                    ("str", Ty::Any) | ("str", Ty::List(_)) | ("str", Ty::Dict(..)) | ("str", Ty::Tuple(_)) | ("str", Ty::Class(_)) => {
                        (self.to_str(&v), Ty::Str)
                    }
                    ("int", Ty::Int) => (v.c, Ty::Int),
                    ("int", Ty::Float) => (format!("sn_f2i({}, {})", v.c, loc), Ty::Int),
                    ("int", Ty::Str) => (format!("sn_str_to_int({}, {})", v.c, loc), Ty::Int),
                    ("int", Ty::Bool) => (format!("((int64_t)({}))", v.c), Ty::Int),
                    ("float", Ty::Float) => (v.c, Ty::Float),
                    ("float", Ty::Int) => (format!("((double)({}))", v.c), Ty::Float),
                    ("float", Ty::Str) => (format!("sn_str_to_float({}, {})", v.c, loc), Ty::Float),
                    _ => {
                        return err(
                            format!("cannot convert {} with {}()", v.ty.name(), name),
                            span,
                        )
                    }
                };
                Ok(E::new(c, ty, v.pure))
            }
            _ => unreachable!(),
        }
    }

    // ---------------- expressions ----------------

    fn expr(&mut self, e: &Expr) -> R<E> {
        match &e.kind {
            ExprKind::Int(n) => Ok(E::new(format!("INT64_C({})", n), Ty::Int, true)),
            ExprKind::Float(f) => Ok(E::new(fmt_float(*f), Ty::Float, true)),
            ExprKind::Str(s) => {
                let c = self.str_lit(s);
                Ok(E::new(c, Ty::Str, true))
            }
            ExprKind::Bool(b) => Ok(E::new(if *b { "true" } else { "false" }, Ty::Bool, true)),
            ExprKind::Null => Ok(E::new("0", Ty::Null, true)),
            ExprKind::Ident(name) => {
                if self.fns.contains_key(name.as_str()) && !self.var_exists(name) {
                    return self.func_value(name, None, e.span);
                }
                let v = self.lookup(name, e.span)?;
                Ok(E::new(v.cname, v.ty, !v.shared))
            }
            ExprKind::FStr(parts) => self.fstr(parts),
            ExprKind::Unary(op, x) => {
                let v = self.val(x)?;
                match (op, v.ty) {
                    (UnOp::Neg, Ty::Int) | (UnOp::Neg, Ty::Float) => {
                        Ok(E::new(format!("(-({}))", v.c), v.ty, v.pure))
                    }
                    (UnOp::Neg, Ty::List(_)) => self.list_neg(v, e.span),
                    (UnOp::Not, Ty::Bool) => Ok(E::new(format!("(!({}))", v.c), Ty::Bool, v.pure)),
                    (UnOp::Neg, t) => err(format!("cannot negate {}", t.name()), e.span),
                    (UnOp::Not, t) => err(
                        format!("'not' needs a Bool, got {} (conditions must be Bool)", t.name()),
                        e.span,
                    ),
                }
            }
            ExprKind::Binary(BinOp::Has, l, r) => self.has_expr(l, r, e.span),
            ExprKind::Binary(BinOp::Coalesce, l, r) => self.coalesce(l, r, e.span),
            ExprKind::Force(x) => self.force_expr(x, e.span),
            ExprKind::Binary(op, l, r) => {
                let lv = self.val(l)?;
                let rv = match op {
                    BinOp::And | BinOp::Or => {
                        let n = self.cond_narrow(l, *op == BinOp::And);
                        self.with_narrow(n, |g| g.val(r))?
                    }
                    _ => self.val(r)?,
                };
                self.binary(*op, lv, rv, e.span)
            }
            ExprKind::Call(callee, args) => self.call(callee, args, e.span),
            ExprKind::Await(inner) => self.await_expr(inner, e.span),
            ExprKind::List(items) => self.list_literal(items, None, e.span),
            ExprKind::Dict(items) => self.dict_literal(items, None, e.span),
            ExprKind::Tuple(items) => self.tuple_literal(items, e.span),
            ExprKind::Index(base, idx) => self.index_expr(base, idx, e.span),
            ExprKind::Member(obj, name) => self.member_expr(obj, name, e.span),
            ExprKind::Range { .. } => nyi("range values (ranges are only usable in 'for' for now)", e.span),
            ExprKind::Predicate(..) => err(
                "comparison predicates like '< 4' are only valid as call arguments",
                e.span,
            ),
        }
    }

    fn fstr(&mut self, parts: &[FStrPart]) -> R<E> {
        let n = self.fresh();
        let b = format!("b{}", n);
        let mut s = format!("({{ SnBuf {}; sn_buf_init(&{}); ", b, b);
        let mut pure = true;
        for p in parts {
            match p {
                FStrPart::Text(t) => {
                    s.push_str(&format!(
                        "sn_buf_text(&{}, \"{}\", {}); ",
                        b,
                        c_escape(t),
                        t.len()
                    ));
                }
                FStrPart::Expr(x) => {
                    let v = self.val(x)?;
                    pure &= v.pure;
                    let piece = self.buf_append(&b, &v.c, v.ty, true);
                    s.push_str(&piece);
                }
            }
        }
        s.push_str(&format!("sn_buf_finish(&{}); }})", b));
        Ok(E::new(s, Ty::Str, pure))
    }

    fn binary(&mut self, op: BinOp, l: E, r: E, span: Span) -> R<E> {
        use BinOp::*;
        let pure = l.pure && r.pure;
        let loc = self.loc(span);
        let (lt, rt) = (l.ty, r.ty);
        if matches!(op, Eq | Ne)
            && (matches!(lt, Ty::Opt(_)) || matches!(rt, Ty::Opt(_)) || ((lt == Ty::Null) != (rt == Ty::Null)))
            && lt != Ty::Any
            && rt != Ty::Any
        {
            return self.opt_equality(op, l, r, span);
        }
        if let Ty::Opt(_) = lt {
            return Err(self.null_use(lt, &format!("use '{}' on", op_sym(op)), span));
        }
        if let Ty::Opt(_) = rt {
            return Err(self.null_use(rt, &format!("use '{}' on", op_sym(op)), span));
        }
        if matches!(lt, Ty::Dict(..)) || matches!(rt, Ty::Dict(..)) {
            return err("dictionaries cannot be used with this operator or compared with ==", span);
        }
        if matches!(lt, Ty::List(_)) || matches!(rt, Ty::List(_)) {
            return self.list_binary(op, l, r, span);
        }
        if let (Ty::Class(a), Ty::Class(b)) = (lt, rt) {
            let related = self.is_subclass(a as usize, b as usize) || self.is_subclass(b as usize, a as usize);
            if matches!(op, Eq | Ne) && related {
                let sym = op_sym(op);
                let c = self.seq2(&l, &r, move |x, y| format!("((void*)({}) {} (void*)({}))", x, sym, y));
                return Ok(E::new(c, Ty::Bool, pure));
            }
            return err(
                format!(
                    "cannot apply '{}' to {} and {}{}",
                    op_sym(op),
                    lt.name(),
                    rt.name(),
                    if matches!(op, Eq | Ne) { " (they are unrelated classes)" } else { "; objects can only be compared with == and !=" }
                ),
                span,
            );
        }
        if lt == Ty::Any || rt == Ty::Any {
            return self.any_binary(op, l, r, span);
        }
        let bad = |lt: Ty, rt: Ty| -> R<E> {
            err(
                format!(
                    "operator '{}' cannot be applied to {} and {}",
                    op_sym(op),
                    lt.name(),
                    rt.name()
                ),
                span,
            )
        };
        match op {
            And | Or => {
                if lt != Ty::Bool || rt != Ty::Bool {
                    return err(
                        format!(
                            "'{}' needs Bool operands, got {} and {} (conditions must be Bool)",
                            op_sym(op),
                            lt.name(),
                            rt.name()
                        ),
                        span,
                    );
                }
                let sym = if op == And { "&&" } else { "||" };
                Ok(E::new(format!("({} {} {})", l.c, sym, r.c), Ty::Bool, pure))
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                let sym = op_sym(op);
                let c = if lt.is_num() && rt.is_num() {
                    if lt == Ty::Int && rt == Ty::Int {
                        self.seq2(&l, &r, |a, b| format!("({} {} {})", a, sym, b))
                    } else {
                        self.seq2(&l, &r, move |a, b| {
                            format!("({} {} {})", to_f(a, lt), sym, to_f(b, rt))
                        })
                    }
                } else if lt == Ty::Str && rt == Ty::Str {
                    match op {
                        Eq => self.seq2(&l, &r, |a, b| format!("sn_str_eq({}, {})", a, b)),
                        Ne => self.seq2(&l, &r, |a, b| format!("(!sn_str_eq({}, {}))", a, b)),
                        _ => self.seq2(&l, &r, move |a, b| {
                            format!("(sn_str_cmp({}, {}) {} 0)", a, b, sym)
                        }),
                    }
                } else if lt == Ty::Bool && rt == Ty::Bool && matches!(op, Eq | Ne) {
                    self.seq2(&l, &r, |a, b| format!("({} {} {})", a, sym, b))
                } else if lt == Ty::Null && rt == Ty::Null && matches!(op, Eq | Ne) {
                    (if op == Eq { "true" } else { "false" }).to_string()
                } else {
                    return err(
                        format!("cannot compare {} with {} using '{}'", lt.name(), rt.name(), sym),
                        span,
                    );
                };
                Ok(E::new(c, Ty::Bool, pure))
            }
            Add if lt == Ty::Str && rt == Ty::Str => {
                let c = self.seq2(&l, &r, |a, b| format!("sn_str_concat({}, {})", a, b));
                Ok(E::new(c, Ty::Str, pure))
            }
            Add | Sub | Mul if lt.is_num() && rt.is_num() => {
                let sym = op_sym(op);
                if lt == Ty::Int && rt == Ty::Int {
                    let c = self.seq2(&l, &r, |a, b| format!("({} {} {})", a, sym, b));
                    Ok(E::new(c, Ty::Int, pure))
                } else {
                    let c = self.seq2(&l, &r, move |a, b| {
                        format!("({} {} {})", to_f(a, lt), sym, to_f(b, rt))
                    });
                    Ok(E::new(c, Ty::Float, pure))
                }
            }
            Div if lt.is_num() && rt.is_num() => {
                let c = self.seq2(&l, &r, move |a, b| {
                    format!("sn_fdiv({}, {}, {})", to_f(a, lt), to_f(b, rt), loc)
                });
                Ok(E::new(c, Ty::Float, pure))
            }
            FloorDiv if lt.is_num() && rt.is_num() => {
                if lt == Ty::Int && rt == Ty::Int {
                    let c = self.seq2(&l, &r, move |a, b| format!("sn_idiv({}, {}, {})", a, b, loc));
                    Ok(E::new(c, Ty::Int, pure))
                } else {
                    let c = self.seq2(&l, &r, move |a, b| {
                        format!("sn_ffloordiv({}, {}, {})", to_f(a, lt), to_f(b, rt), loc)
                    });
                    Ok(E::new(c, Ty::Float, pure))
                }
            }
            Mod if lt.is_num() && rt.is_num() => {
                if lt == Ty::Int && rt == Ty::Int {
                    let c = self.seq2(&l, &r, move |a, b| format!("sn_imod({}, {}, {})", a, b, loc));
                    Ok(E::new(c, Ty::Int, pure))
                } else {
                    let c = self.seq2(&l, &r, move |a, b| {
                        format!("sn_fmod({}, {}, {})", to_f(a, lt), to_f(b, rt), loc)
                    });
                    Ok(E::new(c, Ty::Float, pure))
                }
            }
            Pow if lt.is_num() && rt.is_num() => {
                if lt == Ty::Int && rt == Ty::Int {
                    let c = self.seq2(&l, &r, move |a, b| format!("sn_ipow({}, {}, {})", a, b, loc));
                    Ok(E::new(c, Ty::Int, pure))
                } else {
                    let c = self.seq2(&l, &r, move |a, b| {
                        format!("pow({}, {})", to_f(a, lt), to_f(b, rt))
                    });
                    Ok(E::new(c, Ty::Float, pure))
                }
            }
            _ => bad(lt, rt),
        }
    }

    // ---------------- statements ----------------

    fn push_scope(&mut self) {
        self.cx.scopes.push(HashMap::new());
        self.cx.narrow_added.push(Vec::new());
        self.cx.indent += 1;
    }

    fn pop_scope(&mut self) {
        self.cx.scopes.pop();
        self.narrow_pop();
        self.cx.indent -= 1;
    }

    fn block(&mut self, body: &[Stmt]) -> R<()> {
        self.push_scope();
        for s in body {
            self.stmt(s)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn cond(&mut self, e: &Expr) -> R<String> {
        let v = self.val(e)?;
        if v.ty != Ty::Bool {
            return err(
                format!("condition must be Bool, got {}", v.ty.name()),
                e.span,
            );
        }
        Ok(v.c)
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        match &s.kind {
            StmtKind::VarDecl { name, is_const, ty, init, .. } => {
                if name == "self" || name == "super" {
                    return err(format!("'{}' is a reserved name", name), s.span);
                }
                let declared = match ty {
                    Some(t) => Some(self.resolve_type(t, s.span)?),
                    None => None,
                };
                let v = self.hinted(init, declared)?;
                let (c, t) = match declared {
                    Some(t) => match self.coerce(v.c, v.ty, t) {
                        Some(c) => (c, t),
                        None => {
                            let h = self.null_hint(v.ty, t);
                            return err(
                                format!(
                                    "cannot initialise '{}' of type {} with a value of type {}{}",
                                    name,
                                    t.name(),
                                    v.ty.name(),
                                    h
                                ),
                                s.span,
                            )
                        }
                    },
                    None => {
                        if v.ty == Ty::Null {
                            return err(
                                "cannot infer a type from null; add a type annotation",
                                s.span,
                            );
                        }
                        (v.c, v.ty)
                    }
                };
                let is_global = !self.cx.in_fn && self.cx.scopes.len() == 1;
                if is_global {
                    if self.globals.contains_key(name) {
                        return err(format!("'{}' is already declared", name), s.span);
                    }
                    let cname = format!("g_{}", name);
                    let tc = self.cty(t);
                    self.global_decls.push_str(&format!("static {} {};\n", tc, cname));
                    if t.has_ptrs() {
                        self.global_roots.push(cname.clone());
                    }
                    self.line(&format!("{} = {};", cname, c));
                    self.globals
                        .insert(name.clone(), Var { ty: t, is_const: *is_const, cname, shared: true });
                } else {
                    if self.cx.scopes.last().unwrap().contains_key(name) {
                        return err(format!("'{}' is already declared in this scope", name), s.span);
                    }
                    let cname = format!("v_{}_{}", name, self.fresh());
                    let tc = self.cty(t);
                    let q = if self.cx.volatile.contains(name) { " volatile" } else { "" };
                    self.line(&format!("{}{} {} = {};", tc, q, cname, c));
                    self.cx
                        .scopes
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), Var { ty: t, is_const: *is_const, cname, shared: false });
                }
                Ok(())
            }
            StmtKind::Assign { target, op, value } => {
                let ExprKind::Ident(name) = &target.kind else {
                    return self.assign_other(target, *op, value, s.span);
                };
                let var = if op.is_none() {
                    self.lookup_raw(name, target.span)?
                } else {
                    self.lookup(name, target.span)?
                };
                if var.is_const {
                    return err(format!("cannot assign to constant '{}'", name), s.span);
                }
                let v = match op {
                    None => self.hinted(value, Some(var.ty))?,
                    Some(bop) => {
                        let cur = E::new(var.cname.clone(), var.ty, true);
                        let rhs = self.val(value)?;
                        self.binary(*bop, cur, rhs, s.span)?
                    }
                };
                let Some(c) = self.coerce(v.c, v.ty, var.ty) else {
                    let hint = if op == &Some(BinOp::Div) && var.ty == Ty::Int {
                        " ('/' always produces Float; use '//' for integer division)"
                    } else {
                        ""
                    };
                    let hint2 = self.null_hint(v.ty, var.ty);
                    return err(
                        format!(
                            "cannot assign {} to '{}' which has type {}{}{}",
                            v.ty.name(),
                            name,
                            var.ty.name(),
                            hint,
                            hint2
                        ),
                        s.span,
                    );
                };
                self.line(&format!("{} = {};", var.cname, c));
                if op.is_none() && !var.cname.starts_with('(') {
                    self.cx.narrowed.remove(&var.cname);
                    if let (Ty::Opt(inner), false) = (var.ty, matches!(v.ty, Ty::Null | Ty::Opt(_))) {
                        self.add_narrow(&var.cname, *inner);
                    }
                }
                Ok(())
            }
            StmtKind::Expr(e) => {
                let v = self.expr(e)?;
                if v.ty == Ty::Void {
                    self.line(&format!("{};", v.c));
                } else {
                    self.line(&format!("(void)({});", v.c));
                }
                Ok(())
            }
            StmtKind::If { branches, else_body } => self.if_stmt(branches, else_body),
            StmtKind::While { cond, body } => self.while_stmt(cond, body),
            StmtKind::Repeat { count, body } => self.repeat_stmt(count, body),
            StmtKind::For { var, iter, body } => {
                let ExprKind::Range { start, end, inclusive } = &iter.kind else {
                    return self.for_each(var, iter, body);
                };
                let a = self.val(start)?;
                let b = self.val(end)?;
                if a.ty != Ty::Int || b.ty != Ty::Int {
                    return err(
                        format!(
                            "range bounds must be Int, got {} and {}",
                            a.ty.name(),
                            b.ty.name()
                        ),
                        iter.span,
                    );
                }
                self.forget_assigned(body);
                let n = self.fresh();
                let cname = format!("v_{}_{}", var, n);
                self.line("{");
                self.cx.indent += 1;
                self.line(&format!("int64_t e{} = {};", n, b.c));
                let cmp = if *inclusive { "<=" } else { "<" };
                self.line(&format!(
                    "for (int64_t {} = {}; {} {} e{}; {}++) {{",
                    cname, a.c, cname, cmp, n, cname
                ));
                self.push_scope();
                self.cx.scopes.last_mut().unwrap().insert(
                    var.clone(),
                    Var { ty: Ty::Int, is_const: true, cname, shared: false },
                );
                self.cx.loop_depth += 1;
                for st in body {
                    self.stmt(st)?;
                }
                self.cx.loop_depth -= 1;
                self.pop_scope();
                self.line("}");
                self.cx.indent -= 1;
                self.line("}");
                Ok(())
            }
            StmtKind::Match { subject, cases, default } => {
                let sv = self.val(subject)?;
                let n = self.fresh();
                self.line("{");
                self.cx.indent += 1;
                let tc = self.cty(sv.ty);
                self.line(&format!("{} m{} = {};", tc, n, sv.c));
                let mut first = true;
                for (pat, body) in cases {
                    let pv = self.pattern(pat)?;
                    let subj = E::new(format!("m{}", n), sv.ty, true);
                    let test = self.binary(BinOp::Eq, subj, pv, pat.span)?;
                    if first {
                        self.line(&format!("if ({}) {{", test.c));
                    } else {
                        self.line(&format!("}} else if ({}) {{", test.c));
                    }
                    first = false;
                    self.block(body)?;
                }
                if let Some(d) = default {
                    if first {
                        self.line("{");
                    } else {
                        self.line("} else {");
                    }
                    self.block(d)?;
                    first = false;
                }
                if !first {
                    self.line("}");
                }
                self.cx.indent -= 1;
                self.line("}");
                Ok(())
            }
            StmtKind::Return(v) => {
                if !self.cx.in_fn {
                    return err("'return' outside of a function", s.span);
                }
                match v {
                    None => {
                        if matches!(self.cx.ret_decl, Some(t) if t != Ty::Void) {
                            return err("this function must return a value", s.span);
                        }
                        self.cx.rets.push(Ty::Void);
                        self.emit_jump(Jump::Return(None));
                    }
                    Some(e) => {
                        let hint = self.cx.ret_decl;
                        let v = self.hinted(e, hint)?;
                        let c = match self.cx.ret_decl {
                            Some(t) => match self.coerce(v.c, v.ty, t) {
                                Some(c) => c,
                                None => {
                                    return err(
                                        format!(
                                            "returning {} from a function declared to return {}",
                                            v.ty.name(),
                                            t.name()
                                        ),
                                        s.span,
                                    )
                                }
                            },
                            None => v.c,
                        };
                        let rty = match self.cx.ret_decl {
                            Some(t) => t,
                            None => v.ty,
                        };
                        self.cx.rets.push(v.ty);
                        if rty == Ty::Void {
                            return err("this function returns nothing, so it cannot return a value", s.span);
                        }
                        self.emit_jump(Jump::Return(Some((c, rty))));
                    }
                }
                Ok(())
            }
            StmtKind::Break => {
                if self.cx.loop_depth == 0 {
                    return err("'break' outside of a loop", s.span);
                }
                self.emit_jump(Jump::Break);
                Ok(())
            }
            StmtKind::Continue => {
                if self.cx.loop_depth == 0 {
                    return err("'continue' outside of a loop", s.span);
                }
                self.emit_jump(Jump::Continue);
                Ok(())
            }
            StmtKind::Pass => Ok(()),
            StmtKind::Fn(f) => err(
                format!(
                    "nested function declarations are not supported in V1 (declare '{}' at module level)",
                    f.name
                ),
                f.span,
            ),
            StmtKind::Class(c) => err(
                format!("class '{}' must be declared at module level, not inside a function or block", c.name),
                s.span,
            ),
            StmtKind::Try { body, catch, finally } => self.try_stmt(body, catch, finally),
            StmtKind::Throw(e) => self.throw_stmt(e, s.span),
            StmtKind::Use { .. } => err("'use' must be at the top level of a file", s.span),
            StmtKind::Manages(_) => nyi("'manages'", s.span),
        }
    }

    fn pattern(&mut self, e: &Expr) -> R<E> {
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Null => self.val(e),
            ExprKind::Unary(UnOp::Neg, x) if matches!(x.kind, ExprKind::Int(_) | ExprKind::Float(_)) => {
                self.val(e)
            }
            _ => err("'case' patterns must be literal values in V1", e.span),
        }
    }
}
