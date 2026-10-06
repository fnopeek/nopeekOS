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

// ── Resolution, import maps, `import()` ─────────────────────────────────────

/// A specifier map: key, and the URL it maps to (`None`: a key whose value
/// was invalid, which blocks the specifier instead of falling through).
/// Sorted by key in code unit order, descending, so the longest prefix wins.
pub type SpecifierMap = Vec<(String, Option<String>)>;

/// An import map (HTML §8.1.5.2).
#[derive(Default, Clone)]
pub struct ImportMap {
    pub imports: SpecifierMap,
    /// Scope prefix and its map, sorted like a specifier map.
    pub scopes: Vec<(String, SpecifierMap)>,
}

/// Module loading state beyond the graph: the import map and the dynamic
/// imports waiting for the host.
#[derive(Default)]
pub struct Loader {
    pub map: ImportMap,
    /// `import()` calls whose graph is not complete yet: entry URL and the
    /// promise to settle.
    pub pending: Vec<(Rc<str>, Gc)>,
    /// Module URLs the host could not deliver.
    pub failed: Vec<Rc<str>>,
}

/// URL-like per HTML §8.1.5.5: absolute, or starting with `/`, `./`, `../`.
fn url_like(spec: &str, base: &str) -> Option<String> {
    if spec.starts_with('/') || spec.starts_with("./") || spec.starts_with("../") {
        let b = super::url::parse_abs(base)?;
        return Some(super::url::resolve(spec, &b).href());
    }
    super::url::parse_abs(spec).map(|p| p.href())
}

fn sort_map(m: &mut SpecifierMap) {
    m.sort_by(|a, b| b.0.cmp(&a.0));
}

/// HTML §8.1.5.6 "resolve an imports match". `Err` is a blocked specifier.
fn imports_match(norm: &str, as_url: Option<&str>, map: &SpecifierMap)
    -> Result<Option<String>, String> {
    for (key, target) in map {
        if key == norm {
            return match target {
                Some(t) => Ok(Some(t.clone())),
                None => Err(alloc::format!("import map blocks '{norm}'")),
            };
        }
        let special = as_url.is_none_or(|u| u.starts_with("http:") || u.starts_with("https:"));
        if key.ends_with('/') && norm.starts_with(key.as_str()) && special {
            let Some(t) = target else {
                return Err(alloc::format!("import map blocks '{norm}'"));
            };
            let rest = &norm[key.len()..];
            let Some(b) = super::url::parse_abs(t) else {
                return Err(alloc::format!("import map target is not a URL: {t}"));
            };
            let u = super::url::resolve(rest, &b).href();
            if !u.starts_with(t.as_str()) {
                return Err(alloc::format!("'{norm}' leaves the mapped prefix {key}"));
            }
            return Ok(Some(u));
        }
    }
    Ok(None)
}

impl ImportMap {
    /// HTML §8.1.5.6 "resolve a module specifier". `Err` for a bare
    /// specifier no map covers, or one a map blocks.
    pub fn resolve(&self, base: &str, spec: &str) -> Result<String, String> {
        let spec = spec.trim();
        let as_url = url_like(spec, base);
        let norm = as_url.as_deref().unwrap_or(spec);
        for (prefix, map) in &self.scopes {
            if prefix == base || (prefix.ends_with('/') && base.starts_with(prefix.as_str())) {
                if let Some(u) = imports_match(norm, as_url.as_deref(), map)? { return Ok(u) }
            }
        }
        if let Some(u) = imports_match(norm, as_url.as_deref(), &self.imports)? { return Ok(u) }
        match as_url {
            Some(u) => Ok(u),
            None => Err(alloc::format!(
                "Failed to resolve module specifier \"{spec}\": relative references must start with \"/\", \"./\" or \"../\"")),
        }
    }

    /// Merge a later map in (HTML §8.1.5.4 "merge existing and new import
    /// maps"): what is already mapped stays.
    fn merge(&mut self, new: ImportMap) {
        for (k, v) in new.imports {
            if !self.imports.iter().any(|(e, _)| *e == k) { self.imports.push((k, v)); }
        }
        sort_map(&mut self.imports);
        for (prefix, map) in new.scopes {
            match self.scopes.iter_mut().find(|(p, _)| *p == prefix) {
                Some((_, have)) => {
                    for (k, v) in map {
                        if !have.iter().any(|(e, _)| *e == k) { have.push((k, v)); }
                    }
                    sort_map(have);
                }
                None => self.scopes.push((prefix, map)),
            }
        }
        self.scopes.sort_by(|a, b| b.0.cmp(&a.0));
    }
}

