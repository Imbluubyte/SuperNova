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
