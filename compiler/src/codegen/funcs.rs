//! First-class functions: `Fn(A, B) -> R` values, calls through them, and the
//! list methods that take them (`map`, `each`, `reduce`, `any`, `all`).
//!
//! A function value is a plain C function pointer to one monomorphised
//! instance of a named function, so calling it costs the same as any call.
//! There are no lambdas: only names of top-level functions can become values.

use super::*;

impl<'a> Gen<'a> {
    /// Whether `name` is a variable (local, field of the current class, or global).
    pub(super) fn var_exists(&mut self, name: &str) -> bool {
        if self.cx.scopes.iter().any(|s| s.contains_key(name)) {
            return true;
        }
        if let Some(cc) = self.cx.cur_class {
            if self.ensure_layout(cc).is_ok() && self.classes[cc].fields.iter().any(|f| f.name == name) {
                return true;
            }
        }
        self.globals.contains_key(name)
    }

    /// The function `name` as a value. `ptys` are the parameter types it must be
    /// instantiated with; without them the function's own annotations are used.
    pub(super) fn func_value(&mut self, name: &str, ptys: Option<&[Ty]>, span: Span) -> R<E> {
        let decl = self.fns[name];
        let ptys: Vec<Ty> = match ptys {
            Some(p) => {
                if p.len() != decl.params.len() {
                    return err(
                        format!(
                            "function '{}' takes {} argument(s), but {} are expected here",
                            name,
                            decl.params.len(),
                            p.len()
                        ),
                        span,
                    );
                }
                for (i, (param, want)) in decl.params.iter().zip(p).enumerate() {
                    if let Some(t) = &param.ty {
                        let have = self.resolve_type(t, decl.span)?;
                        if have != *want {
                            return err(
                                format!(
                                    "parameter {} of '{}' is {}, but {} is expected here",
                                    i + 1,
                                    name,
                                    have.name(),
                                    want.name()
                                ),
                                span,
                            );
                        }
                    }
                }
                p.to_vec()
            }
            None => {
                let mut v = Vec::new();
                for param in &decl.params {
                    match &param.ty {
                        Some(t) => v.push(self.resolve_type(t, decl.span)?),
                        None => {
                            return err(
                                format!(
                                    "function '{}' has parameters without types, so it cannot be used as a value here; annotate its parameters, or give the receiving variable or parameter a type such as Fn(Int) -> Int",
                                    name
                                ),
                                span,
                            )
                        }
                    }
                }
                v
            }
        };
        let (cname, ret) = self.instance(decl, ptys.clone(), span)?;
        Ok(self.named_fn_value(&cname, &ptys, ret))
    }

