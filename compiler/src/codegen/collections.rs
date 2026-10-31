//! Lists, tuples and mixed-type (`Any`) values: C type generation, literals,
//! indexing, methods (including `filter`), elementwise arithmetic and
//! formatting. Lives in a child module so it can use the checker's internals.
//!
//! Representation (decided at compile time, never guessed at run time):
//!   * `List[T]`  -> pointer to a generated struct { len, cap, T *data }.
//!     `List[Int]` and `List[Float]` are plain contiguous numeric arrays.
//!     Lists have reference semantics: `var b = a` aliases, `a.copy()` clones.
//!   * `Tuple[A, B]` -> generated struct passed by value; immutable.
//!   * `Any` -> 16-byte tagged `SnVal`; only the element type of mixed lists.

use super::*;

fn expect_args(name: &str, args: &[Arg], k: usize, span: Span) -> R<()> {
    if args.len() == k {
        Ok(())
    } else {
        err(
            format!("'{}' takes {} argument(s), but {} were given", name, k, args.len()),
            span,
        )
    }
}

fn type_pred(name: &str) -> Option<Ty> {
    match name {
        "str" | "String" => Some(Ty::Str),
        "int" | "Int" => Some(Ty::Int),
        "float" | "Float" => Some(Ty::Float),
        "bool" | "Bool" => Some(Ty::Bool),
        _ => None,
    }
}

impl<'a> Gen<'a> {
    // ---------------- C types ----------------

    pub(super) fn cty(&mut self, t: Ty) -> String {
        match t {
            Ty::Int => "int64_t".into(),
            Ty::Float => "double".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "SnStr*".into(),
            Ty::Null => "int8_t".into(),
            Ty::Void => "void".into(),
            Ty::Any => "SnVal".into(),
            Ty::Class(c) => format!("SnO_{}*", c),
            Ty::Task(_) => "SnTask*".into(),
            Ty::List(_) | Ty::Dict(..) => format!("{}*", self.type_name(t)),
            Ty::Tuple(_) => self.type_name(t),
        }
    }

    /// Name of the generated C struct for a list or tuple type.
    pub(super) fn type_name(&mut self, t: Ty) -> String {
        if let Some(n) = self.types.get(&t) {
            return n.clone();
        }
        match t {
            Ty::List(elem) => {
                let ec = self.cty(*elem);
                let name = format!("SnL{}", self.types.len());
                self.type_decls.push_str(&format!(
                    "SN_DEFINE_LIST({}, {}, {})\n",
                    name,
                    ec,
                    if elem.has_ptrs() { 0 } else { 1 }
                ));
                match *elem {
                    Ty::Int | Ty::Float => self
                        .type_decls
                        .push_str(&format!("SN_DEFINE_LIST_NUM({}, {})\n", name, ec)),
                    Ty::Str => self
                        .type_decls
                        .push_str(&format!("SN_DEFINE_LIST_STRSORT({})\n", name)),
                    _ => {}
                }
                self.types.insert(t, name.clone());
                name
            }
            Ty::Dict(kt, vt) => self.dict_type_name(*kt, *vt),
            Ty::Tuple(items) => {
                let mut fields = String::new();
                for (i, it) in items.iter().enumerate() {
                    let c = self.cty(*it);
                    fields.push_str(&format!("{} f{}; ", c, i + 1));
                }
                let name = format!("SnT{}", self.types.len());
                self.type_decls
                    .push_str(&format!("typedef struct {{ {}}} {};\n", fields, name));
                self.types.insert(t, name.clone());
                name
            }
            _ => unreachable!("not a compound type"),
        }
    }

    // ---------------- literals ----------------

    /// Compile an expression; a list literal adopts the element type of `hint`.
    pub(super) fn hinted(&mut self, e: &Expr, hint: Option<Ty>) -> R<E> {
        if let (ExprKind::List(items), Some(Ty::List(inner))) = (&e.kind, hint) {
            return self.list_literal(items, Some(*inner), e.span);
        }
        if let (ExprKind::Dict(items), Some(Ty::Dict(kt, vt))) = (&e.kind, hint) {
            return self.dict_literal(items, Some((*kt, *vt)), e.span);
        }
        self.val(e)
    }

