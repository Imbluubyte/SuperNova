//! Optional types (`T?`) and null safety.
//!
//! An optional is a struct `{ bool has; T v; }`. The type checker refuses to
//! use one where a value is needed (arithmetic, member access, calls, ...)
//! until the program has dealt with null: `if x != null:` (the variable then
//! reads as a plain `T`, see `narrowed` in `Ctx`), `x ?? fallback`, or `x!`.

use super::*;
use std::collections::HashSet;

/// Names assigned anywhere inside `stmts` (including nested blocks).
pub(super) fn assigned_names(stmts: &[Stmt], out: &mut HashSet<String>) {
	for s in stmts {
		match &s.kind {
			StmtKind::Assign { target, .. } => {
				if let ExprKind::Ident(n) = &target.kind {
					out.insert(n.clone());
				}
			}
			StmtKind::If { branches, else_body } => {
				for (_, b) in branches {
					assigned_names(b, out);
				}
				if let Some(b) = else_body {
					assigned_names(b, out);
				}
			}
			StmtKind::While { body, .. } | StmtKind::For { body, .. } | StmtKind::Repeat { body, .. } => {
				assigned_names(body, out)
			}
			StmtKind::Match { cases, default, .. } => {
				for (_, b) in cases {
					assigned_names(b, out);
				}
				if let Some(b) = default {
					assigned_names(b, out);
				}
			}
			StmtKind::Try { body, catch, finally } => {
				assigned_names(body, out);
				if let Some((_, b)) = catch {
					assigned_names(b, out);
				}
				if let Some(b) = finally {
					assigned_names(b, out);
				}
			}
			_ => {}
		}
	}
}

/// Whether control can never fall off the end of `body`.
pub(super) fn diverges(body: &[Stmt]) -> bool {
	matches!(
		body.last().map(|s| &s.kind),
		Some(StmtKind::Return(_) | StmtKind::Throw(_) | StmtKind::Break | StmtKind::Continue)
	) || always_returns(body)
}

fn is_null(e: &Expr) -> bool {
	matches!(e.kind, ExprKind::Null)
}

impl<'a> Gen<'a> {
	// ---------------- narrowing bookkeeping ----------------

	pub(super) fn add_narrow(&mut self, cname: &str, inner: Ty) {
		if !self.cx.narrowed.contains_key(cname) {
			self.cx.narrowed.insert(cname.to_string(), inner);
			if let Some(last) = self.cx.narrow_added.last_mut() {
				last.push(cname.to_string());
			}
		}
	}

	pub(super) fn narrow_pop(&mut self) {
		if let Some(names) = self.cx.narrow_added.pop() {
			for n in names {
				self.cx.narrowed.remove(&n);
			}
		}
	}

	/// Forget what is known about variables that `body` assigns (used before loops,
	/// whose later iterations see the assignments).
	pub(super) fn forget_assigned(&mut self, body: &[Stmt]) {
		let mut names = HashSet::new();
		assigned_names(body, &mut names);
		for n in names {
			if let Ok(v) = self.lookup_raw(&n, Span { line: 0, col: 0, file: 0 }) {
				self.cx.narrowed.remove(&v.cname);
			}
		}
	}

	/// Run `body` as a block in which the given variables are known to be non-null.
	pub(super) fn block_with(&mut self, body: &[Stmt], narrow: &[(String, Ty)]) -> R<()> {
		self.push_scope();
		for (c, t) in narrow {
			self.add_narrow(c, *t);
		}
		let mut r = Ok(());
		for s in body {
			r = self.stmt(s);
			if r.is_err() {
				break;
			}
		}
		self.pop_scope();
		r
	}

