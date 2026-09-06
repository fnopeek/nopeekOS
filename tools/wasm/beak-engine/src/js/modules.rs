//! ES-Module: Graph, Verknuepfung, Auswertung.
//!
//! **Was hier NICHT ist: das Holen.** Die Engine hat kein Netz und kein
//! Dateisystem; sie sagt nur, WELCHE Adressen sie noch braucht
//! (`Program::requests`), und der Wirt legt den Quelltext dazu (`add_module`).
//! Das ist dieselbe Trennung wie bei den Stilblaettern und Bildern.
//!
//! **Drei Schritte, und die Reihenfolge ist der ganze Punkt.**
//!
//! 1. HOLEN, bis der Graph geschlossen ist — Sache des Wirts.
//! 2. VERKNUEPFEN (`link`): jedes Modul bekommt seine Umgebung, seine
//!    Funktionsdeklarationen stehen darin schon, und jeder `import` wird ein
//!    VERWEIS auf die Bindung im Herkunftsmodul.
//! 3. AUSWERTEN (`evaluate`): Tiefe zuerst, jedes Modul genau einmal.
//!
//! Schritt 2 muss vor JEDER Auswertung fertig sein, und zwar fuer den ganzen
//! Graphen. Der Grund sind Zyklen: `main.js` und `oldpage.js` der
//! Fritzbox-Oberflaeche importieren einander. Wer im Kreis zuerst laeuft,
//! greift auf Namen des anderen zu, bevor dessen Rumpf lief — und findet sie,
//! weil die Funktionsdeklarationen beim Verknuepfen schon stehen und der
//! Verweis lebendig ist. Eine Kopie an dieser Stelle saehe `undefined`, still.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::HashMap;

use super::ast::*;
use super::interp::{Abrupt, Binding, C, Env, Interp};
use super::value::*;

/// Wo ein Modul im Ablauf steht.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    /// Geholt und geparst, sonst nichts.
    New,
    /// Umgebung steht, Verweise sind gelegt.
    Linked,
    /// Der Rumpf laeuft GERADE. Trifft der Graph hier wieder ein, ist das ein
    /// Zyklus — und der ist erlaubt, nicht etwa ein Fehler.
    Running,
    Done,
    Failed,
}

pub struct Module {
    pub url: Rc<str>,
    pub prog: Rc<Program>,
    pub env: Rc<RefCell<Env>>,
    /// Der ausgefuehrte Name -> (Modul, lokaler Name dort). Ein
    /// Weiterreichen (`export { x } from …`) zeigt direkt auf die Quelle.
    pub exports: HashMap<Rc<str>, (Rc<str>, Rc<str>)>,
    /// `export * from …` — beim Nachschlagen durchsucht, nicht kopiert: das
    /// Ziel kann selbst noch nicht verknuepft sein.
    pub star: Vec<Rc<str>>,
    /// Rohe Angabe -> aufgeloeste Adresse. **Der Wirt fuellt sie.** Die
    /// Engine kann `"./config.js"` nicht aufloesen: dazu gehoert die Adresse
    /// des Dokuments, und die gehoert dem Wirt (siehe `set_location`).
    pub resolved: HashMap<Rc<str>, Rc<str>>,
    pub state: ModState,
    /// Das Namensraumobjekt, sobald jemand `import * as x` verlangt hat.
    pub ns: Option<Value>,
}

/// Der lokale Name, unter dem `import.meta` in der Modulumgebung liegt.
///
/// **Als Bindung, nicht als Feld am `Interp`.** `import.meta` ist LEXIKALISCH
/// — eine Funktion, die es liest, gehoert zu dem Modul, in dem sie STEHT,
/// nicht zu dem, das sie gerade ruft. Ein „aktuelles Modul" am Interpreter
/// waere beim ersten Rueckruf falsch, und zwar still. Die Umgebungskette hat
/// die Antwort schon.
pub const META_LOCAL: &str = "*meta*";

/// Der lokale Name, unter dem `export default` seinen Wert ablegt. Kein
/// gueltiger Bezeichner, also kann ihn kein Skript treffen.
pub const DEFAULT_LOCAL: &str = "*default*";