    /// Evaluate call arguments left to right, then build the call text with `f`.
    /// Temporaries are only needed when more than one operand has side effects.
    pub(super) fn ordered(&mut self, parts: Vec<E>, f: impl Fn(&[String]) -> String) -> String {
        let impure = parts.iter().filter(|p| !p.pure).count();
        if impure < 2 {
            let cs: Vec<String> = parts.iter().map(|p| p.c.clone()).collect();
            return f(&cs);
        }
        let k = self.fresh();
        let mut s = String::from("({ ");
        let mut names = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            let tc = self.cty(p.ty);
            s.push_str(&format!("{} o{}_{} = {}; ", tc, k, i, p.c));
            names.push(format!("o{}_{}", k, i));
        }
        s.push_str(&f(&names));
        s.push_str("; })");
        s
    }

    /// `f(args)` where `f` is a variable of function type.
    pub(super) fn call_value(&mut self, f: E, ps: &'static [Ty], ret: Ty, args: &[Arg], span: Span) -> R<E> {
        if args.iter().any(|a| a.name.is_some()) {
            return err("functions called through a variable do not accept named arguments", span);
        }
        if args.len() != ps.len() {
            return err(
                format!("this function takes {} argument(s), but {} were given", ps.len(), args.len()),
                span,
            );
        }
        let mut parts = vec![f];
        for (a, p) in args.iter().zip(ps) {
            let v = self.hinted(&a.value, Some(*p))?;
            let vty = v.ty;
            let Some(c) = self.coerce(v.c, vty, *p) else {
                return err(
                    format!("argument has type {}, but the function expects {}", vty.name(), p.name()),
                    a.value.span,
                );
            };
            parts.push(E::new(c, *p, v.pure));
        }
        let fc = self.cty(parts[0].ty);
        let k = self.fresh();
        let text = self.ordered(parts, |c| {
            let rest = if c.len() > 1 { format!(", {}", c[1..].join(", ")) } else { String::new() };
            format!("({{ {fc} fv{k} = {f}; fv{k}.f(fv{k}.env{rest}); }})", fc = fc, k = k, f = c[0], rest = rest)
        });
        Ok(E::new(text, ret, false))
    }

    /// A function argument whose parameter types are known (for `map`, `reduce`, ...).
    fn func_arg(&mut self, e: &Expr, ptys: &[Ty], who: &str) -> R<(E, Ty)> {
        let v = match &e.kind {
            ExprKind::Ident(n) if self.fns.contains_key(n.as_str()) && !self.var_exists(n) => {
                self.func_value(n, Some(ptys), e.span)?
            }
            ExprKind::Lambda(l) => self.lambda_expr(l, Some(ptys), None, e.span)?,
            _ => self.val(e)?,
        };
        let Ty::Func(ps, r) = v.ty else {
            return err(
                format!("{} needs a function (the name of one), not a {}", who, v.ty.name()),
                e.span,
            );
        };
        if ps != ptys {
            let want = ptys.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ");
            return err(
                format!("{} needs a function taking ({}), but this one is {}", who, want, v.ty.name()),
                e.span,
            );
        }
        let ret = *r;
        Ok((v, ret))
    }

    pub(super) fn list_functional(&mut self, o: E, elem: Ty, name: &str, args: &[Arg], span: Span) -> R<E> {
        if args.iter().any(|a| a.name.is_some()) {
            return err("list methods do not accept named arguments", span);
        }
        let ln = self.type_name(o.ty);
        let n = self.fresh();
        let who = format!("{}()", name);
        let want = |k: usize| -> R<()> {
            if args.len() != k {
                err(format!("{}() takes {} argument(s), but {} were given", name, k, args.len()), span)
            } else {
                Ok(())
            }
        };
        match name {
            "join" => {
                want(1)?;
                let sep = self.val(&args[0].value)?;
                if sep.ty != Ty::Str {
                    return err("join() needs a String separator", args[0].value.span);
                }
                let b = format!("b{}", n);
                let piece = self.buf_append(&b, &format!("l{}->data[i{}]", n, n), elem, true);
                let text = self.ordered(vec![o, sep], |c| {
                    format!(
                        "({{ {ln}* l{n} = {l}; SnStr *sep{n} = {s}; SnBuf {b}; sn_buf_init(&{b}); for (int64_t i{n} = 0; i{n} < l{n}->len; i{n}++) {{ if (i{n}) sn_buf_str(&{b}, sep{n}); {piece} }} sn_buf_finish(&{b}); }})",
                        ln = ln, n = n, l = c[0], s = c[1], b = b, piece = piece
                    )
                });
                Ok(E::new(text, Ty::Str, false))
            }
            "map" | "each" | "any" | "all" => {
                want(1)?;
                let (f, ret) = self.func_arg(&args[0].value, &[elem], &who)?;
                let fc = self.cty(f.ty);
                let (rty, body, result) = match name {
                    "map" => {
                        if ret == Ty::Void {
                            return err("map() needs a function that returns a value; use each() to just call it", args[0].value.span);
                        }
                        let rty = Ty::list_of(ret);
                        let rln = self.type_name(rty);
                        (
                            rty,
                            format!("{rln}_push(r{n}, f{n}.f(f{n}.env, l{n}->data[i{n}]));", rln = rln, n = n),
                            format!("{rln}* r{n} = {rln}_new(l{n}->len); ", rln = rln, n = n),
                        )
                    }
                    "each" => (Ty::Void, format!("f{n}.f(f{n}.env, l{n}->data[i{n}]);", n = n), String::new()),
                    _ => {
                        if ret != Ty::Bool {
                            return err(
                                format!("{}() needs a function that returns Bool, but this one returns {}", name, ret.name()),
                                args[0].value.span,
                            );
                        }
                        let (init, test, brk) = if name == "any" {
                            ("false", format!("f{n}.f(f{n}.env, l{n}->data[i{n}])", n = n), "r = true; break;")
                        } else {
                            ("true", format!("!f{n}.f(f{n}.env, l{n}->data[i{n}])", n = n), "r = false; break;")
                        };
                        (
                            Ty::Bool,
                            format!("if ({}) {{ {} }}", test, brk),
                            format!("bool r = {}; ", init),
                        )
                    }
                };
                let ret_expr = match name {
                    "map" => format!("r{}", n),
                    "each" => String::new(),
                    _ => "r".to_string(),
                };
                let text = self.ordered(vec![o, f], |c| {
                    format!(
                        "({{ {ln}* l{n} = {l}; {fc} f{n} = {f}; {result}for (int64_t i{n} = 0; i{n} < l{n}->len; i{n}++) {{ {body} }} {ret_expr}; }})",
                        ln = ln, n = n, l = c[0], fc = fc, f = c[1], result = result, body = body, ret_expr = ret_expr
                    )
                });
                Ok(E::new(text, rty, false))
            }
            _ => {
                // reduce(start, f): f takes (accumulator, element) and returns the new accumulator
                want(2)?;
                let init = self.val(&args[0].value)?;
                let acc = init.ty;
                if acc == Ty::Void || acc == Ty::Null {
                    return err("reduce() needs a starting value with a type", args[0].value.span);
                }
                let (f, ret) = self.func_arg(&args[1].value, &[acc, elem], &who)?;
                if ret != acc {
                    return err(
                        format!(
                            "reduce(): the function returns {}, but the starting value is {} (for Float results start with e.g. 0.0)",
                            ret.name(),
                            acc.name()
                        ),
                        args[1].value.span,
                    );
                }
                let (fc, ac) = (self.cty(f.ty), self.cty(acc));
                let text = self.ordered(vec![o, init, f], |c| {
                    format!(
                        "({{ {ln}* l{n} = {l}; {ac} acc{n} = {i}; {fc} f{n} = {f}; for (int64_t i{n} = 0; i{n} < l{n}->len; i{n}++) acc{n} = f{n}.f(f{n}.env, acc{n}, l{n}->data[i{n}]); acc{n}; }})",
                        ln = ln, n = n, l = c[0], ac = ac, i = c[1], fc = fc, f = c[2]
                    )
                });
                Ok(E::new(text, acc, false))
            }
        }
    }
}
