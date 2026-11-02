//! Embedding in a host program (for example a Kotlin app): `extern fn` declares a
//! function the host provides, `export fn` is a function the host can call, and
//! `snc build --lib` produces a shared library plus a C header.
//!
//! Across the boundary only Int (int64_t), Float (double), Bool (int32_t) and
//! String (UTF-8 `const char *`) are allowed. Strings returned to the host stay
//! valid until the next call into the library on the same thread.

use super::*;

fn abi_type(t: Ty) -> Option<&'static str> {
	match t {
		Ty::Int => Some("int64_t"),
		Ty::Float => Some("double"),
		Ty::Bool => Some("int32_t"),
		Ty::Str => Some("const char *"),
		Ty::Void => Some("void"),
		_ => None,
	}
}

fn zero_of(t: Ty) -> &'static str {
	match t {
		Ty::Str => "0",
		Ty::Void => "",
		_ => "0",
	}
}

impl<'a> Gen<'a> {
	fn abi_check(&self, decl: &FnDecl, ptys: &[Ty], ret: Ty, what: &str) -> R<()> {
		for (p, t) in decl.params.iter().zip(ptys) {
			if matches!(t, Ty::Void) || abi_type(*t).is_none() {
				return err(
					format!(
						"parameter '{}' of {} fn '{}' is {}, but only Int, Float, Bool and String can cross the host boundary",
						p.name,
						what,
						crate::modules::bare_name(&decl.name),
						t.name()
					),
					decl.span,
				);
			}
		}
		if abi_type(ret).is_none() {
			return err(
				format!(
					"{} fn '{}' returns {}, but only Int, Float, Bool, String or nothing can cross the host boundary",
					what,
					crate::modules::bare_name(&decl.name),
					ret.name()
				),
				decl.span,
			);
		}
		Ok(())
	}

	/// A function the host provides. The host registers it with `sn_set_extern_<name>(fnptr)`.
	pub(super) fn extern_instance(&mut self, decl: &'a FnDecl, ptys: Vec<Ty>, span: Span) -> R<(String, Ty)> {
		if decl.is_async {
			return err("an extern fn cannot be async", span);
		}
		let key = (decl.name.clone(), ptys.clone());
		if let Some(inst) = self.insts.get(&key) {
			return Ok((inst.cname.clone(), inst.ret.unwrap_or(Ty::Void)));
		}
		let ret = match &decl.ret {
			Some(t) => self.resolve_type(t, decl.span)?,
			None => Ty::Void,
		};
		self.abi_check(decl, &ptys, ret, "extern")?;
		let cname = format!("f{}_{}", self.insts.len(), decl.name);
		self.insts.insert(key, Inst { cname: cname.clone(), ret: Some(ret) });
		let bare = crate::modules::bare_name(&decl.name);
		let abi_ps: Vec<&str> = ptys.iter().map(|t| abi_type(*t).unwrap()).collect();
		let abi_list = if abi_ps.is_empty() { "void".to_string() } else { abi_ps.join(", ") };
		// the host sees the plain name, whichever module declared it; repeated declarations share one slot
		let ptr = format!("sn_ext_{}", bare);
		let first = match self.ext_seen.get(&bare) {
			Some((p, r)) => {
				if *p != ptys || *r != ret {
					return err(
						format!("extern fn '{}' is declared twice with different signatures", bare),
						span,
					);
				}
				false
			}
			None => {
				self.ext_seen.insert(bare.clone(), (ptys.clone(), ret));
				true
			}
		};
		if first {
			self.global_decls
				.push_str(&format!("static {} (*{})({}) = 0;\n", abi_type(ret).unwrap(), ptr, abi_list));
		}
		let mut params = Vec::new();
		let mut args = Vec::new();
		for (i, t) in ptys.iter().enumerate() {
			params.push(format!("{} a{}", self.cty(*t), i));
			args.push(match t {
				Ty::Str => format!("a{}->data", i),
				Ty::Bool => format!("(int32_t)a{}", i),
				_ => format!("a{}", i),
			});
		}
		let rc = self.cty(ret);
		let sig = format!("static {} {}({})", rc, cname, if params.is_empty() { "void".into() } else { params.join(", ") });
		let call = format!("{}({})", ptr, args.join(", "));
		let body = match ret {
			Ty::Void => format!("{};", call),
			Ty::Str => format!("const char *r = {}; return r ? sn_str_new(r, (int64_t)strlen(r)) : sn_str_new(\"\", 0);", call),
			Ty::Bool => format!("return {} != 0;", call),
			_ => format!("return {};", call),
		};
		self.protos.push_str(&format!("{};\n", sig));
		self.bodies.push_str(&format!(
			"{sig} {{ if (!{ptr}) sn_panic(\"extern {bare}\", \"the host did not provide extern function '{bare}'\"); {body} }}\n\n",
			sig = sig, ptr = ptr, bare = bare, body = body
		));
		if first {
			self.bodies.push_str(&format!(
				"SN_EXPORT void sn_set_extern_{n}(void *fp) {{ {p} = ({t} (*)({l}))fp; }}\n\n",
				n = bare,
				p = ptr,
				t = abi_type(ret).unwrap(),
				l = abi_list
			));
			let sig_text = format!("{} {}({})", abi_type(ret).unwrap(), bare, abi_list);
			self.lib_header.push_str(&format!(
				"/* host function: {} */\nvoid sn_set_extern_{}(void *fn);\n",
				sig_text, bare
			));
		}
		Ok((cname, ret))
	}

