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
pub mod lexer;
pub mod parser;

pub use ast::Program;
pub use parser::{parse, ParseError};

/// Nur pruefen, ob es parst — ohne den Baum zu behalten.
///
/// Das ist die Form, die der Konformanzlauf braucht: bei 50 000 Dateien ist
/// die Frage „nimmt der Parser das an?" und nicht „wie sieht es aus".
pub fn parses(src: &str, module: bool) -> Result<(), ParseError> {
    parse(src, module).map(|_| ())
}