    pub(super) fn unify_elems(&self, es: &[E], span: Span) -> R<Ty> {
        let first = es[0].ty;
        if es.iter().all(|e| e.ty == first) {
            return Ok(first);
        }
        if es.iter().all(|e| e.ty.is_num()) {
            return Ok(Ty::Float);
        }
        if es.iter().all(|e| e.ty.is_scalar() || e.ty == Ty::Any) {
            return Ok(Ty::Any);
        }
        if es.iter().all(|e| matches!(e.ty, Ty::Class(_))) {
            // The nearest class that all the elements are instances of.
            let Ty::Class(first_c) = first else { unreachable!() };
            let mut cur = Some(first_c as usize);
            while let Some(c) = cur {
                if es.iter().all(|e| matches!(e.ty, Ty::Class(k) if self.is_subclass(k as usize, c))) {
                    return Ok(Ty::Class(c as u32));
                }
                cur = self.classes[c].parent;
            }
            return err(
                "list elements are instances of unrelated classes; give them a common parent class",
                span,
            );
        }
        let names: Vec<String> = es.iter().map(|e| e.ty.name()).collect();
        err(
            format!(
                "list elements ({}) cannot share one list; mixed lists may only contain Int, Float, Bool, String and null",
                names.join(", ")
            ),
            span,
        )
    }

    pub(super) fn list_literal(&mut self, items: &[Expr], hint: Option<Ty>, span: Span) -> R<E> {
        let mut es = Vec::new();
        for it in items {
            es.push(self.hinted(it, hint)?);
        }
        let elem = match hint {
            Some(t) => t,
            None => {
                if es.is_empty() {
                    return err(
                        "cannot infer the element type of an empty list; annotate it, e.g. var xs: List[Int] = []",
                        span,
                    );
                }
                self.unify_elems(&es, span)?
            }
        };
        let lty = Ty::list_of(elem);
        let ln = self.type_name(lty);
        let n = self.fresh();
        let l = format!("l{}", n);
        let mut s = format!("({{ {}* {} = {}_new({}); ", ln, l, ln, es.len());
        let mut pure = true;
        for (e, it) in es.into_iter().zip(items) {
            pure &= e.pure;
            let ety = e.ty;
            let Some(c) = self.coerce(e.c, ety, elem) else {
                return err(
                    format!("a {} value does not fit in a List[{}]", ety.name(), elem.name()),
                    it.span,
                );
            };
            s.push_str(&format!("{}_push({}, {}); ", ln, l, c));
        }
        s.push_str(&format!("{}; }})", l));
        Ok(E::new(s, lty, pure))
    }

    pub(super) fn tuple_literal(&mut self, items: &[Expr], _span: Span) -> R<E> {
        let mut es = Vec::new();
        for it in items {
            es.push(self.val(it)?);
        }
        let ty = Ty::tuple_of(es.iter().map(|e| e.ty).collect());
        let tn = self.cty(ty);
        let impure = es.iter().filter(|e| !e.pure).count();
        let text = if impure >= 2 {
            let n = self.fresh();
            let mut s = String::from("({ ");
            let mut names = Vec::new();
            for (i, e) in es.iter().enumerate() {
                let tc = self.cty(e.ty);
                s.push_str(&format!("{} t{}_{} = {}; ", tc, n, i, e.c));
                names.push(format!("t{}_{}", n, i));
            }
            s.push_str(&format!("({}){{{}}}; }})", tn, names.join(", ")));
            s
        } else {
            format!(
                "(({}){{{}}})",
                tn,
                es.iter().map(|e| e.c.clone()).collect::<Vec<_>>().join(", ")
            )
        };
        Ok(E::new(text, ty, impure == 0))
    }

    // ---------------- indexing ----------------

    pub(super) fn index_expr(&mut self, base: &Expr, idx: &Expr, span: Span) -> R<E> {
        let b = self.val(base)?;
        match b.ty {
            Ty::List(elem) => {
                let i = self.val(idx)?;
                if i.ty != Ty::Int {
                    return err(format!("list index must be Int, got {}", i.ty.name()), idx.span);
                }
                let ln = self.type_name(b.ty);
                let loc = self.loc(span);
                let c = self.seq2(&b, &i, move |a, x| {
                    format!("(*{}_at({}, {}, {}))", ln, a, x, loc)
                });
                // Reading an element depends on the list's current contents, so it is
                // not order-independent (a call elsewhere in the expression could change it).
                Ok(E::new(c, *elem, false))
            }
            Ty::Dict(..) => self.dict_index(b, idx, span),
            Ty::Tuple(items) => match &idx.kind {
                ExprKind::Int(n) => {
                    if *n < 1 || *n as usize > items.len() {
                        return err(
                            format!(
                                "tuple index {} is out of range (the tuple has {} element(s); indexes start at 1)",
                                n,
                                items.len()
                            ),
                            idx.span,
                        );
                    }
                    Ok(E::new(
                        format!("({}).f{}", b.c, n),
                        items[(*n - 1) as usize],
                        b.pure,
                    ))
                }
                _ => err(
                    "tuple indexes must be integer literals, because tuple elements can have different types",
                    idx.span,
                ),
            },
            Ty::Str => nyi("string indexing", span),
            other => err(format!("a value of type {} cannot be indexed", other.name()), span),
        }
    }

