//! Die eingebaute Pruefseite: `beak:selftest`.
//!
//! Ein Browser, der nur an fremden Seiten gemessen wird, misst jedes Mal
//! etwas anderes: Wikipedia meldete `keine Behandler`, also lief der ganze
//! Zustellpfad im Geraetelauf nie. Diese Seite antwortet immer gleich, holt
//! nichts aus dem Netz und sagt ihr Ergebnis ZWEIMAL — auf dem Schirm zum
//! Anschauen und im Log zum Weiterreichen.
//!
//! Sie ist absichtlich altmodisch geschrieben: der Rahmen kommt mit `var`
//! und `function` aus, und jede moderne Schreibweise wird ueber `Function()`
//! einzeln geprueft. Sonst nimmt EIN nicht lesbares Sprachmittel die ganze
//! Seite mit, und statt einer Liste mit einer roten Zeile steht da nichts.

/// Das Dokument. Kein Link, kein Bild, kein externes Skript.
///
/// Als eigene Datei, damit derselbe Text auch host-seitig durch die Engine
/// laufen kann (`beak-engine/examples/selftest.rs`) — eine Pruefseite, die
/// nur auf dem Geraet pruefbar ist, wird nicht gepflegt.
pub const HTML: &str = include_str!("selftest.html");

/// Die Adresse, unter der die Seite steht.
pub const URL: &str = "beak:selftest";

/// Ist das die Pruefseite? `about:` wird mitgenommen, weil jeder Browser sie
/// dort hat und der Tippfehler sonst als Websuche endet.
pub fn matches(url: &str) -> bool {
    let u = url.trim();
    u.eq_ignore_ascii_case(URL)
        || u.eq_ignore_ascii_case("about:selftest")
        || u.eq_ignore_ascii_case("beak:test")
}
