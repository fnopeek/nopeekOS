//! Die Auswertung: ein Baumlaeufer.
//!
//! **Warum ein Baumlaeufer und kein Bytecode.** Das Gedaechtnis notiert zu
//! Recht, dass die Form der Verteilerschleife eine Entwurfsentscheidung ist
//! und dass wasms `return_call` dort der moderne Hebel waere. Der Hebel bleibt
//! richtig — er wird nur nicht als erstes gezogen: heute gibt es keine Zahl,
//! gegen die er sich messen liesse, und was zuerst gebraucht wird, ist
//! Richtigkeit. Der test262-Lauf ist danach das Netz, mit dem eine Umstellung
//! auf Bytecode ueberhaupt erst verantwortbar ist.
//!
//! Was hier bewusst NICHT steht: Generatoren, async/await, Proxy, Symbole,
//! BigInt. Jedes davon faellt im Lauf als eigene Zeile auf und ist damit
//! gezaehlt statt vergessen.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::HashMap;

use super::ast::*;
use super::value::*;
pub use super::value::Value;

/// Ein Abbruch: alles, was nicht „der naechste Ausdruck" ist.
pub enum Abrupt {
    Throw(Value),
    Return(Value),
    Break(Option<String>),
    Continue(Option<String>),
}
pub type C<T> = Result<T, Abrupt>;

pub struct Binding {
    pub value: Value,
    pub mutable: bool,
    /// `let`/`const` vor ihrer Deklaration: der Zugriff wirft. Ohne das ist
    /// die zeitliche Totzone unsichtbar und `let` verhaelt sich wie `var`.
    pub initialized: bool,
}

pub struct Env {
    pub vars: HashMap<Rc<str>, Binding>,
    pub parent: Option<Rc<RefCell<Env>>>,
    /// Nur Funktionsumgebungen tragen `this`; ein Block erbt es. Genau daran
    /// haengt, dass ein Pfeil das `this` seiner Umgebung sieht.
    pub this_val: Option<Value>,
    /// Ist das die Umgebung einer Funktion (Ziel fuer `var`-Hochziehen)?
    pub is_func_scope: bool,
}

impl Env {
    pub fn new(parent: Option<Rc<RefCell<Env>>>, func_scope: bool) -> Rc<RefCell<Env>> {
        Rc::new(RefCell::new(Env {
            vars: HashMap::new(), parent, this_val: None, is_func_scope: func_scope,
        }))
    }
}

pub fn env_lookup(env: &Rc<RefCell<Env>>, name: &str) -> Option<Rc<RefCell<Env>>> {
    let mut cur = env.clone();
    loop {
        if cur.borrow().vars.contains_key(name) { return Some(cur); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return None }
    }
}

pub fn env_this(env: &Rc<RefCell<Env>>) -> Value {
    let mut cur = env.clone();
    loop {
        if let Some(t) = &cur.borrow().this_val { return t.clone(); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return Value::Undefined }
    }
}

/// Die eingebauten Objekte einer Ausfuehrungseinheit.
pub struct Realm {
    pub global: Gc,
    pub global_env: Rc<RefCell<Env>>,
    pub object_proto: Gc,
    pub function_proto: Gc,
    pub array_proto: Gc,
    pub string_proto: Gc,
    pub number_proto: Gc,
    pub boolean_proto: Gc,
    pub error_proto: Gc,
    /// Name -> Prototyp der Fehlerarten, fuer `throw_type` & Co.
    pub error_ctors: HashMap<&'static str, Gc>,
    pub node_proto: Gc,
    pub element_proto: Gc,
    pub text_proto: Gc,
    pub document_proto: Gc,
    pub regexp_proto: Gc,
}

