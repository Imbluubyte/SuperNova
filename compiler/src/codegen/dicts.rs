//! Dictionaries: `Dict[K, V]` literals, indexing, assignment and methods.
//!
//! A dictionary is an insertion-ordered hash map (see `SN_DEFINE_DICT` in the
//! runtime). Keys are Int, String or Bool; values may be any type.

use super::*;

impl<'a> Gen<'a> {
    pub(super) fn dict_type_name(&mut self, kt: Ty, vt: Ty) -> String {
        let t = Ty::dict_of(kt, vt);
        if let Some(n) = self.types.get(&t) {
            return n.clone();
        }
        let (kc, vc) = (self.cty(kt), self.cty(vt));
        let (hash, keq, kfail) = match kt {
            Ty::Int => ("sn_hash_int", "sn_keq_int", "sn_kfail_int"),
            Ty::Bool => ("sn_hash_bool", "sn_keq_bool", "sn_kfail_bool"),
            _ => ("sn_hash_str", "sn_keq_str", "sn_kfail_str"),
        };
        let atomic = if kt.has_ptrs() || vt.has_ptrs() { 0 } else { 1 };
        let name = format!("SnD{}", self.types.len());
        self.type_decls.push_str(&format!(
            "SN_DEFINE_DICT({}, {}, {}, {}, {}, {}, {})\n",
            name, kc, vc, atomic, hash, keq, kfail
        ));
        self.types.insert(t, name.clone());
        name
    }

    fn key_check(&self, got: Ty, want: Ty, span: Span) -> R<()> {
        if got == want {
            Ok(())
        } else {
            err(
                format!("this dictionary has {} keys, but the key is a {}", want.name(), got.name()),
                span,
            )
        }
    }

    /// `{k: v, ...}`; `hint` supplies the key and value types of an annotated target.
    pub(super) fn dict_literal(
        &mut self,
        items: &[(Expr, Expr)],
        hint: Option<(Ty, Ty)>,
        span: Span,
    ) -> R<E> {
        let mut ks = Vec::new();
        let mut vs = Vec::new();
        for (k, v) in items {
            ks.push(self.val(k)?);
            vs.push(self.hinted(v, hint.map(|h| h.1))?);
        }
        let (kt, vt) = match hint {
            Some(h) => h,
            None => {
                if items.is_empty() {
                    return err(
                        "cannot infer the types of an empty dictionary; annotate it, e.g. var d: Dict[String, Int] = {}",
                        span,
                    );
                }
                let kt = ks[0].ty;
                if !matches!(kt, Ty::Int | Ty::Str | Ty::Bool) {
                    return err(
                        format!("dictionary keys must be Int, String or Bool, not {}", kt.name()),
                        items[0].0.span,
                    );
                }
                (kt, self.unify_elems(&vs, span)?)
            }
        };
        let dty = Ty::dict_of(kt, vt);
        let dn = self.type_name(dty);
        let n = self.fresh();
        let mut s = format!("({{ {dn}* d{n} = {dn}_new({}); ", items.len(), dn = dn, n = n);
        let mut pure = true;
        let (kc, vc) = (self.cty(kt), self.cty(vt));
        for (i, ((k, v), (kx, _vx))) in ks.into_iter().zip(vs).zip(items.iter().map(|(a, b)| (a, b))).enumerate() {
            pure &= k.pure && v.pure;
            self.key_check(k.ty, kt, kx.span)?;
            let vty = v.ty;
            let Some(c) = self.coerce(v.c, vty, vt) else {
                return err(
                    format!("a {} value does not fit in a Dict[{}, {}]", vty.name(), kt.name(), vt.name()),
                    _vx.span,
                );
            };
            s.push_str(&format!(
                "{{ {kc} k{n}_{i} = {k}; {vc} v{n}_{i} = {c}; {dn}_put(d{n}, k{n}_{i}, v{n}_{i}); }} ",
                kc = kc, vc = vc, k = k.c, c = c, dn = dn, n = n, i = i
            ));
        }
        s.push_str(&format!("d{}; }})", n));
        Ok(E::new(s, dty, pure))
    }

