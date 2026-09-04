//! Der Befehlssatz, in den ein Programm uebersetzt wird — und die Einheit, in
//! der er liegt (`Chunk`).
//!
//! **Warum es das gibt.** Ein Baumlaeufer benutzt den RUST-Stapel als
//! Zustandsspeicher, und ein Rust-Stapel laesst sich nicht anhalten. Damit
//! sind Generatoren, `async`/`await` und alles, was spaeter einmal mitten in
//! einem Ausdruck stehenbleiben soll, nicht bloss ungebaut, sondern
//! unbaubar — man kaeme an den Zustand nicht heran. Die Befehlsliste dreht
//! das um: der Zustand ist ein Feld, das man wegspeichern und weiterlaufen
//! lassen kann.
//!
//! Der Kopf von `interp.rs` hat diesen Umbau vorgesehen und eine Bedingung
//! daran geknuepft: „der test262-Lauf ist danach das Netz, mit dem eine
//! Umstellung auf Bytecode ueberhaupt erst verantwortbar ist." Das Netz gibt
//! es (52,77 %, Fehlerkarte nach Familien, 45 s je Lauf), also wird sie
//! eingeloest.
//!
//! **Was hier NICHT passiert: eine zweite Semantik.** Jeder Befehl unten ruft
//! dieselben Hilfen wie der Baumlaeufer (`Interp::binary`, `Interp::call`,
//! `Value::truthy`, `Env`). Die Maschine tauscht den VERTEILER, nicht die
//! Bedeutung — und solange ein Programm entweder ganz uebersetzt oder ganz
//! vom Baumlaeufer gefahren wird, kann auch keine Mischung entstehen.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::{BinOp, UnaryOp};
use super::value::Value;

