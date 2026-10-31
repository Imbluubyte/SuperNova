//! Modules: `use a.b` loading, dependency ordering, namespacing and `private`.
//!
//! The whole program is compiled together. Each imported file is parsed, then
//! its top-level names are renamed with a unique prefix (`m1_math__add`) and
//! every reference inside it is rewritten to match; `alias.name` in an
//! importing file is rewritten to the target's prefixed name (or rejected if it
//! is `private` or missing). The files are then concatenated in dependency
//! order into one `Module`, so the type checker and C backend need no notion of
//! modules. Top-level statements of an imported file run once, before the
//! statements of the files that import it.

use crate::ast::*;
use crate::lexer::{CompileError, Span};
use crate::parser;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct SrcFile {
    /// Name shown in messages and runtime errors, e.g. `main.sn` or `utils/math.sn`.
    pub display: String,
    pub src: String,
}

pub struct Program {
    pub module: Module,
    pub files: Vec<SrcFile>,
}

type R<T> = Result<T, CompileError>;

thread_local! {
    /// Prefixed name -> name as the user wrote it (`m1_math__add` -> `math.add`).
    static DISPLAY: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
}

/// Replace prefixed internal names in a message with the names the user wrote.
pub fn unmangle(msg: &str) -> String {
    DISPLAY.with(|d| {
        let mut out = msg.to_string();
        for (m, shown) in d.borrow().iter() {
            if out.contains(m.as_str()) {
                out = out.replace(m.as_str(), shown);
            }
        }
        out
    })
}

/// A class name without any module qualification, for `<Name>` output.
pub fn bare_name(name: &str) -> String {
    let shown = unmangle(name);
    shown.rsplit('.').next().unwrap_or(&shown).to_string()
}

struct Export {
    private: bool,
}

struct ModInfo {
    prefix: String,
    stem: String,
    own: HashMap<String, Export>,
    imports: HashMap<String, usize>,
}

#[derive(Clone, PartialEq)]
enum State {
    Visiting,
    Done,
}

struct Loader {
    root: PathBuf,
    search: Vec<PathBuf>,
    files: Vec<SrcFile>,
    asts: Vec<Option<Module>>,
    by_path: HashMap<PathBuf, usize>,
    state: Vec<State>,
    /// (alias, target module, span of the `use`) per module.
    uses: Vec<Vec<(String, usize, Span)>>,
    order: Vec<usize>,
    /// Modules currently being loaded (for circular-import reports).
    stack: Vec<usize>,
}

pub fn load(entry: &str) -> Result<Program, (CompileError, Vec<SrcFile>)> {
    DISPLAY.with(|d| d.borrow_mut().clear());
    let entry_path = PathBuf::from(entry);
    let root = entry_path
        .parent()
        .map(|p| p.to_path_buf())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut search = Vec::new();
    if let Ok(paths) = std::env::var("SN_PATH") {
        for p in paths.split(':').filter(|p| !p.is_empty()) {
            search.push(PathBuf::from(p));
        }
    }
    let mut ld = Loader {
        root,
        search,
        files: Vec::new(),
        asts: Vec::new(),
        by_path: HashMap::new(),
        state: Vec::new(),
        uses: Vec::new(),
        order: Vec::new(),
        stack: Vec::new(),
    };
    let first = ld.visit(&entry_path, None);
    let first = match first {
        Ok(i) => i,
        Err(e) => return Err((e, ld.files)),
    };
    debug_assert_eq!(first, 0);
    match ld.link() {
        Ok(module) => Ok(Program { module, files: ld.files }),
        Err(e) => Err((e, ld.files)),
    }
}

fn fake_span() -> Span {
    Span { line: 1, col: 1, file: 0 }
}

impl Loader {
    fn display_for(&self, path: &Path, idx: usize) -> String {
        if idx == 0 {
            return path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
        }
        match path.strip_prefix(&self.root) {
            Ok(rel) => rel.to_string_lossy().to_string(),
            Err(_) => path.to_string_lossy().to_string(),
        }
    }