pub struct Interp {
    pub realm: Realm,
    /// Aufruftiefe. Ein Baumlaeufer benutzt den RUST-Stapel, also wird ein
    /// zu tiefes JS-Programm zum Stapelueberlauf des Wirts — und das ist im
    /// Kernel ein Absturz, kein Fehler. Die Grenze ist deshalb Pflicht, nicht
    /// Komfort.
    depth: usize,
    max_depth: usize,
    /// Ausgefuehrte Anweisungen. Ohne Deckel haengt ein `while(true)` den
    /// ganzen Lauf auf — und ein Testlaeufer, der an EINEM Programm stehen
    /// bleibt, misst gar nichts mehr.
    pub steps: u64,
    pub max_steps: u64,
    /// Eine Uhr, die nur steigt. Ersatz, bis beak die echte einreicht —
    /// `beak-engine` ist hostfrei und hat keine.
    pub fake_now: f64,
    /// Das Dokument, auf dem `document` arbeitet. `None`, solange keins
    /// eingereicht wurde — dann gibt es `document` gar nicht erst, statt eins
    /// vorzutaeuschen, das nichts enthaelt.
    pub doc: Option<super::dombind::Doc>,
    /// Angemeldete Zeitgeber-Rueckrufe. Noch laeuft niemand sie; sie zu HALTEN
    /// kostet nichts und ist die Stelle, an der beaks Schleife ansetzt.
    pub timers: Vec<Value>,
}

pub const MAX_DEPTH: usize = 400;

/// Wie weit eine Prototypkette laufen darf.
///
/// **Ein Sicherungsnetz, keine Regel der Sprache.** Eine Kette kann einen
/// Zyklus enthalten (`Object.setPrototypeOf` muss ihn zwar ablehnen, aber
/// darauf allein soll sich hier nichts verlassen), und dann laeuft jeder
/// Eigenschaftszugriff fuer immer — in NATIVEM Code, an der Schrittgrenze
/// vorbei. Eine fremde Seite haette damit drei Zeilen gebraucht, um beak
/// aufzuhaengen. Echte Ketten sind ein Dutzend Glieder tief.
pub const MAX_PROTO_CHAIN: usize = 1000;

/// Was ein Testlaeufer setzt.
///
/// Gegen eine Messung gesetzt, nicht gegen ein Gefuehl: ein gewoehnlicher
/// test262-Test kostet **1,9 µs**, also grob hundert Schritte. 2 Mio. waren
/// das Zwanzigtausendfache — und weil diese Maschine rund 11 Mio. Schritte je
/// Sekunde schafft, kostete JEDER Test, der absichtlich mit einer absurden
/// Array-Laenge arbeitet, 180 ms. Davon gibt es in `built-ins/Array` Tausende.
///
/// 200 000 sind immer noch hundertfache Reserve und decken hoechstens 18 ms.
/// Was darueber faellt, verschwindet nicht still: „step budget exhausted"
/// steht als eigene Zeile in der Fehlerkarte.
pub const TEST_STEPS: u64 = 200_000;

impl Interp {
    pub fn new() -> Interp {
        let mut realm = super::builtins::make_realm();
        super::dombind::install(&mut realm);
        super::regexp::install(&mut realm);
        Interp { realm, depth: 0, max_depth: MAX_DEPTH, steps: 0, max_steps: u64::MAX, fake_now: 0.0, doc: None, timers: Vec::new() }
    }

    /// Ein Dokument einreichen und `document` global sichtbar machen.
    ///
    /// Erst hier entsteht `document` — vorher gibt es den Namen nicht, und ein
    /// Skript, das ihn prueft, bekommt die Wahrheit statt eine leere Huelle.
    pub fn set_document(&mut self, doc: super::dombind::Doc) {
        let root = doc.doc;
        self.doc = Some(doc);
        let v = super::dombind::wrap(self, root);
        self.realm.global.borrow_mut().define("document", Prop::builtin(v));
    }

