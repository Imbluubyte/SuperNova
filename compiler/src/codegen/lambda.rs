//! Lambdas: `fn(x, y) => expr`.
//!
//! A function value is a pair `{ fn pointer, env }`. Named functions get a tiny
//! thunk that ignores `env`; a lambda becomes an ordinary function whose first
//! parameters are the variables it captured, plus a thunk that unpacks them from
//! a GC-allocated `env` struct. Captured variables are copied when the lambda
//! is created (objects, lists and dictionaries are shared references, so
//! mutating those from inside a lambda is visible outside).

use super::*;

/// Names used in `e` that are not bound by an enclosing lambda parameter, in order of first use.
fn free_idents(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
	match &e.kind {
		ExprKind::Ident(n) => {
			if !bound.contains(n) && !out.contains(n) {
				out.push(n.clone());
			}
		}
		ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Null => {}
		ExprKind::FStr(parts) => {
			for p in parts {
				if let FStrPart::Expr(x) = p {
					free_idents(x, bound, out);
				}
			}
		}
		ExprKind::List(items) | ExprKind::Tuple(items) => items.iter().for_each(|i| free_idents(i, bound, out)),
		ExprKind::Dict(items) => {
			for (k, v) in items {
				free_idents(k, bound, out);
				free_idents(v, bound, out);
			}
		}
		ExprKind::Unary(_, x) | ExprKind::Await(x) | ExprKind::Force(x) | ExprKind::Predicate(_, x) => free_idents(x, bound, out),
		ExprKind::Member(x, _) => free_idents(x, bound, out),
		ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
			free_idents(l, bound, out);
			free_idents(r, bound, out);
		}
		ExprKind::Call(c, args) => {
			free_idents(c, bound, out);
			for a in args {
				free_idents(&a.value, bound, out);
			}
		}
		ExprKind::Range { start, end, .. } => {
			free_idents(start, bound, out);
			free_idents(end, bound, out);
		}
		ExprKind::Lambda(l) => {
			let n = bound.len();
			bound.extend(l.params.iter().map(|p| p.name.clone()));
			free_idents(&l.body, bound, out);
			bound.truncate(n);
		}
	}
}

impl<'a> Gen<'a> {
	/// A C function `target(args...)` adapted to the function-value calling convention.
	/// `env_fields` are the captured values, passed first.
	fn thunk(&mut self, target: &str, env: Option<&str>, caps: &[Ty], ps: &[Ty], ret: Ty) -> String {
		let name = format!("thunk{}_{}", self.thunks, target);
		self.thunks += 1;
		let rc = self.cty(ret);
		let mut params = vec!["void *env".to_string()];
		let mut args = Vec::new();
		for (i, c) in caps.iter().enumerate() {
			let _ = c;
			args.push(format!("e->c{}", i));
		}
		for (i, p) in ps.iter().enumerate() {
			params.push(format!("{} a{}", self.cty(*p), i));
			args.push(format!("a{}", i));
		}
		let sig = format!("static {} {}({})", rc, name, params.join(", "));
		let unpack = match env {
			Some(en) => format!("{}* e = ({}*)env; ", en, en),
			None => "(void)env; ".to_string(),
		};
		let call = format!("{}({})", target, args.join(", "));
		let body = if ret == Ty::Void { format!("{}{};", unpack, call) } else { format!("{}return {};", unpack, call) };
		self.protos.push_str(&format!("{};\n", sig));
		self.bodies.push_str(&format!("{} {{ {} }}\n\n", sig, body));
		name
	}

	/// The value for a named function `cname` with the given signature.
	pub(super) fn named_fn_value(&mut self, cname: &str, ptys: &[Ty], ret: Ty) -> E {
		let fty = Ty::func_of(ptys.to_vec(), ret);
		let tn = self.type_name(fty);
		let key = format!("{}", cname);
		let thunk = match self.thunk_cache.get(&key) {
			Some(t) => t.clone(),
			None => {
				let t = self.thunk(cname, None, &[], ptys, ret);
				self.thunk_cache.insert(key, t.clone());
				t
			}
		};
		E::new(format!("(({}){{{}, 0}})", tn, thunk), fty, true)
	}