    /// `xs[i] = v` and `xs[i] += v`.
    pub(super) fn assign_other(
        &mut self,
        target: &Expr,
        op: Option<BinOp>,
        value: &Expr,
        span: Span,
    ) -> R<()> {
        if let ExprKind::Member(ox, fname) = &target.kind {
            return self.assign_field(ox, fname, op, value, span);
        }
        let ExprKind::Index(bx, ix) = &target.kind else {
            return err("this expression cannot be assigned to", span);
        };
        let b = self.val(bx)?;
        if let Ty::Dict(kt, vt) = b.ty {
            return self.dict_assign(b, (*kt, *vt), ix, op, value, span);
        }
        let elem = match b.ty {
            Ty::List(e) => *e,
            Ty::Tuple(_) => return err("tuples are immutable; build a new tuple instead", span),
            t => return err(format!("cannot assign into an index of {}", t.name()), span),
        };
        let i = self.val(ix)?;
        if i.ty != Ty::Int {
            return err(format!("list index must be Int, got {}", i.ty.name()), ix.span);
        }
        let rhs = match op {
            None => self.hinted(value, Some(elem))?,
            Some(_) => self.val(value)?,
        };
        let n = self.fresh();
        let (pn, rv) = (format!("p{}", n), format!("r{}", n));
        let combined = match op {
            None => E::new(rv.clone(), rhs.ty, true),
            Some(bop) => {
                let cur = E::new(format!("(*{})", pn), elem, true);
                self.binary(bop, cur, E::new(rv.clone(), rhs.ty, true), span)?
            }
        };
        let cty_ = combined.ty;
        let Some(c) = self.coerce(combined.c, cty_, elem) else {
            return err(
                format!("cannot store {} in a List[{}]", cty_.name(), elem.name()),
                span,
            );
        };
        let ln = self.type_name(b.ty);
        let ec = self.cty(elem);
        let rc = self.cty(rhs.ty);
        let loc = self.loc(target.span);
        // The right-hand side is evaluated before the element pointer is taken,
        // because evaluating it may itself grow (and move) the list.
        self.line(&format!(
            "{{ {ln}* l{n} = {b}; int64_t i{n} = {i}; {rc} {rv} = {r}; {ec}* {pn} = {ln}_at(l{n}, i{n}, {loc}); *{pn} = {c}; }}",
            ln = ln,
            n = n,
            b = b.c,
            i = i.c,
            rc = rc,
            rv = rv,
            r = rhs.c,
            ec = ec,
            pn = pn,
            loc = loc,
            c = c
        ));
        Ok(())
    }

    /// `obj.field = v` and `obj.field += v`.
    fn assign_field(
        &mut self,
        ox: &Expr,
        fname: &str,
        op: Option<BinOp>,
        value: &Expr,
        span: Span,
    ) -> R<()> {
        if matches!(&ox.kind, ExprKind::Ident(n) if n == "super") {
            return err("'super' can only be used as super.method(...)", span);
        }
        let o = self.val(ox)?;
        let Ty::Class(c) = o.ty else {
            return err(format!("cannot assign to a field of a {}", o.ty.name()), span);
        };
        let f = self.field_of(c as usize, fname, span)?;
        if f.is_const {
            return err(format!("cannot assign to constant field '{}'", fname), span);
        }
        let rhs = match op {
            None => self.hinted(value, Some(f.ty))?,
            Some(_) => self.val(value)?,
        };
        let n = self.fresh();
        let rv = format!("r{}", n);
        let combined = match op {
            None => E::new(rv.clone(), rhs.ty, true),
            Some(bop) => {
                let cur = E::new(format!("o{}->f_{}", n, fname), f.ty, true);
                self.binary(bop, cur, E::new(rv.clone(), rhs.ty, true), span)?
            }
        };
        let cty_ = combined.ty;
        let Some(cv) = self.coerce(combined.c, cty_, f.ty) else {
            return err(
                format!("cannot assign {} to field '{}' which has type {}", cty_.name(), fname, f.ty.name()),
                span,
            );
        };
        let oc = self.cty(o.ty);
        let rc = self.cty(rhs.ty);
        self.line(&format!(
            "{{ {oc} o{n} = {obj}; {rc} {rv} = {r}; o{n}->f_{f} = {cv}; }}",
            oc = oc, n = n, obj = o.c, rc = rc, rv = rv, r = rhs.c, f = fname, cv = cv
        ));
        Ok(())
    }

