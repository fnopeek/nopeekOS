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
//! **Stufe 4 und ihre Entwurfsfrage.** Sie lautete: ein Generator, der von
//! einem EINGEBAUTEN gerufen wird (`[...gen]`, `Array.from(gen)`), sitzt unter
//! einem Rust-Rahmen — dort kann er nicht anhalten. Die Antwort ist, die
//! Voraussetzung der Frage zu streichen:
//!
//! **Ein Generator ist keine Rahmen in fremder Maschine, er ist eine EIGENE.**
//! Jedes Generatorobjekt haelt seine `Vm` mit genau einem Wurzelrahmen. Ein
//! `yield` muss deshalb nie ueber einen Rust-Rahmen zurueck — zwischen dem
//! `yield` und dem `Vm::resume`, das darauf wartet, liegt keiner:
//!
//!     Array.from                (Rust)
//!       Interp::call(gen.next)  (Rust)
//!         next                  (Rust)
//!           Vm::resume          ← die EIGENE Maschine des Generators
//!             Frame(rumpf) ip=17 … Op::Yield → zurueck mit „angehalten"
//!
//! Ein `next()` kostet damit EINEN Rust-Rahmen, nicht einen je `yield`. Und
//! weil das so ist, ist es voellig gleichgueltig, wer ruft: der Baumlaeufer,
//! ein Eingebautes, `Op::Call` einer anderen Maschine oder ein zweiter
//! Generator. Niemand muss etwas dazulernen, es gibt keinen eifrigen
//! Rueckfall und keine Falle mit unendlichen Generatoren.
//!
//! Ein `yield` steht immer im Rumpf des Generators SELBST (in einer inneren
//! Funktion waere es deren eigenes), und ein Aufruf von dort muss
//! zurueckkehren, bevor es weitergeht — beim `Op::Yield` ist der Wurzelrahmen
//! also der einzige. Deshalb reicht ein Wurzelrahmen je Maschine.

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
    /// Ist das der Rahmen eines PROGRAMMS? Dessen Wert ist sein letzter
    /// Ausdruckswert, der einer Funktion ihr `return`.
    is_program: bool,
    /// Der UNTERSTE Rahmen dieser Maschine. Ein Wurf sucht darueber hinaus
    /// keinen Behandler mehr, ein `Ret` beendet den Lauf, und die Aufruftiefe
    /// wird nicht mitgezaehlt — dieser Rahmen gehoert nicht dem Rufer.
    ///
    /// Getrennt von `is_program`, weil ein Generatorrumpf zwar Wurzel ist,
    /// aber KEINEN Abschlusswert hat: ein `{ x = 1; }` in ihm setzt
    /// `SetCompletion`, und das duerfte sein `return` nicht ueberschreiben.
    root: bool,
    /// Offene Laufzustaende von `for…of` und `for…in`. Sie stehen HIER und
    /// nicht auf dem Wertestapel: ein `break` oder ein Wurf mitten in der
    /// Schleife muesste sie sonst einzeln wegraeumen, und das ist genau die
    /// Buchhaltung, an der solche Maschinen scheitern.
    ///
    /// Beide in EINER Liste, weil die ganze Aufraeumerei — Behandlertiefe,
    /// `Ret`, `unwind` — sie dann nur einmal kennen muss.
    iters: Vec<Iter>,
}

