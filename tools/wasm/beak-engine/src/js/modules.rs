//! ES modules: graph, linking, evaluation.
//!
//! Fetching is not here. The engine has no network and no file system; it
//! reports which addresses it still needs (`Program::requests`) and the host
//! supplies the source (`add_module`), as with stylesheets and images.
//!
//! Three steps, in this order:
//!
//! 1. Fetch until the graph is closed (host).
//! 2. Link (`link`): every module gets its environment with its function
//!    declarations already in place, and every `import` becomes a reference
//!    to the binding in the source module.
//! 3. Evaluate (`evaluate`): depth first, each module exactly once.
//!
//! Step 2 must finish for the whole graph before any evaluation, because of
//! cycles: the module that runs first in a cycle reads names of the other
//! before that body ran, and finds them only because declarations exist
//! after linking and the reference is live. A copy would silently see
//! `undefined`.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::HashMap;

use super::ast::*;
use super::interp::{Abrupt, Binding, C, Env, Interp};
use super::value::*;

/// Where a module stands in the pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    /// Fetched and parsed, nothing more.
    New,
    /// Environment exists, references are in place.
    Linked,
    /// The body is running. Reaching it again means a cycle, which is
    /// allowed, not an error.
    Running,
    Done,
    Failed,
}

pub struct Module {
    pub url: Rc<str>,
    pub prog: Rc<Program>,
    pub env: Rc<RefCell<Env>>,
    /// Exported name -> (module, local name there). A re-export
    /// (`export { x } from …`) points directly at the source.
    pub exports: HashMap<Rc<str>, (Rc<str>, Rc<str>)>,
    /// `export * from …`: searched on lookup, not copied, because the target
    /// may not be linked yet.
    pub star: Vec<Rc<str>>,
    /// Raw specifier -> resolved address, filled by the host. The engine
    /// cannot resolve `"./config.js"`: that needs the document address,
    /// which belongs to the host (see `set_location`).
    pub resolved: HashMap<Rc<str>, Rc<str>>,
    pub state: ModState,
    /// The namespace object, once someone asked for `import * as x`.
    pub ns: Option<Value>,
}

/// The local name under which `import.meta` lives in the module environment.
///
/// A binding rather than a field on `Interp`: `import.meta` is lexical. A
/// function reading it belongs to the module it is written in, not the one
/// currently calling it, and a "current module" on the interpreter would be
/// wrong on the first callback.
pub const META_LOCAL: &str = "*meta*";

/// The local name under which `export default` stores its value. Not a
/// valid identifier, so no script can hit it.
pub const DEFAULT_LOCAL: &str = "*default*";

/// The addresses this program still needs, in source order, without
/// duplicates.
pub fn requests(prog: &Program) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| { if !out.iter().any(|x| x == s) { out.push(s.to_string()) } };
    for st in &prog.body {
        match st {
            Stmt::Import(i) => add(&i.source),
            Stmt::ExportNamed { source: Some(s), .. } => add(s),
            Stmt::ExportAll { source, .. } => add(source),
            _ => {}
        }
    }
    out
}

impl Interp {
    /// Registers a fetched and parsed module. The address is the key and
    /// must already be resolved; the engine knows no relative paths.
    pub fn add_module(&mut self, url: &str, prog: Rc<Program>) {
        if self.modules.contains_key(url) { return; }
        let env = Env::new(Some(self.realm.global_env.clone()), true);
        {
            let mut e = env.borrow_mut();
            // A module is always strict, and its `this` is `undefined`, not
            // the window.
            e.strict = true;
            e.this_val = Some(Value::Undefined);
            e.imports = Some(alloc::boxed::Box::new(HashMap::new()));
        }
        let url: Rc<str> = Rc::from(url);
        {
            let meta = new_obj(Some(self.realm.object_proto.clone()));
            meta.borrow_mut().define("url", Prop::data(Value::str(&url)));
            env.borrow_mut().vars.insert(Rc::from(META_LOCAL), Binding {
                value: Value::Obj(meta), mutable: false, initialized: true });
        }
        self.modules.insert(url.clone(), Rc::new(RefCell::new(Module {
            url, prog, env, exports: HashMap::new(), star: Vec::new(),
            resolved: HashMap::new(), state: ModState::New, ns: None,
        })));
    }

    pub fn has_module(&self, url: &str) -> bool { self.modules.contains_key(url) }

