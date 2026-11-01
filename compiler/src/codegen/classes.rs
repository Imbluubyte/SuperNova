//! Classes: declaration checks, field layout, construction, methods and dispatch.
//!
//! Representation: an instance is a pointer to a GC-allocated C struct
//! `{ int64_t cls; <all fields, parent fields first> }`. `cls` is the class's
//! depth-first id, so "is this an instance of C or a subclass?" is a range check
//! and a call through a base-class variable is a `switch` on `cls` that calls the
//! right implementation directly (no vtables, no indirect calls: the whole program
//! is known at compile time). Calls that no subclass overrides compile to a plain
//! direct call.
//!
//! Methods are compiled once per (declaring class, name, parameter types), so an
//! unannotated parameter is specialised just like in a free function.

use super::*;

/// Does this code contain `super.onCreation(...)`?
fn expr_has_super_ctor(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Call(callee, args) => {
            if let ExprKind::Member(o, m) = &callee.kind {
                if m == "onCreation" && matches!(&o.kind, ExprKind::Ident(n) if n == "super") {
                    return true;
                }
            }
            expr_has_super_ctor(callee) || args.iter().any(|a| expr_has_super_ctor(&a.value))
        }
        ExprKind::FStr(parts) => parts
            .iter()
            .any(|p| matches!(p, FStrPart::Expr(x) if expr_has_super_ctor(x))),
        ExprKind::List(v) | ExprKind::Tuple(v) => v.iter().any(expr_has_super_ctor),
        ExprKind::Dict(v) => v.iter().any(|(k, x)| expr_has_super_ctor(k) || expr_has_super_ctor(x)),
        ExprKind::Unary(_, x)
        | ExprKind::Await(x)
        | ExprKind::Predicate(_, x)
        | ExprKind::Member(x, _) => expr_has_super_ctor(x),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            expr_has_super_ctor(a) || expr_has_super_ctor(b)
        }
        ExprKind::Range { start, end, .. } => expr_has_super_ctor(start) || expr_has_super_ctor(end),
        _ => false,
    }
}

fn stmts_have_super_ctor(body: &[Stmt]) -> bool {
    body.iter().any(stmt_has_super_ctor)
}

fn stmt_has_super_ctor(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::VarDecl { init, .. } => expr_has_super_ctor(init),
        StmtKind::Assign { target, value, .. } => {
            expr_has_super_ctor(target) || expr_has_super_ctor(value)
        }
        StmtKind::Expr(e) | StmtKind::Throw(e) => expr_has_super_ctor(e),
        StmtKind::Return(Some(e)) => expr_has_super_ctor(e),
        StmtKind::If { branches, else_body } => {
            branches.iter().any(|(c, b)| expr_has_super_ctor(c) || stmts_have_super_ctor(b))
                || else_body.as_ref().map_or(false, |b| stmts_have_super_ctor(b))
        }
        StmtKind::While { cond, body } => expr_has_super_ctor(cond) || stmts_have_super_ctor(body),
        StmtKind::For { iter, body, .. } => expr_has_super_ctor(iter) || stmts_have_super_ctor(body),
        StmtKind::Match { subject, cases, default } => {
            expr_has_super_ctor(subject)
                || cases.iter().any(|(p, b)| expr_has_super_ctor(p) || stmts_have_super_ctor(b))
                || default.as_ref().map_or(false, |b| stmts_have_super_ctor(b))
        }
        StmtKind::Try { body, catch, finally } => {
            stmts_have_super_ctor(body)
                || catch.as_ref().map_or(false, |(_, b)| stmts_have_super_ctor(b))
                || finally.as_ref().map_or(false, |b| stmts_have_super_ctor(b))
        }
        _ => false,
    }
}

fn number_classes(classes: &mut [ClassInfo], children: &[Vec<usize>], c: usize, next: &mut i64) {
    classes[c].id_lo = *next;
    *next += 1;
    for &k in &children[c] {
        number_classes(classes, children, k, next);
    }
    classes[c].id_hi = *next - 1;
}

impl<'a> Gen<'a> {
    // ---------------- declaration ----------------

