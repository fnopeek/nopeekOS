//! Die Befehlsmaschine — eine Schleife statt eines Rust-Stapels.
//!
//! **Was hier anders ist als im Baumlaeufer, und nur DAS:** der Zustand einer
//! laufenden Auswertung liegt in Feldern (`stack`, `frames`), nicht in
//! Rust-Aufrufrahmen. Wegspeichern und weiterlaufen lassen ist damit
//! moeglich — das ist die ganze Begruendung des Umbaus, und alles, was
//! Generatoren und `async`/`await` brauchen.
//!
//! **Was hier NICHT anders ist: die Bedeutung.** Jeder Befehl ruft dieselbe
//! Hilfe wie der Baumlaeufer — `binary`, `unary_val`, `vm_load`, `vm_store`,
//! `get`, `set`, `call`, `construct`, `make_closure`. Wo diese Datei rechnet,
//! statt zu rufen, waere eine zweite Semantik, und die laeuft still
//! auseinander.
//!
//! **Stand dieser Stufe.** Aufrufe gehen weiter durch `Interp::call`, also
//! ueber den Rust-Stapel; ein Rahmen je JS-Funktion kommt in der naechsten
//! Stufe. Diese hier baut den Rahmenstapel und beweist mit test262, dass die
//! Bedeutung sich nicht bewegt hat — die Zahl darf sich NICHT aendern.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::code::{Chunk, Op};
use super::interp::{Abrupt, Env, Interp, C};
use super::value::Value;

/// Ein Aufrufrahmen. Heute gibt es genau einen (das Programm); die Form steht
/// schon, weil sie der Punkt der Uebung ist.
struct Frame {
    chunk: Rc<Chunk>,
    ip: usize,
    /// Die Umgebung, in der dieser Rahmen laeuft. `PushEnv`/`PopEnv` schieben
    /// hier, nicht auf dem Rust-Stapel.
    envs: Vec<Rc<RefCell<Env>>>,
    /// Der Stapelstand beim Betreten — beim Verlassen wird darauf zurueck-
    /// geschnitten, damit ein `Ret` mitten im Ausdruck nichts liegenlaesst.
    base: usize,
    /// Offene `try`-Behandler, innerster zuletzt.
    handlers: Vec<Handler>,
    /// Offene `for…of`-Iteratoren. Sie stehen HIER und nicht auf dem
    /// Wertestapel: ein `break` oder ein Wurf mitten in der Schleife muesste
    /// sie sonst einzeln wegraeumen, und das ist genau die Buchhaltung, an
    /// der solche Maschinen scheitern.
    iters: Vec<Value>,
}

/// Ein offener `try`. Die drei Tiefen sind der Punkt: ein Wurf kann mitten in
/// einem Ausdruck passieren, und dann liegen halbe Werte auf dem Stapel,
/// offene Blockumgebungen im Rahmen und angefangene Iterationen daneben.
struct Handler {
    catch_ip: Option<u32>,
    finally_ip: Option<u32>,
    stack: usize,
    envs: usize,
    iters: usize,
}

pub struct Vm {
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// Der Abschlusswert des Programms (sein letzter Ausdruckswert).
    completion: Value,
}

impl Vm {
    pub fn new() -> Vm {
        Vm { stack: Vec::new(), frames: Vec::new(), completion: Value::Undefined }
    }