	/// The C-callable wrapper for `export fn`, used by library builds.
	pub(super) fn export_wrapper(&mut self, decl: &'a FnDecl) -> R<()> {
		let mut ptys = Vec::new();
		for p in &decl.params {
			match &p.ty {
				Some(t) => ptys.push(self.resolve_type(t, decl.span)?),
				None => {
					return err(
						format!("export fn '{}': parameter '{}' needs a type", crate::modules::bare_name(&decl.name), p.name),
						decl.span,
					)
				}
			}
		}
		let (cname, ret) = self.instance(decl, ptys.clone(), decl.span)?;
		self.abi_check(decl, &ptys, ret, "export")?;
		let mut params = Vec::new();
		let mut args = Vec::new();
		for (i, t) in ptys.iter().enumerate() {
			params.push(format!("{} a{}", abi_type(*t).unwrap(), i));
			args.push(match t {
				Ty::Str => format!("sn_lib_str(a{})", i),
				Ty::Bool => format!("(a{} != 0)", i),
				_ => format!("a{}", i),
			});
		}
		let rt = abi_type(ret).unwrap();
		let plist = if params.is_empty() { "void".to_string() } else { params.join(", ") };
		let call = format!("{}({})", cname, args.join(", "));
		let (decl_r, set_r) = match ret {
			Ty::Void => (String::new(), format!("{};", call)),
			Ty::Str => (format!("{} volatile r = 0; ", rt), format!("r = sn_lib_ret({});", call)),
			Ty::Bool => (format!("{} volatile r = 0; ", rt), format!("r = {} ? 1 : 0;", call)),
			_ => (format!("{} volatile r = 0; ", rt), format!("r = {};", call)),
		};
		let _ = zero_of;
		let ret_stmt = if ret == Ty::Void { "return;" } else { "return r;" };
		self.lib_exports.push_str(&format!(
			"SN_EXPORT {rt} sn_{n}({plist}) {{\n    {decl_r}SN_LIB_ENTER({failret})\n    if (setjmp(sn_lt.jb) == 0) {{ {set_r} sn_try_top = sn_lt.prev; }} else sn_lib_fail();\n    SN_LIB_LEAVE\n    {ret_stmt}\n}}\n\n",
			rt = rt,
			n = decl.name,
			plist = plist,
			decl_r = decl_r,
			set_r = set_r,
			failret = if ret == Ty::Void { "" } else { "0" },
			ret_stmt = ret_stmt
		));
		self.lib_header.push_str(&format!("{} sn_{}({});\n", rt, decl.name, plist));
		Ok(())
	}