/// Was eine laufende Schleife festhaelt.
enum Iter {
    /// Ein echter Iterator (`for…of`). Sein `return()` gehoert bei jedem
    /// vorzeitigen Verlassen gerufen.
    Obj(Value),
    /// Die Schluesselliste eines `for…in`, RUECKWAERTS — dann ist `pop` der
    /// naechste Schritt und es braucht keinen Index daneben. Es gibt hier
    /// nichts zu schliessen: die Liste steht schon fest.
    Keys(Vec<Value>),
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

/// Wie ein Lauf geendet hat.
pub enum Step {
    /// Der Wurzelrahmen ist zurueck.
    Done(Value),
    /// `yield` — die Maschine steht und laesst sich wieder aufnehmen.
    Yield(Value),
    /// `await` — dasselbe Anhalten, nur wartet hier ein Versprechen darauf,
    /// sie wieder anzuwerfen, statt eines `next()`.
    Await(Value),
}

/// Wie ein einzelner Befehl ausgegangen ist.
enum Flow {
    /// Weiter zum naechsten.
    Go,
    Done(Value),
    Yield(Value),
    Await(Value),
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
                                 is_program: true, root: true,
                                 handlers: Vec::new(), iters: Vec::new() });
        match self.drive(i)? {
            Step::Done(v) => Ok(v),
            // Ein Programmrumpf wird mit `in_gen == false` uebersetzt; ein
            // `Op::Yield` kann darin nicht stehen. Kein `panic!`: ein Absturz
            // der Maschine ist in einem Kernel keine Fehlermeldung.
            Step::Yield(_) => Err(i.throw_kind("TypeError", "yield outside a generator")),
            Step::Await(_) => Err(i.throw_kind("TypeError", "await outside an async function")),
        }
    }

    /// Einen FUNKTIONSRUMPF auf der Maschine fahren, wenn der Ruf NICHT von
    /// ihr kommt.
    ///
    /// `Op::Call` in der Maschine legt einen Rahmen an und laeuft weiter —
    /// aber jeder Aufruf, der von aussen kommt (aus einem eingebauten
    /// Rueckruf, aus der Microtask-Schlange, aus einem Ereignisbehandler, aus
    /// einem Generator), ging ueber `Interp::run_js_body` und damit auf den
    /// BAUMLAEUFER. Und weil dessen Aufrufe wieder dort landen, blieb alles
    /// darunter beim Baumlaeufer: die Fritzbox-Anmeldung fuhr 320 721
    /// Schritte, davon 4 286 auf der Maschine.
    ///
    /// Es ist derselbe Chunk, den `Op::Call` benutzt haette — kein zweiter
    /// Semantikpfad, nur derselbe von einem anderen Rufer aus erreicht.
    pub fn run_function(i: &mut Interp, chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> C<Value> {
        let mut vm = Vm::new();
        vm.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                               is_program: false, root: true,
                               handlers: Vec::new(), iters: Vec::new() });
        match vm.drive(i)? {
            Step::Done(v) => Ok(v),
            Step::Yield(_) => Err(i.throw_kind("TypeError", "yield outside a generator")),
            Step::Await(_) => Err(i.throw_kind("TypeError", "await outside an async function")),
        }
    }

    /// Eine Maschine fuer einen GENERATOR- oder ASYNC-RUMPF: ein einziger
    /// Wurzelrahmen,
    /// noch nichts gelaufen. Die Umgebung hat `Interp::call_env` gebaut —
    /// Parameter und `this` stehen beim AUFRUF fest, der Rumpf laeuft erst
    /// beim ersten `next()`. Genau diese Reihenfolge verlangt die Spec.
    pub fn for_generator(chunk: Rc<Chunk>, env: &Rc<RefCell<Env>>) -> Vm {
        let mut vm = Vm::new();
        vm.frames.push(Frame { chunk, ip: 0, envs: alloc::vec![env.clone()], base: 0,
                               is_program: false, root: true,
                               handlers: Vec::new(), iters: Vec::new() });
        vm
    }

    /// Den Wert von `next(v)` an die Stelle legen, an der `Op::Yield` seinen
    /// abgegeben hat — er ist der Wert des `yield`-Ausdrucks.
    pub fn send(&mut self, v: Value) {
        self.stack.push(v);
    }

    /// `gen.throw(v)`: den Wurf an der Anhaltestelle einwerfen. `false`
    /// heisst, dass ihn hier keiner faengt — dann ist der Generator fertig
    /// und der Wurf geht an den Rufer.
    pub fn inject_throw(&mut self, i: &mut Interp, v: Value) -> bool {
        self.unwind(i, v)
    }

    /// `gen.return(v)`: die Maschine aufgeben. Offene `for…of`-Iterationen
    /// werden GESCHLOSSEN (von innen nach aussen), nicht bloss vergessen —
    /// sonst faehrt ein fremder Generator seinen `finally`-Block nie.
    ///
    /// Ein anhaengiger `finally` KANN es hier nicht geben: ein `yield` unter
    /// einem solchen ist schon beim Uebersetzen abgelehnt (`Compiler::fin`).
    pub fn close(&mut self, i: &mut Interp) {
        while let Some(f) = self.frames.pop() {
            for it in f.iters.iter().rev() {
                if let Iter::Obj(v) = it { i.iter_close(v); }
            }
            if !f.root { i.depth -= 1; }
        }
        self.stack.clear();
    }

    /// Alles, was diese Maschine festhaelt — fuer den Abbau eines Realms.
    ///
    /// Ein angehaltener Generator ist der einzige Ort, an dem Umgebungen und
    /// halbfertige Werte leben, ohne in einer Eigenschaft oder Bindung zu
    /// stehen. `Interp::teardown` faende sie sonst nicht, und ein Rc-Ring
    /// darin kaeme nie auf null.
    pub fn roots(&self, objs: &mut Vec<super::value::Gc>,
                 envs: &mut Vec<Rc<RefCell<Env>>>) {
        for v in &self.stack {
            if let Value::Obj(o) = v { objs.push(o.clone()); }
        }
        if let Value::Obj(o) = &self.completion { objs.push(o.clone()); }
        for f in &self.frames {
            envs.extend(f.envs.iter().cloned());
            for it in &f.iters {
                if let Iter::Obj(Value::Obj(o)) = it { objs.push(o.clone()); }
            }
        }
    }

    /// Die Schleife. Sie laeuft, bis der Wurzelrahmen zurueck ist oder ein
    /// `yield` sie anhaelt — und beim naechsten Aufruf genau dort weiter.
    pub fn drive(&mut self, i: &mut Interp) -> C<Step> {
        // **Der Chunk wird GEHALTEN, nicht je Befehl neu geliehen.** Ein
        // `Rc::clone` mit dem Freigeben danach sind zwei Zaehleroperationen —
        // je BEFEHL, auf dem heissesten Pfad des Motors. Gewechselt wird er
        // nur, wenn sich der Rahmen aendert, und das prueft ein
        // Zeigervergleich.
        let mut held: Option<(usize, Rc<Chunk>)> = None;
        loop {
            let flen = self.frames.len();
            let (ip, need) = {
                let f = self.frames.last().unwrap();
                let need = match &held {
                    Some((n, c)) => *n != flen || !Rc::ptr_eq(c, &f.chunk),
                    None => true,
                };
                (f.ip, need)
            };
            if need {
                held = Some((flen, self.frames.last().unwrap().chunk.clone()));
            }
            let chunk: &Chunk = &held.as_ref().unwrap().1;
            if ip >= chunk.ops.len() {
                return Ok(Step::Done(core::mem::replace(&mut self.completion, Value::Undefined)));
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
                Op::Call { .. } | Op::New(_) | Op::CallSpread(_) | Op::NewSpread
                | Op::SetCompletion | Op::DeclVar { .. } | Op::Ret
                | Op::Yield | Op::Await | Op::ForInNext(_) | Op::SuperCall(_) => true,
                _ => false,
            };
            i.vm_ops += 1;
            if counts {
                i.steps += 1;
                if i.steps > i.max_steps {
                    return Err(i.throw_kind("RangeError", "step budget exhausted"));
                }
                // Und die Uhr — dieselbe Koernung wie im Baumlaeufer, sonst
                // ist es eine andere Uhr. Siehe `Interp::check_deadline`:
                // sie stand nur in `tick`, und der zaehlt nur eingebaute
                // Schleifen. Genau die Rechnung, die Minuten dauert, kam
                // deshalb nie an ihr vorbei.
                if i.steps & 0xFFFF == 0 { i.check_deadline()?; }
            }
            match self.step(i, chunk, ip) {
                Ok(Flow::Done(v)) => return Ok(Step::Done(v)),
                Ok(Flow::Yield(v)) => return Ok(Step::Yield(v)),
                Ok(Flow::Await(v)) => return Ok(Step::Await(v)),
                Ok(Flow::Go) => {}
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

    fn step(&mut self, i: &mut Interp, chunk: &Chunk, ip: usize) -> C<Flow> {
        // **Hier und nicht je Arm.** Ein Versuch, sie erst beim Gebrauch zu
        // holen, sparte ein `Rc`-Zaehlerpaar je Befehl — und war falsch:
        // einige Befehle AENDERN die Umgebungskette, bevor sie sie benutzen,
        // und bekamen dann die neue statt der alten
        // (`cannot access 'dialog' before initialization`). Wer das noch
        // einmal angeht, muss je Arm nachweisen, dass er vor jeder Aenderung
        // liest — nicht es annehmen.
        let env = self.frames.last().unwrap().envs.last().unwrap().clone();
        match &chunk.ops[ip] {
            Op::Const(k) => self.push(chunk.constants[*k as usize].clone()),
            Op::LoadVar(n) => {
                let v = match chunk.hints.get(ip) {
                    Some(h) => i.vm_load_at(&chunk.names[*n as usize], &env, h)?,
                    None => i.vm_load(&chunk.names[*n as usize], &env)?,
                };
                self.push(v);
            }
            Op::StoreVar(n) => {
                let v = self.top();
                match chunk.hints.get(ip) {
                    Some(h) => i.vm_store_at(&chunk.names[*n as usize], v, &env, h)?,
                    None => i.vm_store(&chunk.names[*n as usize], v, &env)?,
                }
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
                let v = super::interp::this_observed(i, &env);
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
            Op::ToNumeric => { let v = self.pop(); let r = i.to_numeric(&v)?; self.push(r); }
            Op::Step(up) => { let v = self.pop(); let r = i.step_numeric(&v, *up)?; self.push(r); }
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
            Op::JumpTrue(t) => {
                let v = self.pop();
                if v.truthy() {
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
                // **Ein ganzzahliger Index geht ohne Haufen.** `a[i]` baute
                // bisher aus `i` eine Zeichenkette AUF DEM HAUFEN (`Rc<str>`),
                // gab sie an `get`, und `ta_read`/`array_index` lasen die Zahl
                // sofort wieder heraus. Der Schluessel ist derselbe — er wird
                // nur in einen Puffer auf dem Stapel geschrieben statt
                // alloziert und wieder freigegeben. Auf einer Seite, die
                // rechnet (SHA-256, Bildbearbeitung, ein Parser), ist das der
                // haeufigste Befehl ueberhaupt.
                if let Some(ix) = int_index(&key) {
                    let b = IdxBuf::new(ix);
                    let v = i.get(&obj, b.as_str())?;
                    self.push(v);
                } else {
                    // `to_prop_key`, NICHT `to_string`: ein Symbol ist ein
                    // Schluessel und keine Zeichenkette, und es zu einer zu
                    // machen hat `Symbol.iterator` & Co. ins Leere zeigen
                    // lassen — 35 Tests, gefunden vom Diff gegen den
                    // Baumlaeufer.
                    let k = i.to_prop_key(&key)?;
                    let v = i.get(&obj, &k)?;
                    self.push(v);
                }
            }
            Op::SetProp(n) => {
                let val = self.pop();
                let obj = self.pop();
                let throw = super::interp::env_strict(&env);
                i.set(&obj, &chunk.names[*n as usize], val.clone(), throw)?;
                self.push(val);
            }
            Op::SetIndex => {
                let val = self.pop();
                let key = self.pop();
                let obj = self.pop();
                let throw = super::interp::env_strict(&env);
                if let Some(ix) = int_index(&key) {
                    let b = IdxBuf::new(ix);
                    i.set(&obj, b.as_str(), val.clone(), throw)?;
                } else {
                    let k = i.to_prop_key(&key)?;
                    i.set(&obj, &k, val.clone(), throw)?;
                }
                self.push(val);
            }
            Op::Call { argc, name } => {
                let args = self.take(*argc as usize);
                let this = self.pop();
                let callee = self.pop();
                let n = chunk.names.get(*name as usize).map(|s| &**s);
                // Ein DIREKTER `eval`-Aufruf ist kein gewoehnlicher: er sieht
                // den Bereich des Rufers. Erkannt wird er am Namen UND an der
                // Sache — eine eigene Funktion namens `eval` ist keiner.
                if n == Some("eval") && i.is_eval_fn(&callee) {
                    // Ab jetzt kann eine Bindung weiter INNEN entstehen, als
                    // ein Wegweiser zeigt. Siehe `Interp::hints_ok`.
                    i.hints_ok = false;
                    let env = self.frames.last().unwrap().envs.last().unwrap().clone();
                    let c = args.first().cloned().unwrap_or(Value::Undefined);
                    let v = i.perform_eval(&c, Some(env))?;
                    self.push(v);
                    return Ok(Flow::Go);
                }
                self.invoke(i, callee, this, args, n)?;
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
            Op::SuperGet(n) => {
                let (v, _) = i.super_get(&chunk.names[*n as usize], &env)?;
                self.push(v);
            }
            Op::SuperCallee(n) => {
                let (v, t) = i.super_get(&chunk.names[*n as usize], &env)?;
                self.push(v);
                self.push(t);
            }
            Op::SuperCall(argc) => {
                let args = self.take(*argc as usize);
                let v = i.super_call(&args, &env)?;
                self.push(v);
            }
            Op::JumpNullishTo(t) => {
                if matches!(self.top(), Value::Undefined | Value::Null) {
                    self.jump(*t);
                }
            }
            Op::Closure(f) => {
                let v = i.func_value(chunk.funcs[*f as usize].clone(), &env);
                self.push(v);
            }
            // Dieselben Hilfen wie der Baumlaeufer — siehe `Op::BindPat`.
            Op::BindPat { pat, mode } => {
                let v = self.pop();
                let p = &chunk.pats[*pat as usize];
                match mode {
                    super::code::BindMode::Init => i.bind_pattern(p, v, &env, true)?,
                    super::code::BindMode::Assign => i.bind_pattern(p, v, &env, false)?,
                    super::code::BindMode::Declare => i.declare_pattern(p, v, &env)?,
                }
            }
            Op::BindHead(h) => {
                let v = self.pop();
                i.for_head_bind(&chunk.heads[*h as usize], v, &env)?;
            }
            // Dieselbe Funktion, die der Baumlaeufer ruft — siehe `Op::Class`.
            Op::Class(c) => {
                let v = i.eval_class(&chunk.classes[*c as usize], &env)?;
                self.push(v);
            }
            Op::NewObject => {
                let g = super::value::new_obj(Some(i.realm.object_proto.clone()));
                self.push(Value::Obj(g));
            }
            Op::DefineProp(n) => {
                let val = self.pop();
                let key: Rc<str> = chunk.names[*n as usize].clone();
                // `{ m(){} }` und `{ a: function(){} }` bekommen den
                // Schluessel als Namen — dieselbe Regel wie `var f = …`.
                i.name_function(&val, &key);
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(key, super::value::Prop::data(val));
                }
            }
            Op::DefinePropComputed => {
                let val = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                i.name_function(&val, &k);
                if let Value::Obj(g) = self.top() {
                    g.borrow_mut().set_prop(k, super::value::Prop::data(val));
                }
            }
            Op::DefineAccessor { name, get } => {
                let f = self.pop();
                let key: Rc<str> = chunk.names[*name as usize].clone();
                let show = alloc::format!("{} {key}", if *get { "get" } else { "set" });
                i.name_function(&f, &show);
                if let Value::Obj(g) = self.top() {
                    i.define_accessor(&g, key, f, *get);
                }
            }
            Op::DefineAccessorComputed { get } => {
                let f = self.pop();
                let key = self.pop();
                let k = i.to_prop_key(&key)?;
                let show = alloc::format!("{} {k}", if *get { "get" } else { "set" });
                i.name_function(&f, &show);
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
            Op::Rot4 => {
                let n = self.stack.len();
                let c = self.stack.remove(n - 1);
                self.stack.insert(n - 4, c);
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
            Op::PrivateIn(n) => {
                let obj = self.pop();
                let v = i.private_in(&chunk.names[*n as usize], &obj)?;
                self.push(v);
            }
            Op::DeleteProp(n) => {
                let obj = self.pop();
                let key = &chunk.names[*n as usize];
                let v = i.delete_key(&obj, key)?;
                if !v {
                    super::interp::strict_site!(i, 7);
                    if super::interp::env_strict(&env) {
                        return i.type_err(&alloc::format!("cannot delete property '{key}'"));
                    }
                }
                self.push(Value::Bool(v));
            }
            Op::DeleteIndex => {
                let key = self.pop();
                let obj = self.pop();
                let k = i.to_prop_key(&key)?;
                let v = i.delete_key(&obj, &k)?;
                if !v {
                    super::interp::strict_site!(i, 7);
                    if super::interp::env_strict(&env) {
                        return i.type_err(&alloc::format!("cannot delete property '{k}'"));
                    }
                }
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
            Op::CallSpread(name) => {
                let args = self.pop();
                let this = self.pop();
                let callee = self.pop();
                let a = i.iterate(&args)?;
                let n = chunk.names.get(*name as usize).map(|s| &**s);
                self.invoke(i, callee, this, a, n)?;
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
                self.frames.last_mut().unwrap().iters.push(Iter::Obj(it));
            }
            Op::IterNext(done) => {
                let it = match self.frames.last().unwrap().iters.last() {
                    Some(Iter::Obj(v)) => v.clone(),
                    _ => return Err(i.throw_kind("TypeError", "no iterator here")),
                };
                match i.iter_next(&it) {
                    Ok(Some(v)) => self.push(v),
                    Ok(None) => {
                        self.jump(*done);
                    }
                    // **Wirft `next()` SELBST, wird nicht geschlossen.** Der
                    // Iterator ist dann in unbekanntem Zustand, und `return()`
                    // darauf waere spec-widrig — anders als bei einem Wurf aus
                    // dem RUMPF, der ihn sehr wohl schliessen muss. Deshalb
                    // faellt er hier aus der Liste, bevor der Wurf geht: sonst
                    // holt ihn der naechste Behandler oder das Verlassen des
                    // Rahmens nach.
                    Err(e) => {
                        self.frames.last_mut().unwrap().iters.pop();
                        return Err(e);
                    }
                }
            }
            Op::ForInAll => {
                let v = self.pop();
                let mut keys = i.for_in_keys(&v)?;
                // Rueckwaerts, damit `pop` der naechste Schritt ist.
                keys.reverse();
                let vals = keys.into_iter().map(Value::Str).collect();
                self.frames.last_mut().unwrap().iters.push(Iter::Keys(vals));
            }
            Op::ForInNext(done) => {
                let next = match self.frames.last_mut().unwrap().iters.last_mut() {
                    Some(Iter::Keys(ks)) => ks.pop(),
                    _ => return Err(i.throw_kind("TypeError", "no key list here")),
                };
                match next {
                    Some(k) => self.push(k),
                    None => self.jump(*done),
                }
            }
            Op::IterDrop => {
                self.frames.last_mut().unwrap().iters.pop();
            }
            Op::IterClose => {
                if let Some(Iter::Obj(it)) = self.frames.last_mut().unwrap().iters.pop() {
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
                // Ein `return` aus einer `for…of`-Schleife heraus muss ihren
                // Iterator SCHLIESSEN — sonst faehrt ein Generator seinen
                // eigenen `finally`-Block nie. Von innen nach aussen.
                for it in f.iters.iter().rev() {
                    if let Iter::Obj(v) = it { i.iter_close(v); }
                }
                self.stack.truncate(f.base);
                if f.root {
                    if f.is_program {
                        // Der Wert eines PROGRAMMS ist sein letzter
                        // Ausdruckswert, nicht das, was am Ende auf dem Stapel
                        // liegt. Ein Funktions- oder Generatorrumpf hat keinen.
                        let c = core::mem::replace(&mut self.completion, Value::Undefined);
                        return Ok(Flow::Done(if matches!(c, Value::Undefined) { v } else { c }));
                    }
                    return Ok(Flow::Done(v));
                }
                i.depth -= 1;
                if self.frames.is_empty() {
                    return Ok(Flow::Done(v));
                }
                self.push(v);
            }
            // Der Rahmen bleibt stehen, wo er steht — `ip` zeigt schon auf den
            // naechsten Befehl, und `Vm::send` legt den Wert von `next(v)` an
            // die Stelle, an der dieser hier seinen abgegeben hat.
            Op::Yield => {
                let v = self.pop();
                return Ok(Flow::Yield(v));
            }
            Op::Await => {
                let v = self.pop();
                return Ok(Flow::Await(v));
            }
        }
        Ok(Flow::Go)
    }

    /// Den innersten Behandler suchen, der den Wurf nimmt. `false` heisst:
    /// keiner da, der Wurf verlaesst diese Maschine.
    ///
    /// Zurueckgeschnitten wird auf die Tiefen, die der Behandler sich gemerkt
    /// hat — Wertestapel, Umgebungen UND offene Iterationen. Wer eins davon
    /// vergisst, bekommt einen `catch`-Block, der auf fremdem Zustand steht.
    fn unwind(&mut self, i: &mut Interp, v: Value) -> bool {
        // **Ueber Rahmengrenzen hinweg.** Ein `throw` tief in einer Funktion
        // sucht sein `try` beim RUFER, wenn es dort keins gibt — genau das
        // macht ein Aufrufstapel aus. Ohne diese Schleife blieb ein Wurf im
        // eigenen Rahmen haengen und kam als „UNCAUGHT" heraus, obwohl zwei
        // Ebenen darueber ein `catch` stand.
        let h = loop {
            let Some(f) = self.frames.last_mut() else { return false };
            if let Some(h) = f.handlers.pop() {
                f.envs.truncate(h.envs);
                break h;
            }
            // Kein Behandler in diesem Rahmen: ihn verlassen und weitersuchen.
            // Das PROGRAMM ist die Grenze — darueber gibt es nur den Rufer in
            // Rust, und dorthin geht der Wurf als `Err`.
            if f.root {
                return false;
            }
            let f = self.frames.pop().unwrap();
            // **Auch beim Verlassen eines Rahmens gehoeren offene `for…of`
            // geschlossen** — von innen nach aussen, genau wie im `Ret`. Ohne
            // diese Schleife faehrt ein fremder Generator seinen
            // `finally`-Block nie, wenn der Wurf aus dem Schleifenrumpf durch
            // die Funktion nach draussen geht. Gefunden vom Diff, als
            // `for ([x.attr] of it)` uebersetzbar wurde und ein werfender
            // Schreiber im KOPF dasselbe ausloeste.
            for it in f.iters.iter().rev() {
                if let Iter::Obj(v) = it { i.iter_close(v); }
            }
            self.stack.truncate(f.base);
            i.depth -= 1;
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
            if let Iter::Obj(v) = it { i.iter_close(&v); }
        }
        let Some(t) = h.catch_ip.or(h.finally_ip) else { return false };
        self.frames.last_mut().unwrap().ip = t as usize;
        self.stack.truncate(h.stack);
        self.stack.push(v);
        true
    }

    /// Einen Aufruf ausfuehren.
    ///
    /// **Der Punkt der ganzen Stufe:** ist der Gerufene eine JS-Funktion,
    /// deren Rumpf sich uebersetzen laesst, bekommt er einen RAHMEN — der
    /// Rust-Stapel waechst nicht mit. Alles andere (eingebaute Funktionen,
    /// gebundene, Generatoren) geht weiter durch `Interp::call`, und das ist
    /// richtig so: die haben keinen Rumpf aus Befehlen.
    ///
    /// Die Tiefe wird trotzdem gezaehlt. Ein Rahmenstapel kann nicht
    /// ueberlaufen, aber eine endlose JS-Rekursion soll denselben
    /// `RangeError` geben wie vorher — sonst haengt sie, bis der Speicher
    /// ausgeht.
    fn invoke(&mut self, i: &mut Interp, callee: Value, this: Value, args: Vec<Value>,
              name: Option<&str>) -> C<()> {
        let d = match &callee {
            Value::Obj(o) => match &o.borrow().kind {
                super::value::ObjKind::Function(d) => Some(d.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(d) = d else {
            // Den NAMEN nennen, nicht nur das Ereignis — dieselbe Hilfe wie im
            // Baumlaeufer. Ohne diese Zeile verlor jedes Skript, das auf die
            // Maschine wanderte, still seine Fehlerdiagnose.
            if !i.is_callable(&callee) {
                return Err(i.not_a_function(name));
            }
            i.vm_calls_native += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        };
        // **Ein Generator und eine async-Funktion bekommen hier keinen
        // Rahmen.** Ihr Aufruf baut ein Objekt bzw. ein Versprechen, und der
        // Rumpf laeuft auf einer EIGENEN Maschine — beides tut `Interp::call`
        // an einer Stelle, fuer beide Maschinen dieselbe. Die Pruefung steht
        // vor `func_chunk`, weil dessen Chunk hier sonst als gewoehnlicher
        // Funktionsrumpf losliefe.
        if d.node.is_generator || d.node.is_async {
            i.vm_calls_slow += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        }
        let Some(chunk) = i.func_chunk(&d.node) else {
            i.vm_calls_slow += 1;
            let v = i.call(&callee, this, &args)?;
            self.push(v);
            return Ok(());
        };
        i.vm_calls += 1;
        i.depth += 1;
        if i.depth > i.max_depth {
            i.depth -= 1;
            return Err(i.throw_kind("RangeError", "Maximum call stack size exceeded"));
        }
        let env = match i.call_env(&d, this, &args) {
            Ok(e) => e,
            Err(e) => { i.depth -= 1; return Err(e) }
        };
        if let Err(e) = i.hoist_body(&d.node.body, &env) {
            i.depth -= 1;
            return Err(e);
        }
        let base = self.stack.len();
        self.frames.push(Frame {
            chunk, ip: 0, envs: alloc::vec![env], base,
            is_program: false, root: false, handlers: Vec::new(), iters: Vec::new(),
        });
        Ok(())
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

/// Ein Feldindex als Zahl, wenn der Schluessel einer ist.
///
/// Die Grenze ist die von `array_index`: `0 <= n < 2^32-1` und ganzzahlig.
/// Genau in diesem Bereich ist die Zeichenkette einer Zahl in JS die schlichte
/// Dezimaldarstellung, also derselbe Schluessel, den `to_string` gebaut haette.
/// `-0` faellt mit hinein und wird zu `"0"` — was JS auch tut.
#[inline]
fn int_index(v: &Value) -> Option<u32> {
    match v {
        Value::Num(n) if *n >= 0.0 && *n < 4294967295.0 && libm::floor(*n) == *n => Some(*n as u32),
        _ => None,
    }
}

/// Zehn Ziffern auf dem STAPEL — die groesste Zahl in diesem Bereich hat
/// zehn (`4294967294`).
struct IdxBuf {
    b: [u8; 10],
    at: usize,
}

impl IdxBuf {
    #[inline]
    fn new(mut v: u32) -> IdxBuf {
        let mut b = [0u8; 10];
        let mut at = 10;
        loop {
            at -= 1;
            b[at] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 { break }
        }
        IdxBuf { b, at }
    }
    #[inline]
    fn as_str(&self) -> &str {
        // SAFETY: nur ASCII-Ziffern geschrieben, also gueltiges UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.b[self.at..]) }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    /// **Die Uhr des Wirts muss BEIDE Maschinen erreichen.**
    ///
    /// Bis 0.117.0 stand sie nur in `Interp::tick`, und `tick` ruft nur, wer
    /// in einem EINGEBAUTEN schleift (`Array.prototype.*`, `JSON`, die
    /// Iterator-Hilfen). Eine reine JS-Schleife lief an ihr vorbei: kein
    /// Herzschlag, und das Zeitbudget war fuer genau den Fall unwirksam, fuer
    /// den es gedacht ist. Am Geraet waren das vier Minuten Stillstand ohne
    /// eine Zeile im Log.
    ///
    /// Der Test schleift deshalb OHNE eingebauten Aufruf. Mit einem darin
    /// haette er auch vorher bestanden — und genau das ist die Falle, die die
    /// Luecke so lange offen gehalten hat.
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn clock() -> bool {
        CALLS.fetch_add(1, Ordering::Relaxed);
        false // sofort abgelaufen: der Lauf muss hier enden
    }

    fn run(novm: bool) -> (bool, u32) {
        CALLS.store(0, Ordering::Relaxed);
        let mut i = super::Interp::new();
        i.vm_off = novm;
        i.deadline = Some(clock);
        let src = "var x = 0; for (var k = 0; k < 400000; k++) { x = x + 1; }";
        let prog = crate::js::parse(src, false).expect("parst");
        let err = i.run_program(&prog).is_err();
        (err, CALLS.load(Ordering::Relaxed))
    }

    #[test]
    fn die_uhr_erreicht_die_befehlsmaschine() {
        let (abgebrochen, gefragt) = run(false);
        assert!(gefragt > 0, "die Uhr wurde nie gefragt");
        assert!(abgebrochen, "der Lauf lief ueber die abgelaufene Uhr hinaus");
    }

    #[test]
    fn die_uhr_erreicht_den_baumlaeufer() {
        let (abgebrochen, gefragt) = run(true);
        assert!(gefragt > 0, "die Uhr wurde nie gefragt");
        assert!(abgebrochen, "der Lauf lief ueber die abgelaufene Uhr hinaus");
    }

    /// **Ein Wegweiser darf die BEDEUTUNG nicht aendern.**
    ///
    /// `Chunk::hints` merkt sich, in welcher Tiefe ein Name beim letzten Mal
    /// stand. Entstuende danach eine Bindung WEITER INNEN, zeigte er daran
    /// vorbei — und das waere kein Absturz, sondern ein falscher Wert.
    ///
    /// Der Test faehrt dieselben Programme ZWEIMAL, einmal mit Wegweisern und
    /// einmal ohne, und vergleicht. Ein Test, der nur „laeuft durch" prueft,
    /// saehe genau den Fehler nicht, um den es geht.
    fn zweimal(src: &str) -> (alloc::string::String, alloc::string::String) {
        let lauf = |hints: bool| {
            let mut i = super::Interp::new();
            i.hints_ok = hints;
            let prog = match crate::js::parse(src, false) {
                Ok(p) => p,
                Err(e) => return alloc::format!("SyntaxError @{}", e.at),
            };
            match i.run_program(&prog) {
                Ok(v) => i.to_string(&v).map(|s| s.to_string())
                          .unwrap_or_else(|_| alloc::string::String::from("?")),
                Err(super::Abrupt::Throw(v)) => {
                    let m = i.get(&v, "message").ok()
                        .and_then(|m| i.to_string(&m).ok())
                        .unwrap_or_else(|| alloc::rc::Rc::from("?"));
                    alloc::format!("THROW {m}")
                }
                Err(_) => alloc::string::String::from("ABRUPT"),
            }
        };
        (lauf(true), lauf(false))
    }

    #[test]
    fn wegweiser_aendert_die_bedeutung_nicht() {
        let faelle: &[&str] = &[
            // Verschattung in einem Block, in einer Schleife: dieselbe
            // Befehlsstelle, viele Durchlaeufe.
            "var o=[];var x='a';for(var k=0;k<3;k++){let x='b'+k;o.push(x);}o.push(x);o.join(',')",
            // Ein Abschluss liest nach aussen, waehrend innen gleich heisst.
            "var x='aussen';function f(){return x}function g(){var x='innen';return f()+'|'+x}g()",
            // Zeitliche Totzone: der Name STEHT hier, ist aber noch nichts.
            "function f(){ try { return y } catch(e) { return 'TDZ:'+e.name } finally { } } var r=f(); let y=1; r",
            // `const` beschreiben — der Fehler muss derselbe bleiben.
            "const c=1; try { c=2; return } catch(e) { e.name }",
            // Ein direktes `eval`, das eine Bindung WEITER INNEN anlegt.
            "var x='aussen';function f(){function inner(){return x}var a=inner();eval(\"var x='innen'\");return a+'|'+inner()}f()",
            // Tief geschachtelt, damit der Weg wirklich mehrere Spruenge hat.
            "var a=1;function f(){var b=2;return function(){var c=3;return function(){return a+b+c}}}f()()()",
            // Derselbe Name auf mehreren Ebenen, gelesen von innen nach aussen.
            "var n='g';function f(){var n='f';{let n='b';return n+f2()}}function f2(){return n}f()",
        ];
        for (k, src) in faelle.iter().enumerate() {
            let (mit, ohne) = zweimal(src);
            assert_eq!(mit, ohne, "Fall {k} laeuft mit und ohne Wegweiser auseinander: {src}");
        }
    }

    /// Ein direktes `eval` MUSS die Wegweiser abschalten — das ist der
    /// einzige Weg, auf dem eine Bindung nachtraeglich weiter innen entsteht.
    #[test]
    fn direktes_eval_schaltet_die_wegweiser_ab() {
        let mut i = super::Interp::new();
        assert!(i.hints_ok, "am Anfang sind sie an");
        let prog = crate::js::parse("var q = 1; eval(\"q = 2\"); q", false).expect("parst");
        let _ = i.run_program(&prog);
        assert!(!i.hints_ok, "nach einem direkten eval muessen sie aus sein");
    }
}