/// Die Adressen, die dieses Programm noch braucht — in Quelltextreihenfolge,
/// ohne Doppelte.
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
    /// Ein geholtes und geparstes Modul eintragen. Die Adresse ist der
    /// Schluessel und muss schon aufgeloest sein — die Engine kennt keine
    /// relativen Pfade.
    pub fn add_module(&mut self, url: &str, prog: Rc<Program>) {
        if self.modules.contains_key(url) { return; }
        let env = Env::new(Some(self.realm.global_env.clone()), true);
        {
            let mut e = env.borrow_mut();
            // Ein Modul ist IMMER streng, und sein `this` ist `undefined` —
            // nicht das Fenster. Skripte, die beides verwechseln, verhalten
            // sich sonst je nach Ladeweg anders.
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

    /// Wohin eine Angabe aus diesem Modul zeigt. Der Wirt sagt es, EINMAL je
    /// Paar — ohne die Zuordnung sucht der Lader spaeter unter `"./x.js"`
    /// und findet nichts.
    pub fn map_module_dep(&mut self, url: &str, spec: &str, target: &str) {
        if let Some(m) = self.modules.get(url) {
            m.borrow_mut().resolved.insert(Rc::from(spec), Rc::from(target));
        }
    }

    /// Eine Angabe aus diesem Modul, aufgeloest. Ohne Zuordnung bleibt sie
    /// wie sie ist — dann waren die Angaben schon absolut.
    fn dep_url(&self, url: &str, spec: &str) -> Rc<str> {
        match self.modules.get(url).and_then(|m| m.borrow().resolved.get(spec).cloned()) {
            Some(r) => r,
            None => Rc::from(spec),
        }
    }

    /// Die AUFGELOESTEN Adressen, die dieses Modul braucht.
    pub fn module_deps(&self, url: &str) -> Vec<Rc<str>> {
        self.module_requests(url).iter().map(|s| self.dep_url(url, s)).collect()
    }

    /// Welche Adressen dieses eingetragene Modul noch braucht, AUFGELOEST
    /// gegen seine eigene — der Wirt fragt das nach jeder Holrunde.
    pub fn module_requests(&self, url: &str) -> Vec<String> {
        match self.modules.get(url) {
            Some(m) => requests(&m.borrow().prog),
            None => Vec::new(),
        }
    }

    /// Den Graphen ab `url` verknuepfen. Ohne Auswertung.
    ///
    /// **Zwei Durchlaeufe, und das ist keine Bequemlichkeit.** Erst bekommt
    /// JEDES Modul seine Deklarationen und seine Ausfuhrtabelle, dann erst
    /// wird ein einziger Verweis gelegt. Bei einem Zyklus ist das der
    /// Unterschied zwischen „laeuft" und „`hallo` wird von a.js nicht
    /// ausgefuehrt": wer zuerst verknuepft, fragt sonst eine Tabelle ab, die
    /// es noch nicht gibt. Die Spezifikation macht es genauso
    /// (`InnerModuleLinking` sammelt vor `InitializeEnvironment`).
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

    /// Alle erreichbaren Module, Tiefe zuerst, ohne Doppelte.
    fn collect(&mut self, url: &str, out: &mut Vec<Rc<str>>) -> C<()> {
        if out.iter().any(|u| &**u == url) { return Ok(()); }
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        out.push(m.borrow().url.clone());
        for dep in self.module_deps(url) { self.collect(&dep, out)?; }
        Ok(())
    }

    /// Die Ausfuhrtabelle des Moduls aus seinem Quelltext.
    fn build_exports(&mut self, m: &Rc<RefCell<Module>>) -> C<()> {
        let (prog, me) = { let b = m.borrow(); (b.prog.clone(), b.url.clone()) };
        let mut exports: HashMap<Rc<str>, (Rc<str>, Rc<str>)> = HashMap::new();
        let mut star: Vec<Rc<str>> = Vec::new();
        for st in &prog.body {
            match st {
                Stmt::ExportNamed { decl, specifiers, source } => {
                    // `export { a as b } from "m"` zeigt DIREKT auf `m` —
                    // nicht ueber eine lokale Bindung, die es nicht gibt.
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
                    // `export * as ns from "m"` ist ein EINZELNER Name, kein
                    // Durchreichen — er braucht das Namensraumobjekt.
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

    /// Jeden `import` als Verweis in die Umgebung des Moduls legen.
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
                        // Ein Namensraum ist ein WERT, kein Verweis: das
                        // Objekt selbst wechselt nie, nur seine Felder.
                        let ns = self.namespace(&from)?;
                        env.borrow_mut().vars.insert(Rc::from(local.as_str()),
                            Binding { value: ns, mutable: false, initialized: true });
                    }
                }
            }
        }
        Ok(())
    }

    /// Einen Namen im Zielmodul aufloesen und den Verweis eintragen.
    fn alias(&mut self, env: &Rc<RefCell<Env>>, local: &str, from: &str, name: &str) -> C<()> {
        if name == "*namespace*" {
            let ns = self.namespace(from)?;
            env.borrow_mut().vars.insert(Rc::from(local),
                Binding { value: ns, mutable: false, initialized: true });
            return Ok(());
        }
        let Some((tenv, tname)) = self.resolve_export(from, name, &mut Vec::new()) else {
            // KEIN stiller Ausfall: ein Name, den das Zielmodul nicht
            // ausfuehrt, ist ein Fehler, und er nennt beide Seiten.
            return self.ref_err(&alloc::format!(
                "'{name}' is not exported by {from}"));
        };
        let mut e = env.borrow_mut();
        let map = e.imports.get_or_insert_with(|| alloc::boxed::Box::new(HashMap::new()));
        map.insert(Rc::from(local), (tenv, tname));
        Ok(())
    }

    /// Wo liegt der ausgefuehrte Name wirklich? Folgt Weiterreichungen und
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
        // `export *` reicht alles ausser `default` weiter.
        if name != "default" {
            for s in star {
                if let Some(r) = self.resolve_export(&s.clone(), name, seen) { return Some(r); }
            }
        }
        None
    }

    /// Das Namensraumobjekt eines Moduls: eine Momentaufnahme seiner Ausfuhr.
    ///
    /// **Eine Aufnahme, kein lebendes Objekt.** Die Spezifikation will exotic
    /// getter, die bei jedem Zugriff nachsehen; das ist hier bewusst nicht
    /// gebaut, weil kein gemessener Aufruf im Zielkorpus darauf angewiesen
    /// ist. Was es kostet, steht damit fest: ein Namensraum, der VOR der
    /// Auswertung des Zielmoduls gelesen wird, zeigt nur die Funktionen.
    fn namespace(&mut self, url: &str) -> C<Value> {
        if let Some(m) = self.modules.get(url) {
            if let Some(ns) = m.borrow().ns.clone() { return Ok(ns); }
        }
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        let g = new_obj(None);
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

    /// Den Graphen ab `url` auswerten — Tiefe zuerst, jedes Modul einmal.
    pub fn eval_module(&mut self, url: &str) -> C<()> {
        let Some(m) = self.modules.get(url).cloned() else {
            return self.ref_err(&alloc::format!("module not loaded: {url}"));
        };
        // Den Zustand HERAUSKOPIEREN. Die Ausleihe eines `match`-Gegenstands
        // lebt bis zum Ende des ganzen `match`, und `link_module` schreibt in
        // dasselbe Modul.
        let state = m.borrow().state;
        match state {
            // `Running` heisst: wir stehen IM Zyklus. Zurueckkehren, nicht
            // ein zweites Mal fahren.
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
        // Der Rumpf. Die Deklarationen stehen seit dem Verknuepfen, also NUR
        // ausfuehren — ein zweites Hochziehen wuerde eine `let`-Bindung
        // zurueck auf „nicht bereit" stellen, die ein Zyklus schon gefuellt
        // hat.
        let r = (|| -> C<()> {
            for st in &prog.body {
                match self.exec(st, &env) { Ok(_) => {}, Err(e) => return Err(e) }
            }
            Ok(())
        })();
        m.borrow_mut().state = if r.is_ok() { ModState::Done } else { ModState::Failed };
        // Den ERSTEN Werfer festhalten, nicht den letzten: der aeussere
        // Aufrufer reicht denselben Fehler nach oben durch, und dessen Name
        // wuerde den echten ueberschreiben.
        if r.is_err() && self.module_fail.is_none() {
            self.module_fail = Some(m.borrow().url.clone());
        }
        // Ein Namensraum, der VOR dem Rumpf gebaut wurde, hat die spaeter
        // zugewiesenen Werte nicht. Hier nachziehen — das ist der billige
        // Ersatz fuer die exotic getter der Spezifikation.
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

/// Die Namen, die eine Deklaration einfuehrt — hinter einem `export`.
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

/// Steckt hinter diesem `export` eine Deklaration? Dann ist SIE die
/// Anweisung, die laufen und hochgezogen werden muss.
pub fn unexport(st: &Stmt) -> Option<&Stmt> {
    match st {
        Stmt::ExportNamed { decl: Some(d), .. } => Some(d),
        _ => None,
    }
}

/// Der Ausgang einer Auswertung als Fehlertext — der Wirt hat keinen Zugriff
/// auf `Abrupt`.
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