    /// `d[key]`
    pub(super) fn dict_index(&mut self, b: E, idx: &Expr, span: Span) -> R<E> {
        let Ty::Dict(kt, vt) = b.ty else { unreachable!() };
        let k = self.val(idx)?;
        self.key_check(k.ty, *kt, idx.span)?;
        let dn = self.type_name(b.ty);
        let loc = self.loc(span);
        let c = self.seq2(&b, &k, move |a, x| format!("(*{}_at({}, {}, {}))", dn, a, x, loc));
        Ok(E::new(c, *vt, false))
    }

    /// `d[key] = v` and `d[key] += v`
    pub(super) fn dict_assign(
        &mut self,
        b: E,
        (kt, vt): (Ty, Ty),
        ix: &Expr,
        op: Option<BinOp>,
        value: &Expr,
        span: Span,
    ) -> R<()> {
        let k = self.val(ix)?;
        self.key_check(k.ty, kt, ix.span)?;
        let rhs = match op {
            None => self.hinted(value, Some(vt))?,
            Some(_) => self.val(value)?,
        };
        let n = self.fresh();
        let (pn, rv) = (format!("p{}", n), format!("r{}", n));
        let dn = self.type_name(b.ty);
        let (kc, vc, rc) = (self.cty(kt), self.cty(vt), self.cty(rhs.ty));
        let loc = self.loc(span);
        let combined = match op {
            None => E::new(rv.clone(), rhs.ty, true),
            Some(bop) => {
                let cur = E::new(format!("(*{})", pn), vt, true);
                self.binary(bop, cur, E::new(rv.clone(), rhs.ty, true), span)?
            }
        };
        let cty_ = combined.ty;
        let Some(c) = self.coerce(combined.c, cty_, vt) else {
            return err(
                format!("cannot store {} in a Dict[{}, {}]", cty_.name(), kt.name(), vt.name()),
                span,
            );
        };
        if op.is_none() {
            self.line(&format!(
                "{{ {dn}* d{n} = {d}; {kc} k{n} = {k}; {rc} {rv} = {r}; {dn}_put(d{n}, k{n}, {c}); }}",
                dn = dn, n = n, d = b.c, kc = kc, k = k.c, rc = rc, rv = rv, r = rhs.c, c = c
            ));
        } else {
            // The entry must already exist; the pointer is taken after the right-hand side is evaluated.
            self.line(&format!(
                "{{ {dn}* d{n} = {d}; {kc} k{n} = {k}; {rc} {rv} = {r}; {vc}* {pn} = {dn}_at(d{n}, k{n}, {loc}); *{pn} = {c}; }}",
                dn = dn, n = n, d = b.c, kc = kc, k = k.c, rc = rc, rv = rv, r = rhs.c, vc = vc, pn = pn, loc = loc, c = c
            ));
        }
        Ok(())
    }

    /// The keys of a dictionary as a new `List[K]`.
    pub(super) fn dict_keys(&mut self, d: E) -> E {
        self.dict_listing(d, true)
    }

    fn dict_listing(&mut self, d: E, keys: bool) -> E {
        let Ty::Dict(kt, vt) = d.ty else { unreachable!() };
        let elem = if keys { *kt } else { *vt };
        let lty = Ty::list_of(elem);
        let (dn, ln) = (self.type_name(d.ty), self.type_name(lty));
        let n = self.fresh();
        let field = if keys { "k" } else { "v" };
        let c = format!(
            "({{ {dn}* d{n} = {d}; {ln}* r{n} = {ln}_new(d{n}->count); for (int64_t i = 0; i < d{n}->used; i++) if (d{n}->ent[i].live) {ln}_push(r{n}, d{n}->ent[i].{f}); r{n}; }})",
            dn = dn, ln = ln, n = n, d = d.c, f = field
        );
        E::new(c, lty, false)
    }