    /// Ein uebersetztes Programm fahren. `env` ist die Umgebung, in die der
    /// Rufer schon hochgezogen hat — das Hochziehen bleibt beim Baumlaeufer,
    /// weil es auf der UMGEBUNG arbeitet und fuer beide Maschinen dasselbe ist.
    pub fn run(&mut self, i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {
        self.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                                 handlers: Vec::new(), iters: Vec::new() });
        loop {
            let (chunk, ip) = {
                let f = self.frames.last().unwrap();
                (f.chunk.clone(), f.ip)
            };
            if ip >= chunk.ops.len() {
                return Ok(core::mem::replace(&mut self.completion, Value::Undefined));
            }
            self.frames.last_mut().unwrap().ip += 1;
            // Der Deckel gegen `while(true)` — aber mit derselben KOERNUNG wie
            // im Baumlaeufer, sonst ist er ein anderer Deckel.
            //
            // Der zaehlt einen Schritt je ANWEISUNG. Je Befehl zu zaehlen
            // waere feiner und damit strenger: derselbe Test, der dort
            // durchlief, brach hier ab (`encodeURI` mit seiner langen
            // Zeichentabelle). Gezaehlt wird deshalb, was eine Schleife
            // wirklich vorantreibt — ein Rueckwaertssprung, ein Aufruf, eine
            // Anweisungsgrenze. Das ist dieselbe Groessenordnung und bleibt
            // eine echte Abbruchgarantie.
            let counts = match &chunk.ops[ip] {
                Op::Jump(t) => (*t as usize) <= ip,
                Op::Call(_) | Op::New(_) | Op::CallSpread | Op::NewSpread
                | Op::SetCompletion | Op::DeclVar { .. } | Op::Ret => true,
                _ => false,
            };
            if counts {
                i.steps += 1;
                if i.steps > i.max_steps {
                    return Err(i.throw_kind("RangeError", "step budget exhausted"));
                }
            }
            match self.step(i, &chunk, ip) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                // Ein Wurf sucht sich seinen Behandler. Findet er keinen, geht
                // er an den Rufer — dann faehrt ihn der Rust-Stapel hoch, und
                // das ist richtig, solange Aufrufe noch so laufen.
                Err(Abrupt::Throw(v)) => {
                    if !self.unwind(i, v.clone()) {
                        return Err(Abrupt::Throw(v));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// `Ok(Some(v))` heisst: der aeusserste Rahmen ist zurueck.
    fn step(&mut self, i: &mut Interp, chunk: &Chunk, ip: usize) -> C<Option<Value>> {
        let env = self.frames.last().unwrap().envs.last().unwrap().clone();
        match &chunk.ops[ip] {
            Op::Const(k) => self.push(chunk.constants[*k as usize].clone()),
            Op::LoadVar(n) => {
                let v = i.vm_load(&chunk.names[*n as usize], &env)?;
                self.push(v);
            }
            Op::StoreVar(n) => {
                let v = self.top();
                i.vm_store(&chunk.names[*n as usize], v, &env)?;
            }
            Op::DeclVar { name, mutable, lexical } => {
                let v = self.pop();
                let n = &chunk.names[*name as usize];
                if *lexical {
                    // GENAU HIER — `init_binding` liefe die Kette hoch und
                    // schriebe eine gleichnamige Bindung weiter aussen.
                    i.bind_here(n, v, &env);
                } else {
                    i.init_binding(n, v, &env);
                }
                if !*mutable {
                    i.make_const(n, &env);
                }
            }
            Op::NameFunc(n) => {
                let v = self.top();
                i.name_function(&v, &chunk.names[*n as usize]);
            }
            Op::ToKey => {
                let v = self.pop();
                let k = i.to_prop_key(&v)?;
                self.push(Value::Str(k));
            }
            Op::This => {
                let v = super::interp::env_this(&env);
                self.push(v);
            }
            Op::Pop => {
                self.pop();
            }
            Op::Dup => {
                let v = self.top();
                self.push(v);
            }
            // `a b` → `b a`: der Aufruf braucht `callee` unter `this`, und der
            // Uebersetzer legt sie in der anderen Reihenfolge ab.
            Op::Swap => {
                let n = self.stack.len();
                self.stack.swap(n - 1, n - 2);
            }
            Op::Un(op) => {
                let v = self.pop();
                let r = i.unary_val(*op, v)?;
                self.push(r);
            }
            Op::TypeofVar(n) => {
                let v = i.typeof_ident(&chunk.names[*n as usize], &env)?;
                self.push(v);
            }
            Op::Bin(op) => {
                let r = self.pop();
                let l = self.pop();
                let v = i.binary(*op, l, r)?;
                self.push(v);
            }
            Op::Jump(t) => self.jump(*t),
            Op::JumpFalse(t) => {
                let v = self.pop();
                if !v.truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpFalseKeep(t) => {
                if !self.top().truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpTrueKeep(t) => {
                if self.top().truthy() {
                    self.jump(*t);
                }
            }
            Op::JumpNullishKeep(t) => {
                if !matches!(self.top(), Value::Undefined | Value::Null) {
                    self.jump(*t);
                }
            }
            Op::GetProp(n) => {
                let obj = self.pop();
                let v = i.get(&obj, &chunk.names[*n as usize])?;
                self.push(v);
            }
            Op::GetIndex => {
                let key = self.pop();
                let obj = self.pop();
                // `to_prop_key`, NICHT `to_string`: ein Symbol ist ein
                // Schluessel und keine Zeichenkette, und es zu einer zu machen
                // hat `Symbol.iterator` & Co. ins Leere zeigen lassen — 35
                // Tests, gefunden vom Diff gegen den Baumlaeufer.
                let k = i.to_prop_key(&key)?;
                let v = i.get(&obj, &k)?;
                self.push(v);
            }
            Op::SetProp(n) => {
                let val = self.pop();
                let obj = self.pop();
                i.set(&obj, &chunk.names[*n as usize], val.clone())?;
                self.push(val);
            }
            Op::SetIndex => {
                let val = self.pop();
                let key = self.pop();
                let obj = self.pop();
                let k = i.to_prop_key(&key)?;
                i.set(&obj, &k, val.clone())?;
                self.push(val);
            }
            Op::Call(argc) => {
                let args = self.take(*argc as usize);
                let this = self.pop();
                let callee = self.pop();
                let v = i.call(&callee, this, &args)?;
                self.push(v);
            }
            Op::New(argc) => {
                let args = self.take(*argc as usize);
                let callee = self.pop();
                let v = i.construct(&callee, &args)?;
                self.push(v);
            }
            Op::MakeArray(n) => {
                let items = self.take(*n as usize);
                let v = i.new_array(items);
                self.push(v);
            }
            Op::Closure(f) => {
                let v = i.func_value(chunk.funcs[*f as usize].clone(), &env);
                self.push(v);
            }
            Op::NewObject => {
                let g = super::value::new_obj(Some(i.realm.object_proto.clone()));
                self.push(Value::Obj(g));
            }
            Op::DefineProp(n) => {
                let val = self.pop();
                let key: Rc<str> = chunk.names[*n as usize].clone();
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(key, super::value::Prop::data(val));
                }
            }
            Op::DefinePropComputed => {
                let val = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(k, super::value::Prop::data(val));
                }
            }
            Op::DefineAccessor { name, get } => {
                let f = self.pop();
                let key: Rc<str> = chunk.names[*name as usize].clone();
                if let Value::Obj(g) = self.top() {
                    i.define_accessor(&g, key, f, *get);
                }
            }
            Op::DefineAccessorComputed { get } => {
                let f = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                if let Value::Obj(g) = self.top() {
                    i.define_accessor(&g, k, f, *get);
                }
            }
            Op::SpreadInto => {
                let src = self.pop();
                if let Value::Obj(g) = self.top() {
                    i.spread_into(&g, &src)?;
                }
            }
            Op::Rot3 => {
                let n = self.stack.len();
                let c = self.stack.remove(n - 1);
                self.stack.insert(n - 3, c);
            }
            Op::Dup2 => {
                let n = self.stack.len();
                let (a, b) = (self.stack[n - 2].clone(), self.stack[n - 1].clone());
                self.push(a);
                self.push(b);
            }
            Op::Regex { body, flags } => {
                let v = super::regexp::make(i, &chunk.names[*body as usize],
                                            &chunk.names[*flags as usize])?;
                self.push(v);
            }
            Op::Concat(n) => {
                let parts = self.take(*n as usize);
                let mut out = alloc::string::String::new();
                for p in &parts {
                    out.push_str(&i.to_string(p)?);
                }
                self.push(Value::string(out));
            }
            Op::DeleteProp(n) => {
                let obj = self.pop();
                let v = i.delete_key(&obj, &chunk.names[*n as usize])?;
                self.push(Value::Bool(v));
            }
            Op::DeleteIndex => {
                let key = self.pop();
                let obj = self.pop();
                let k = i.to_prop_key(&key)?;
                let v = i.delete_key(&obj, &k)?;
                self.push(Value::Bool(v));
            }
            Op::MakeArraySpread { n, spread } => {
                let items = self.take(*n as usize);
                let mask = &chunk.blocks_spread[*spread as usize];
                let mut out = Vec::new();
                for (k, v) in items.into_iter().enumerate() {
                    if mask.get(k).copied().unwrap_or(false) {
                        out.extend(i.iterate(&v)?);
                    } else {
                        out.push(v);
                    }
                }
                let a = i.new_array(out);
                self.push(a);
            }
            Op::CallSpread => {
                let args = self.pop();
                let this = self.pop();
                let callee = self.pop();
                let a = i.iterate(&args)?;
                let v = i.call(&callee, this, &a)?;
                self.push(v);
            }
            Op::NewSpread => {
                let args = self.pop();
                let callee = self.pop();
                let a = i.iterate(&args)?;
                let v = i.construct(&callee, &a)?;
                self.push(v);
            }
            Op::Throw | Op::Rethrow => {
                let v = self.pop();
                return Err(Abrupt::Throw(v));
            }
            Op::TryStart { catch, finally } => {
                let (s, e, it) = {
                    let f = self.frames.last().unwrap();
                    (self.stack.len(), f.envs.len(), f.iters.len())
                };
                self.frames.last_mut().unwrap().handlers.push(Handler {
                    catch_ip: (*catch != u32::MAX).then_some(*catch),
                    finally_ip: (*finally != u32::MAX).then_some(*finally),
                    stack: s, envs: e, iters: it,
                });
            }
            Op::TryEnd => {
                self.frames.last_mut().unwrap().handlers.pop();
            }
            Op::BindCatch(n) => {
                let v = self.pop();
                i.bind_here(&chunk.names[*n as usize], v, &env);
            }
            Op::IterAll => {
                let v = self.pop();
                let it = i.get_iterator(&v)?;
                self.frames.last_mut().unwrap().iters.push(it);
            }
            Op::IterNext(done) => {
                let it = self.frames.last().unwrap().iters.last().unwrap().clone();
                match i.iter_next(&it)? {
                    Some(v) => self.push(v),
                    None => {
                        self.jump(*done);
                    }
                }
            }
            Op::IterDrop => {
                self.frames.last_mut().unwrap().iters.pop();
            }
            Op::IterClose => {
                if let Some(it) = self.frames.last_mut().unwrap().iters.pop() {
                    i.iter_close(&it);
                }
            }
            Op::PushEnv(b) => {
                let child = Env::new(Some(env.clone()), false);
                // Erst binden, dann laufen: `let` steht von Blockanfang an in
                // der Totzone, eine Funktionsdeklaration ist von Blockanfang
                // an fertig. Genau die zwei Schleifen aus `Interp::hoist`.
                for d in &chunk.blocks[*b as usize] {
                    match d {
                        super::code::BlockDecl::Tdz { name, mutable } =>
                            i.declare_tdz(&chunk.names[*name as usize], *mutable, &child),
                        super::code::BlockDecl::Func { name, func } => {
                            let v = i.make_closure(chunk.funcs[*func as usize].clone(), &child, None);
                            i.bind_here(&chunk.names[*name as usize], v, &child);
                        }
                    }
                }
                self.frames.last_mut().unwrap().envs.push(child);
            }
            Op::PopEnv => {
                self.frames.last_mut().unwrap().envs.pop();
            }
            Op::SetCompletion => {
                self.completion = self.pop();
            }
            Op::Ret => {
                let v = if self.stack.len() > self.frames.last().unwrap().base {
                    self.pop()
                } else {
                    Value::Undefined
                };
                let f = self.frames.pop().unwrap();
                self.stack.truncate(f.base);
                if self.frames.is_empty() {
                    // Der Wert eines PROGRAMMS ist sein letzter Ausdruckswert,
                    // nicht das, was am Ende auf dem Stapel liegt.
                    let c = core::mem::replace(&mut self.completion, Value::Undefined);
                    return Ok(Some(if matches!(c, Value::Undefined) { v } else { c }));
                }
                self.push(v);
            }
        }
        Ok(None)
    }

    /// Den innersten Behandler suchen, der den Wurf nimmt. `false` heisst:
    /// keiner da, der Wurf verlaesst diese Maschine.
    ///
    /// Zurueckgeschnitten wird auf die Tiefen, die der Behandler sich gemerkt
    /// hat — Wertestapel, Umgebungen UND offene Iterationen. Wer eins davon
    /// vergisst, bekommt einen `catch`-Block, der auf fremdem Zustand steht.
    fn unwind(&mut self, i: &mut Interp, v: Value) -> bool {
        let h = {
            let Some(f) = self.frames.last_mut() else { return false };
            let Some(h) = f.handlers.pop() else { return false };
            f.envs.truncate(h.envs);
            h
        };
        // Ein Wurf aus einer `for…of`-Schleife heraus muss ihren Iterator
        // SCHLIESSEN, nicht nur vergessen — `return()` ist der Weg, auf dem
        // ein Generator seinen `finally`-Block noch faehrt.
        loop {
            let it = {
                let f = self.frames.last_mut().unwrap();
                if f.iters.len() <= h.iters { break }
                f.iters.pop().unwrap()
            };
            i.iter_close(&it);
        }
        let Some(t) = h.catch_ip.or(h.finally_ip) else { return false };
        self.frames.last_mut().unwrap().ip = t as usize;
        self.stack.truncate(h.stack);
        self.stack.push(v);
        true
    }

    fn jump(&mut self, t: u32) {
        self.frames.last_mut().unwrap().ip = t as usize;
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    /// Der Uebersetzer erzeugt nur ausgeglichenen Code; ein leerer Stapel hier
    /// waere ein Fehler IM UEBERSETZER, kein Programmfehler. `Undefined` statt
    /// `panic!`, weil ein Absturz der Maschine in einem Kernel keine
    /// Fehlermeldung ist, sondern ein Halt.
    fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::Undefined)
    }

    fn top(&mut self) -> Value {
        self.stack.last().cloned().unwrap_or(Value::Undefined)
    }

    fn take(&mut self, n: usize) -> Vec<Value> {
        let at = self.stack.len().saturating_sub(n);
        self.stack.split_off(at)
    }
}
