//! Standard library support: string methods and the `__name` intrinsics that
//! the embedded `std.*` modules (stdlib/*.sn) are built on.

use super::*;

/// (name, parameter types, return type, C template). `{0}`, `{1}`: arguments, `{loc}`: source location.
fn intrinsic(name: &str) -> Option<(Vec<Ty>, Ty, &'static str)> {
    use Ty::*;
    let m1 = |c: &'static str| Some((vec![Float], Float, c));
    Some(match name {
        "__sqrt" => return m1("sqrt({0})"),
        "__sin" => return m1("sin({0})"),
        "__cos" => return m1("cos({0})"),
        "__tan" => return m1("tan({0})"),
        "__asin" => return m1("asin({0})"),
        "__acos" => return m1("acos({0})"),
        "__atan" => return m1("atan({0})"),
        "__exp" => return m1("exp({0})"),
        "__log" => return m1("log({0})"),
        "__log10" => return m1("log10({0})"),
        "__atan2" => (vec![Float, Float], Float, "atan2({0}, {1})"),
        "__pow" => (vec![Float, Float], Float, "pow({0}, {1})"),
        "__floor" => (vec![Float], Int, "sn_floor_i({0}, {loc})"),
        "__ceil" => (vec![Float], Int, "sn_ceil_i({0}, {loc})"),
        "__round" => (vec![Float], Int, "sn_round_i({0}, {loc})"),
        "__trunc" => (vec![Float], Int, "sn_trunc_i({0}, {loc})"),
        "__isNaN" => (vec![Float], Bool, "(isnan({0}) != 0)"),
        "__random" => (vec![], Float, "sn_random()"),
        "__seed" => (vec![Int], Void, "sn_seed({0})"),
        "__randInt" => (vec![Int, Int], Int, "sn_rand_int({0}, {1}, {loc})"),
        "__now" => (vec![], Float, "sn_wall_time()"),
        "__monotonic" => (vec![], Float, "sn_mono_time()"),
        "__argCount" => (vec![], Int, "sn_arg_count()"),
        "__arg" => (vec![Int], Str, "sn_arg({0}, {loc})"),
        "__write" => (vec![Str], Void, "sn_write({0})"),
        "__exit" => (vec![Int], Void, "sn_exit({0})"),
        "__env" => (vec![Str], Str, "sn_getenv({0})"),
        "__hasEnv" => (vec![Str], Bool, "sn_hasenv({0})"),
        "__readLine" => (vec![], Str, "sn_read_line({loc})"),
        "__readText" => (vec![Str], Str, "sn_read_text({0}, {loc})"),
        "__writeText" => (vec![Str, Str], Void, "sn_write_text({0}, {1}, false, {loc})"),
        "__appendText" => (vec![Str, Str], Void, "sn_write_text({0}, {1}, true, {loc})"),
        "__fileExists" => (vec![Str], Bool, "sn_file_exists({0})"),
        _ => return None,
    })
}

impl<'a> Gen<'a> {
    pub(super) fn intrinsic_call(&mut self, name: &str, args: &[Arg], span: Span) -> R<E> {
        let Some((ptys, ret, tmpl)) = intrinsic(name) else {
            return err(format!("undefined function '{}'", name), span);
        };
        if args.iter().any(|a| a.name.is_some()) {
            return err(format!("'{}' does not accept named arguments", name), span);
        }
        if args.len() != ptys.len() {
            return err(format!("'{}' takes {} argument(s), but {} were given", name, ptys.len(), args.len()), span);
        }
        let mut parts = Vec::new();
        for (a, p) in args.iter().zip(&ptys) {
            let v = self.val(&a.value)?;
            let vty = v.ty;
            let Some(c) = self.coerce(v.c, vty, *p) else {
                return err(format!("'{}' needs {}, got {}", name, p.name(), vty.name()), a.value.span);
            };
            parts.push(E::new(c, *p, v.pure));
        }
        let loc = self.loc(span);
        let text = self.ordered(parts, |c| {
            let mut s = tmpl.replace("{loc}", &loc);
            for (i, a) in c.iter().enumerate() {
                s = s.replace(&format!("{{{}}}", i), a);
            }
            s
        });
        Ok(E::new(text, ret, false))
    }