    /// Register every top-level class (so classes can be used before their
    /// declaration), then link parents and number the hierarchy.
    pub(super) fn collect_classes(&mut self, module: &'a Module) -> R<()> {
        let reserved = ["Int", "Float", "Bool", "String", "Any", "List", "Tuple", "null"];
        for s in &module.items {
            let StmtKind::Class(c) = &s.kind else { continue };
            if reserved.contains(&c.name.as_str()) || BUILTINS.contains(&c.name.as_str()) {
                return err(format!("'{}' is a built-in name and cannot be used for a class", c.name), c.span);
            }
            if self.class_ix.contains_key(&c.name) {
                return err(format!("class '{}' is already defined", c.name), c.span);
            }
            if self.fns.contains_key(&c.name) {
                return err(format!("'{}' is already used by a function", c.name), c.span);
            }
            let mut methods: HashMap<String, &'a FnDecl> = HashMap::new();
            for m in &c.methods {
                if matches!(m.name.as_str(), "self" | "super") {
                    return err(format!("'{}' is a reserved name", m.name), m.span);
                }
                if methods.insert(m.name.clone(), m).is_some() {
                    return err(
                        format!("method '{}' is defined twice in class '{}'", m.name, c.name),
                        m.span,
                    );
                }
            }
            let idx = self.classes.len();
            self.class_ix.insert(c.name.clone(), idx);
            CLASS_NAMES.with(|n| n.borrow_mut().push(c.name.clone()));
            self.fwd_decls
                .push_str(&format!("typedef struct SnO_{} SnO_{};\n", idx, idx));
            self.classes.push(ClassInfo {
                name: c.name.clone(),
                decl: c,
                parent: None,
                id_lo: 0,
                id_hi: 0,
                layout: 0,
                fields: Vec::new(),
                methods,
                ctor_emitted: false,
            });
        }
        let n = self.classes.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for i in 0..n {
            let decl = self.classes[i].decl;
            if let Some(pn) = &decl.parent {
                match self.class_ix.get(pn) {
                    Some(&p) => {
                        if p == i {
                            return err(format!("class '{}' cannot extend itself", decl.name), decl.span);
                        }
                        self.classes[i].parent = Some(p);
                        children[p].push(i);
                    }
                    None => {
                        return err(
                            format!("class '{}' extends '{}', which is not a class", decl.name, pn),
                            decl.span,
                        )
                    }
                }
            }
        }
        for i in 0..n {
            let (mut cur, mut steps) = (i, 0);
            while let Some(p) = self.classes[cur].parent {
                cur = p;
                steps += 1;
                if steps > n {
                    return err(
                        format!("class '{}' is part of an inheritance cycle", self.classes[i].name),
                        self.classes[i].decl.span,
                    );
                }
            }
        }
        let mut next = 0i64;
        for i in 0..n {
            if self.classes[i].parent.is_none() {
                number_classes(&mut self.classes, &children, i, &mut next);
            }
        }
        Ok(())
    }

    pub(super) fn is_subclass(&self, a: usize, b: usize) -> bool {
        let mut cur = Some(a);
        while let Some(c) = cur {
            if c == b {
                return true;
            }
            cur = self.classes[c].parent;
        }
        false
    }

