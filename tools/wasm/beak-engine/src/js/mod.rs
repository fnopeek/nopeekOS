//! JavaScript: Lexer, Syntaxbaum, Parser.
//!
//! **Warum selbst geschrieben und nicht portiert.** Bei WebP war die ehrliche
//! Antwort die umgekehrte (`super::webp`): `image-webp` beruehrte `std` in vier
//! Zeilen, also wurde portiert statt neu gebaut. Hier liegt der Fall anders —
//! die guten JS-Parser (swc, oxc, boa) sind gross, `std`-gebunden und auf
//! Arenen gebaut; sie nach `no_std` zu ziehen waere mehr Arbeit als diese
//! Datei, und der Baum, den sie liefern, ist auf ihre eigenen Motoren
//! zugeschnitten. Der Rest der Engine ist aus demselben Grund handgeschrieben.
//!
//! Gemessen wird gegen **test262** (`tools/test262/`), mit der V8-Grundlinie
//! als Vergleich: ein Test, den wir reissen und V8 besteht, ist unsere Luecke.
//!
//! Was hier NICHT ist: eine Auswertung. Der Parser baut den Baum, mehr nicht.

pub mod ast;
/// Der Befehlssatz und der Uebersetzer dorthin — siehe `code.rs` fuer die
/// Begruendung des Umbaus.
pub mod code;
pub mod compile;
pub mod vm;
pub mod bigint;
pub mod builtins;
pub mod date;
pub mod iterhelp;
pub mod dombind;
pub mod eval;
pub mod json;
pub mod modules;
pub mod expr;
pub mod generator;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod promise;
pub mod proxy;
pub mod url;
pub mod regexp;
pub mod value;

pub use ast::Program;
pub use parser::{parse, ParseError};
pub use interp::TEST_STEPS;
pub use interp::{STRICT_SITES, STRICT_SITE_NAMES};

/// Ein Programm laufen lassen. Fehler kommen als geworfener JS-Wert zurueck,
/// nicht als Rust-Fehler — ein `throw` ist ein normaler Ausgang.
pub fn run(src: &str, module: bool) -> Result<(), alloc::string::String> {
    run_capped(src, module, u64::MAX)
}

/// Wie `run`, aber mit einer Schrittgrenze — was ein Testlaeufer braucht.
pub fn run_capped(src: &str, module: bool, max_steps: u64) -> Result<(), alloc::string::String> {
    use alloc::string::ToString;
    let prog = parse(src, module).map_err(|e| alloc::format!("SyntaxError: {} @{}", e.msg, e.at))?;
    let mut i = interp::Interp::new();
    i.max_steps = max_steps;
    match i.run_program(&prog) {
        Ok(_) => Ok(()),
        Err(interp::Abrupt::Throw(v)) => {
            let name = i.get(&v, "name").ok().and_then(|n| i.to_string(&n).ok());
            let msg = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
            Err(match (name, msg) {
                (Some(n), Some(m)) if !m.is_empty() => alloc::format!("{n}: {m}"),
                (Some(n), _) => n.to_string(),
                _ => "uncaught exception".to_string(),
            })
        }
        Err(_) => Err("illegal completion".to_string()),
    }
}

/// Eine Ausfuehrungseinheit, in der MEHRERE Programme nacheinander laufen.
///
/// Der Testlaeufer braucht genau das: der Vorspann (`assert.js` + `sta.js`)
/// wird EINMAL geparst und dann vor jedem Test nur noch ausgefuehrt. Ihn je
/// Variante neu zu parsen war der erste Entwurf, und bei 78 000 Varianten sind
/// das ein halbes Gigabyte Parsen fuer nichts.
pub struct Session {
    pub interp: interp::Interp,
}

impl Session {
    pub fn new(max_steps: u64) -> Session {
        let mut interp = interp::Interp::new();
        interp.max_steps = max_steps;
        Session { interp }
    }

    /// Dieselbe Sitzung, aber ohne die Befehlsmaschine — fuer die Gegenprobe,
    /// die sagt, WELCHE Tests die Umstellung kostet.
    pub fn new_without_vm(max_steps: u64) -> Session {
        let mut s = Session::new(max_steps);
        s.interp.vm_off = true;
        s
    }

    /// Ein bereits geparstes Programm laufen lassen.
    pub fn run(&mut self, prog: &Program) -> Result<(), alloc::string::String> {
        match self.interp.run_program(prog) {
            Ok(_) => Ok(()),
            Err(interp::Abrupt::Throw(v)) => Err(self.describe(v)),
            Err(_) => Err(alloc::string::String::from("illegal completion")),
        }
    }

    fn describe(&mut self, v: value::Value) -> alloc::string::String {
        use alloc::string::ToString;
        let name = self.interp.get(&v, "name").ok().and_then(|n| self.interp.to_string(&n).ok());
        let msg = self.interp.get(&v, "message").ok().and_then(|m| self.interp.to_string(&m).ok());
        match (name, msg) {
            (Some(n), Some(m)) if !m.is_empty() => alloc::format!("{n}: {m}"),
            (Some(n), _) if !n.is_empty() => n.to_string(),
            _ => self.interp.to_string(&v).map(|s| s.to_string())
                    .unwrap_or_else(|_| "uncaught exception".to_string()),
        }
    }
}

/// Nur pruefen, ob es parst — ohne den Baum zu behalten.
///
/// Das ist die Form, die der Konformanzlauf braucht: bei 50 000 Dateien ist
/// die Frage „nimmt der Parser das an?" und nicht „wie sieht es aus".
pub fn parses(src: &str, module: bool) -> Result<(), ParseError> {
    parse(src, module).map(|_| ())
}