impl Interp {
    /// Where a specifier from the module or script at `base` points: the
    /// import map first, then URL resolution. The one resolution both hosts
    /// use, for static imports and `import()` alike.
    pub fn resolve_module(&self, base: &str, spec: &str) -> Result<String, String> {
        self.loader.map.resolve(base, spec)
    }

    /// Register `<script type="importmap">` text (HTML §8.1.5.3 "parse an
    /// import map string"), resolved against `base`.
    pub fn add_import_map(&mut self, text: &str, base: &str) -> Result<(), String> {
        let json = self.realm.global.borrow().get_own("JSON").and_then(|p| p.value.clone())
            .unwrap_or(Value::Undefined);
        let parse = self.get(&json, "parse").map_err(|_| String::from("no JSON"))?;
        let v = match self.call(&parse, json, &[Value::str(text)]) {
            Ok(v) => v,
            Err(e) => return Err(alloc::format!("import map: {}", describe(self, e))),
        };
        let Value::Obj(top) = &v else { return Err(String::from("import map: not an object")) };
        let mut map = ImportMap::default();
        let imports = top.borrow().get_own("imports").and_then(|p| p.value.clone());
        if let Some(im) = imports {
            map.imports = self.specifier_map(&im, base)?;
        }
        let scopes = top.borrow().get_own("scopes").and_then(|p| p.value.clone());
        if let Some(Value::Obj(sc)) = scopes {
            let keys = sc.borrow().own_keys();
            for k in keys {
                let Some(prefix) = super::url::parse_abs(base)
                    .map(|b| super::url::resolve(&k, &b).href()) else { continue };
                let m = sc.borrow().get_own(&k).and_then(|p| p.value.clone());
                if let Some(m) = m { map.scopes.push((prefix, self.specifier_map(&m, base)?)); }
            }
            map.scopes.sort_by(|a, b| b.0.cmp(&a.0));
        }
        self.loader.map.merge(map);
        Ok(())
    }