    /// Ein Arbeitsschritt in einer EINGEBAUTEN Schleife.
    ///
    /// Der Deckel in `exec` zaehlt nur Anweisungen — die Schleifen in
    /// `Array.prototype.*` und `iterate` laufen daran vorbei. `new
    /// Array(2**32-1).join()` haengt damit unbegrenzt, und genau das hat den
    /// ersten Ausfuehrungslauf ueber den Zeitdeckel getragen. Also zaehlen
    /// diese Schleifen mit.
    pub fn tick(&mut self) -> C<()> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(self.throw_kind("RangeError", "step budget exhausted"));
        }
        Ok(())
    }

    // ── Fehler ───────────────────────────────────────────────────────────
    pub fn throw_kind(&mut self, kind: &'static str, msg: &str) -> Abrupt {
        let proto = self.realm.error_ctors.get(kind).cloned()
            .unwrap_or_else(|| self.realm.error_proto.clone());
        let e = new_kind(Some(proto), ObjKind::Error);
        e.borrow_mut().define("message", Prop::builtin(Value::str(msg)));
        Abrupt::Throw(Value::Obj(e))
    }
    pub fn type_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("TypeError", msg)) }
    pub fn range_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("RangeError", msg)) }
    pub fn ref_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("ReferenceError", msg)) }

    // ── Umwandlungen ─────────────────────────────────────────────────────
    /// `ToPrimitive`. `hint_string` waehlt die Reihenfolge von `toString` und
    /// `valueOf` — das ist der ganze Unterschied zwischen `"" + obj` und
    /// `1 * obj`.
    pub fn to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        let order: [&str; 2] = if hint_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for m in order {
            let f = self.get(&Value::Obj(o.clone()), m)?;
            if self.is_callable(&f) {
                let r = self.call(&f, v.clone(), &[])?;
                if !matches!(r, Value::Obj(_)) { return Ok(r); }
            }
        }
        self.type_err("cannot convert object to primitive value")
    }

    pub fn to_number(&mut self, v: &Value) -> C<f64> {
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            Value::Num(n) => *n,
            Value::Str(s) => string_to_num(s),
            Value::Obj(_) => { let p = self.to_primitive(v, false)?; self.to_number(&p)? }
        })
    }

    pub fn to_string(&mut self, v: &Value) -> C<Rc<str>> {
        Ok(match v {
            Value::Undefined => Rc::from("undefined"),
            Value::Null => Rc::from("null"),
            Value::Bool(b) => Rc::from(if *b { "true" } else { "false" }),
            Value::Num(n) => Rc::from(num_to_string(*n).as_str()),
            Value::Str(s) => s.clone(),
            Value::Obj(_) => { let p = self.to_primitive(v, true)?; self.to_string(&p)? }
        })
    }

    /// `ToObject`: Primitive bekommen ihre Huelle. Das ist der Weg, ueber den
    /// `"abc".length` funktioniert.
    pub fn to_object(&mut self, v: &Value) -> C<Gc> {
        match v {
            Value::Obj(o) => Ok(o.clone()),
            Value::Str(s) => {
                let g = new_kind(Some(self.realm.string_proto.clone()), ObjKind::StrWrap(s.clone()));
                {
                    let mut b = g.borrow_mut();
                    b.define("length", Prop::frozen(Value::Num(s.chars().count() as f64)));
                    for (i, c) in s.chars().enumerate() {
                        let mut t = String::new(); t.push(c);
                        b.define(&num_to_string(i as f64), Prop {
                            value: Some(Value::string(t)), get: None, set: None,
                            writable: false, enumerable: true, configurable: false });
                    }
                }
                Ok(g)
            }
            Value::Num(n) => Ok(new_kind(Some(self.realm.number_proto.clone()), ObjKind::NumWrap(*n))),
            Value::Bool(b) => Ok(new_kind(Some(self.realm.boolean_proto.clone()), ObjKind::BoolWrap(*b))),
            Value::Undefined | Value::Null =>
                self.type_err("cannot convert undefined or null to object"),
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        matches!(v, Value::Obj(o) if matches!(o.borrow().kind,
            ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. }))
    }

    // ── Eigenschaften ────────────────────────────────────────────────────
    pub fn get(&mut self, base: &Value, key: &str) -> C<Value> {
        // Primitive bekommen KEINE Huelle fuer einen blossen Lesezugriff —
        // ausser bei Zeichenketten, wo Laenge und Index direkt beantwortet
        // werden. Eine Huelle je Zugriff waere sonst der teuerste Weg zu
        // `s.length`.
        if let Value::Str(s) = base {
            if key == "length" { return Ok(Value::Num(s.chars().count() as f64)); }
            if let Some(i) = array_index(key) {
                return Ok(match s.chars().nth(i as usize) {
                    Some(c) => { let mut t = String::new(); t.push(c); Value::string(t) }
                    None => Value::Undefined,
                });
            }
        }
        let start = match base {
            Value::Obj(o) => o.clone(),
            Value::Undefined | Value::Null =>
                return self.type_err(&alloc::format!("cannot read '{key}' of {}",
                    if matches!(base, Value::Null) { "null" } else { "undefined" })),
            _ => self.to_object(base)?,
        };
        // Array-`length` lebt in der Eigenschaftstabelle wie alles andere;
        // nur die Kette darunter wird hier gelaufen.
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(o) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = o.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(g) = &p.get {
                    return self.call(&g.clone(), base.clone(), &[]);
                }
                if p.is_accessor() { return Ok(Value::Undefined); }
                return Ok(p.value.clone().unwrap_or(Value::Undefined));
            }
            let next = o.borrow().proto.clone();
            cur = next;
        }
        Ok(Value::Undefined)
    }

    pub fn set(&mut self, base: &Value, key: &str, val: Value) -> C<()> {
        let Value::Obj(o) = base else {
            // Zuweisung an eine Eigenschaft eines Primitivs verpufft still
            // (im lockeren Modus). Der strenge Modus wuerde werfen — das
            // gehoert zu den Dingen, die der Lauf als offen ausweist.
            return Ok(());
        };
        // Ein Setzer irgendwo in der Kette gewinnt vor dem eigenen Feld.
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = c.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(s) = &p.set { self.call(&s.clone(), base.clone(), &[val])?; return Ok(()); }
                if p.is_accessor() { return Ok(()); }        // nur Getter: still
                if Rc::ptr_eq(&c, o) {
                    if !p.writable { return Ok(()); }
                    let mut np = p.clone();
                    np.value = Some(val);
                    o.borrow_mut().set_prop(Rc::from(key), np);
                    return Ok(());
                }
                if !p.writable { return Ok(()); }
                break;
            }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        if !o.borrow().extensible { return Ok(()); }
        o.borrow_mut().set_prop(Rc::from(key), Prop::data(val));
        self.fix_array_length(o, key);
        Ok(())
    }

    /// Ein Array haelt `length` selbst nach: eine Zuweisung an einen Index
    /// jenseits der Laenge schiebt sie nach. Ohne das ist `push` gebaut, aber
    /// `a[0]=1; a.length` bleibt 0.
    fn fix_array_length(&mut self, o: &Gc, key: &str) {
        if !matches!(o.borrow().kind, ObjKind::Array) { return; }
        if let Some(i) = array_index(key) {
            let cur = o.borrow().get_own("length").and_then(|p| p.value.clone());
            let n = match cur { Some(Value::Num(n)) => n, _ => 0.0 };
            if (i as f64) >= n {
                o.borrow_mut().define("length", Prop {
                    value: Some(Value::Num(i as f64 + 1.0)), get: None, set: None,
                    writable: true, enumerable: false, configurable: false });
            }
        }
    }

    pub fn has_property(&mut self, o: &Gc, key: &str) -> bool {
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return false; }
            if c.borrow().has_own(key) { return true; }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        false
    }

    // ── Aufrufen ─────────────────────────────────────────────────────────
    pub fn call(&mut self, callee: &Value, this_val: Value, args: &[Value]) -> C<Value> {
        let Value::Obj(f) = callee else {
            return self.type_err("value is not a function");
        };
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return self.range_err("Maximum call stack size exceeded");
        }
        let r = self.call_inner(f, this_val, args);
        self.depth -= 1;
        r
    }

    fn call_inner(&mut self, f: &Gc, this_val: Value, args: &[Value]) -> C<Value> {
        enum Which { Native(Rc<NativeData>), Js(Rc<FuncData>), Bound(Gc, Value, Vec<Value>) }
        let which = match &f.borrow().kind {
            ObjKind::Native(n) => Which::Native(n.clone()),
            ObjKind::Function(d) => Which::Js(d.clone()),
            ObjKind::Bound { target, this_val, args } =>
                Which::Bound(target.clone(), this_val.clone(), args.clone()),
            _ => return self.type_err("value is not a function"),
        };
        match which {
            Which::Native(n) => (n.func)(self, this_val, args),
            Which::Bound(t, bt, mut ba) => {
                ba.extend_from_slice(args);
                self.call(&Value::Obj(t), bt, &ba)
            }
            Which::Js(d) => {
                let env = Env::new(Some(d.env.clone()), true);
                // Ein Pfeil bekommt KEIN eigenes `this` — dadurch findet
                // `this_of` das der umgebenden Funktion.
                if !d.node.is_arrow {
                    env.borrow_mut().this_val = Some(d.this_val.clone().unwrap_or(this_val));
                    let ao = self.make_arguments(args);
                    env.borrow_mut().vars.insert(Rc::from("arguments"),
                        Binding { value: ao, mutable: true, initialized: true });
                }
                self.bind_params(&d.node.params, args, &env)?;
                match self.run_body(&d.node.body, &env) {
                    Ok(()) => Ok(Value::Undefined),
                    Err(Abrupt::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
        }
    }

    fn make_arguments(&mut self, args: &[Value]) -> Value {
        let g = new_kind(Some(self.realm.object_proto.clone()), ObjKind::Arguments);
        {
            let mut o = g.borrow_mut();
            for (i, a) in args.iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(a.clone()));
            }
            o.define("length", Prop::builtin(Value::Num(args.len() as f64)));
        }
        Value::Obj(g)
    }

    fn bind_params(&mut self, params: &[Pat], args: &[Value], env: &Rc<RefCell<Env>>) -> C<()> {
        let mut i = 0;
        for p in params {
            if let Pat::Rest(inner) = p {
                let rest: Vec<Value> = args.iter().skip(i).cloned().collect();
                let arr = self.new_array(rest);
                self.bind_pattern(inner, arr, env, true)?;
                break;
            }
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            self.bind_pattern(p, v, env, true)?;
            i += 1;
        }
        Ok(())
    }

    // ── Programm ─────────────────────────────────────────────────────────
    pub fn run_program(&mut self, prog: &Program) -> C<Value> {
        let env = self.realm.global_env.clone();
        self.hoist(&prog.body, &env, &env)?;
        let mut last = Value::Undefined;
        for st in &prog.body {
            if let Some(v) = self.exec(st, &env)? { last = v; }
        }
        Ok(last)
    }

    fn run_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        self.hoist(body, env, env)?;
        for st in body { self.exec(st, env)?; }
        Ok(())
    }

    /// `var` und Funktionsdeklarationen nach vorn ziehen.
    ///
    /// `var` steigt bis zur naechsten FUNKTIONSGRENZE, `let`/`const`/`class`
    /// bleiben im Block und stehen bis zur Deklaration auf „nicht bereit" —
    /// das ist die zeitliche Totzone, und ohne sie ist `let` nur ein `var`
    /// mit anderem Namen.
    fn hoist(&mut self, body: &[Stmt], block: &Rc<RefCell<Env>>, func: &Rc<RefCell<Env>>) -> C<()> {
        for st in body { self.hoist_vars(st, func); }
        for st in body {
            match st {
                Stmt::Func(f) => {
                    if let Some(n) = &f.name {
                        let v = self.make_closure(f.clone(), block, None);
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: v, mutable: true, initialized: true });
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    let mut names = Vec::new();
                    for dec in &d.decls { collect_names(&dec.id, &mut names); }
                    for n in names {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()), Binding {
                            value: Value::Undefined,
                            mutable: d.kind != VarKind::Const,
                            initialized: false,
                        });
                    }
                }
                Stmt::Class(c) => {
                    if let Some(n) = &c.name {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: Value::Undefined, mutable: true, initialized: false });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `var` durch Bloecke und Schleifen hindurch einsammeln — aber NICHT
    /// durch Funktionen: dort faengt ein neuer Bereich an.
    fn hoist_vars(&mut self, st: &Stmt, func: &Rc<RefCell<Env>>) {
        let mut put = |names: Vec<String>| {
            for n in names {
                let key: Rc<str> = Rc::from(n.as_str());
                if !func.borrow().vars.contains_key(&key) {
                    func.borrow_mut().vars.insert(key,
                        Binding { value: Value::Undefined, mutable: true, initialized: true });
                }
            }
        };
        match st {
            Stmt::VarDecl(d) if d.kind == VarKind::Var => {
                let mut names = Vec::new();
                for dec in &d.decls { collect_names(&dec.id, &mut names); }
                put(names);
            }
            Stmt::Block(b) => for s in b { self.hoist_vars(s, func) },
            Stmt::If { cons, alt, .. } => {
                self.hoist_vars(cons, func);
                if let Some(a) = alt { self.hoist_vars(a, func); }
            }
            Stmt::For { init, body, .. } => {
                if let Some(i) = init {
                    if let ForInit::VarDecl(d) = &**i {
                        if d.kind == VarKind::Var {
                            let mut names = Vec::new();
                            for dec in &d.decls { collect_names(&dec.id, &mut names); }
                            put(names);
                        }
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
                if let ForHead::VarDecl(d) = &**left {
                    if d.kind == VarKind::Var {
                        let mut names = Vec::new();
                        for dec in &d.decls { collect_names(&dec.id, &mut names); }
                        put(names);
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. }
            | Stmt::Labeled { body, .. } | Stmt::With { body, .. } => self.hoist_vars(body, func),
            Stmt::Try { block, handler, finalizer } => {
                for s in block { self.hoist_vars(s, func); }
                if let Some(h) = handler { for s in &h.body { self.hoist_vars(s, func); } }
                if let Some(f) = finalizer { for s in f { self.hoist_vars(s, func); } }
            }
            Stmt::Switch { cases, .. } => {
                for c in cases { for s in &c.body { self.hoist_vars(s, func); } }
            }
            _ => {}
        }
    }

    pub fn new_array(&mut self, items: Vec<Value>) -> Value {
        let g = new_kind(Some(self.realm.array_proto.clone()), ObjKind::Array);
        {
            let mut o = g.borrow_mut();
            let n = items.len();
            for (i, v) in items.into_iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(v));
            }
            o.define("length", Prop {
                value: Some(Value::Num(n as f64)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }

    pub fn make_closure(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>) -> Value {
        let g = new_kind(Some(self.realm.function_proto.clone()),
            ObjKind::Function(Rc::new(FuncData {
                node: f.clone(), env: env.clone(), this_val, home_object: None,
            })));
        {
            let mut o = g.borrow_mut();
            let len = f.params.iter().take_while(|p| matches!(p, Pat::Ident(_))).count();
            o.define("length", Prop { value: Some(Value::Num(len as f64)), get: None, set: None,
                writable: false, enumerable: false, configurable: true });
            o.define("name", Prop { value: Some(Value::str(f.name.as_deref().unwrap_or(""))),
                get: None, set: None, writable: false, enumerable: false, configurable: true });
        }
        // Ein Pfeil hat kein `prototype` — er kann nicht als Konstruktor
        // dienen, und ein vorhandenes `prototype` waere ein sichtbarer
        // Unterschied zu jedem echten Motor.
        if !f.is_arrow {
            let proto = new_obj(Some(self.realm.object_proto.clone()));
            proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(g.clone())));
            g.borrow_mut().define("prototype", Prop {
                value: Some(Value::Obj(proto)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }
}

/// Namen, die ein Muster bindet (fuer das Hochziehen).
fn collect_names(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Ident(n) => out.push(n.clone()),
        Pat::Array(items) => for it in items.iter().flatten() { collect_names(it, out) },
        Pat::Object { props, rest } => {
            for pr in props { collect_names(&pr.value, out); }
            if let Some(r) = rest { collect_names(r, out); }
        }
        Pat::Assign { left, .. } => collect_names(left, out),
        Pat::Rest(inner) => collect_names(inner, out),
        Pat::Expr(_) => {}
    }
}