	/// Prelude shared by the generated library entry points.
	pub(super) fn lib_prelude() -> &'static str {
		r#"
#include <pthread.h>
#ifdef SN_LIB_ASYNC
#define SN_LIB_SYNC_BASE sn_main_base = sn_gc.stack_base;
#else
#define SN_LIB_SYNC_BASE
#endif
static pthread_mutex_t sn_lib_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_t sn_lib_owner;
static int sn_lib_depth, sn_lib_ready;
static char sn_lib_err[1024];
static __thread char *sn_lib_retbuf;
static __thread size_t sn_lib_retcap;
static inline SnStr *sn_lib_str(const char *s) { return s ? sn_str_new(s, (int64_t)strlen(s)) : sn_str_new("", 0); }
static const char *sn_lib_ret(SnStr *s) {
    if ((size_t)s->len + 1 > sn_lib_retcap) {
        sn_lib_retcap = (size_t)s->len + 64;
        sn_lib_retbuf = (char *)realloc(sn_lib_retbuf, sn_lib_retcap);
    }
    memcpy(sn_lib_retbuf, s->data, (size_t)s->len);
    sn_lib_retbuf[s->len] = 0;
    return sn_lib_retbuf;
}
static void sn_lib_fail(void) {
    snprintf(sn_lib_err, sizeof sn_lib_err, "%s: %s\n  at %s", sn_exc_kind ? "RuntimeError" : "Error",
             sn_exc ? sn_exc->data : "?", sn_exc_loc ? sn_exc_loc : "?");
}
/* One thread at a time may run script code; calls from the same thread nest (host callbacks may call back in). */
#define SN_LIB_ENTER(failret) \
    if (pthread_equal(sn_lib_owner, pthread_self()) && sn_lib_depth > 0) sn_lib_depth++; \
    else { pthread_mutex_lock(&sn_lib_mu); sn_lib_owner = pthread_self(); sn_lib_depth = 1; \
           sn_gc.stack_base = (uintptr_t)__builtin_frame_address(0) + 256; SN_LIB_SYNC_BASE sn_lib_err[0] = 0; } \
    if (!sn_lib_ready) { snprintf(sn_lib_err, sizeof sn_lib_err, "sn_init() has not been called"); \
        if (--sn_lib_depth == 0) pthread_mutex_unlock(&sn_lib_mu); return failret; } \
    SnTry sn_lt; sn_lt.prev = sn_try_top; sn_try_top = &sn_lt;
#define SN_LIB_LEAVE if (--sn_lib_depth == 0) pthread_mutex_unlock(&sn_lib_mu);
SN_EXPORT const char *sn_last_error(void) { return sn_lib_err; }
#ifdef SN_LIB_ASYNC
/* Run tasks that are ready (and fire due timers / finished file reads) without blocking.
 * Call this once per frame. Returns 1 if tasks are still waiting, 0 if idle, -1 if a task
 * failed without being awaited (see sn_last_error). */
SN_EXPORT int sn_pump(void) {
    int volatile result = 0;
    SN_LIB_ENTER(0)
    if (setjmp(sn_lt.jb) == 0) {
        sn_poll_events();
        for (int i = 0; i < 10000; i++) {
            SnTask *t = sn_ready_pop();
            if (!t) break;
            sn_resume(t);
            sn_poll_events();
        }
        for (SnTask *t = sn_unobserved; t; t = t->unobs_next) {
            if (t->observed || !t->failed) continue;
            t->observed = 1;
            snprintf(sn_lib_err, sizeof sn_lib_err, "%s: %s\n  at %s\n  (in a task that was never awaited)",
                     t->exc_kind ? "RuntimeError" : "Error", t->exc->data, t->exc_loc);
            result = -1;
            break;
        }
        if (result == 0 && (sn_ready_head || sn_timers || sn_io_pending)) result = 1;
        sn_try_top = sn_lt.prev;
    } else { sn_lib_fail(); result = -1; }
    SN_LIB_LEAVE
    return result;
}
#else
SN_EXPORT int sn_pump(void) { return 0; }
#endif
/* Handles: small integers that stand for script objects (the table keeps them alive until released). */
static void **sn_hand;
static int64_t sn_hand_cap, sn_hand_next = 1, *sn_hand_free, sn_hand_nfree;
static int64_t sn_hand_new(void *obj) {
    int64_t h;
    if (sn_hand_nfree > 0) h = sn_hand_free[--sn_hand_nfree];
    else {
        if (sn_hand_next >= sn_hand_cap) {
            int64_t nc = sn_hand_cap ? sn_hand_cap * 2 : 256;
            void **nt = (void **)sn_gc_alloc(sizeof(void *) * (size_t)nc, 0);
            if (sn_hand) memcpy(nt, sn_hand, sizeof(void *) * (size_t)sn_hand_cap);
            sn_hand = nt; sn_hand_cap = nc;
        }
        h = sn_hand_next++;
    }
    sn_hand[h] = obj;
    return h;
}
static void *sn_hand_get(int64_t h, int64_t lo, int64_t hi, const char *cls) {
    if (h < 1 || h >= sn_hand_next || !sn_hand[h]) sn_panic("<host call>", "invalid or released handle %" PRId64, h);
    int64_t c = *(int64_t *)sn_hand[h];
    if (c < lo || c > hi) sn_panic("<host call>", "handle %" PRId64 " is not a %s", h, cls);
    return sn_hand[h];
}
SN_EXPORT void sn_release(int64_t h) {
    pthread_mutex_lock(&sn_lib_mu);
    if (h >= 1 && h < sn_hand_next && sn_hand[h]) {
        sn_hand[h] = 0;
        sn_hand_free = (int64_t *)realloc(sn_hand_free, sizeof(int64_t) * (size_t)(sn_hand_nfree + 1));
        sn_hand_free[sn_hand_nfree++] = h;
    }
    pthread_mutex_unlock(&sn_lib_mu);
}
"#
	}
}