    /// Register every `<script type="importmap">` of the document, in tree
    /// order, against the document URL. Errors go to the console, as in
    /// browsers; the page goes on without that map. An external import map
    /// (`src`) is not allowed (HTML §4.12.1) and is reported.
    pub fn load_import_maps(&mut self) {
        let texts: Vec<(Option<String>, String)> = match &self.doc {
            Some(d) => {
                let mut all = Vec::new();
                d.descendants(d.doc, &mut all);
                all.into_iter().filter(|&id| {
                    let n = &d.nodes[id as usize];
                    &*n.tag == "script" && n.attr("type")
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("importmap"))
                }).map(|id| (d.nodes[id as usize].attr("src").map(|s| s.to_string()), d.text_of(id)))
                  .collect()
            }
            None => return,
        };
        let base = self.loc_href.clone();
        for (src, text) in texts {
            if let Some(s) = src {
                self.console_push(alloc::format!("import map with src is not supported: {s}"));
                continue;
            }
            if let Err(e) = self.add_import_map(&text, &base) { self.console_push(e); }
        }
    }

    /// HTML §8.1.5.3 "sort and normalize a specifier map".
    fn specifier_map(&mut self, v: &Value, base: &str) -> Result<SpecifierMap, String> {
        let Value::Obj(o) = v else { return Err(String::from("import map: a map is not an object")) };
        let mut out = SpecifierMap::new();
        let keys = o.borrow().own_keys();
        for k in keys {
            if k.is_empty() { continue }
            let key = url_like(&k, base).unwrap_or_else(|| k.to_string());
            let target = match o.borrow().get_own(&k).and_then(|p| p.value.clone()) {
                Some(Value::Str(s)) => url_like(&s, base),
                _ => None,
            };
            // A prefix key must map to a prefix.
            let target = target.filter(|t| !key.ends_with('/') || t.ends_with('/'));
            out.push((key, target));
        }
        sort_map(&mut out);
        Ok(out)
    }

    /// `import(spec)` (ES 13.3.10, HTML §8.1.5.7 HostLoadImportedModule).
    ///
    /// Returns a promise at once. The graph is fetched by the host: it reads
    /// `dynamic_import_wants`, delivers with `add_module` (or
    /// `module_unavailable`), and calls `settle_dynamic_imports`.
    pub fn dynamic_import(&mut self, spec: &Value, env: &Rc<RefCell<Env>>) -> C<Value> {
        let p = super::promise::new_promise(self);
        let s = match self.to_string(spec) {
            Ok(s) => s,
            Err(Abrupt::Throw(e)) => { super::promise::settle(self, &p, e, true); return Ok(Value::Obj(p)) }
            Err(e) => return Err(e),
        };
        let base = self.referrer_url(env);
        match self.resolve_module(&base, &s) {
            Ok(u) => {
                // Settled by the host's next `settle_dynamic_imports`, never
                // inside this call: the module must not run before the code
                // that imported it continues.
                self.loader.pending.push((Rc::from(u.as_str()), p.clone()));
            }
            Err(msg) => {
                let Abrupt::Throw(e) = self.throw_kind("TypeError", &msg) else { return Ok(Value::Obj(p)) };
                super::promise::settle(self, &p, e, true);
            }
        }
        Ok(Value::Obj(p))
    }

    /// The base URL of the code running in `env`: its module's URL, else
    /// the document's.
    fn referrer_url(&mut self, env: &Rc<RefCell<Env>>) -> String {
        let n = META_LOCAL;
        let meta = super::interp::env_lookup(env, n)
            .and_then(|e| e.borrow().vars.get(n).map(|b| b.value.clone()));
        if let Some(Value::Obj(m)) = meta {
            if let Some(Value::Str(u)) = m.borrow().get_own("url").and_then(|p| p.value.clone()) {
                return u.to_string();
            }
        }
        self.loc_href.clone()
    }

    /// Walk the graph from `entry` and map every specifier on the way.
    /// Returns the modules not yet delivered and whether one has failed.
    fn graph_missing(&mut self, entry: &str) -> (Vec<String>, Option<String>) {
        let mut seen: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut failed = None;
        let mut queue = alloc::vec![entry.to_string()];
        while let Some(u) = queue.pop() {
            if seen.iter().any(|x| *x == u) { continue }
            seen.push(u.clone());
            if self.loader.failed.iter().any(|f| **f == *u) {
                failed.get_or_insert(u);
                continue;
            }
            if !self.has_module(&u) { missing.push(u); continue }
            for spec in self.module_requests(&u) {
                match self.resolve_module(&u, &spec) {
                    Ok(r) => { self.map_module_dep(&u, &spec, &r); queue.push(r); }
                    Err(msg) => { failed.get_or_insert(alloc::format!("{u}: {msg}")); }
                }
            }
        }
        (missing, failed)
    }

    /// The module URLs the waiting `import()` calls still need, for the host
    /// to fetch.
    pub fn dynamic_import_wants(&mut self) -> Vec<String> {
        let entries: Vec<Rc<str>> = self.loader.pending.iter().map(|(u, _)| u.clone()).collect();
        let mut out: Vec<String> = Vec::new();
        for e in entries {
            let (missing, _) = self.graph_missing(&e);
            for m in missing { if !out.contains(&m) { out.push(m); } }
        }
        out
    }

    /// The host could not deliver this module; whatever waits on it rejects.
    pub fn module_unavailable(&mut self, url: &str) {
        if !self.loader.failed.iter().any(|f| &**f == url) {
            self.loader.failed.push(Rc::from(url));
        }
    }

    /// Evaluate every waiting `import()` whose graph is complete and settle
    /// its promise: with the namespace, or with the error. Returns how many
    /// were settled; the reactions run at the next microtask checkpoint.
    pub fn settle_dynamic_imports(&mut self) -> usize {
        let mut done = 0;
        let mut k = 0;
        while k < self.loader.pending.len() {
            let entry = self.loader.pending[k].0.clone();
            let (missing, failed) = self.graph_missing(&entry);
            if missing.is_empty() || failed.is_some() {
                let (_, p) = self.loader.pending.remove(k);
                done += 1;
                if let Some(f) = failed {
                    let msg = alloc::format!("Failed to fetch dynamically imported module: {f}");
                    if let Abrupt::Throw(e) = self.throw_kind("TypeError", &msg) {
                        super::promise::settle(self, &p, e, true);
                    }
                    continue;
                }
                match self.eval_module(&entry).and_then(|_| self.namespace(&entry)) {
                    Ok(ns) => super::promise::resolve_promise(self, &p, ns),
                    Err(Abrupt::Throw(e)) => super::promise::settle(self, &p, e, true),
                    Err(_) => {}
                }
                continue;
            }
            k += 1;
        }
        done
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn interp() -> Interp {
        let mut i = Interp::new();
        i.set_location("https://ex.test/app/page.html");
        i
    }

    #[test]
    fn import_map_resolves_bare_prefix_and_scoped_specifiers() {
        let mut i = interp();
        i.add_import_map(r#"{
            "imports": { "react": "/js/react.js", "lib/": "https://cdn.test/lib/v2/",
                         "blocked": null, "./local.js": "/js/other.js" },
            "scopes": { "/legacy/": { "react": "/js/react-old.js" } }
        }"#, "https://ex.test/app/page.html").unwrap();
        let base = "https://ex.test/app/main.js";
        assert_eq!(i.resolve_module(base, "react").unwrap(), "https://ex.test/js/react.js");
        assert_eq!(i.resolve_module(base, "lib/a/b.js").unwrap(), "https://cdn.test/lib/v2/a/b.js");
        assert_eq!(i.resolve_module("https://ex.test/legacy/x.js", "react").unwrap(),
                   "https://ex.test/js/react-old.js");
        // A URL-like key matches after resolution against the importer.
        assert_eq!(i.resolve_module(base, "./local.js").unwrap(), "https://ex.test/js/other.js");
        assert_eq!(i.resolve_module(base, "../x.js").unwrap(), "https://ex.test/x.js");
        assert!(i.resolve_module(base, "blocked").is_err());
        assert!(i.resolve_module(base, "unmapped").is_err());
        // A later map does not override an earlier entry.
        i.add_import_map(r#"{"imports":{"react":"/nope.js","vue":"/vue.js"}}"#,
                         "https://ex.test/").unwrap();
        assert_eq!(i.resolve_module(base, "react").unwrap(), "https://ex.test/js/react.js");
        assert_eq!(i.resolve_module(base, "vue").unwrap(), "https://ex.test/vue.js");
    }

    fn console_after_import(vm_off: bool) -> alloc::vec::Vec<String> {
        let mut i = interp();
        i.vm_off = vm_off;
        i.add_import_map(r#"{"imports":{"dep":"/m/dep.js"}}"#, "https://ex.test/").unwrap();
        let prog = super::super::parse(
            "function go(s) { return import(s); }\
             go('./m/a.js').then(ns => console.log('a', ns.x, ns.default, typeof ns.f));\
             go('dep').then(ns => console.log('dep', ns.y));\
             go('./m/missing.js').catch(e => console.log('missing', e instanceof TypeError));\
             go('bare-unmapped').catch(e => console.log('bare', e.name));\
             console.log('sync done');", false).unwrap();
        let _ = i.run_program(&prog);
        super::super::promise::run_jobs(&mut i);
        let mut wants = i.dynamic_import_wants();
        wants.sort();
        assert_eq!(wants, ["https://ex.test/app/m/a.js", "https://ex.test/app/m/missing.js",
                           "https://ex.test/m/dep.js"]);
        let a = super::super::parse(
            "import { y } from 'dep'; export const x = y + 1; export default 'D';\
             export function f() { return import.meta.url; }", true).unwrap();
        i.add_module("https://ex.test/app/m/a.js", Rc::new(a));
        let dep = super::super::parse("export const y = 41;", true).unwrap();
        i.add_module("https://ex.test/m/dep.js", Rc::new(dep));
        i.module_unavailable("https://ex.test/app/m/missing.js");
        assert!(i.dynamic_import_wants().is_empty());
        assert_eq!(i.settle_dynamic_imports(), 3);
        super::super::promise::run_jobs(&mut i);
        let prog = super::super::parse(
            "import('./m/a.js').then(ns => console.log(ns.f()));", false).unwrap();
        let _ = i.run_program(&prog);
        i.settle_dynamic_imports();
        super::super::promise::run_jobs(&mut i);
        i.take_console()
    }

    #[test]
    fn dynamic_import_waits_for_the_host_and_settles_with_the_namespace() {
        for vm_off in [false, true] {
            let out = console_after_import(vm_off);
            assert_eq!(out, ["sync done", "bare TypeError", "a 42 D function", "dep 41",
                             "missing true", "https://ex.test/app/m/a.js"], "vm_off={vm_off}");
        }
    }
}