    pub(super) fn str_method(&mut self, o: E, name: &str, args: &[Arg], span: Span) -> R<E> {
        if args.iter().any(|a| a.name.is_some()) {
            return err("String methods do not accept named arguments", span);
        }
        let loc = self.loc(span);
        // (parameter types, return type, C template with {0} = the string, {1}.. = arguments)
        let (ptys, ret, tmpl): (Vec<Ty>, Ty, &str) = match name {
            "length" => (vec![], Ty::Int, "({0})->len"),
            "upper" => (vec![], Ty::Str, "sn_str_upper({0})"),
            "lower" => (vec![], Ty::Str, "sn_str_lower({0})"),
            "trim" => (vec![], Ty::Str, "sn_str_trim({0})"),
            "contains" => (vec![Ty::Str], Ty::Bool, "(sn_str_find({0}, {1}, 1) > 0)"),
            "startsWith" => (vec![Ty::Str], Ty::Bool, "sn_str_starts({0}, {1})"),
            "endsWith" => (vec![Ty::Str], Ty::Bool, "sn_str_ends({0}, {1})"),
            "replace" => (vec![Ty::Str, Ty::Str], Ty::Str, "sn_str_replace({0}, {1}, {2}, {loc})"),
            "slice" => (vec![Ty::Int, Ty::Int], Ty::Str, "sn_str_slice({0}, {1}, {2}, {loc})"),
            "repeat" => (vec![Ty::Int], Ty::Str, "sn_str_repeat({0}, {1}, {loc})"),
            "find" if args.len() == 2 => (vec![Ty::Str, Ty::Int], Ty::Int, "sn_str_find({0}, {1}, {2})"),
            "find" => (vec![Ty::Str], Ty::Int, "sn_str_find({0}, {1}, 1)"),
            "split" | "chars" => return self.str_split(o, name, args, span),
            _ => {
                return err(
                    format!(
                        "String has no method '{}' (available: length, upper, lower, trim, contains, startsWith, endsWith, find, replace, slice, repeat, split, chars)",
                        name
                    ),
                    span,
                )
            }
        };
        if args.len() != ptys.len() {
            return err(
                format!("{}() takes {} argument(s), but {} were given", name, ptys.len(), args.len()),
                span,
            );
        }
        let mut parts = vec![o];
        for (a, p) in args.iter().zip(&ptys) {
            let v = self.val(&a.value)?;
            if v.ty != *p {
                return err(
                    format!("{}() needs a {} here, got {}", name, p.name(), v.ty.name()),
                    a.value.span,
                );
            }
            parts.push(v);
        }
        let pure = parts.iter().all(|p| p.pure);
        let text = self.ordered(parts, |c| {
            let mut s = tmpl.replace("{loc}", &loc);
            for (i, a) in c.iter().enumerate() {
                s = s.replace(&format!("{{{}}}", i), a);
            }
            s
        });
        Ok(E::new(text, ret, pure && name != "replace" && name != "slice" && name != "repeat"))
    }

    fn str_split(&mut self, o: E, name: &str, args: &[Arg], span: Span) -> R<E> {
        let lty = Ty::list_of(Ty::Str);
        let ln = self.type_name(lty);
        let n = self.fresh();
        let loc = self.loc(span);
        if name == "chars" {
            if !args.is_empty() {
                return err("chars() takes no arguments", span);
            }
            let c = format!(
                "({{ SnStr *s{n} = {s}; {ln}* r{n} = {ln}_new(s{n}->len); for (int64_t i{n} = 1; i{n} <= s{n}->len; i{n}++) {ln}_push(r{n}, sn_str_at(s{n}, i{n}, {loc})); r{n}; }})",
                n = n, s = o.c, ln = ln, loc = loc
            );
            return Ok(E::new(c, lty, false));
        }
        if args.len() != 1 {
            return err("split() takes 1 argument (the separator)", span);
        }
        let sep = self.val(&args[0].value)?;
        if sep.ty != Ty::Str {
            return err(format!("split() needs a String separator, got {}", sep.ty.name()), args[0].value.span);
        }
        let text = self.ordered(vec![o, sep], |c| {
            format!(
                "({{ SnStr *s{n} = {s}; SnStr *sep{n} = {p}; if (sep{n}->len == 0) sn_panic({loc}, \"split() needs a non-empty separator\"); {ln}* r{n} = {ln}_new(8); int64_t st{n} = 1; for (;;) {{ int64_t i{n} = sn_str_find(s{n}, sep{n}, st{n}); if (!i{n}) {{ {ln}_push(r{n}, sn_str_sub(s{n}, st{n}, s{n}->len + 1 - st{n})); break; }} {ln}_push(r{n}, sn_str_sub(s{n}, st{n}, i{n} - st{n})); st{n} = i{n} + sep{n}->len; }} r{n}; }})",
                n = n, s = c[0], p = c[1], ln = ln, loc = loc
            )
        });
        Ok(E::new(text, lty, false))
    }
}