    /// The class (this one or the nearest ancestor) that defines `name`.
    pub(super) fn find_method(&self, class: usize, name: &str) -> Option<(usize, &'a FnDecl)> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            if let Some(&m) = self.classes[c].methods.get(name) {
                return Some((c, m));
            }
            cur = self.classes[c].parent;
        }
        None
    }

    /// `c` and all its subclasses, in id order.
    fn subtree(&self, c: usize) -> Vec<usize> {
        let (lo, hi) = (self.classes[c].id_lo, self.classes[c].id_hi);
        let mut v: Vec<usize> = (0..self.classes.len())
            .filter(|&k| self.classes[k].id_lo >= lo && self.classes[k].id_lo <= hi)
            .collect();
        v.sort_by_key(|&k| self.classes[k].id_lo);
        v
    }

    pub(super) fn check_access(
        &self,
        owner: usize,
        is_private: bool,
        what: &str,
        name: &str,
        span: Span,
    ) -> R<()> {
        if is_private && self.cx.cur_class != Some(owner) {
            return err(
                format!(
                    "{} '{}' is private to class '{}' and cannot be used here",
                    what, name, self.classes[owner].name
                ),
                span,
            );
        }
        Ok(())
    }

    /// Work out the field list (inheriting the parent's) and emit the C struct.
    pub(super) fn ensure_layout(&mut self, c: usize) -> R<()> {
        match self.classes[c].layout {
            2 => return Ok(()),
            1 => {
                return err(
                    format!(
                        "class '{}' creates an instance of itself in a field initialiser, which would never finish",
                        self.classes[c].name
                    ),
                    self.classes[c].decl.span,
                )
            }
            _ => {}
        }
        self.classes[c].layout = 1;
        let decl = self.classes[c].decl;
        let mut fields: Vec<FieldInfo> = Vec::new();
        if let Some(p) = self.classes[c].parent {
            self.ensure_layout(p)?;
            fields = self.classes[p].fields.clone();
        }
        for f in &decl.fields {
            let StmtKind::VarDecl { name, is_const, is_private, ty, init } = &f.kind else {
                unreachable!("class bodies only contain field declarations here")
            };
            if matches!(name.as_str(), "self" | "super") {
                return err(format!("'{}' is a reserved name", name), f.span);
            }
            if let Some(prev) = fields.iter().find(|x| &x.name == name) {
                let where_ = if prev.owner == c {
                    "in this class".to_string()
                } else {
                    format!("in parent class '{}'", self.classes[prev.owner].name)
                };
                return err(format!("field '{}' is already declared {}", name, where_), f.span);
            }
            let declared = match ty {
                Some(t) => Some(self.resolve_type(t, f.span)?),
                None => None,
            };
            // Field initialisers run with no access to `self`, locals or other fields.
            let saved = std::mem::replace(&mut self.cx, Ctx::new(true));
            let v = self.hinted(init, declared);
            self.cx = saved;
            let v = v?;
            let (init_c, t) = match declared {
                Some(t) => match self.coerce(v.c, v.ty, t) {
                    Some(cc) => (cc, t),
                    None => {
                        return err(
                            format!(
                                "cannot initialise field '{}' of type {} with a value of type {}",
                                name,
                                t.name(),
                                v.ty.name()
                            ),
                            f.span,
                        )
                    }
                },
                None => {
                    if v.ty == Ty::Null {
                        return err("cannot infer a field type from null; add a type annotation", f.span);
                    }
                    (v.c, v.ty)
                }
            };
            fields.push(FieldInfo {
                name: name.clone(),
                ty: t,
                owner: c,
                is_private: *is_private,
                is_const: *is_const,
                init_c,
            });
        }
        for m in &decl.methods {
            if fields.iter().any(|f| f.name == m.name) {
                return err(
                    format!("'{}' is both a field and a method of class '{}'", m.name, decl.name),
                    m.span,
                );
            }
        }
        let mut st = format!("struct SnO_{} {{ int64_t cls; ", c);
        for f in &fields {
            let tc = self.cty(f.ty);
            st.push_str(&format!("{} f_{}; ", tc, f.name));
        }
        st.push_str("};\n");
        self.class_structs.push_str(&st);
        self.classes[c].fields = fields;
        self.classes[c].layout = 2;
        Ok(())
    }

    /// Type-check everything in every class, including methods that are never called
    /// (when their parameter types are all annotated).
    pub(super) fn check_classes(&mut self) -> R<()> {
        for c in 0..self.classes.len() {
            self.ensure_layout(c)?;
            let decl = self.classes[c].decl;
            for m in &decl.methods {
                if let Some(p) = self.classes[c].parent {
                    if let Some((po, pd)) = self.find_method(p, &m.name) {
                        if m.name != "onCreation" {
                            if pd.is_private {
                                return err(
                                    format!(
                                        "class '{}' already has a private method '{}'; choose another name",
                                        self.classes[po].name, m.name
                                    ),
                                    m.span,
                                );
                            }
                            if pd.params.len() != m.params.len() || pd.is_private != m.is_private {
                                return err(
                                    format!(
                                        "method '{}' of class '{}' must have the same parameters and visibility as the method it overrides in '{}'",
                                        m.name, self.classes[c].name, self.classes[po].name
                                    ),
                                    m.span,
                                );
                            }
                        }
                    }
                }
                if m.params.iter().all(|p| p.ty.is_some()) {
                    let mut ptys = Vec::new();
                    for p in &m.params {
                        ptys.push(self.resolve_type(p.ty.as_ref().unwrap(), m.span)?);
                    }
                    let (_, ret) = self.instance_method(c, m, ptys, m.span)?;
                    if m.name == "toString" && (ret != Ty::Str || !m.params.is_empty()) {
                        return err(
                            "toString() must take no parameters and return a String",
                            m.span,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    // ---------------- methods ----------------

    pub(super) fn instance_method(
        &mut self,
        owner: usize,
        decl: &'a FnDecl,
        ptys: Vec<Ty>,
        span: Span,
    ) -> R<(String, Ty)> {
        self.ensure_layout(owner)?;
        let key = (owner, decl.name.clone(), ptys.clone());
        let cls_name = self.classes[owner].name.clone();
        if let Some(inst) = self.minsts.get(&key) {
            return match inst.ret {
                Some(r) => Ok((inst.cname.clone(), r)),
                None => err(
                    format!(
                        "recursive method '{}.{}' needs a return type annotation (-> Type)",
                        cls_name, decl.name
                    ),
                    span,
                ),
            };
        }
        if decl.params.len() != ptys.len() {
            return err(
                format!(
                    "method '{}' of class '{}' has {} parameter(s) but the method it overrides has {}",
                    decl.name,
                    cls_name,
                    decl.params.len(),
                    ptys.len()
                ),
                decl.span,
            );
        }
        for (p, t) in decl.params.iter().zip(&ptys) {
            if let Some(ann) = &p.ty {
                let at = self.resolve_type(ann, decl.span)?;
                if at != *t {
                    return err(
                        format!(
                            "parameter '{}' of '{}.{}' is declared as {}, but it is called with {}",
                            p.name,
                            cls_name,
                            decl.name,
                            at.name(),
                            t.name()
                        ),
                        decl.span,
                    );
                }
            }
        }
        let declared = match &decl.ret {
            Some(t) => Some(self.resolve_type(t, decl.span)?),
            None => None,
        };
        let cname = format!("m{}_{}_{}", self.minsts.len(), cls_name, decl.name);
        if decl.is_async {
            if decl.name == "onCreation" {
                return err("onCreation cannot be async", decl.span);
            }
            self.uses_async = true;
        }
        let body_name = if decl.is_async { format!("{}_body", cname) } else { cname.clone() };
        let shown_ret = if decl.is_async { declared.map(Ty::task_of) } else { declared };
        self.minsts
            .insert(key.clone(), Inst { cname: cname.clone(), ret: shown_ret });

        let mut cx = Ctx::new(true);
        cx.ret_decl = declared;
        cx.volatile = super::assigned_in_try(&decl.body);
        cx.is_async = decl.is_async;
        cx.cur_class = Some(owner);
        cx.fn_name = decl.name.clone();
        cx.scopes[0].insert(
            "self".to_string(),
            Var { ty: Ty::Class(owner as u32), is_const: true, cname: "v_self".into(), shared: false },
        );
        let mut params_c = vec![format!("SnO_{}* v_self", owner)];
        for (p, t) in decl.params.iter().zip(&ptys) {
            if matches!(p.name.as_str(), "self" | "super") {
                return err(format!("'{}' is a reserved name", p.name), decl.span);
            }
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
        let res = self.method_body(owner, decl, declared);
        let fcx = std::mem::replace(&mut self.cx, saved);
        let ret = res?;
        if decl.name == "onCreation" && ret != Ty::Void {
            return err("onCreation cannot return a value", decl.span);
        }
        let ret_c = self.cty(ret);
        let sig = format!("static {} {}({})", ret_c, body_name, params_c.join(", "));
        self.protos.push_str(&format!("{};\n", sig));
        self.bodies.push_str(&format!("{} {{\n{}}}\n\n", sig, fcx.out));
        if decl.is_async {
            let mut ptys_c = vec![format!("SnO_{}*", owner)];
            for t in &ptys {
                ptys_c.push(self.cty(*t));
            }
            self.async_wrapper(&cname, &body_name, ptys_c, ret);
            let tt = Ty::task_of(ret);
            self.minsts.get_mut(&key).unwrap().ret = Some(tt);
            return Ok((cname, tt));
        }
        self.minsts.get_mut(&key).unwrap().ret = Some(ret);
        Ok((cname, ret))
    }

    fn method_body(&mut self, owner: usize, decl: &'a FnDecl, declared: Option<Ty>) -> R<Ty> {
        if decl.name == "onCreation" {
            self.auto_super_ctor(owner, decl)?;
        }
        self.fn_body(decl, declared)
    }

    /// A parent's onCreation runs automatically before the child's, if it can be
    /// called without arguments. Otherwise the child must call it explicitly.
    fn auto_super_ctor(&mut self, owner: usize, decl: &'a FnDecl) -> R<()> {
        let Some(p) = self.classes[owner].parent else { return Ok(()) };
        let Some((powner, pdecl)) = self.find_method(p, "onCreation") else { return Ok(()) };
        if stmts_have_super_ctor(&decl.body) {
            return Ok(());
        }
        if !pdecl.params.iter().all(|q| q.default.is_some()) {
            return err(
                format!(
                    "'{}.onCreation' must call super.onCreation(...) because '{}.onCreation' needs arguments",
                    self.classes[owner].name, self.classes[powner].name
                ),
                decl.span,
            );
        }
        let bound = self.bind_args(pdecl, "super.onCreation", &[], decl.span)?;
        let (cname, _) = self.instance_method(powner, pdecl, bound.ptys.clone(), decl.span)?;
        let args: Vec<String> = bound.cargs.iter().map(|a| a.c.clone()).collect();
        let mut all = vec![format!("(SnO_{}*)v_self", powner)];
        all.extend(args);
        self.line(&format!("{}({});", cname, all.join(", ")));
        Ok(())
    }

    /// Call `name` on `recv` (static type: class `class`). Virtual unless `is_super`
    /// or the method is private.
    pub(super) fn call_method(
        &mut self,
        recv: E,
        class: usize,
        name: &str,
        args: &[Arg],
        span: Span,
        is_super: bool,
    ) -> R<E> {
        self.ensure_layout(class)?;
        let cls_name = self.classes[class].name.clone();
        let Some((owner0, decl0)) = self.find_method(class, name) else {
            if let Some(f) = self.classes[class].fields.iter().find(|f| f.name == name).cloned() {
                if let Ty::Func(ps, r) = f.ty {
                    self.check_access(f.owner, f.is_private, "field", name, span)?;
                    let fe = E::new(format!("(({})->f_{})", recv.c, name), f.ty, false);
                    return self.call_value(fe, ps, *r, args, span);
                }
            }
            let hint = if self.classes[class].fields.iter().any(|f| f.name == name) {
                format!(" ('{}' is a field, not a method)", name)
            } else {
                let mut names: Vec<String> = Vec::new();
                let mut k = Some(class);
                while let Some(i) = k {
                    names.extend(self.classes[i].methods.keys().map(|s| s.to_string()));
                    k = self.classes[i].parent;
                }
                super::did_you_mean(name, names.iter().map(|s| s.as_str()))
            };
            return err(format!("class '{}' has no method '{}'{}", cls_name, name, hint), span);
        };
        if name == "onCreation" && !is_super {
            return err(
                "onCreation runs automatically when an instance is created; call super.onCreation(...) from a subclass if you need the parent's",
                span,
            );
        }
        self.check_access(owner0, decl0.is_private, "method", name, span)?;
        let bound = self.bind_args(decl0, &format!("{}.{}", cls_name, name), args, span)?;

        let mut groups: Vec<(usize, &'a FnDecl, Vec<i64>)> = Vec::new();
        if is_super || decl0.is_private {
            groups.push((owner0, decl0, Vec::new()));
        } else {
            for k in self.subtree(class) {
                let (o, d) = self.find_method(k, name).expect("subclass inherits the method");
                let id = self.classes[k].id_lo;
                match groups.iter_mut().find(|g| g.0 == o) {
                    Some(g) => g.2.push(id),
                    None => groups.push((o, d, vec![id])),
                }
            }
        }
        let mut targets: Vec<(String, usize, Vec<i64>)> = Vec::new();
        let mut ret: Option<(Ty, usize)> = None;
        for (o, d, ids) in &groups {
            let (cname, r) = self.instance_method(*o, d, bound.ptys.clone(), span)?;
            match ret {
                None => ret = Some((r, *o)),
                Some((prev, po)) if prev != r => {
                    return err(
                        format!(
                            "method '{}' returns {} in class '{}' but {} in class '{}'; overriding methods must return the same type",
                            name,
                            prev.name(),
                            self.classes[po].name,
                            r.name(),
                            self.classes[*o].name
                        ),
                        span,
                    )
                }
                _ => {}
            }
            targets.push((cname, *o, ids.clone()));
        }
        let ret = ret.expect("at least one target").0;

        let n = self.fresh();
        let r = format!("r{}", n);
        let mut s = format!("({{ SnO_{}* {} = (SnO_{}*)({}); ", class, r, class, recv.c);
        for &i in &bound.written {
            let tc = self.cty(bound.cargs[i].ty);
            s.push_str(&format!("{} a{}_{} = {}; ", tc, n, i, bound.cargs[i].c));
        }
        let finals: Vec<String> = (0..bound.cargs.len())
            .map(|i| {
                if bound.written.contains(&i) {
                    format!("a{}_{}", n, i)
                } else {
                    bound.cargs[i].c.clone()
                }
            })
            .collect();
        let call_for = |cname: &str, owner: usize| -> String {
            let mut all = vec![format!("(SnO_{}*){}", owner, r)];
            all.extend(finals.iter().cloned());
            format!("{}({})", cname, all.join(", "))
        };
        if targets.len() == 1 {
            s.push_str(&format!("{}; }})", call_for(&targets[0].0, targets[0].1)));
        } else {
            let void = ret == Ty::Void;
            if !void {
                let rc = self.cty(ret);
                s.push_str(&format!("{} res{}; ", rc, n));
            }
            s.push_str(&format!("switch ({}->cls) {{ ", r));
            for (cname, o, ids) in &targets {
                for id in ids {
                    s.push_str(&format!("case {}: ", id));
                }
                let call = call_for(cname, *o);
                if void {
                    s.push_str(&format!("{}; break; ", call));
                } else {
                    s.push_str(&format!("res{} = {}; break; ", n, call));
                }
            }
            s.push_str("default: __builtin_unreachable(); } ");
            if void {
                s.push_str("})");
            } else {
                s.push_str(&format!("res{}; }})", n));
            }
        }
        Ok(E::new(s, ret, false))
    }

    /// `super.method(...)`: call the parent's implementation without dynamic dispatch.
    pub(super) fn super_call(&mut self, method: &str, args: &[Arg], span: Span) -> R<E> {
        let Some(cc) = self.cx.cur_class else {
            return err("'super' can only be used inside a class method", span);
        };
        let Some(p) = self.classes[cc].parent else {
            return err(
                format!("class '{}' has no parent class, so 'super' cannot be used", self.classes[cc].name),
                span,
            );
        };
        if method == "onCreation" && self.cx.fn_name != "onCreation" {
            return err("super.onCreation(...) can only be called from onCreation", span);
        }
        let recv = E::new("v_self", Ty::Class(cc as u32), true);
        self.call_method(recv, p, method, args, span, true)
    }

    // ---------------- construction ----------------

    /// Emit `new_K()`: allocate the object, set its class id and run the field initialisers.
    fn ensure_ctor_fn(&mut self, c: usize) {
        if self.classes[c].ctor_emitted {
            return;
        }
        self.classes[c].ctor_emitted = true;
        let fields = self.classes[c].fields.clone();
        let atomic = if fields.iter().any(|f| f.ty.has_ptrs()) { 0 } else { 1 };
        let mut body = format!(
            "    SnO_{c}* o = (SnO_{c}*)sn_gc_alloc(sizeof(SnO_{c}), {atomic});\n    o->cls = {id};\n",
            c = c,
            atomic = atomic,
            id = self.classes[c].id_lo
        );
        for f in &fields {
            body.push_str(&format!("    o->f_{} = {};\n", f.name, f.init_c));
        }
        body.push_str("    return o;\n");
        let sig = format!("static SnO_{c}* new_{c}(void)", c = c);
        self.protos.push_str(&format!("{};\n", sig));
        self.bodies.push_str(&format!("{} {{\n{}}}\n\n", sig, body));
    }

    pub(super) fn construct(&mut self, class: usize, args: &[Arg], span: Span) -> R<E> {
        self.ensure_layout(class)?;
        self.ensure_ctor_fn(class);
        let cn = self.classes[class].name.clone();
        let ty = Ty::Class(class as u32);
        let Some((owner, decl)) = self.find_method(class, "onCreation") else {
            if !args.is_empty() {
                return err(
                    format!(
                        "class '{}' has no onCreation method, so it takes no arguments ({} given)",
                        cn,
                        args.len()
                    ),
                    span,
                );
            }
            return Ok(E::new(format!("new_{}()", class), ty, false));
        };
        let bound = self.bind_args(decl, &cn, args, span)?;
        let (cname, _) = self.instance_method(owner, decl, bound.ptys.clone(), span)?;
        let n = self.fresh();
        let mut s = String::from("({ ");
        for &i in &bound.written {
            let tc = self.cty(bound.cargs[i].ty);
            s.push_str(&format!("{} a{}_{} = {}; ", tc, n, i, bound.cargs[i].c));
        }
        let finals: Vec<String> = (0..bound.cargs.len())
            .map(|i| {
                if bound.written.contains(&i) {
                    format!("a{}_{}", n, i)
                } else {
                    bound.cargs[i].c.clone()
                }
            })
            .collect();
        let mut all = vec![format!("(SnO_{}*)o{}", owner, n)];
        all.extend(finals);
        s.push_str(&format!(
            "SnO_{c}* o{n} = new_{c}(); {call}({args}); o{n}; }})",
            c = class,
            n = n,
            call = cname,
            args = all.join(", ")
        ));
        Ok(E::new(s, ty, false))
    }

    // ---------------- members ----------------

    pub(super) fn member_expr(&mut self, obj: &Expr, name: &str, span: Span) -> R<E> {
        if matches!(&obj.kind, ExprKind::Ident(n) if n == "super") {
            return err("'super' can only be used as super.method(...)", span);
        }
        if let Some(r) = self.enum_variant(obj, name, span) {
            return r;
        }
        let o = self.val(obj)?;
        if let Ty::Opt(_) = o.ty {
            return Err(self.null_use(o.ty, &format!("read '.{}' of", name), span));
        }
        if let Ty::Enum(e) = o.ty {
            return self.enum_member(o, e, name, span);
        }
        let Ty::Class(c) = o.ty else {
            return err(format!("a value of type {} has no field '{}'", o.ty.name(), name), span);
        };
        let c = c as usize;
        let f = self.field_of(c, name, span)?;
        Ok(E::new(format!("({})->f_{}", o.c, name), f.ty, false))
    }

    /// Look up a field of class `c`, checking that it exists and is accessible here.
    pub(super) fn field_of(&mut self, c: usize, name: &str, span: Span) -> R<FieldInfo> {
        self.ensure_layout(c)?;
        let Some(f) = self.classes[c].fields.iter().find(|f| f.name == name).cloned() else {
            let hint = if self.find_method(c, name).is_some() {
                format!(" ('{}' is a method; call it as {}(...))", name, name)
            } else {
                let mut names: Vec<String> = Vec::new();
                let mut k = Some(c);
                while let Some(i) = k {
                    names.extend(self.classes[i].fields.iter().map(|f| f.name.clone()));
                    k = self.classes[i].parent;
                }
                super::did_you_mean(name, names.iter().map(|s| s.as_str()))
            };
            return err(
                format!("class '{}' has no field '{}'{}", self.classes[c].name, name, hint),
                span,
            );
        };
        self.check_access(f.owner, f.is_private, "field", name, span)?;
        Ok(f)
    }
}