	pub(super) fn lambda_expr(&mut self, lam: &Lambda, ptys_hint: Option<&[Ty]>, ret_hint: Option<Ty>, span: Span) -> R<E> {
		// parameter types: annotation, or from where the lambda is used
		let mut ptys = Vec::new();
		if let Some(h) = ptys_hint {
			if h.len() != lam.params.len() {
				return err(
					format!("this lambda takes {} parameter(s), but {} are expected here", lam.params.len(), h.len()),
					span,
				);
			}
		}
		for (i, p) in lam.params.iter().enumerate() {
			let t = match (&p.ty, ptys_hint) {
				(Some(t), hint) => {
					let have = self.resolve_type(t, span)?;
					if let Some(h) = hint {
						if h[i] != have {
							return err(
								format!("parameter '{}' is {}, but {} is expected here", p.name, have.name(), h[i].name()),
								span,
							);
						}
					}
					have
				}
				(None, Some(h)) => h[i],
				(None, None) => {
					return err(
						format!(
							"cannot tell the type of lambda parameter '{}'; write it as fn({}: Int) => ..., or give the variable or parameter that receives the lambda a type like Fn(Int) -> Int",
							p.name, p.name
						),
						span,
					)
				}
			};
			ptys.push(t);
		}
		// what the lambda uses from the surrounding code
		let mut bound: Vec<String> = lam.params.iter().map(|p| p.name.clone()).collect();
		let mut used = Vec::new();
		free_idents(&lam.body, &mut bound, &mut used);
		let mut caps: Vec<(String, E)> = Vec::new();
		let mut needs_self = false;
		for name in &used {
			if name == "self" {
				needs_self = true;
				continue;
			}
			if self.cx.scopes.iter().any(|s| s.contains_key(name.as_str())) {
				let v = self.lookup(name, span)?;
				caps.push((name.clone(), E::new(v.cname, v.ty, true)));
			} else if let Some(cc) = self.cx.cur_class {
				if self.ensure_layout(cc).is_ok()
					&& (self.classes[cc].fields.iter().any(|f| &f.name == name) || self.find_method(cc, name).is_some())
				{
					needs_self = true;
				}
			}
		}
		let cur = self.cx.cur_class;
		let mut params: Vec<Param> = Vec::new();
		let mut full_tys: Vec<Ty> = Vec::new();
		let mut cap_vals: Vec<E> = Vec::new();
		let mut cap_tys: Vec<Ty> = Vec::new();
		if needs_self {
			let Some(cc) = cur else {
				return err("'self' can only be used inside a class method", span);
			};
			params.push(Param { name: "self".into(), ty: None, default: None });
			let t = Ty::Class(cc as u32);
			full_tys.push(t);
			cap_tys.push(t);
			cap_vals.push(E::new("v_self", t, true));
		}
		for (n, e) in &caps {
			params.push(Param { name: n.clone(), ty: None, default: None });
			full_tys.push(e.ty);
			cap_tys.push(e.ty);
			cap_vals.push(E::new(e.c.clone(), e.ty, true));
		}
		for (p, t) in lam.params.iter().zip(&ptys) {
			params.push(Param { name: p.name.clone(), ty: None, default: None });
			full_tys.push(*t);
		}
		let ret_expr_first = ret_hint != Some(Ty::Void);
		let mut result = None;
		for attempt in 0..2 {
			let as_return = (attempt == 0) == ret_expr_first;
			let sp = lam.body.span;
			let kind = if as_return { StmtKind::Return(Some(lam.body.clone())) } else { StmtKind::Expr(lam.body.clone()) };
			let id = self.fresh();
			let decl = FnDecl {
				name: format!("lambda{}", id),
				params: params.clone(),
				ret: lam.ret.clone(),
				body: vec![Stmt { kind, span: sp }],
				is_async: false,
				is_private: false,
				span,
			};
			let decl: &'a FnDecl = Box::leak(Box::new(decl));
			if needs_self {
				self.lambda_cls.insert(decl.name.clone(), cur.unwrap());
			}
			match self.instance(decl, full_tys.clone(), span) {
				Ok(r) => {
					result = Some(r);
					break;
				}
				Err(e) if attempt == 0 && (e.msg.contains("produces no value") || e.msg.contains("returns nothing")) => continue,
				Err(e) => return Err(e),
			}
		}
		let Some((cname, ret)) = result else {
			return err("this lambda produces no value", span);
		};
		if let Some(h) = ret_hint {
			if h != ret && !(h == Ty::Void) {
				let ok = ret == Ty::Void && false;
				if !ok {
					return err(
						format!("this lambda returns {}, but {} is expected here", ret.name(), h.name()),
						span,
					);
				}
			}
		}
		let fty = Ty::func_of(ptys.clone(), ret);
		let tn = self.type_name(fty);
		if cap_vals.is_empty() {
			let thunk = self.thunk(&cname, None, &[], &ptys, ret);
			return Ok(E::new(format!("(({}){{{}, 0}})", tn, thunk), fty, true));
		}
		let en = format!("SnEnv{}", self.thunks);
		let mut fields = String::new();
		for (i, t) in cap_tys.iter().enumerate() {
			let c = self.cty(*t);
			fields.push_str(&format!("{} c{}; ", c, i));
		}
		self.type_decls.push_str(&format!("typedef struct {{ {}}} {};\n", fields, en));
		let thunk = self.thunk(&cname, Some(&en), &cap_tys, &ptys, ret);
		let mut s = format!("({{ {en}* env = ({en}*)sn_gc_alloc(sizeof({en}), 0); ", en = en);
		for (i, v) in cap_vals.iter().enumerate() {
			s.push_str(&format!("env->c{} = {}; ", i, v.c));
		}
		s.push_str(&format!("(({}){{{}, env}}); }})", tn, thunk));
		Ok(E::new(s, fty, false))
	}
}