    pub(super) fn dict_method(
        &mut self,
        o: E,
        (kt, vt): (Ty, Ty),
        name: &str,
        args: &[Arg],
        span: Span,
    ) -> R<E> {
        if args.iter().any(|a| a.name.is_some()) {
            return err("dictionary methods do not accept named arguments", span);
        }
        let dn = self.type_name(o.ty);
        let n = self.fresh();
        let want = |k: usize| -> R<()> {
            if args.len() != k {
                err(
                    format!("{}() takes {} argument(s), but {} were given", name, k, args.len()),
                    span,
                )
            } else {
                Ok(())
            }
        };
        match name {
            "length" => {
                want(0)?;
                Ok(E::new(format!("({})->count", o.c), Ty::Int, o.pure))
            }
            "items" => {
                want(0)?;
                let tt = Ty::tuple_of(vec![kt, vt]);
                let lty = Ty::list_of(tt);
                let (tn, ln) = (self.type_name(tt), self.type_name(lty));
                Ok(E::new(
                    format!(
                        "({{ {dn}* d{n} = {d}; {ln}* r{n} = {ln}_new(d{n}->count); for (int64_t i = 0; i < d{n}->used; i++) if (d{n}->ent[i].live) {ln}_push(r{n}, ({tn}){{d{n}->ent[i].k, d{n}->ent[i].v}}); r{n}; }})",
                        dn = dn, ln = ln, tn = tn, n = n, d = o.c
                    ),
                    lty,
                    false,
                ))
            }
            "keys" => {
                want(0)?;
                Ok(self.dict_listing(o, true))
            }
            "values" => {
                want(0)?;
                Ok(self.dict_listing(o, false))
            }
            "clear" => {
                want(0)?;
                Ok(E::new(format!("{}_clear({})", dn, o.c), Ty::Void, false))
            }
            "contains" | "remove" => {
                want(1)?;
                let k = self.val(&args[0].value)?;
                self.key_check(k.ty, kt, args[0].value.span)?;
                let (kc, op, ty) = (self.cty(kt), if name == "contains" { "has" } else { "remove" }, if name == "contains" { Ty::Bool } else { Ty::Void });
                Ok(E::new(
                    format!("({{ {dn}* d{n} = {d}; {kc} k{n} = {k}; {dn}_{op}(d{n}, k{n}); }})", dn = dn, n = n, d = o.c, kc = kc, k = k.c, op = op),
                    ty,
                    false,
                ))
            }
            "get" if args.len() == 1 => {
                let k = self.val(&args[0].value)?;
                self.key_check(k.ty, kt, args[0].value.span)?;
                let oty = Ty::opt_of(vt);
                let on = self.type_name(oty);
                let (kc, vc) = (self.cty(kt), self.cty(vt));
                Ok(E::new(
                    format!("({{ {dn}* d{n} = {d}; {kc} k{n} = {k}; {on} r{n} = {{0}}; if ({dn}_has(d{n}, k{n})) {{ r{n}.has = true; r{n}.v = {dn}_getor(d{n}, k{n}, ({vc}){{0}}); }} r{n}; }})", dn = dn, n = n, d = o.c, kc = kc, k = k.c, on = on, vc = vc),
                    oty,
                    false,
                ))
            }
            "get" => {
                want(2)?;
                let k = self.val(&args[0].value)?;
                self.key_check(k.ty, kt, args[0].value.span)?;
                let def = self.hinted(&args[1].value, Some(vt))?;
                let dty = def.ty;
                let Some(dc) = self.coerce(def.c, dty, vt) else {
                    return err(
                        format!("the default is a {}, but this dictionary holds {} values", dty.name(), vt.name()),
                        args[1].value.span,
                    );
                };
                let (kc, vc) = (self.cty(kt), self.cty(vt));
                Ok(E::new(
                    format!("({{ {dn}* d{n} = {d}; {kc} k{n} = {k}; {vc} f{n} = {dc}; {dn}_getor(d{n}, k{n}, f{n}); }})", dn = dn, n = n, d = o.c, kc = kc, k = k.c, vc = vc, dc = dc),
                    vt,
                    false,
                ))
            }
            "merge" => {
                want(1)?;
                let other = self.val(&args[0].value)?;
                if other.ty != o.ty {
                    return err(
                        format!("cannot merge a {} into a {}", other.ty.name(), o.ty.name()),
                        args[0].value.span,
                    );
                }
                Ok(E::new(
                    format!("({{ {dn}* d{n} = {d}; {dn}* o{n} = {o}; {dn}_merge(d{n}, o{n}); }})", dn = dn, n = n, d = o.c, o = other.c),
                    Ty::Void,
                    false,
                ))
            }
            _ => err(
                format!(
                    "{} has no method '{}'{} (available: length, keys, values, items, contains, get, remove, merge, clear)",
                    o.ty.name(),
                    name,
                    super::did_you_mean(name, ["length", "keys", "values", "items", "contains", "get", "remove", "merge", "clear"])
                ),
                span,
            ),
        }
    }
}