fn mk(kind: ExprKind, span: Span) -> Expr {
	Expr { kind, span }
}

impl<'a> Gen<'a> {
	/// Emit one C-callable function. `params` are the ABI parameters (after the handle, if any);
	/// `body` runs inside the error-catching frame and sets `r` for a non-void result.
	fn lib_fn(&mut self, cname: &str, params: &[(Ty, String)], ret: Ty, body: &str) {
		let rt = abi_type(ret).unwrap();
		let mut plist: Vec<String> = params.iter().map(|(t, n)| format!("{} {}", abi_type(*t).unwrap_or("int64_t"), n)).collect();
		if plist.is_empty() {
			plist.push("void".into());
		}
		let decl_r = if ret == Ty::Void { String::new() } else { format!("{} volatile r = 0; ", rt) };
		let ret_stmt = if ret == Ty::Void { "return;" } else { "return r;" };
		self.lib_exports.push_str(&format!(
			"SN_EXPORT {rt} {n}({pl}) {{\n    {decl_r}SN_LIB_ENTER({failret})\n    if (setjmp(sn_lt.jb) == 0) {{\n{body}        sn_try_top = sn_lt.prev;\n    }} else sn_lib_fail();\n    SN_LIB_LEAVE\n    {ret_stmt}\n}}\n\n",
			rt = rt,
			n = cname,
			pl = plist.join(", "),
			decl_r = decl_r,
			failret = if ret == Ty::Void { "" } else { "0" },
			body = body,
			ret_stmt = ret_stmt
		));
		self.lib_header.push_str(&format!("{} {}({});\n", rt, cname, plist.join(", ")));
	}