    /// Where a specifier from this module points. The host says so once per
    /// pair; without the mapping the loader would later look under
    /// `"./x.js"` and find nothing.
    pub fn map_module_dep(&mut self, url: &str, spec: &str, target: &str) {
        if let Some(m) = self.modules.get(url) {
            m.borrow_mut().resolved.insert(Rc::from(spec), Rc::from(target));
        }
    }

    /// A specifier from this module, resolved. Without a mapping it stays
    /// as is (the specifiers were already absolute).
    fn dep_url(&self, url: &str, spec: &str) -> Rc<str> {
        match self.modules.get(url).and_then(|m| m.borrow().resolved.get(spec).cloned()) {
            Some(r) => r,
            None => Rc::from(spec),
        }
    }

    /// The resolved addresses this module needs.
    pub fn module_deps(&self, url: &str) -> Vec<Rc<str>> {
        self.module_requests(url).iter().map(|s| self.dep_url(url, s)).collect()
    }

    /// Which addresses this registered module still needs, resolved against
    /// its own address. The host asks after every fetch round.
    pub fn module_requests(&self, url: &str) -> Vec<String> {
        match self.modules.get(url) {
            Some(m) => requests(&m.borrow().prog),
            None => Vec::new(),
        }
    }

    /// Links the graph from `url`, without evaluating.
    ///
    /// Two passes: first every module gets its declarations and export
    /// table, only then is any reference created. In a cycle, linking one
    /// module first would query an export table that does not exist yet.
    /// The spec does the same (`InnerModuleLinking` before
    /// `InitializeEnvironment`).
    pub fn link_module(&mut self, url: &str) -> C<()> {
        let mut order: Vec<Rc<str>> = Vec::new();
        self.collect(url, &mut order)?;
        for u in &order {
            let m = self.modules.get(&**u).cloned();
            let Some(m) = m else { continue };
            if m.borrow().state != ModState::New { continue }
            let (prog, env) = { let b = m.borrow(); (b.prog.clone(), b.env.clone()) };
            self.hoist_body(&prog.body, &env)?;
            self.build_exports(&m)?;
        }
        for u in &order {
            let m = self.modules.get(&**u).cloned();
            let Some(m) = m else { continue };
            if m.borrow().state != ModState::New { continue }
            self.bind_imports(&m)?;
        }
        for u in &order {
            if let Some(m) = self.modules.get(&**u) {
                let mut b = m.borrow_mut();
                if b.state == ModState::New { b.state = ModState::Linked; }
            }
        }
        Ok(())
    }

    /// All reachable modules, depth first, without duplicates.
    fn collect(&mut self, url: &str, out: &mut Vec<Rc<str>>) -> C<()> {
        if out.iter().any(|u| &**u == url) { return Ok(()); }
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        out.push(m.borrow().url.clone());
        for dep in self.module_deps(url) { self.collect(&dep, out)?; }
        Ok(())
    }

    /// The module's export table, from its source.
    fn build_exports(&mut self, m: &Rc<RefCell<Module>>) -> C<()> {
        let (prog, me) = { let b = m.borrow(); (b.prog.clone(), b.url.clone()) };
        let mut exports: HashMap<Rc<str>, (Rc<str>, Rc<str>)> = HashMap::new();
        let mut star: Vec<Rc<str>> = Vec::new();
        for st in &prog.body {
            match st {
                Stmt::ExportNamed { decl, specifiers, source } => {
                    // `export { a as b } from "m"` points directly at `m`,
                    // not through a local binding that does not exist.
                    let from: Rc<str> = match source {
                        Some(s) => self.dep_url(&me, s),
                        None => me.clone(),
                    };
                    for sp in specifiers {
                        exports.insert(Rc::from(sp.exported.as_str()),
                                       (from.clone(), Rc::from(sp.local.as_str())));
                    }
                    if let Some(d) = decl {
                        for n in decl_names(d) {
                            exports.insert(Rc::from(n.as_str()), (me.clone(), Rc::from(n.as_str())));
                        }
                    }
                }
                Stmt::ExportDefault(_) => {
                    exports.insert(Rc::from("default"), (me.clone(), Rc::from(DEFAULT_LOCAL)));
                }
                Stmt::ExportAll { source, alias } => match alias {
                    // `export * as ns from "m"` is a single name, not a
                    // pass-through; it needs the namespace object.
                    Some(a) => { exports.insert(Rc::from(a.as_str()),
                                                (self.dep_url(&me, source), Rc::from("*namespace*"))); }
                    None => star.push(self.dep_url(&me, source)),
                },
                _ => {}
            }
        }
        let mut b = m.borrow_mut();
        b.exports = exports;
        b.star = star;
        Ok(())
    }