    fn visit(&mut self, path: &Path, from: Option<Span>) -> R<usize> {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(&i) = self.by_path.get(&canon) {
            if self.state[i] == State::Visiting {
                let mut chain: Vec<String> = Vec::new();
                let mut started = false;
                for &m in self.order_stack_hint() {
                    if m == i {
                        started = true;
                    }
                    if started {
                        chain.push(self.files[m].display.clone());
                    }
                }
                chain.push(self.files[i].display.clone());
                return Err(CompileError::new(
                    format!("circular import: {}", chain.join(" -> ")),
                    from.unwrap_or_else(fake_span),
                ));
            }
            return Ok(i);
        }
        let src = std::fs::read_to_string(path).map_err(|e| {
            CompileError::new(
                format!("cannot read {}: {}", path.display(), e),
                from.unwrap_or_else(fake_span),
            )
        })?;
        let idx = self.files.len();
        let display = self.display_for(path, idx);
        self.files.push(SrcFile { display, src: src.clone() });
        self.asts.push(None);
        self.state.push(State::Visiting);
        self.uses.push(Vec::new());
        self.by_path.insert(canon, idx);
        self.stack.push(idx);
        let module = parser::parse_file(&src, idx as u16)?;
        let mut seen: HashSet<String> = HashSet::new();
        for s in &module.items {
            if let StmtKind::Use { path: up, alias } = &s.kind {
                let ns = alias.clone().unwrap_or_else(|| up.last().unwrap().clone());
                if !seen.insert(ns.clone()) {
                    return Err(CompileError::new(
                        format!("'{}' is already used as a module name in this file; use 'use ... as other_name'", ns),
                        s.span,
                    ));
                }
                let target = self.resolve(up, s.span)?;
                let child = self.visit(&target, Some(s.span))?;
                self.uses[idx].push((ns, child, s.span));
            }
        }
        self.stack.pop();
        self.asts[idx] = Some(module);
        self.state[idx] = State::Done;
        self.order.push(idx);
        Ok(idx)
    }

    fn order_stack_hint(&self) -> &Vec<usize> {
        &self.stack
    }

    fn resolve(&self, path: &[String], span: Span) -> R<PathBuf> {
        let mut rel = PathBuf::new();
        for (i, seg) in path.iter().enumerate() {
            if i + 1 == path.len() {
                rel.push(format!("{}.sn", seg));
            } else {
                rel.push(seg);
            }
        }
        let mut tried = Vec::new();
        let mut roots = vec![self.root.clone()];
        roots.extend(self.search.iter().cloned());
        for r in roots {
            let cand = r.join(&rel);
            if cand.is_file() {
                return Ok(cand);
            }
            tried.push(cand.to_string_lossy().to_string());
        }
        Err(CompileError::new(
            format!("cannot find module '{}' (looked for {})", path.join("."), tried.join(", ")),
            span,
        ))
    }

    fn link(&mut self) -> R<Module> {
        let n = self.files.len();
        // Module info: prefixes and top-level names.
        let mut infos: Vec<ModInfo> = Vec::new();
        for i in 0..n {
            let stem: String = {
                let d = &self.files[i].display;
                let base = Path::new(d).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                base.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect()
            };
            let prefix = if i == 0 { String::new() } else { format!("m{}_{}__", i, stem) };
            let mut own = HashMap::new();
            for s in &self.asts[i].as_ref().unwrap().items {
                let (name, private) = match &s.kind {
                    StmtKind::Fn(f) => (f.name.clone(), f.is_private),
                    StmtKind::Class(c) => (c.name.clone(), c.is_private),
                    StmtKind::VarDecl { name, is_private, .. } => (name.clone(), *is_private),
                    _ => continue,
                };
                own.insert(name, Export { private });
            }
            let mut imports = HashMap::new();
            for (ns, child, span) in &self.uses[i] {
                if own.contains_key(ns) {
                    return Err(CompileError::new(
                        format!("the module name '{}' clashes with a top-level name in this file; use 'use ... as other_name'", ns),
                        *span,
                    ));
                }
                imports.insert(ns.clone(), *child);
            }
            infos.push(ModInfo { prefix, stem, own, imports });
        }
        DISPLAY.with(|d| {
            let mut d = d.borrow_mut();
            for info in infos.iter().skip(1) {
                for name in info.own.keys() {
                    d.push((format!("{}{}", info.prefix, name), format!("{}.{}", info.stem, name)));
                }
            }
            // Longest first so a name is never replaced inside a longer one.
            d.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        });
        // Class member tables (for bare field/method names inside methods).
        let mut classes: HashMap<String, (Option<String>, HashSet<String>)> = HashMap::new();
        for i in 0..n {
            for s in &self.asts[i].as_ref().unwrap().items {
                if let StmtKind::Class(c) = &s.kind {
                    let parent = match &c.parent {
                        Some(p) => Some(resolve_type_name(&infos, i, p, c.span)?),
                        None => None,
                    };
                    let mut members = HashSet::new();
                    for f in &c.fields {
                        if let StmtKind::VarDecl { name, .. } = &f.kind {
                            members.insert(name.clone());
                        }
                    }
                    for m in &c.methods {
                        members.insert(m.name.clone());
                    }
                    classes.insert(format!("{}{}", infos[i].prefix, c.name), (parent, members));
                }
            }
        }
        // Rewrite every module, then concatenate in dependency order.
        let mut out = Vec::new();
        let order = self.order.clone();
        for &i in &order {
            let mut module = self.asts[i].take().unwrap();
            let mut rn = Rn {
                infos: &infos,
                classes: &classes,
                me: i,
                scopes: Vec::new(),
                members: HashSet::new(),
            };
            for mut s in std::mem::take(&mut module.items) {
                if matches!(s.kind, StmtKind::Use { .. }) {
                    continue;
                }
                rn.stmt(&mut s, true)?;
                out.push(s);
            }
        }
        Ok(Module { items: out })
    }
}