	/// Variables that `cond` proves non-null when it evaluates to `when`.
	pub(super) fn cond_narrow(&mut self, cond: &Expr, when: bool) -> Vec<(String, Ty)> {
		match &cond.kind {
			ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), l, r) => {
				// x != null is true / x == null is false  =>  x is not null
				let non_null_when = *op == BinOp::Ne;
				if when != non_null_when {
					return Vec::new();
				}
				let var = match (&l.kind, is_null(r), &r.kind, is_null(l)) {
					(ExprKind::Ident(n), true, _, _) => n,
					(_, _, ExprKind::Ident(n), true) => n,
					_ => return Vec::new(),
				};
				self.narrowable(var).into_iter().collect()
			}
			ExprKind::Unary(UnOp::Not, e) => self.cond_narrow(e, !when),
			ExprKind::Binary(BinOp::And, a, b) if when => {
				let mut v = self.cond_narrow(a, true);
				v.extend(self.cond_narrow(b, true));
				v
			}
			ExprKind::Binary(BinOp::Or, a, b) if !when => {
				let mut v = self.cond_narrow(a, false);
				v.extend(self.cond_narrow(b, false));
				v
			}
			_ => Vec::new(),
		}
	}

	/// `Some((cname, T))` if `name` is a local optional not yet known to be non-null.
	fn narrowable(&mut self, name: &str) -> Option<(String, Ty)> {
		let v = self.lookup_raw(name, Span { line: 0, col: 0, file: 0 }).ok()?;
		if v.cname.starts_with('(') || self.cx.narrowed.contains_key(&v.cname) {
			return None;
		}
		match v.ty {
			Ty::Opt(inner) => Some((v.cname, *inner)),
			_ => None,
		}
	}

	/// Compile `f` with extra narrowings active (for the right side of `and` / `or`).
	pub(super) fn with_narrow<T>(&mut self, narrow: Vec<(String, Ty)>, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
		self.cx.narrow_added.push(Vec::new());
		for (c, t) in &narrow {
			self.add_narrow(c, *t);
		}
		let r = f(self);
		self.narrow_pop();
		r
	}

	// ---------------- statements ----------------

	pub(super) fn if_stmt(&mut self, branches: &[(Expr, Vec<Stmt>)], else_body: &Option<Vec<Stmt>>) -> R<()> {
		let mut acc: Vec<(String, Ty)> = Vec::new(); // known once every earlier condition was false
		let mut paths: Vec<(Vec<(String, Ty)>, &[Stmt])> = Vec::new(); // ways to reach the code after the `if`
		for (i, (c, body)) in branches.iter().enumerate() {
			let cc = self.with_narrow(acc.clone(), |g| g.cond(c))?;
			let mut entry = acc.clone();
			entry.extend(self.cond_narrow(c, true));
			if i == 0 {
				self.line(&format!("if ({}) {{", cc));
			} else {
				self.line(&format!("}} else if ({}) {{", cc));
			}
			self.block_with(body, &entry)?;
			if !diverges(body) {
				paths.push((entry, body));
			}
			let more = self.cond_narrow(c, false);
			acc.extend(more);
		}
		match else_body {
			Some(e) => {
				self.line("} else {");
				self.block_with(e, &acc)?;
				if !diverges(e) {
					paths.push((acc.clone(), e));
				}
			}
			None => paths.push((acc.clone(), &[])),
		}
		self.line("}");
		// `if x == null: return` leaves x non-null for the rest of the block.
		if paths.len() == 1 {
			let (set, body) = &paths[0];
			let mut assigned = HashSet::new();
			assigned_names(body, &mut assigned);
			let mut skip = HashSet::new();
			for n in &assigned {
				if let Ok(v) = self.lookup_raw(n, Span { line: 0, col: 0, file: 0 }) {
					skip.insert(v.cname);
				}
			}
			for (c, t) in set {
				if !skip.contains(c) {
					self.add_narrow(c, *t);
				}
			}
		}
		Ok(())
	}

	pub(super) fn while_stmt(&mut self, cond: &Expr, body: &[Stmt]) -> R<()> {
		self.forget_assigned(body);
		let cc = self.cond(cond)?;
		let narrow = self.cond_narrow(cond, true);
		self.line(&format!("while ({}) {{", cc));
		self.cx.loop_depth += 1;
		let r = self.block_with(body, &narrow);
		self.cx.loop_depth -= 1;
		r?;
		self.line("}");
		Ok(())
	}

	pub(super) fn repeat_stmt(&mut self, count: &Expr, body: &[Stmt]) -> R<()> {
		let n = self.val(count)?;
		if n.ty != Ty::Int {
			return err(format!("repeat(...) needs an Int count, got {}", n.ty.name()), count.span);
		}
		self.forget_assigned(body);
		let k = self.fresh();
		self.line("{");
		self.cx.indent += 1;
		self.line(&format!("int64_t n{k} = {}; ", n.c, k = k));
		self.line(&format!("for (int64_t i{k} = 0; i{k} < n{k}; i{k}++) {{", k = k));
		self.cx.loop_depth += 1;
		let r = self.block(body);
		self.cx.loop_depth -= 1;
		r?;
		self.line("}");
		self.cx.indent -= 1;
		self.line("}");
		Ok(())
	}

	// ---------------- expressions ----------------

	/// The error for using an optional where a value is needed.
	pub(super) fn null_use(&self, ty: Ty, what: &str, span: Span) -> CompileError {
		CompileError::new(
			format!(
				"cannot {} a value of type {} because it might be null; check it first (`if x != null:`), give a fallback (`x ?? fallback`) or assert it (`x!`)",
				what,
				ty.name()
			),
			span,
		)
	}

	/// Extra advice for a failed conversion from `T?` to `T`.
	pub(super) fn null_hint(&mut self, from: Ty, to: Ty) -> String {
		if let Ty::Opt(inner) = from {
			if *inner == to || (to != Ty::Null && self.coerce("x".to_string(), *inner, to).is_some()) {
				return " (the value might be null: check it with `if x != null:`, or use `x ?? fallback` or `x!`)".to_string();
			}
		}
		String::new()
	}

	/// `a has b`: rewritten to the matching `contains` call.
	pub(super) fn has_expr(&mut self, l: &Expr, r: &Expr, span: Span) -> R<E> {
		let callee = Expr { kind: ExprKind::Member(Box::new(l.clone()), "contains".to_string()), span };
		let call = Expr {
			kind: ExprKind::Call(Box::new(callee), vec![Arg { name: None, value: r.clone() }]),
			span,
		};
		let v = self.expr(&call).map_err(|e| {
			if e.msg.contains("has no method 'contains'") {
				CompileError::new("'has' works on lists, dictionaries and strings", span)
			} else {
				e
			}
		})?;
		Ok(v)
	}

	pub(super) fn coalesce(&mut self, l: &Expr, r: &Expr, span: Span) -> R<E> {
		let lv = self.val(l)?;
		let Ty::Opt(inner) = lv.ty else {
			return err(
				format!("the left side of '??' must be optional (T?), but it is a {} and never null", lv.ty.name()),
				span,
			);
		};
		let inner = *inner;
		let rv = self.hinted(r, Some(inner))?;
		let on = self.type_name(lv.ty);
		let k = self.fresh();
		if rv.ty == Ty::Null || matches!(rv.ty, Ty::Opt(_)) {
			// a ?? b where b may also be null: the result stays optional
			let Some(rc) = self.coerce(rv.c, rv.ty, lv.ty) else {
				return err(format!("cannot combine {} with {} using '??'", lv.ty.name(), rv.ty.name()), span);
			};
			return Ok(E::new(
				format!("({{ {on} t{k} = {}; t{k}.has ? t{k} : {}; }})", lv.c, rc, on = on, k = k),
				lv.ty,
				false,
			));
		}
		let rty = rv.ty;
		let Some(rc) = self.coerce(rv.c, rty, inner) else {
			return err(
				format!("the fallback after '??' is a {}, but the value is {}", rty.name(), inner.name()),
				r.span,
			);
		};
		Ok(E::new(
			format!("({{ {on} t{k} = {}; t{k}.has ? t{k}.v : {}; }})", lv.c, rc, on = on, k = k),
			inner,
			false,
		))
	}

	pub(super) fn force_expr(&mut self, inner: &Expr, span: Span) -> R<E> {
		let v = self.val(inner)?;
		let Ty::Opt(t) = v.ty else {
			return err(format!("'!' is for optional values (T?), but this is a {} and never null", v.ty.name()), span);
		};
		let on = self.type_name(v.ty);
		let k = self.fresh();
		let loc = self.loc(span);
		Ok(E::new(
			format!(
				"({{ {on} t{k} = {}; if (!t{k}.has) sn_panic({loc}, \"tried to unwrap a null value with '!'\"); t{k}.v; }})",
				v.c, on = on, k = k, loc = loc
			),
			*t,
			false,
		))
	}

	/// `==` / `!=` when at least one side is optional or null.
	pub(super) fn opt_equality(&mut self, op: BinOp, l: E, r: E, span: Span) -> R<E> {
		let eq = op == BinOp::Eq;
		let pure = l.pure && r.pure;
		let flag = |has: &str| if eq { format!("(!({}))", has) } else { format!("(({}))", has) };
		match (l.ty, r.ty) {
			(Ty::Null, Ty::Null) => Ok(E::new(if eq { "true" } else { "false" }, Ty::Bool, true)),
			(Ty::Opt(_), Ty::Null) => Ok(E::new(flag(&format!("({}).has", l.c)), Ty::Bool, pure)),
			(Ty::Null, Ty::Opt(_)) => Ok(E::new(flag(&format!("({}).has", r.c)), Ty::Bool, pure)),
			(Ty::Null, _) | (_, Ty::Null) => {
				// a plain value is never null
				let side = if l.ty == Ty::Null { r } else { l };
				Ok(E::new(
					format!("((void)({}), {})", side.c, if eq { "false" } else { "true" }),
					Ty::Bool,
					side.pure,
				))
			}
			_ => {
				// compare the contents; two nulls are equal, null never equals a value
				let target = match (l.ty, r.ty) {
					(Ty::Opt(_), _) => l.ty,
					_ => r.ty,
				};
				let Ty::Opt(inner) = target else { unreachable!() };
				let (lt, rt) = (l.ty, r.ty);
				let (Some(lc), Some(rc)) = (self.coerce(l.c, lt, target), self.coerce(r.c, rt, target)) else {
					return err(
						format!("cannot compare {} with {}", lt.name(), rt.name()),
						span,
					);
				};
				let on = self.type_name(target);
				let k = self.fresh();
				let a = E::new(format!("a{}.v", k), *inner, true);
				let b = E::new(format!("b{}.v", k), *inner, true);
				let same = self.binary(BinOp::Eq, a, b, span)?;
				let all = format!(
					"({{ {on} a{k} = {lc}; {on} b{k} = {rc}; (a{k}.has == b{k}.has) && (!a{k}.has || {same}); }})",
					on = on, k = k, lc = lc, rc = rc, same = same.c
				);
				Ok(E::new(if eq { all } else { format!("(!{})", all) }, Ty::Bool, false))
			}
		}
	}
}