    /// Puts every `import` into the module's environment as a reference.
    fn bind_imports(&mut self, m: &Rc<RefCell<Module>>) -> C<()> {
        let (prog, env, me) = { let b = m.borrow(); (b.prog.clone(), b.env.clone(), b.url.clone()) };
        for st in &prog.body {
            let Stmt::Import(im) = st else { continue };
            let from = self.dep_url(&me, &im.source);
            for sp in &im.specifiers {
                match sp {
                    ImportSpec::Default(local) =>
                        self.alias(&env, local, &from, "default")?,
                    ImportSpec::Named { imported, local } =>
                        self.alias(&env, local, &from, imported)?,
                    ImportSpec::Namespace(local) => {
                        // A namespace is a value, not a reference: the object
                        // itself never changes, only its fields.
                        let ns = self.namespace(&from)?;
                        env.borrow_mut().vars.insert(Rc::from(local.as_str()),
                            Binding { value: ns, mutable: false, initialized: true });
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves a name in the target module and records the reference.
    fn alias(&mut self, env: &Rc<RefCell<Env>>, local: &str, from: &str, name: &str) -> C<()> {
        if name == "*namespace*" {
            let ns = self.namespace(from)?;
            env.borrow_mut().vars.insert(Rc::from(local),
                Binding { value: ns, mutable: false, initialized: true });
            return Ok(());
        }
        let Some((tenv, tname)) = self.resolve_export(from, name, &mut Vec::new()) else {
            // A name the target module does not export is an error, naming
            // both sides; never a silent miss.
            return self.ref_err(&alloc::format!(
                "'{name}' is not exported by {from}"));
        };
        let mut e = env.borrow_mut();
        let map = e.imports.get_or_insert_with(|| alloc::boxed::Box::new(HashMap::new()));
        map.insert(Rc::from(local), (tenv, tname));
        Ok(())
    }

    /// Where does the exported name really live? Follows re-exports and
    /// `export *`.
    fn resolve_export(&mut self, url: &str, name: &str, seen: &mut Vec<String>)
        -> Option<(Rc<RefCell<Env>>, Rc<str>)> {
        if seen.iter().any(|s| s == url) { return None; }
        seen.push(url.to_string());
        let m = self.modules.get(url)?.clone();
        let (hit, star, env) = {
            let b = m.borrow();
            (b.exports.get(name).cloned(), b.star.clone(), b.env.clone())
        };
        if let Some((from, local)) = hit {
            if &*from == url { return Some((env, local)); }
            return self.resolve_export(&from.clone(), &local.clone(), seen);
        }
        // `export *` passes on everything except `default`.
        if name != "default" {
            for s in star {
                if let Some(r) = self.resolve_export(&s.clone(), name, seen) { return Some(r); }
            }
        }
        None
    }

    /// The namespace object of a module (`ObjKind::ModuleNs`).
    ///
    /// Properties are defined with the values at creation time; property
    /// reads go through `ns_live`, which returns the current binding.
    fn namespace(&mut self, url: &str) -> C<Value> {
        if let Some(m) = self.modules.get(url) {
            if let Some(ns) = m.borrow().ns.clone() { return Ok(ns); }
        }
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        let g = new_obj(None);
        g.borrow_mut().kind = super::value::ObjKind::ModuleNs(Rc::from(url));
        g.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("Module")));
        let names: Vec<Rc<str>> = {
            let b = m.borrow();
            let mut n: Vec<Rc<str>> = b.exports.keys().cloned().collect();
            n.sort();
            n
        };
        let ns = Value::Obj(g.clone());
        m.borrow_mut().ns = Some(ns.clone());
        for n in names {
            let v = match self.resolve_export(url, &n, &mut Vec::new()) {
                Some((e, ln)) => e.borrow().vars.get(&*ln).map(|b| b.value.clone())
                                  .unwrap_or(Value::Undefined),
                None => Value::Undefined,
            };
            g.borrow_mut().define(&n, Prop {
                value: Some(v), get: None, set: None,
                writable: false, enumerable: true, configurable: false });
        }
        Ok(ns)
    }

    /// The current value of an export: the live binding behind a namespace
    /// property. `None` if the name is not an export (then the ordinary
    /// property table answers, e.g. for `Symbol.toStringTag`).
    pub fn ns_live(&mut self, url: &str, name: &str) -> Option<Value> {
        let known = self.modules.get(url)?.borrow().exports.contains_key(name);
        if !known {
            return None;
        }
        let (env, local) = self.resolve_export(url, name, &mut Vec::new())?;
        Some(env.borrow().vars.get(&*local).map(|b| b.value.clone()).unwrap_or(Value::Undefined))
    }

    /// Evaluates the graph from `url`: depth first, each module once.
    pub fn eval_module(&mut self, url: &str) -> C<()> {
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        // Copy the state out: the borrow of a `match` scrutinee lives until
        // the end of the whole `match`, and `link_module` writes to the same
        // module.
        let state = m.borrow().state;
        match state {
            // `Running` means we are inside the cycle: return, do not run it
            // a second time.
            ModState::Running | ModState::Done => return Ok(()),
            ModState::Failed => return self.type_err(&alloc::format!("module failed: {url}")),
            ModState::New => { self.link_module(url)?; }
            ModState::Linked => {}
        }
        m.borrow_mut().state = ModState::Running;
        for dep in self.module_deps(url) {
            if let Err(e) = self.eval_module(&dep) {
                m.borrow_mut().state = ModState::Failed;
                return Err(e);
            }
        }
        let (prog, env) = { let b = m.borrow(); (b.prog.clone(), b.env.clone()) };
        // The body. Declarations exist since linking, so only execute;
        // hoisting again would reset a `let` binding that a cycle already
        // filled back to uninitialised.
        let r = (|| -> C<()> {
            for st in &prog.body {
                match self.exec(st, &env) { Ok(_) => {}, Err(e) => return Err(e) }
            }
            Ok(())
        })();
        m.borrow_mut().state = if r.is_ok() { ModState::Done } else { ModState::Failed };
        // Keep the first thrower, not the last: the outer caller propagates
        // the same error and its name would overwrite the real one.
        if r.is_err() && self.module_fail.is_none() {
            self.module_fail = Some(m.borrow().url.clone());
        }
        // A namespace built before the body ran lacks the values assigned
        // later; refresh it here.
        if r.is_ok() { self.refresh_namespace(&m); }
        r
    }

    fn refresh_namespace(&mut self, m: &Rc<RefCell<Module>>) {
        let (ns, url) = { let b = m.borrow(); (b.ns.clone(), b.url.clone()) };
        let Some(Value::Obj(g)) = ns else { return };
        let names: Vec<Rc<str>> = m.borrow().exports.keys().cloned().collect();
        for n in names {
            let v = match self.resolve_export(&url, &n, &mut Vec::new()) {
                Some((e, ln)) => e.borrow().vars.get(&*ln).map(|b| b.value.clone())
                                  .unwrap_or(Value::Undefined),
                None => continue,
            };
            g.borrow_mut().define(&n, Prop {
                value: Some(v), get: None, set: None,
                writable: false, enumerable: true, configurable: false });
        }
    }
}

/// The names a declaration introduces behind an `export`.
pub fn decl_names(d: &Stmt) -> Vec<String> {
    let mut out = Vec::new();
    match d {
        Stmt::Func(f) => if let Some(n) = &f.name { out.push(n.clone()) },
        Stmt::Class(c) => if let Some(n) = &c.name { out.push(n.clone()) },
        Stmt::VarDecl(v) => for dec in &v.decls { super::eval::names_of(&dec.id, &mut out) },
        _ => {}
    }
    out
}

/// Is there a declaration behind this `export`? Then that declaration is the
/// statement to run and hoist.
pub fn unexport(st: &Stmt) -> Option<&Stmt> {
    match st {
        Stmt::ExportNamed { decl: Some(d), .. } => Some(d),
        _ => None,
    }
}

/// The outcome of an evaluation as error text; the host has no access to
/// `Abrupt`.
pub fn describe(i: &mut Interp, e: Abrupt) -> String {
    match e {
        Abrupt::Throw(v) => {
            let name = i.get(&v, "name").ok().and_then(|n| i.to_string(&n).ok());
            let msg = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
            match (name, msg) {
                (Some(n), Some(m)) if !m.is_empty() => alloc::format!("{n}: {m}"),
                (Some(n), _) if !n.is_empty() => n.to_string(),
                _ => i.to_string(&v).map(|s| s.to_string()).unwrap_or_else(|_| "uncaught exception".to_string()),
            }
        }
        _ => "illegal completion".to_string(),
    }
}