fn export_name(infos: &[ModInfo], me: usize, alias: &str, target: usize, name: &str, span: Span) -> R<String> {
    let _ = me;
    match infos[target].own.get(name) {
        None => Err(CompileError::new(
            format!("module '{}' has no member '{}'", alias, name),
            span,
        )),
        Some(ex) if ex.private => Err(CompileError::new(
            format!("'{}' is private to module '{}'", name, alias),
            span,
        )),
        Some(_) => Ok(format!("{}{}", infos[target].prefix, name)),
    }
}

/// Resolve a (possibly module-qualified) class or type name.
fn resolve_type_name(infos: &[ModInfo], me: usize, name: &str, span: Span) -> R<String> {
    if let Some((alias, rest)) = name.split_once('.') {
        let Some(&target) = infos[me].imports.get(alias) else {
            return Err(CompileError::new(
                format!("'{}' is not an imported module (add 'use {}')", alias, alias),
                span,
            ));
        };
        return export_name(infos, me, alias, target, rest, span);
    }
    if infos[me].own.contains_key(name) {
        return Ok(format!("{}{}", infos[me].prefix, name));
    }
    Ok(name.to_string())
}

struct Rn<'a> {
    infos: &'a [ModInfo],
    classes: &'a HashMap<String, (Option<String>, HashSet<String>)>,
    me: usize,
    scopes: Vec<HashSet<String>>,
    /// Field and method names visible by bare name in the current method.
    members: HashSet<String>,
}

impl<'a> Rn<'a> {
    fn shadowed(&self, n: &str) -> bool {
        self.members.contains(n) || self.scopes.iter().any(|s| s.contains(n))
    }

