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
		let ptr = format!("sn_ext_{}", decl.name);
		self.global_decls
			.push_str(&format!("static {} (*{})({}) = 0;\n", abi_type(ret).unwrap(), ptr, abi_list));
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
		self.bodies.push_str(&format!(
			"SN_EXPORT void sn_set_extern_{n}(void *fp) {{ {p} = ({t} (*)({l}))fp; }}\n\n",
			n = decl.name,
			p = ptr,
			t = abi_type(ret).unwrap(),
			l = abi_list
		));
		let sig_text = format!("{} {}({})", abi_type(ret).unwrap(), bare, abi_list);
		self.lib_header.push_str(&format!(
			"/* host function: {} */\nvoid sn_set_extern_{}(void *fn);\n",
			sig_text, decl.name
		));
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
           sn_gc.stack_base = (uintptr_t)__builtin_frame_address(0) + 256; sn_lib_err[0] = 0; } \
    if (!sn_lib_ready) { snprintf(sn_lib_err, sizeof sn_lib_err, "sn_init() has not been called"); \
        if (--sn_lib_depth == 0) pthread_mutex_unlock(&sn_lib_mu); return failret; } \
    SnTry sn_lt; sn_lt.prev = sn_try_top; sn_try_top = &sn_lt;
#define SN_LIB_LEAVE if (--sn_lib_depth == 0) pthread_mutex_unlock(&sn_lib_mu);
SN_EXPORT const char *sn_last_error(void) { return sn_lib_err; }
"#
	}
}