    /// `for x in some_list:`
    pub(super) fn for_each(&mut self, var: &str, iter: &Expr, body: &[Stmt]) -> R<()> {
        let mut l = self.val(iter)?;
        if let Ty::Dict(..) = l.ty {
            // Iterating a dictionary visits its keys (a snapshot, so the body may modify it).
            l = self.dict_keys(l);
        }
        let Ty::List(elem) = l.ty else {
            return err(
                format!("cannot iterate over a value of type {}; use a range or a list", l.ty.name()),
                iter.span,
            );
        };
        let ln = self.type_name(l.ty);
        let ec = self.cty(*elem);
        let n = self.fresh();
        let cname = format!("v_{}_{}", var, n);
        self.line("{");
        self.cx.indent += 1;
        self.line(&format!("{}* l{} = {};", ln, n, l.c));
        self.line(&format!(
            "for (int64_t i{n} = 0; i{n} < l{n}->len; i{n}++) {{",
            n = n
        ));
        self.push_scope();
        self.line(&format!("{} {} = l{}->data[i{}];", ec, cname, n, n));
        self.cx.scopes.last_mut().unwrap().insert(
            var.to_string(),
            Var { ty: *elem, is_const: true, cname, shared: false },
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

    // ---------------- methods ----------------

    pub(super) fn method_call(
        &mut self,
        obj: &Expr,
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> R<E> {
        let o = self.val(obj)?;
        if let Ty::Class(c) = o.ty {
            return self.call_method(o, c as usize, name, args, span, false);
        }
        if let Ty::Dict(kt, vt) = o.ty {
            return self.dict_method(o, (*kt, *vt), name, args, span);
        }
        let Ty::List(elem_ref) = o.ty else {
            return err(format!("{} has no method '{}'", o.ty.name(), name), span);
        };
        let elem: Ty = *elem_ref;
        if args.iter().any(|a| a.name.is_some()) && name != "filter" {
            return err("list methods do not accept named arguments", span);
        }
        let ln = self.type_name(o.ty);
        let ec = self.cty(elem);
        let loc = self.loc(span);
        let n = self.fresh();
        let l = format!("l{}", n);
        let head = format!("{}* {} = {};", ln, l, o.c);
        match name {
            "length" => {
                expect_args(name, args, 0, span)?;
                Ok(E::new(format!("({})->len", o.c), Ty::Int, o.pure))
            }
            "add" => {
                expect_args(name, args, 1, span)?;
                let v = self.hinted(&args[0].value, Some(elem))?;
                let vty = v.ty;
                let Some(c) = self.coerce(v.c, vty, elem) else {
                    return err(
                        format!("cannot add a {} value to a {}", vty.name(), o.ty.name()),
                        span,
                    );
                };
                Ok(E::new(
                    format!("({{ {head} {ec} v{n} = {c}; {ln}_push({l}, v{n}); }})"),
                    Ty::Void,
                    false,
                ))
            }
            "remove" | "contains" => {
                expect_args(name, args, 1, span)?;
                let v = self.val(&args[0].value)?;
                let vty = v.ty;
                let Some(c) = self.coerce(v.c, vty, elem) else {
                    return err(
                        format!("a {} cannot contain a {} value", o.ty.name(), vty.name()),
                        span,
                    );
                };
                let x = E::new(format!("x{}", n), elem, true);
                let cur = E::new(format!("{}->data[i{}]", l, n), elem, true);
                let test = self.binary(BinOp::Eq, cur, x, span)?;
                let del = if name == "remove" {
                    format!(
                        "memmove(&{l}->data[i{n}], &{l}->data[i{n} + 1], sizeof({ec}) * (size_t)({l}->len - i{n} - 1)); {l}->len--; "
                    )
                } else {
                    String::new()
                };
                let test_c = test.c;
                Ok(E::new(
                    format!(
                        "({{ {head} {ec} x{n} = {c}; bool found{n} = false; for (int64_t i{n} = 0; i{n} < {l}->len; i{n}++) {{ if ({test_c}) {{ {del}found{n} = true; break; }} }} found{n}; }})"
                    ),
                    Ty::Bool,
                    false,
                ))
            }
            "merge" => {
                expect_args(name, args, 1, span)?;
                let other = self.val(&args[0].value)?;
                if other.ty != o.ty {
                    return err(
                        format!("merge() needs a {}, got {}", o.ty.name(), other.ty.name()),
                        span,
                    );
                }
                Ok(E::new(
                    format!("({{ {head} {ln}* o{n} = {}; {ln}_merge({l}, o{n}); }})", other.c),
                    Ty::Void,
                    false,
                ))
            }
            "copy" => {
                expect_args(name, args, 0, span)?;
                Ok(E::new(format!("{}_copy({})", ln, o.c), o.ty, o.pure))
            }
            "clear" | "reverse" => {
                expect_args(name, args, 0, span)?;
                Ok(E::new(
                    format!("({{ {head} {ln}_{name}({l}); }})"),
                    Ty::Void,
                    false,
                ))
            }
            "sum" | "min" | "max" | "mean" => {
                expect_args(name, args, 0, span)?;
                if !elem.is_num() {
                    return err(
                        format!("{}() needs a list of numbers, got {}", name, o.ty.name()),
                        span,
                    );
                }
                let (c, ty) = match name {
                    "sum" => (format!("{}_sum({})", ln, o.c), elem),
                    "mean" => (format!("{}_mean({}, {})", ln, o.c, loc), Ty::Float),
                    _ => (format!("{}_{}({}, {})", ln, name, o.c, loc), elem),
                };
                Ok(E::new(c, ty, false))
            }
            "sort" => {
                expect_args(name, args, 0, span)?;
                if !(elem.is_num() || elem == Ty::Str) {
                    return err(
                        format!("sort() needs a list of numbers or strings, got {}", o.ty.name()),
                        span,
                    );
                }
                Ok(E::new(
                    format!("({{ {head} {ln}_sort({l}); }})"),
                    Ty::Void,
                    false,
                ))
            }
            "filter" => self.filter_call(&o, elem, &ln, &ec, args, span),
            _ => err(format!("{} has no method '{}'", o.ty.name(), name), span),
        }
    }

    fn filter_call(
        &mut self,
        o: &E,
        elem: Ty,
        ln: &str,
        ec: &str,
        args: &[Arg],
        span: Span,
    ) -> R<E> {
        let usage = "filter() needs a predicate: a type (str, int, float, bool), a comparison such as '< 4', or the name of a function";
        if args.is_empty() {
            return err(usage, span);
        }
        let n = self.fresh();
        let l = format!("l{}", n);
        let x = format!("x{}", n);
        let mut pre = String::new();
        let first = &args[0].value;

        let type_test = |target: Ty, x: &str| -> String {
            if elem == Ty::Any {
                let tag = match target {
                    Ty::Int => "SN_T_INT",
                    Ty::Float => "SN_T_FLOAT",
                    Ty::Bool => "SN_T_BOOL",
                    Ty::Str => "SN_T_STR",
                    _ => "SN_T_NULL",
                };
                format!("({}.tag == {})", x, tag)
            } else if elem == target {
                "true".to_string()
            } else {
                "false".to_string()
            }
        };

        // When the predicate is a type, the result list has that (narrower) element type.
        let mut narrow: Option<(Ty, String)> = None;
        let cond: String = match &first.kind {
            ExprKind::Predicate(op, operand) => {
                if args.len() != 1 {
                    return err("a comparison predicate takes no extra arguments", span);
                }
                let pv = self.val(operand)?;
                let pty = pv.ty;
                let pc = self.cty(pty);
                pre.push_str(&format!("{} p{} = {}; ", pc, n, pv.c));
                let test = self.binary(
                    *op,
                    E::new(x.clone(), elem, true),
                    E::new(format!("p{}", n), pty, true),
                    first.span,
                )?;
                test.c
            }
            ExprKind::Null if args.len() == 1 => type_test(Ty::Null, &x),
            ExprKind::Ident(name) if self.class_ix.contains_key(name) => {
                if args.len() != 1 {
                    return err("a type predicate takes no extra arguments", span);
                }
                let k = self.class_ix[name];
                let Ty::Class(e) = elem else {
                    return err(
                        format!("cannot filter a {} by class {}", o.ty.name(), name),
                        first.span,
                    );
                };
                let e = e as usize;
                if self.is_subclass(e, k) {
                    "true".to_string() // every element already is a `name`
                } else if self.is_subclass(k, e) {
                    let (lo, hi) = (self.classes[k].id_lo, self.classes[k].id_hi);
                    narrow = Some((Ty::Class(k as u32), format!("((SnO_{}*){})", k, x)));
                    format!("({x}->cls >= {lo} && {x}->cls <= {hi})", x = x, lo = lo, hi = hi)
                } else {
                    return err(
                        format!(
                            "no element of a {} can be a {}: the classes are unrelated",
                            o.ty.name(),
                            name
                        ),
                        first.span,
                    );
                }
            }
            ExprKind::Ident(name) if !self.fns.contains_key(name) && type_pred(name).is_some() => {
                if args.len() != 1 {
                    return err("a type predicate takes no extra arguments", span);
                }
                let target = type_pred(name).unwrap();
                if elem == Ty::Any {
                    let field = match target {
                        Ty::Int => "i",
                        Ty::Float => "f",
                        Ty::Bool => "b",
                        _ => "s",
                    };
                    narrow = Some((target, format!("{}.u.{}", x, field)));
                }
                type_test(target, &x)
            }
            ExprKind::Ident(name) if self.fns.contains_key(name) => {
                // Lower to the call `name($x, $p0, ...)` where `$x` is the current element and
                // the extra arguments are evaluated once, before the loop.
                let mut scope: HashMap<String, Var> = HashMap::new();
                scope.insert(
                    "$x".to_string(),
                    Var { ty: elem, is_const: true, cname: x.clone(), shared: false },
                );
                let mut cargs = vec![Arg {
                    name: None,
                    value: Expr { kind: ExprKind::Ident("$x".into()), span: first.span },
                }];
                for (k, a) in args[1..].iter().enumerate() {
                    let v = self.val(&a.value)?;
                    let vty = v.ty;
                    let pc = self.cty(vty);
                    let pname = format!("p{}_{}", n, k);
                    pre.push_str(&format!("{} {} = {}; ", pc, pname, v.c));
                    let vn = format!("$p{}", k);
                    scope.insert(vn.clone(), Var { ty: vty, is_const: true, cname: pname, shared: false });
                    cargs.push(Arg {
                        name: a.name.clone(),
                        value: Expr { kind: ExprKind::Ident(vn), span: a.value.span },
                    });
                }
                self.cx.scopes.push(scope);
                let callee = Expr { kind: ExprKind::Ident(name.clone()), span: first.span };
                let r = self.call(&callee, &cargs, span);
                self.cx.scopes.pop();
                let r = r?;
                if r.ty != Ty::Bool {
                    return err(
                        format!(
                            "the filter function '{}' must return Bool, but it returns {}",
                            name,
                            r.ty.name()
                        ),
                        first.span,
                    );
                }
                r.c
            }
            _ => return err(usage, first.span),
        };
        let head = format!("{}* {} = {};", ln, l, o.c);
        let (rty, push_v) = match &narrow {
            Some((t, conv)) => (Ty::list_of(*t), conv.clone()),
            None => (o.ty, x.clone()),
        };
        let rln = self.type_name(rty);
        Ok(E::new(
            format!(
                "({{ {head} {pre}{rln}* r{n} = {rln}_new({l}->len); for (int64_t i{n} = 0; i{n} < {l}->len; i{n}++) {{ {ec} {x} = {l}->data[i{n}]; if ({cond}) {rln}_push(r{n}, {push_v}); }} r{n}; }})"
            ),
            rty,
            false,
        ))
    }

    // ---------------- arithmetic ----------------

    /// Elementwise arithmetic: list-list, list-number and number-list.
    pub(super) fn list_binary(&mut self, op: BinOp, l: E, r: E, span: Span) -> R<E> {
        use BinOp::*;
        if !matches!(op, Add | Sub | Mul | Div | FloorDiv | Mod | Pow) {
            return err(
                format!(
                    "operator '{}' cannot be applied to lists; use filter() or a loop",
                    op_sym(op)
                ),
                span,
            );
        }
        let loc = self.loc(span);
        let n = self.fresh();
        let elem_of = |t: Ty| match t {
            Ty::List(e) => Some(*e),
            _ => None,
        };
        let (le, re) = (elem_of(l.ty), elem_of(r.ty));
        let lt = le.unwrap_or(l.ty);
        let rt = re.unwrap_or(r.ty);
        if !lt.is_num() || !rt.is_num() {
            return err(
                format!(
                    "elementwise arithmetic needs lists of numbers and numbers, got {} and {}",
                    l.ty.name(),
                    r.ty.name()
                ),
                span,
            );
        }
        let (ltmp, rtmp) = (format!("la{}", n), format!("lb{}", n));
        let lelem = if le.is_some() { format!("{}->data[i{}]", ltmp, n) } else { ltmp.clone() };
        let relem = if re.is_some() { format!("{}->data[i{}]", rtmp, n) } else { rtmp.clone() };
        let res = self.binary(op, E::new(lelem, lt, true), E::new(relem, rt, true), span)?;
        let rl = Ty::list_of(res.ty);
        let rn = self.type_name(rl);
        let (lc, rc) = (self.cty(l.ty), self.cty(r.ty));
        let mut s = format!("({{ {} {} = {}; {} {} = {}; ", lc, ltmp, l.c, rc, rtmp, r.c);
        let len = if le.is_some() { format!("{}->len", ltmp) } else { format!("{}->len", rtmp) };
        if le.is_some() && re.is_some() {
            s.push_str(&format!(
                "if ({a}->len != {b}->len) sn_panic({loc}, \"elementwise operation on lists of different lengths (%\" PRId64 \" and %\" PRId64 \")\", {a}->len, {b}->len); ",
                a = ltmp,
                b = rtmp,
                loc = loc
            ));
        }
        s.push_str(&format!(
            "{rn}* res{n} = {rn}_new({len}); for (int64_t i{n} = 0; i{n} < {len}; i{n}++) res{n}->data[i{n}] = {e}; res{n}->len = {len}; res{n}; }})",
            rn = rn,
            n = n,
            len = len,
            e = res.c
        ));
        Ok(E::new(s, rl, false))
    }

    pub(super) fn list_neg(&mut self, v: E, span: Span) -> R<E> {
        let Ty::List(elem) = v.ty else { unreachable!() };
        if !elem.is_num() {
            return err(format!("cannot negate a {}", v.ty.name()), span);
        }
        let ln = self.type_name(v.ty);
        let n = self.fresh();
        Ok(E::new(
            format!(
                "({{ {ln}* a{n} = {c}; {ln}* r{n} = {ln}_new(a{n}->len); for (int64_t i{n} = 0; i{n} < a{n}->len; i{n}++) r{n}->data[i{n}] = -(a{n}->data[i{n}]); r{n}->len = a{n}->len; r{n}; }})",
                ln = ln,
                n = n,
                c = v.c
            ),
            v.ty,
            false,
        ))
    }

    /// Operations on `Any` values: only equality is defined.
    pub(super) fn any_binary(&mut self, op: BinOp, l: E, r: E, span: Span) -> R<E> {
        let ok = |t: Ty| t.is_scalar() || t == Ty::Any;
        if matches!(op, BinOp::Eq | BinOp::Ne) && ok(l.ty) && ok(r.ty) {
            let (lt, rt) = (l.ty, r.ty);
            let pure = l.pure && r.pure;
            let ne = op == BinOp::Ne;
            let c = self.seq2(&l, &r, move |a, b| {
                let x = if lt == Ty::Any { a.to_string() } else { box_any(a, lt) };
                let y = if rt == Ty::Any { b.to_string() } else { box_any(b, rt) };
                if ne {
                    format!("(!sn_val_eq({}, {}))", x, y)
                } else {
                    format!("sn_val_eq({}, {})", x, y)
                }
            });
            return Ok(E::new(c, Ty::Bool, pure));
        }
        err(
            format!(
                "operator '{}' cannot be applied to {} and {}; values from a mixed-type list can only be compared with == and !=. Filter the list by type first, e.g. items.filter(int)",
                op_sym(op),
                l.ty.name(),
                r.ty.name()
            ),
            span,
        )
    }

    // ---------------- formatting ----------------

    /// C statement(s) appending `c` (of type `ty`) to the SnBuf named `b`.
    /// `top` is true for the outermost value (strings print bare); elements of
    /// lists and tuples are shown with quotes around strings.
    pub(super) fn buf_append(&mut self, b: &str, c: &str, ty: Ty, top: bool) -> String {
        match ty {
            Ty::Int => format!("sn_buf_int(&{}, {}); ", b, c),
            Ty::Float => format!("sn_buf_float(&{}, {}); ", b, c),
            Ty::Bool => format!("sn_buf_bool(&{}, {}); ", b, c),
            Ty::Null => format!("sn_buf_null(&{}, {}); ", b, c),
            Ty::Str => {
                if top {
                    format!("sn_buf_str(&{}, {}); ", b, c)
                } else {
                    format!("sn_buf_str_repr(&{}, {}); ", b, c)
                }
            }
            Ty::Any => {
                if top {
                    format!("sn_buf_val(&{}, {}); ", b, c)
                } else {
                    format!("sn_buf_val_repr(&{}, {}); ", b, c)
                }
            }
            Ty::List(_) | Ty::Dict(..) | Ty::Tuple(_) => {
                let f = self.fmt_fn(ty);
                format!("{}(&{}, {}); ", f, b, c)
            }
            Ty::Class(k) => {
                let k = k as usize;
                let has_to_string = self
                    .find_method(k, "toString")
                    .map_or(false, |(_, d)| d.params.is_empty() && !d.is_private);
                let call = if has_to_string {
                    let recv = E::new(c.to_string(), ty, true);
                    let sp = Span { line: 0, col: 0, file: 0 };
                    self.call_method(recv, k, "toString", &[], sp, false).ok().filter(|e| e.ty == Ty::Str)
                } else {
                    None
                };
                match call {
                    Some(e) => format!("sn_buf_str(&{}, {}); ", b, e.c),
                    None => {
                        let text = format!("<{}>", crate::modules::bare_name(&self.classes[k].name));
                        format!("sn_buf_text(&{}, \"{}\", {}); ", b, text, text.len())
                    }
                }
            }
            Ty::Task(_) => format!("sn_buf_text(&{}, \"<Task>\", 6); ", b),
            Ty::Void => String::new(),
        }
    }

    fn fmt_fn(&mut self, t: Ty) -> String {
        if let Some(n) = self.fmt_fns.get(&t) {
            return n.clone();
        }
        let (params, body) = match t {
            Ty::List(elem) => {
                let ln = self.type_name(t);
                let inner = self.buf_append("(*b)", "l->data[i]", *elem, false);
                (
                    format!("SnBuf *b, {} *l", ln),
                    format!(
                        "sn_buf_text(b, \"[\", 1); for (int64_t i = 0; i < l->len; i++) {{ if (i) sn_buf_text(b, \", \", 2); {} }} sn_buf_text(b, \"]\", 1);",
                        inner
                    ),
                )
            }
            Ty::Dict(kt, vt) => {
                let dn = self.type_name(t);
                let kc = self.buf_append("(*b)", "e->k", *kt, false);
                let vc = self.buf_append("(*b)", "e->v", *vt, false);
                (
                    format!("SnBuf *b, {} *d", dn),
                    format!(
                        "sn_buf_text(b, \"{{\", 1); int64_t n = 0; for (int64_t i = 0; i < d->used; i++) {{ {dn}_e *e = &d->ent[i]; if (!e->live) continue; if (n++) sn_buf_text(b, \", \", 2); {kc} sn_buf_text(b, \": \", 2); {vc} }} sn_buf_text(b, \"}}\", 1);",
                        dn = dn, kc = kc, vc = vc
                    ),
                )
            }
            Ty::Tuple(items) => {
                let tn = self.type_name(t);
                let mut body = String::from("sn_buf_text(b, \"(\", 1); ");
                for (k, it) in items.iter().enumerate() {
                    if k > 0 {
                        body.push_str("sn_buf_text(b, \", \", 2); ");
                    }
                    let piece = self.buf_append("(*b)", &format!("t.f{}", k + 1), *it, false);
                    body.push_str(&piece);
                }
                if items.len() == 1 {
                    body.push_str("sn_buf_text(b, \",\", 1); ");
                }
                body.push_str("sn_buf_text(b, \")\", 1);");
                (format!("SnBuf *b, {} t", tn), body)
            }
            _ => unreachable!("only lists and tuples have formatter functions"),
        };
        let name = format!("fmt{}", self.fmt_fns.len());
        self.fmt_decls
            .push_str(&format!("static void {}({}) {{ {} }}\n", name, params, body));
        self.fmt_fns.insert(t, name.clone());
        name
    }

    /// A C expression producing a `SnStr*` for a list, tuple or Any value.
    pub(super) fn to_str(&mut self, v: &E) -> String {
        let n = self.fresh();
        let b = format!("b{}", n);
        let piece = self.buf_append(&b, &v.c, v.ty, true);
        format!("({{ SnBuf {b}; sn_buf_init(&{b}); {piece}sn_buf_finish(&{b}); }})")
    }
}
