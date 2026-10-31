//! `async fn`, `await` and the `Task[T]` type.
//!
//! Calling an `async fn` allocates a task object that holds the arguments and
//! the result, queues it, and returns the handle (`Task[T]`). The function body
//! runs later on its own stack; `await` suspends the awaiting task (or runs the
//! scheduler, at top level) until the awaited task has finished.

use super::*;

impl<'a> Gen<'a> {
    /// The typed task struct for `Task[R]`: the runtime header followed by the result.
    pub(super) fn task_struct(&mut self, r: Ty) -> String {
        let t = Ty::task_of(r);
        if let Some(n) = self.types.get(&t) {
            return n.clone();
        }
        let name = format!("SnTk{}", self.types.len());
        let field = if r == Ty::Void { String::new() } else { format!("{} result; ", self.cty(r)) };
        self.type_decls.push_str(&format!("typedef struct {{ SnTask t; {}}} {};\n", field, name));
        self.types.insert(t, name.clone());
        name
    }

    /// Emit the wrapper that spawns `body` as a task: `cname(args) -> SnTask*`.
    pub(super) fn async_wrapper(&mut self, cname: &str, body: &str, ptys_c: Vec<String>, ret: Ty) {
        let n = self.fresh();
        let st = format!("SnTI{}", n);
        let mut fields = String::new();
        let mut params = Vec::new();
        let mut names = Vec::new();
        let mut assigns = String::new();
        for (i, tc) in ptys_c.iter().enumerate() {
            fields.push_str(&format!("{} a{}; ", tc, i));
            params.push(format!("{} a{}", tc, i));
            names.push(format!("k->a{}", i));
            assigns.push_str(&format!("k->a{0} = a{0}; ", i));
        }
        let result = if ret == Ty::Void { String::new() } else { format!("{} result; ", self.cty(ret)) };
        let call = format!("{}({})", body, names.join(", "));
        let run = if ret == Ty::Void { format!("{};", call) } else { format!("k->result = {};", call) };
        let plist = if params.is_empty() { "void".to_string() } else { params.join(", ") };
        self.protos.push_str(&format!("static SnTask *{}({});\n", cname, plist));
        self.bodies.push_str(&format!(
            "typedef struct {st} {{ SnTask t; {result}{fields}}} {st};\n\
             static void {cname}_thunk(SnTask *t) {{ {st} *k = ({st} *)t; {run} }}\n\
             static SnTask *{cname}({plist}) {{ {st} *k = ({st} *)sn_task_new(sizeof({st}), {cname}_thunk); {assigns}sn_task_spawn(&k->t); return &k->t; }}\n\n",
            st = st, result = result, fields = fields, cname = cname, run = run, plist = plist, assigns = assigns
        ));
    }

    pub(super) fn await_expr(&mut self, inner: &Expr, span: Span) -> R<E> {
        if self.cx.in_fn && !self.cx.is_async {
            return err(
                "'await' can only be used inside an 'async fn' or at the top level of a file",
                span,
            );
        }
        self.uses_async = true;
        let v = self.val(inner)?;
        let Ty::Task(r) = v.ty else {
            return err(
                format!("'await' needs a Task (the result of calling an async function), not {}", v.ty.name()),
                inner.span,
            );
        };
        let r = *r;
        let n = self.fresh();
        if r == Ty::Void {
            return Ok(E::new(format!("({{ SnTask *w{n} = {}; sn_await(w{n}); }})", v.c, n = n), Ty::Void, false));
        }
        let tn = self.task_struct(r);
        Ok(E::new(
            format!("({{ SnTask *w{n} = {}; sn_await(w{n}); (({tn} *)w{n})->result; }})", v.c, n = n, tn = tn),
            r,
            false,
        ))
    }
}
