//! Enums: `enum Direction:` followed by one variant per line.
//!
//! An enum value is an Int at run time but a distinct type at compile time,
//! so it cannot be mixed up with numbers or with another enum.

use super::*;

impl<'a> Gen<'a> {
	pub(super) fn collect_enums(&mut self, module: &'a Module) -> R<()> {
		for s in &module.items {
			let StmtKind::Enum(e) = &s.kind else { continue };
			let reserved = ["Int", "Float", "Bool", "String", "Any", "List", "Tuple", "null"];
			if reserved.contains(&e.name.as_str()) || BUILTINS.contains(&e.name.as_str()) {
				return err(format!("'{}' is a built-in name and cannot be used for an enum", e.name), e.span);
			}
			if self.enum_ix.contains_key(&e.name) || self.class_ix.contains_key(&e.name) || self.fns.contains_key(&e.name) {
				return err(format!("'{}' is already defined", e.name), e.span);
			}
			self.enum_ix.insert(e.name.clone(), self.enums.len());
			ENUM_NAMES.with(|n| n.borrow_mut().push(e.name.clone()));
			self.enums.push(EnumInfo { name: e.name.clone(), variants: e.variants.clone() });
		}
		Ok(())
	}

	/// C expression for the text of enum value `c`: "Direction.Up" (`full`) or "Up".
	pub(super) fn enum_text(&mut self, e: usize, full: bool, c: &str) -> String {
		let ename = crate::modules::bare_name(&self.enums[e].name);
		let variants = self.enums[e].variants.clone();
		let k = self.fresh();
		let mut s = format!("({{ int64_t ev{} = {}; ", k, c);
		for (n, v) in &variants {
			let text = if full { format!("{}.{}", ename, n) } else { n.clone() };
			let lit = self.str_lit(&text);
			s.push_str(&format!("ev{k} == INT64_C({v}) ? {lit} : ", k = k, v = v, lit = lit));
		}
		let unknown = self.str_lit(&format!("{}.?", ename));
		s.push_str(&format!("{}; }})", unknown));
		s
	}

	/// `Direction.Up`, if `obj` names an enum. Returns None when it does not.
	pub(super) fn enum_variant(&mut self, obj: &Expr, name: &str, span: Span) -> Option<R<E>> {
		let ExprKind::Ident(en) = &obj.kind else { return None };
		if self.var_exists(en) {
			return None;
		}
		let &ix = self.enum_ix.get(en.as_str())?;
		let found = self.enums[ix].variants.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
		Some(match found {
			Some(v) => Ok(E::new(format!("INT64_C({})", v), Ty::Enum(ix as u32), true)),
			None => {
				let names: Vec<String> = self.enums[ix].variants.iter().map(|(n, _)| n.clone()).collect();
				let hint = super::did_you_mean(name, names.iter().map(|s| s.as_str()));
				err(
					format!(
						"enum '{}' has no variant '{}'{} (variants: {})",
						crate::modules::bare_name(&self.enums[ix].name),
						name,
						hint,
						names.join(", ")
					),
					span,
				)
			}
		})
	}

	/// `.name` and `.value` of an enum value.
	pub(super) fn enum_member(&mut self, o: E, e: u32, name: &str, span: Span) -> R<E> {
		match name {
			"name" => {
				let t = self.enum_text(e as usize, false, &o.c);
				Ok(E::new(t, Ty::Str, false))
			}
			"value" => Ok(E::new(o.c, Ty::Int, o.pure)),
			_ => err(format!("an enum value has only 'name' and 'value', not '{}'", name), span),
		}
	}

	/// `Direction.values()`: every variant, in declaration order.
	pub(super) fn enum_static_call(&mut self, obj: &Expr, method: &str, args: &[Arg], span: Span) -> Option<R<E>> {
		let ExprKind::Ident(en) = &obj.kind else { return None };
		if self.var_exists(en) {
			return None;
		}
		let &ix = self.enum_ix.get(en.as_str())?;
		Some((|| {
			if method != "values" {
				return err(format!("enums have the function values(), not '{}'", method), span);
			}
			if !args.is_empty() {
				return err("values() takes no arguments", span);
			}
			let lt = Ty::list_of(Ty::Enum(ix as u32));
			let ln = self.type_name(lt);
			let n = self.fresh();
			let mut s = format!("({{ {ln}* r{n} = {ln}_new({cnt}); ", ln = ln, n = n, cnt = self.enums[ix].variants.len());
			for (_, v) in self.enums[ix].variants.clone() {
				s.push_str(&format!("{}_push(r{}, INT64_C({})); ", ln, n, v));
			}
			s.push_str(&format!("r{}; }})", n));
			Ok(E::new(s, lt, false))
		})())
	}
}