    fn declare(&mut self, n: &str) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(n.to_string());
        }
    }

    fn mangle(&self, n: &str) -> String {
        format!("{}{}", self.infos[self.me].prefix, n)
    }

    fn ty(&mut self, t: &mut TypeExpr, span: Span) -> R<()> {
        match t {
            TypeExpr::Named(n) => *n = resolve_type_name(self.infos, self.me, n, span)?,
            TypeExpr::List(i) | TypeExpr::Optional(i) | TypeExpr::Task(i) => self.ty(i, span)?,
            TypeExpr::Dict(k, v) => {
                self.ty(k, span)?;
                self.ty(v, span)?;
            }
            TypeExpr::Tuple(items) => {
                for i in items {
                    self.ty(i, span)?;
                }
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &mut Expr) -> R<()> {
        let span = e.span;
        match &mut e.kind {
            ExprKind::Ident(n) => {
                if !self.shadowed(n) && self.infos[self.me].own.contains_key(n.as_str()) {
                    *n = self.mangle(n);
                }
            }
            ExprKind::Member(obj, name) => {
                if let ExprKind::Ident(a) = &obj.kind {
                    if !self.shadowed(a) {
                        if let Some(&target) = self.infos[self.me].imports.get(a.as_str()) {
                            let mangled = export_name(self.infos, self.me, a, target, name, span)?;
                            e.kind = ExprKind::Ident(mangled);
                            return Ok(());
                        }
                    }
                }
                self.expr(obj)?;
            }
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Null => {}
            ExprKind::FStr(parts) => {
                for p in parts {
                    if let FStrPart::Expr(x) = p {
                        self.expr(x)?;
                    }
                }
            }
            ExprKind::List(items) | ExprKind::Tuple(items) => {
                for i in items {
                    self.expr(i)?;
                }
            }
            ExprKind::Dict(items) => {
                for (k, v) in items {
                    self.expr(k)?;
                    self.expr(v)?;
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Await(x) | ExprKind::Predicate(_, x) => self.expr(x)?,
            ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
                self.expr(l)?;
                self.expr(r)?;
            }
            ExprKind::Call(callee, args) => {
                self.expr(callee)?;
                for a in args {
                    self.expr(&mut a.value)?;
                }
            }
            ExprKind::Range { start, end, .. } => {
                self.expr(start)?;
                self.expr(end)?;
            }
        }
        Ok(())
    }

    fn block(&mut self, stmts: &mut [Stmt], declared: &[&str]) -> R<()> {
        self.scopes.push(declared.iter().map(|s| s.to_string()).collect());
        for s in stmts {
            self.stmt(s, false)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn function(&mut self, f: &mut FnDecl, members: Option<HashSet<String>>) -> R<()> {
        for p in &mut f.params {
            if let Some(t) = &mut p.ty {
                self.ty(t, f.span)?;
            }
            if let Some(d) = &mut p.default {
                self.expr(d)?;
            }
        }
        if let Some(t) = &mut f.ret {
            self.ty(t, f.span)?;
        }
        let saved_scopes = std::mem::take(&mut self.scopes);
        let saved_members = std::mem::take(&mut self.members);
        if let Some(m) = members {
            self.members = m;
        }
        let names: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let r = self.block(&mut f.body, &refs);
        self.scopes = saved_scopes;
        self.members = saved_members;
        r
    }

    fn chain_members(&self, class: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut cur = Some(class.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 1000 {
                break;
            }
            match self.classes.get(&c) {
                Some((parent, members)) => {
                    out.extend(members.iter().cloned());
                    cur = parent.clone();
                }
                None => break,
            }
        }
        out
    }

    fn stmt(&mut self, s: &mut Stmt, top: bool) -> R<()> {
        let span = s.span;
        match &mut s.kind {
            StmtKind::VarDecl { name, ty, init, .. } => {
                if let Some(t) = ty {
                    self.ty(t, span)?;
                }
                self.expr(init)?;
                if top {
                    *name = self.mangle(name);
                } else {
                    self.declare(&name.clone());
                }
            }
            StmtKind::Assign { target, value, .. } => {
                self.expr(target)?;
                self.expr(value)?;
            }
            StmtKind::Expr(e) => self.expr(e)?,
            StmtKind::If { branches, else_body } => {
                for (c, b) in branches {
                    self.expr(c)?;
                    self.block(b, &[])?;
                }
                if let Some(b) = else_body {
                    self.block(b, &[])?;
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond)?;
                self.block(body, &[])?;
            }
            StmtKind::For { var, iter, body } => {
                self.expr(iter)?;
                let v = var.clone();
                self.block(body, &[v.as_str()])?;
            }
            StmtKind::Match { subject, cases, default } => {
                self.expr(subject)?;
                for (c, b) in cases {
                    self.expr(c)?;
                    self.block(b, &[])?;
                }
                if let Some(b) = default {
                    self.block(b, &[])?;
                }
            }
            StmtKind::Return(Some(e)) | StmtKind::Throw(e) => self.expr(e)?,
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue | StmtKind::Pass => {}
            StmtKind::Fn(f) => {
                if top {
                    f.name = self.mangle(&f.name);
                }
                self.function(f, None)?;
            }
            StmtKind::Class(c) => {
                let mangled = if top { self.mangle(&c.name) } else { c.name.clone() };
                let members = self.chain_members(&mangled);
                if let Some(p) = &mut c.parent {
                    *p = resolve_type_name(self.infos, self.me, p, c.span)?;
                }
                // Field initialisers run without `self`, so bare names are globals.
                for f in &mut c.fields {
                    self.stmt(f, false)?;
                }
                for m in &mut c.methods {
                    self.function(m, Some(members.clone()))?;
                }
                c.name = mangled;
            }
            StmtKind::Try { body, catch, finally } => {
                self.block(body, &[])?;
                if let Some((n, b)) = catch {
                    let n = n.clone();
                    self.block(b, &[n.as_str()])?;
                }
                if let Some(b) = finally {
                    self.block(b, &[])?;
                }
            }
            StmtKind::Use { .. } | StmtKind::Manages(_) => {}
        }
        Ok(())
    }
}