	/// C statements that turn ABI parameter `a<i>` into the script value `av<i>`.
	fn lib_conv(&self, params: &[Ty]) -> String {
		let mut s = String::new();
		for (i, t) in params.iter().enumerate() {
			match t {
				Ty::Str => s.push_str(&format!("        SnStr *av{i} = sn_lib_str(a{i});\n", i = i)),
				Ty::Bool => s.push_str(&format!("        bool av{i} = a{i} != 0;\n", i = i)),
				Ty::Int => s.push_str(&format!("        int64_t av{i} = a{i};\n", i = i)),
				_ => s.push_str(&format!("        double av{i} = a{i};\n", i = i)),
			}
		}
		s
	}

	fn abi_ok(t: Ty) -> bool {
		abi_type(t).is_some()
	}

	/// Resolve a parameter list to ABI types; None if a parameter is untyped or not an ABI type.
	fn abi_params(&mut self, decl: &FnDecl) -> Option<Vec<Ty>> {
		let mut v = Vec::new();
		for p in &decl.params {
			let t = self.resolve_type(p.ty.as_ref()?, decl.span).ok()?;
			if t == Ty::Void || !Self::abi_ok(t) {
				return None;
			}
			v.push(t);
		}
		Some(v)
	}

	/// Compile `build` in a scratch function context where `o` is the object and `av<i>` the arguments.
	fn with_scratch<T>(&mut self, cls: usize, ptys: &[Ty], f: impl FnOnce(&mut Self) -> R<T>) -> R<(T, String)> {
		let mut cx = Ctx::new(true);
		cx.indent = 2;
		cx.scopes[0].insert("o".into(), Var { ty: Ty::Class(cls as u32), is_const: false, cname: "o".into(), shared: false });
		for (i, t) in ptys.iter().enumerate() {
			cx.scopes[0].insert(format!("av{}", i), Var { ty: *t, is_const: false, cname: format!("av{}", i), shared: false });
		}
		let saved = std::mem::replace(&mut self.cx, cx);
		let r = f(self);
		let used = std::mem::replace(&mut self.cx, saved);
		Ok((r?, used.out))
	}

	fn result_stmt(ret: Ty, c: &str) -> String {
		match ret {
			Ty::Void => format!("        {};\n", c),
			Ty::Str => format!("        r = sn_sn_ret_placeholder({});\n", c).replace("sn_sn_ret_placeholder", "sn_lib_ret"),
			Ty::Bool => format!("        r = ({}) ? 1 : 0;\n", c),
			_ => format!("        r = {};\n", c),
		}
	}