/// Ein Befehl. Sprungziele sind absolute Indizes in `Chunk::ops` — relative
/// waeren beim Zurueckflicken (`patch`) eine zweite Rechnung, und die erste
/// ist schon fehleranfaellig genug.
#[derive(Debug, Clone)]
pub enum Op {
    /// `constants[i]` auf den Stapel.
    Const(u32),
    /// Den Wert des Namens `names[i]` auf den Stapel.
    LoadVar(u32),
    /// Oben zuweisen an `names[i]`; der Wert BLEIBT auf dem Stapel (eine
    /// Zuweisung ist ein Ausdruck).
    StoreVar(u32),
    /// `names[i]` binden. Nimmt den Wert vom Stapel.
    ///
    /// `lexical` unterscheidet die beiden Faelle, und der Unterschied ist
    /// beobachtbar: ein `let`/`const` gehoert GENAU HIER hin, ein `var` in die
    /// naechste Funktionsumgebung — die das Hochziehen schon angelegt hat, und
    /// die kann weiter oben liegen. `for (const x = 1; …)` hat den Fehler
    /// gezeigt: die Zuweisung fand das aeussere `x`.
    DeclVar { name: u32, mutable: bool, lexical: bool },
    /// Einer eben gebauten Funktion den Namen der Variablen geben, an die sie
    /// gerade gebunden wird (`var f = function(){}` -> `f.name === "f"`).
    /// Sichtbar in Stapelspuren und in `f.name`; sechs Tests.
    NameFunc(u32),
    /// Oben in einen Eigenschaftsschluessel wandeln — VOR der Auswertung des
    /// Wertes, weil `ToPropertyKey` Nebenwirkungen haben kann und die Spec
    /// ihre Reihenfolge festlegt.
    ToKey,
    /// `this`.
    This,
    Pop,
    Dup,
    /// Oben verwerfen und darunter liegenden Wert behalten (`a, b` → a weg).
    Swap,
    Un(UnaryOp),
    Bin(BinOp),
    /// `typeof x` auf einem NAMEN — muss ohne ReferenceError auskommen, wenn
    /// es den Namen nicht gibt, und ist deshalb kein `LoadVar` + `Un`.
    TypeofVar(u32),
    Jump(u32),
    /// Springt, wenn oben falsy ist; nimmt den Wert IMMER vom Stapel.
    JumpFalse(u32),
    /// Fuer `&&`/`||`/`??`: springt bei falsy/truthy/nullish und LAESST den
    /// Wert liegen — das ist der Wert des Ausdrucks.
    JumpFalseKeep(u32),
    JumpTrueKeep(u32),
    JumpNullishKeep(u32),
    /// `obj[names[i]]` — Stapel: obj → wert.
    GetProp(u32),
    /// `obj[key]` — Stapel: obj, key → wert.
    GetIndex,
    /// Stapel: obj, wert → wert (die Zuweisung ist ein Ausdruck).
    SetProp(u32),
    /// Stapel: obj, key, wert → wert.
    SetIndex,
    /// Stapel: callee, this, arg0..argN → ergebnis.
    Call(u16),
    /// Stapel: callee, arg0..argN → ergebnis.
    New(u16),
    /// Feld aus den obersten `n` Werten. `true` an Stelle `k` heisst: dort
    /// stand eine LUECKE (`[1, , 3]`), und die ist nicht dasselbe wie
    /// `undefined` — `in` findet sie nicht.
    MakeArray(u16),
    /// Ein leerer Gegenstand mit `Object.prototype`.
    NewObject,
    /// Stapel: obj, wert → obj. Eine Dateneigenschaft unter `names[i]`.
    DefineProp(u32),
    /// Stapel: obj, schluessel, wert → obj.
    DefinePropComputed,
    /// Stapel: obj, funktion → obj. `get` unterscheidet Leser von Schreiber;
    /// beide muessen sich auf DERSELBEN Eigenschaft treffen koennen.
    DefineAccessor { name: u32, get: bool },
    DefineAccessorComputed { get: bool },
    /// Stapel: obj, quelle → obj. `{...src}` kopiert die aufzaehlbaren
    /// EIGENEN Eigenschaften.
    SpreadInto,
    /// Die obersten ZWEI verdoppeln — fuer `o[k]++`, wo Objekt und Schluessel
    /// nur EINMAL ausgewertet werden duerfen.
    Dup2,
    /// `[a, b, c]` → `[c, a, b]`. Fuer `o.p++`, wo der ALTE Wert das Ergebnis
    /// ist und trotzdem unter dem Objekt hindurch nach unten muss.
    Rot3,
    /// Ein regulaerer Ausdruck aus `names[body]` und `names[flags]`.
    Regex { body: u32, flags: u32 },
    /// Die obersten `n` Werte zu einer Zeichenkette verketten (Vorlage).
    Concat(u16),
    /// `delete obj[names[i]]` bzw. `delete obj[key]`.
    DeleteProp(u32),
    DeleteIndex,
    /// Ein Feld aus den obersten `n` EINTRAEGEN bauen, von denen jeder
    /// entweder ein Wert oder ein zu spreizender ist (`spread[k]`).
    MakeArraySpread { n: u16, spread: u32 },
    /// Wie `Call`/`New`, aber die Argumente stehen als FELD auf dem Stapel —
    /// so kann `f(...xs)` dieselbe Aufrufhilfe benutzen.
    CallSpread,
    NewSpread,
    /// Aus `names[i]` eine Funktion bauen — der Index zeigt in `funcs`.
    Closure(u32),
    Throw,
    /// Den Wert oben werfen — der Rueckweg aus einem `finally`, das nicht
    /// gefangen hat.
    Rethrow,
    /// Einen Behandler aufmachen. `catch`/`finally` sind Sprungziele,
    /// `u32::MAX` heisst „gibt es nicht".
    ///
    /// Der Behandler merkt sich AUCH die Stapel- und Umgebungstiefe: ein Wurf
    /// mitten in einem Ausdruck laesst halbe Werte liegen, und ohne das
    /// Zurueckschneiden faende der `catch`-Block einen Stapel vor, den niemand
    /// gebaut hat.
    TryStart { catch: u32, finally: u32 },
    /// Den obersten Behandler wieder zumachen.
    TryEnd,
    /// Die geworfene Sache in `names[i]` binden — der Kopf eines `catch`.
    BindCatch(u32),
    /// Den Iterator des Wertes oben holen und im Rahmen ablegen.
    ///
    /// FAUL, ueber `get_iterator`/`iter_next`/`iter_close` — dieselben Hilfen
    /// wie im Baumlaeufer. Die Werte vorher einzusammeln waere kuerzer und
    /// falsch: ein Rumpf, der die Quelle veraendert, muss das sehen, und ein
    /// vorzeitiger Ausstieg muss `return()` rufen. Fuenf Tests haben genau
    /// das gesagt.
    IterAll,
    /// Den naechsten Wert auf den Stapel; ist der Iterator zu Ende, springen.
    IterNext(u32),
    /// Den Iterator vergessen — er ist zu Ende, `return()` waere falsch.
    IterDrop,
    /// Den Iterator SCHLIESSEN (`return()`) und vergessen — der Weg fuer
    /// `break` und fuer jeden Abbruch.
    IterClose,
    /// Aus dem Rahmen zurueck; oben liegt der Wert.
    Ret,
    /// **Anhalten.** Oben liegt der Wert, den `next()` zurueckgibt; der Rahmen
    /// bleibt stehen, wo er steht.
    ///
    /// Beim Wiederaufnehmen legt `Vm::send` den Wert von `next(v)` an genau
    /// dieselbe Stelle des Stapels — und der ist der Wert des
    /// `yield`-Ausdrucks. Mehr ist ein `yield` nicht: der halbe Ausdruck
    /// darunter (`a + (yield 1)` hat `a` liegen) steht im Wertestapel des
    /// Rahmens und ueberlebt das Anhalten, weil er ein FELD ist und kein
    /// Rust-Stapel. Das ist die ganze Begruendung des Umbaus, eingeloest.
    Yield,
    /// **Warten.** Oben liegt das Erwartete; die Maschine haelt an, und was
    /// sie wieder anwirft, ist die Aufloesung des Versprechens.
    ///
    /// Derselbe Mechanismus wie `Yield` — nur legt sich hier ein Versprechen
    /// davor, und das Wiederaufnehmen kommt aus der Microtask-Schlange statt
    /// von einem `next()`. Genau das meinte der Bauplan mit „`async`/`await`
    /// ist derselbe Mechanismus mit einem Promise davor".
    Await,
    /// Eine Umgebung fuer einen Block oeffnen — mit den Bindungen, die dort
    /// HOCHGEZOGEN gehoeren (`blocks[i]`). Ohne sie steht `let` erst ab seiner
    /// Zeile, statt von Blockanfang an in der zeitlichen Totzone, und eine
    /// Funktionsdeklaration im Block gaebe es vor ihrer Zeile gar nicht.
    PushEnv(u32),
    PopEnv,
    /// Das Ergebnis des Programms merken (der Wert eines Programms ist sein
    /// letzter Ausdruckswert — `eval` und die Konsole leben davon).
    SetCompletion,
}

