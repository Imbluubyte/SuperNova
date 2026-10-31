//! `try` / `catch` / `finally` and `throw`.
//!
//! Handlers are setjmp frames chained through `sn_try_top` (see `snrt.h`). A
//! `finally` block adds an outer frame so that it runs for exceptions as well
//! as for `return`, `break` and `continue` that leave the protected region.
//! Those jumps record what they wanted in a `pend` variable and `goto` the
//! finally code, which re-issues them afterwards (through any outer frames).

use super::*;
use std::collections::HashSet;

pub(super) struct FinInfo {
    label: String,
    pend: String,
    retv: String,
    /// Return type seen inside the protected region (None = no `return` yet).
    ret_ty: Option<Ty>,
    used_break: bool,
    used_continue: bool,
}

pub(super) struct TryFrame {
    var: String,
    loop_depth: usize,
    fin: Option<FinInfo>,
}

pub(super) enum Jump {
    /// `return`, with the already-coerced C value and its type (None for a bare return).
    Return(Option<(String, Ty)>),
    Break,
    Continue,
}

/// Local variables that are assigned inside a `try` body or `catch` block.
/// After a longjmp the compiler may not rely on registers for them, so they
/// are declared `volatile` (the C standard requires this for correctness).
pub(super) fn assigned_in_try(body: &[Stmt]) -> HashSet<String> {
    fn walk(stmts: &[Stmt], in_try: bool, out: &mut HashSet<String>) {
        for s in stmts {
            match &s.kind {
                StmtKind::Assign { target, .. } => {
                    if in_try {
                        if let ExprKind::Ident(n) = &target.kind {
                            out.insert(n.clone());
                        }
                    }
                }
                StmtKind::If { branches, else_body } => {
                    for (_, b) in branches {
                        walk(b, in_try, out);
                    }
                    if let Some(b) = else_body {
                        walk(b, in_try, out);
                    }
                }
                StmtKind::While { body, .. } | StmtKind::For { body, .. } => walk(body, in_try, out),
                StmtKind::Match { cases, default, .. } => {
                    for (_, b) in cases {
                        walk(b, in_try, out);
                    }
                    if let Some(b) = default {
                        walk(b, in_try, out);
                    }
                }
                StmtKind::Try { body, catch, finally } => {
                    walk(body, true, out);
                    if let Some((_, b)) = catch {
                        walk(b, true, out);
                    }
                    if let Some(b) = finally {
                        walk(b, in_try, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = HashSet::new();
    walk(body, false, &mut out);
    out
}

impl<'a> Gen<'a> {
    pub(super) fn throw_stmt(&mut self, e: &Expr, span: Span) -> R<()> {
        let v = self.val(e)?;
        if v.ty != Ty::Str {
            return err(
                format!("'throw' needs a String message, not {}; use str(...) to convert it", v.ty.name()),
                e.span,
            );
        }
        let loc = self.loc(span);
        self.line(&format!("sn_throw_str({}, {}, 0);", loc, v.c));
        Ok(())
    }

    /// Emit `return`, `break` or `continue`, unwinding any handlers and running
    /// the `finally` blocks that lie between here and the target.
    pub(super) fn emit_jump(&mut self, j: Jump) {
        let ld = self.cx.loop_depth;
        let all = matches!(j, Jump::Return(_));
        // Frames between here and the jump target, innermost last.
        let first = if all {
            0
        } else {
            self.cx.try_stack.iter().position(|f| f.loop_depth >= ld).unwrap_or(self.cx.try_stack.len())
        };
        let frames = &self.cx.try_stack[first..];
        let fin_pos = frames.iter().rposition(|f| f.fin.is_some()).map(|p| p + first);
        let plain = |j: &Jump, rv: &str| -> String {
            match j {
                Jump::Return(Some(_)) => format!("return {};", rv),
                Jump::Return(None) => "return;".to_string(),
                Jump::Break => "break;".to_string(),
                Jump::Continue => "continue;".to_string(),
            }
        };
        match fin_pos {
            None => {
                if first == self.cx.try_stack.len() {
                    let c = match &j {
                        Jump::Return(Some((c, _))) => c.clone(),
                        _ => String::new(),
                    };
                    self.line(&plain(&j, &c));
                    return;
                }
                let outer = self.cx.try_stack[first].var.clone();
                match &j {
                    Jump::Return(Some((c, ty))) => {
                        let tc = self.cty(*ty);
                        self.line(&format!("{{ {tc} rv = {c}; sn_try_top = {outer}.prev; return rv; }}"));
                    }
                    _ => {
                        let p = plain(&j, "");
                        self.line(&format!("{{ sn_try_top = {}.prev; {} }}", outer, p));
                    }
                }
            }
            Some(i) => {
                let (var, label, pend, retv) = {
                    let f = &self.cx.try_stack[i];
                    let fi = f.fin.as_ref().unwrap();
                    (f.var.clone(), fi.label.clone(), fi.pend.clone(), fi.retv.clone())
                };
                let code = match &j {
                    Jump::Return(v) => {
                        let fi = self.cx.try_stack[i].fin.as_mut().unwrap();
                        fi.ret_ty = Some(v.as_ref().map(|x| x.1).unwrap_or(Ty::Void));
                        2
                    }
                    Jump::Break => {
                        self.cx.try_stack[i].fin.as_mut().unwrap().used_break = true;
                        3
                    }
                    Jump::Continue => {
                        self.cx.try_stack[i].fin.as_mut().unwrap().used_continue = true;
                        4
                    }
                };
                let set = match &j {
                    Jump::Return(Some((c, _))) => format!("{} = {}; ", retv, c),
                    _ => String::new(),
                };
                self.line(&format!(
                    "{{ {set}{pend} = {code}; sn_try_top = {var}.prev; goto {label}; }}"
                ));
            }
        }
    }

    pub(super) fn try_stmt(
        &mut self,
        body: &[Stmt],
        catch: &Option<(String, Vec<Stmt>)>,
        finally: &Option<Vec<Stmt>>,
    ) -> R<()> {
        let ld = self.cx.loop_depth;
        let Some(fin_body) = finally else {
            return self.try_core(body, catch, ld);
        };
        let n = self.fresh();
        let (fr, pend, label, retv) = (format!("fr{}", n), format!("pend{}", n), format!("fin{}", n), format!("retv{}", n));
        let marker = format!("/*RETV{}*/", n);
        self.line("{");
        self.cx.indent += 1;
        self.line(&format!(
            "SnTry {fr}; int volatile {pend} = 0; SnStr *x_exc{n} = 0; const char *x_loc{n} = 0; int x_kind{n} = 0;"
        ));
        self.line(&marker);
        self.line(&format!("{fr}.prev = sn_try_top; sn_try_top = &{fr};"));
        self.line(&format!("if (setjmp({fr}.jb) == 0) {{"));
        self.cx.try_stack.push(TryFrame {
            var: fr.clone(),
            loop_depth: ld,
            fin: Some(FinInfo {
                label: label.clone(),
                pend: pend.clone(),
                retv: retv.clone(),
                ret_ty: None,
                used_break: false,
                used_continue: false,
            }),
        });
        self.cx.indent += 1;
        let r = self.try_core(body, catch, ld);
        self.cx.indent -= 1;
        let frame = self.cx.try_stack.pop().unwrap();
        r?;
        self.line(&format!(
            "sn_try_top = {fr}.prev; }} else {{ {pend} = 1; x_exc{n} = sn_exc; x_loc{n} = sn_exc_loc; x_kind{n} = sn_exc_kind; }}"
        ));
        self.line(&format!("{}: ;", label));
        self.block(fin_body)?;
        self.line(&format!("if ({pend} == 1) sn_throw_str(x_loc{n}, x_exc{n}, x_kind{n});"));
        let fi = frame.fin.unwrap();
        if let Some(rt) = fi.ret_ty {
            if rt == Ty::Void {
                self.line(&format!("if ({pend} == 2) {{"));
                self.emit_jump(Jump::Return(None));
            } else {
                self.line(&format!("if ({pend} == 2) {{"));
                self.emit_jump(Jump::Return(Some((retv.clone(), rt))));
            }
            self.line("}");
            let decl = if rt == Ty::Void {
                String::new()
            } else {
                let tc = self.cty(rt);
                format!("{} volatile {};", tc, retv)
            };
            self.cx.out = self.cx.out.replace(&marker, &decl);
        } else {
            self.cx.out = self.cx.out.replace(&marker, "");
        }
        if fi.used_break {
            self.line(&format!("if ({pend} == 3) {{"));
            self.emit_jump(Jump::Break);
            self.line("}");
        }
        if fi.used_continue {
            self.line(&format!("if ({pend} == 4) {{"));
            self.emit_jump(Jump::Continue);
            self.line("}");
        }
        self.cx.indent -= 1;
        self.line("}");
        Ok(())
    }

    fn try_core(
        &mut self,
        body: &[Stmt],
        catch: &Option<(String, Vec<Stmt>)>,
        ld: usize,
    ) -> R<()> {
        let Some((name, cbody)) = catch else {
            return self.block(body);
        };
        if name == "self" || name == "super" {
            return err(format!("'{}' is a reserved name", name), Span { line: 0, col: 0, file: 0 });
        }
        let t = format!("tr{}", self.fresh());
        self.line(&format!("{{ SnTry {t}; {t}.prev = sn_try_top; sn_try_top = &{t};"));
        self.line(&format!("if (setjmp({t}.jb) == 0) {{"));
        self.cx.try_stack.push(TryFrame { var: t.clone(), loop_depth: ld, fin: None });
        let r = self.block(body);
        self.cx.try_stack.pop();
        r?;
        self.line(&format!("sn_try_top = {t}.prev; }} else {{"));
        self.push_scope();
        let cname = format!("v_{}_{}", name, self.fresh());
        self.line(&format!("SnStr *{} = sn_exc;", cname));
        self.cx.scopes.last_mut().unwrap().insert(
            name.clone(),
            Var { ty: Ty::Str, is_const: true, cname, shared: false },
        );
        for s in cbody {
            self.stmt(s)?;
        }
        self.pop_scope();
        self.line("} }");
        Ok(())
    }
}