	/// Wrappers for `export class`: `sn_new_<Class>`, `sn_<Class>_<method>`, `sn_<Class>_get_/set_<field>`.
	pub(super) fn export_class(&mut self, ix: usize) -> R<()> {
		self.ensure_layout(ix)?;
		let cname = self.classes[ix].name.clone();
		let bare = crate::modules::bare_name(&cname);
		let span = self.classes[ix].decl.span;
		let (lo, hi) = (self.classes[ix].id_lo, self.classes[ix].id_hi);
		let ctype = format!("SnO_{}", ix);
		// constructor
		let ctor_params: Vec<Ty> = match self.find_method(ix, "onCreation") {
			Some((_, d)) => match self.abi_params(d) {
				Some(p) => p,
				None => {
					return err(
						format!("export class '{}': the parameters of onCreation must be annotated Int, Float, Bool or String", bare),
						span,
					)
				}
			},
			None => Vec::new(),
		};
		let (e, text) = self.with_scratch(ix, &ctor_params, |g| {
			let args = (0..ctor_params.len())
				.map(|i| Arg { name: None, value: mk(ExprKind::Ident(format!("av{}", i)), span) })
				.collect();
			g.expr(&mk(ExprKind::Call(Box::new(mk(ExprKind::Ident(cname.clone()), span)), args), span))
		})?;
		let mut body = self.lib_conv(&ctor_params);
		body.push_str(&text);
		body.push_str(&format!("        r = sn_hand_new((void *){});\n", e.c));
		let ps: Vec<(Ty, String)> = ctor_params.iter().enumerate().map(|(i, t)| (*t, format!("a{}", i))).collect();
		self.lib_fn(&format!("sn_new_{}", bare), &ps, Ty::Int, &body);
		let lookup = format!("        {ct} *o = ({ct} *)sn_hand_get(h, {lo}, {hi}, \"{b}\");\n", ct = ctype, lo = lo, hi = hi, b = bare);
		// methods (including inherited ones)
		let mut names: Vec<String> = Vec::new();
		let mut k = Some(ix);
		while let Some(i) = k {
			let mut ms: Vec<String> = self.classes[i].methods.keys().cloned().collect();
			ms.sort();
			for m in ms {
				if !names.contains(&m) {
					names.push(m);
				}
			}
			k = self.classes[i].parent;
		}
		for m in names {
			if m == "onCreation" || m.starts_with('_') {
				continue;
			}
			let (_, d) = self.find_method(ix, &m).unwrap();
			if d.is_private || d.is_async {
				continue;
			}
			let Some(ptys) = self.abi_params(d) else { continue };
			let compiled = self.with_scratch(ix, &ptys, |g| {
				let args = (0..ptys.len())
					.map(|i| Arg { name: None, value: mk(ExprKind::Ident(format!("av{}", i)), span) })
					.collect();
				let callee = mk(ExprKind::Member(Box::new(mk(ExprKind::Ident("o".into()), span)), m.clone()), span);
				g.expr(&mk(ExprKind::Call(Box::new(callee), args), span))
			});
			let Ok((e, text)) = compiled else { continue };
			if !Self::abi_ok(e.ty) {
				continue;
			}
			let mut body = lookup.clone();
			body.push_str(&self.lib_conv(&ptys));
			body.push_str(&text);
			body.push_str(&Self::result_stmt(e.ty, &e.c));
			let mut ps = vec![(Ty::Int, "h".to_string())];
			ps.extend(ptys.iter().enumerate().map(|(i, t)| (*t, format!("a{}", i))));
			self.lib_fn(&format!("sn_{}_{}", bare, m), &ps, e.ty, &body);
		}
		// fields
		let fields: Vec<(String, Ty, bool)> = self.classes[ix]
			.fields
			.iter()
			.filter(|f| !f.is_private && !f.name.starts_with('_') && Self::abi_ok(f.ty) && f.ty != Ty::Void)
			.map(|f| (f.name.clone(), f.ty, f.is_const))
			.collect();
		for (name, ty, is_const) in fields {
			let (e, text) = self.with_scratch(ix, &[], |g| {
				g.expr(&mk(ExprKind::Member(Box::new(mk(ExprKind::Ident("o".into()), span)), name.clone()), span))
			})?;
			let mut body = lookup.clone();
			body.push_str(&text);
			body.push_str(&Self::result_stmt(ty, &e.c));
			self.lib_fn(&format!("sn_{}_get_{}", bare, name), &[(Ty::Int, "h".into())], ty, &body);
			if !is_const {
				let ((), text) = self.with_scratch(ix, &[ty], |g| {
					let target = mk(ExprKind::Member(Box::new(mk(ExprKind::Ident("o".into()), span)), name.clone()), span);
					g.stmt(&Stmt {
						kind: StmtKind::Assign { target, op: None, value: mk(ExprKind::Ident("av0".into()), span) },
						span,
					})
				})?;
				let mut body = lookup.clone();
				body.push_str(&self.lib_conv(&[ty]));
				body.push_str(&text);
				self.lib_fn(
					&format!("sn_{}_set_{}", bare, name),
					&[(Ty::Int, "h".into()), (ty, "a0".into())],
					Ty::Void,
					&body,
				);
			}
		}
		Ok(())
	}
}