impl<'a> Gen<'a> {
	/// A type name used as a value (`Int`, `String`, a class or enum name): a Type constant.
	pub(super) fn type_value(&mut self, name: &str) -> Option<E> {
		let known = matches!(name, "Int" | "Float" | "Bool" | "String" | "Null" | "List" | "Dict" | "Tuple")
			|| self.class_ix.contains_key(name)
			|| self.enum_ix.contains_key(name);
		if !known {
			return None;
		}
		let shown = crate::modules::bare_name(name);
		Some(E::new(self.str_lit(&shown), Ty::Type, true))
	}

	fn type_lit(&mut self, s: &str) -> String {
		self.str_lit(s)
	}

	/// `x.type()`: the type of a value. Lists, dictionaries and tuples report "List", "Dict" and
	/// "Tuple"; objects report their actual class.
	pub(super) fn type_of(&mut self, o: E, span: Span) -> R<E> {
		let _ = span;
		let k = self.fresh();
		let text = match o.ty {
			Ty::Int => Some("Int".to_string()),
			Ty::Float => Some("Float".to_string()),
			Ty::Bool => Some("Bool".to_string()),
			Ty::Str => Some("String".to_string()),
			Ty::Null => Some("Null".to_string()),
			Ty::List(_) => Some("List".to_string()),
			Ty::Dict(..) => Some("Dict".to_string()),
			Ty::Tuple(_) => Some("Tuple".to_string()),
			Ty::Func(..) => Some("Fn".to_string()),
			Ty::Task(_) => Some("Task".to_string()),
			Ty::Type => Some("Type".to_string()),
			Ty::Enum(e) => Some(crate::modules::bare_name(&self.enums[e as usize].name)),
			_ => None,
		};
		if let Some(t) = text {
			let lit = self.type_lit(&t);
			let c = if o.pure { lit } else { format!("({{ (void)({}); {}; }})", o.c, lit) };
			return Ok(E::new(c, Ty::Type, o.pure));
		}
		match o.ty {
			Ty::Class(_) => {
				let mut s = format!("({{ int64_t ct{k} = ({c})->cls; ", k = k, c = o.c);
				let n = self.classes.len();
				for i in 0..n {
					let lit = self.type_lit(&crate::modules::bare_name(&self.classes[i].name));
					s.push_str(&format!("ct{} == {} ? {} : ", k, self.classes[i].id_lo, lit));
				}
				let unk = self.type_lit("Object");
				s.push_str(&format!("{}; }})", unk));
				Ok(E::new(s, Ty::Type, false))
			}
			Ty::Any => {
				let names = [("SN_T_INT", "Int"), ("SN_T_FLOAT", "Float"), ("SN_T_BOOL", "Bool"), ("SN_T_STR", "String")];
				let mut s = format!("({{ SnVal av{k} = {c}; ", k = k, c = o.c);
				for (tag, n) in names {
					let lit = self.type_lit(n);
					s.push_str(&format!("av{}.tag == {} ? {} : ", k, tag, lit));
				}
				let lit = self.type_lit("Null");
				s.push_str(&format!("{}; }})", lit));
				Ok(E::new(s, Ty::Type, false))
			}
			Ty::Opt(inner) => {
				let on = self.type_name(o.ty);
				let inner_e = E::new(format!("ov{}.v", k), *inner, true);
				let t = self.type_of(inner_e, span)?;
				let null = self.type_lit("Null");
				Ok(E::new(
					format!("({{ {on} ov{k} = {c}; ov{k}.has ? {t} : {null}; }})", on = on, k = k, c = o.c, t = t.c, null = null),
					Ty::Type,
					false,
				))
			}
			_ => err("this value has no type()", span),
		}
	}
}