/// Was beim Betreten eines Blocks gebunden wird, bevor die erste Zeile laeuft.
///
/// Die Liste entsteht beim Uebersetzen aus denselben zwei Schleifen wie
/// `Interp::hoist` — dieselbe Reihenfolge, dieselben Faelle. Sie hier
/// nachzubauen statt den AST mitzuschleppen kostet zwei Zahlen je Bindung
/// statt einer Kopie des ganzen Rumpfes.
#[derive(Debug, Clone)]
pub enum BlockDecl {
    /// `let`/`const`/`class`: gebunden, aber NICHT bereit — die zeitliche
    /// Totzone. Ohne sie ist `let` nur ein `var` mit anderem Namen.
    Tdz { name: u32, mutable: bool },
    /// Eine Funktionsdeklaration: sofort fertig, damit sie vor ihrer Zeile
    /// aufrufbar ist.
    Func { name: u32, func: u32 },
}

/// Uebersetzter Code samt allem, worauf seine Befehle zeigen.
pub struct Chunk {
    pub ops: Vec<Op>,
    pub constants: Vec<Value>,
    /// Namen (Bezeichner und Eigenschaften), einmal abgelegt statt je Befehl.
    pub names: Vec<Rc<str>>,
    pub funcs: Vec<Rc<super::ast::Func>>,
    pub blocks: Vec<Vec<BlockDecl>>,
    /// Je `MakeArraySpread` eine Maske: welcher Eintrag war ein `...x`?
    pub blocks_spread: Vec<Vec<bool>>,
}

impl Chunk {
    pub fn new() -> Chunk {
        Chunk { ops: Vec::new(), constants: Vec::new(), names: Vec::new(),
                funcs: Vec::new(), blocks: Vec::new(), blocks_spread: Vec::new() }
    }

    pub fn emit(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    /// Einen noch unbekannten Sprung eintragen und seine Stelle zurueckgeben.
    pub fn emit_jump(&mut self, make: fn(u32) -> Op) -> usize {
        self.emit(make(u32::MAX))
    }

    /// Das Ziel eines vorgemerkten Sprungs auf HIER setzen.
    pub fn patch(&mut self, at: usize) {
        let here = self.ops.len() as u32;
        match &mut self.ops[at] {
            Op::Jump(t) | Op::JumpFalse(t) | Op::JumpFalseKeep(t)
            | Op::JumpTrueKeep(t) | Op::JumpNullishKeep(t) | Op::IterNext(t) => *t = here,
            other => panic!("patch auf {other:?}"),
        }
    }

    pub fn konst(&mut self, v: Value) -> u32 {
        self.constants.push(v);
        (self.constants.len() - 1) as u32
    }

    /// Namen werden dedupliziert: eine Schleife, die `i` zwanzigmal liest,
    /// legt ihn einmal ab.
    pub fn name(&mut self, s: &str) -> u32 {
        if let Some(i) = self.names.iter().position(|n| &**n == s) {
            return i as u32;
        }
        self.names.push(Rc::from(s));
        (self.names.len() - 1) as u32
    }

    pub fn block(&mut self, d: Vec<BlockDecl>) -> u32 {
        self.blocks.push(d);
        (self.blocks.len() - 1) as u32
    }

    pub fn spread_mask(&mut self, m: Vec<bool>) -> u32 {
        self.blocks_spread.push(m);
        (self.blocks_spread.len() - 1) as u32
    }

    pub fn func(&mut self, f: Rc<super::ast::Func>) -> u32 {
        self.funcs.push(f);
        (self.funcs.len() - 1) as u32
    }

    pub fn here(&self) -> u32 {
        self.ops.len() as u32
    }
}

/// Was der Uebersetzer noch nicht kann. **Kein Fehler, eine Absage** — der
/// Rufer faehrt das Programm dann GANZ mit dem Baumlaeufer, nie halb.
///
/// Der Text ist der Name der Form, nicht ein Satz: er wird gezaehlt, und eine
/// Zaehlung braucht einen Schluessel, keine Prosa.
#[derive(Debug)]
pub struct Unsupported(pub &'static str);

pub type CompileResult<T> = Result<T, Unsupported>;

/// Damit ein Zaehler ueber viele Laeufe etwas sagt.
impl core::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(self.0)
    }
}

impl From<Unsupported> for String {
    fn from(u: Unsupported) -> String {
        String::from(u.0)
    }
}
