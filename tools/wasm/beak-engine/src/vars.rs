//! vars.rs — CSS Custom Properties (`--name`) und `var()`.
//!
//! **Sie werden in der KASKADE aufgeloest, je Element** — `style::resolve_in`
//! sammelt sie aus den Regeln, die dieses Element wirklich treffen, erbt sie
//! vom Elternteil und setzt sie beim Anwenden eines Wertes ein. Hier steht
//! nur noch, was eine Karte IST und wie aus einem Wert ein fertiger wird.
//!
//! Bis 0.59.0 lief davor ein Textlauf ueber das ganze Blatt, mit einer
//! globalen Karte: ein Wert je Name fuer das ganze Dokument. Das traegt genau
//! ein Muster — `:root` setzt eine Palette, alles liest daraus — und bricht
//! bei dem, das jedes moderne Rahmenwerk benutzt: die Basisklasse liest die
//! Variable, jede Variante setzt sie neu.
//!
//!     .btn         { --bs-btn-bg: transparent; background: var(--bs-btn-bg) }
//!     .btn-primary { --bs-btn-bg: #0d6efd }
//!     .btn-link    { --bs-btn-bg: transparent }   <- steht ZULETZT im Blatt
//!
//! `.btn-link` trifft einen `<button class="btn btn-primary">` nie und gewann
//! trotzdem die globale Karte: **jeder Bootstrap-Knopf war durchsichtig.**
//! Dasselbe traf Hinweise, Tabellenstreifen und `list-group .active`.
//!
//! Mit dem Textlauf sind auch seine Heuristiken weg — er musste RATEN, welcher
//! Block „unbedingt" gilt, und tat das an der Selektor-Zeichenkette. Ein
//! Kommentar davor (Bootstraps Kopfzeile, mit Versionsnummer und URLs) reichte,
//! um `:root` fuer bedingt zu halten; dann gewann `[data-bs-theme=dark]`, und
//! die Seite war dunkel, ohne dass irgendwo ein `data-bs-theme` stand. Die
//! Kaskade muss nichts raten: sie TRIFFT.
//!
//! Gemessen hat der Umbau nichts gekostet — auf drei eingefrorenen Seiten
//! 56,6/45,5/56,2 ms vorher gegen 57,5/44,5/47,2 ms nachher. Der Textlauf ueber
//! ein 368-KB-Blatt war eben auch nicht gratis.

use alloc::string::{String, ToString};

fn parse_var_args(input: &str, open: usize) -> Option<(usize, String, Option<String>)> {
    let b = input.as_bytes();
    let n = b.len();
    let first = open + 1;
    let mut i = first;
    let mut depth: i32 = 0;
    let mut comma: Option<usize> = None;
    let mut close: Option<usize> = None;
    while i < n {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            i = skip_string(b, i);
            continue;
        }
        match c {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    close = Some(i);
                    break;
                }
                depth -= 1;
            }
            b',' if depth == 0 && comma.is_none() => comma = Some(i),
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    let (name_end, fallback) = match comma {
        Some(cp) => (cp, Some(input[cp + 1..close].trim().to_string())),
        None => (close, None),
    };
    let name = input[first..name_end].trim();
    if !name.starts_with("--") {
        return None;
    }
    Some((close + 1, name.to_string(), fallback))
}

// ── low-level helpers ───────────────────────────────────────────────────────

/// `true` if bytes at `i` spell `var(` (case-insensitive on `var`).
fn is_var_at(b: &[u8], i: usize) -> bool {
    i + 4 <= b.len()
        && (b[i] | 0x20) == b'v'
        && (b[i + 1] | 0x20) == b'a'
        && (b[i + 2] | 0x20) == b'r'
        && b[i + 3] == b'('
}

/// Cheap scan: does the text contain a `var(` anywhere?
fn contains_var(b: &[u8]) -> bool {
    if b.len() < 4 {
        return false;
    }
    let mut i = 0;
    while i + 4 <= b.len() {
        if is_var_at(b, i) {
            return true;
        }
        i += 1;
    }
    false
}

/// Index one past a `/* … */` comment that starts at `i`.
fn skip_comment(b: &[u8], i: usize) -> usize {
    let n = b.len();
    let mut k = i + 2;
    while k + 1 < n && !(b[k] == b'*' && b[k + 1] == b'/') {
        k += 1;
    }
    // Advance past the closing `*/` (or to end if unterminated).
    if k + 1 < n {
        k + 2
    } else {
        n
    }
}

/// Index one past a `"…"` or `'…'` string that starts at `i` (with `\` escapes).
fn skip_string(b: &[u8], i: usize) -> usize {
    let n = b.len();
    let q = b[i];
    let mut k = i + 1;
    while k < n {
        if b[k] == b'\\' {
            k += 2;
            continue;
        }
        if b[k] == q {
            return k + 1;
        }
        k += 1;
    }
    n
}

/// CSS ident byte (ASCII alnum, `-`, `_`, or any non-ASCII / UTF-8 byte).
fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

// ── Je Element, nicht je Dokument ───────────────────────────────────────────
//
// Frueher lief hier ein Textlauf ueber das ganze Blatt: EINE Karte, ein Wert
// je Name fuer das ganze Dokument. Das traegt das Muster, fuer das es gebaut
// war (`:root` setzt eine Palette), und bricht bei dem, das jedes moderne
// Rahmenwerk benutzt — die Basisklasse liest die Variable, jede Variante
// setzt sie neu:
//
//     .btn         { --bs-btn-bg: transparent; background: var(--bs-btn-bg) }
//     .btn-primary { --bs-btn-bg: #0d6efd }
//     .btn-link    { --bs-btn-bg: transparent }   <- steht ZULETZT im Blatt
//
// `.btn-link` trifft einen `<button class="btn btn-primary">` nie und gewann
// trotzdem: jeder Bootstrap-Knopf war durchsichtig. Eine Custom Property ist
// eine GEERBTE Eigenschaft — sie gehoert in die Kaskade, je Element. Dort
// steht sie jetzt (`style::resolve_in`); hier bleibt nur, was eine Karte ist
// und wie ein Wert daraus entsteht.

/// Die Custom Properties, die auf einem Element gelten.
///
/// Eine flache Liste und keine Karte: ein Element traegt selten mehr als ein
/// paar Dutzend, und ein linearer Vergleich ueber kurze Namen ist billiger
/// als das Hashen, das eine Karte je Zugriff kostet.
pub type VarMap = alloc::vec::Vec<(alloc::rc::Rc<str>, alloc::rc::Rc<str>)>;

/// Steht ein `var()` in diesem Wert? Ein Bytescan, damit der Normalfall —
/// die allermeisten Deklarationen haben keins — nichts kostet.
pub fn has_var(v: &str) -> bool { contains_var(v.as_bytes()) }

pub fn var_get<'a>(map: &'a VarMap, name: &str) -> Option<&'a str> {
    map.iter().find(|(k, _)| &**k == name).map(|(_, v)| &**v)
}

/// Setzen oder ersetzen. Ersetzen statt Anhaengen, damit die Liste nicht mit
/// jeder ueberschriebenen Deklaration waechst.
pub fn var_set(map: &mut VarMap, name: &str, value: &str) {
    match map.iter_mut().find(|(k, _)| &**k == name) {
        Some(slot) => slot.1 = alloc::rc::Rc::from(value),
        None => map.push((alloc::rc::Rc::from(name), alloc::rc::Rc::from(value))),
    }
}

/// `var()` in einem Wert ersetzen, gegen die Karte DIESES Elements.
///
/// `skip` ist der Name, dessen eigener Wert gerade ausgerechnet wird: er darf
/// sich nicht selbst einsetzen. `--x: var(--x, 1rem)` ist die Schreibweise,
/// mit der eine Seite „nimm den geerbten Wert, sonst 1rem" sagt (Wikipedia
/// tut das); wuerde er sich selbst finden, bliebe ein `var()` stehen und die
/// Deklaration waere ungueltig.
pub fn expand(value: &str, map: &VarMap, skip: Option<&str>) -> String {
    if !contains_var(value.as_bytes()) {
        return value.into();
    }
    let mut cur = expand_pass(value, map, skip);
    for _ in 1..MAX_PASSES {
        if !contains_var(cur.0.as_bytes()) || !cur.1 {
            break;
        }
        cur = expand_pass(&cur.0, map, skip);
    }
    cur.0
}

/// Deckel gegen Ringe: `--a: var(--b); --b: var(--a)` hoert von selbst nicht
/// auf. Was danach noch ein `var()` traegt, ist ungueltig — und das ist die
/// richtige Antwort, nicht ein erfundener Wert.
const MAX_PASSES: usize = 16;

fn expand_pass(input: &str, map: &VarMap, skip: Option<&str>) -> (String, bool) {
    let b = input.as_bytes();
    let mut out = String::with_capacity(input.len() + 16);
    let mut i = 0;
    let mut changed = false;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let j = skip_comment(b, i);
            out.push_str(&input[i..j]);
            i = j;
            continue;
        }
        if b[i] == b'"' || b[i] == b'\'' {
            let j = skip_string(b, i);
            out.push_str(&input[i..j]);
            i = j;
            continue;
        }
        if is_var_at(b, i) {
            if let Some((end, name, fallback)) = parse_var_args(input, i + 3) {
                let hit = if skip == Some(name.as_str()) { None } else { var_get(map, &name) };
                match (hit, fallback) {
                    (Some(v), _) => out.push_str(v),
                    (None, Some(f)) => out.push_str(&f),
                    // Kein Wert und kein Rueckfall: das `var()` bleibt stehen,
                    // der Wertparser scheitert daran, und die Deklaration
                    // faellt weg — CSS Variables 1 §3.
                    (None, None) => { out.push_str(&input[i..end]); i = end; continue }
                }
                changed = true;
                i = end;
                continue;
            }
        }
        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    (out, changed)
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DrawOp;

    fn map(pairs: &[(&str, &str)]) -> VarMap {
        pairs.iter().map(|(k, v)| (alloc::rc::Rc::from(*k), alloc::rc::Rc::from(*v))).collect()
    }

    /// Die Farbe, die eine Seite auf ihr erstes Textstueck malt.
    ///
    /// Der kuerzeste ehrliche Weg, einen KASKADIERTEN Wert zu pruefen: er geht
    /// durch Parser, Treffer, Kaskade, Vererbung und Einsetzung — also durch
    /// alles, was hier zu pruefen ist. Eine Probe direkt auf `expand` sagt
    /// ueber die Kaskade nichts.
    fn painted_text(html: &str, css: &str) -> Option<(u8, u8, u8)> {
        let mut eng = crate::Engine::new();
        let lay = eng.layout_ext(html, css, 800);
        lay.ops.iter().find_map(|o| match o {
            DrawOp::Text { color, .. } => Some((color.c.0, color.c.1, color.c.2)),
            _ => None,
        })
    }

    /// Alle Fuellfarben einer Seite. Eine Liste und kein „die erste": welche
    /// Fuellung die Leinwand ist und welche das Element, haengt am Aufbau der
    /// Seite — und die Probe soll den WERT pruefen, nicht die Malreihenfolge.
    fn fills(html: &str, css: &str) -> alloc::vec::Vec<(u8, u8, u8)> {
        let mut eng = crate::Engine::new();
        let lay = eng.layout_ext(html, css, 800);
        lay.ops.iter().filter_map(|o| match o {
            DrawOp::Rect { color, .. } | DrawOp::RoundRect { color, .. } =>
                Some((color.c.0, color.c.1, color.c.2)),
            _ => None,
        }).collect()
    }

    // ── Die Kaskade: WER entscheidet den Wert ───────────────────────────────

    /// **Der Fehler, wegen dem die Aufloesung in die Kaskade gezogen wurde.**
    ///
    /// Bis 0.59.0 lief ein Textlauf ueber das ganze Blatt mit einer globalen
    /// Karte. `.c` trifft das Element nicht und gewann trotzdem — auf einer
    /// echten Seite hiess das: `.btn-link{--bs-btn-bg:transparent}` steht
    /// zuletzt im Blatt, und JEDER Bootstrap-Knopf war durchsichtig.
    #[test]
    fn a_rule_that_does_not_match_must_not_decide_the_value() {
        let css = ".a{--x:#00f;color:var(--x)} .b{--x:#f00} .c{--x:#0f0}";
        assert_eq!(painted_text("<p class='a b'>x</p>", css), Some((255, 0, 0)));
    }

    /// Und die Umkehrung: die eigene Regel des Elements gewinnt gegen eine
    /// gleichnamige, die woanders steht.
    #[test]
    fn the_element_own_declaration_wins_over_a_later_foreign_one() {
        let css = ".btn{--bg:transparent;background-color:var(--bg)}\
                   .btn-primary{--bg:#0d6efd}\
                   .btn-link{--bg:transparent}";
        let f = fills("<p class='btn btn-primary'>x</p>", css);
        assert!(f.contains(&(13, 110, 253)), "die Fuellung des Elements fehlt: {f:?}");
    }

    /// Eine Custom Property wird VERERBT — der Kern der Sache, und der Grund,
    /// warum sie nicht bloss je Element gilt.
    #[test]
    fn a_custom_property_inherits_to_descendants() {
        let css = ".wrap{--c:#0f0} .deep{color:var(--c)}";
        assert_eq!(painted_text("<div class='wrap'><div><p class='deep'>x</p></div></div>", css),
                   Some((0, 255, 0)));
    }

    /// Und ein Nachfahre darf sie ueberschreiben, ohne den Vorfahren zu
    /// beruehren.
    #[test]
    fn a_descendant_may_shadow_an_inherited_value() {
        let css = ".wrap{--c:#f00} .inner{--c:#00f} .t{color:var(--c)}";
        assert_eq!(painted_text("<div class='wrap'><div class='inner'><p class='t'>x</p></div></div>", css),
                   Some((0, 0, 255)));
    }

    #[test]
    fn root_palette_reaches_everything() {
        assert_eq!(painted_text("<p>x</p>", ":root{--c:#f00} p{color:var(--c)}"), Some((255, 0, 0)));
    }

    /// Ein Kommentar vor einer Regel gehoert nicht in ihren Selektor —
    /// und seit die Kaskade wirklich TRIFFT, kann er es auch nicht mehr.
    /// Gemessen an Bootstrap 5.3.3: die Kopfzeile trug Versionsnummer und
    /// URLs, und der Dunkelblock gewann die ganze helle Palette.
    #[test]
    fn a_theme_block_that_matches_nothing_stays_out() {
        let css = "/*! Thing v5.3.3 (https://example.com/) */\
                   :root,[data-t=light]{--bg:#fff}\
                   [data-t=dark]{--bg:#000}\
                   p{color:var(--bg)}";
        assert_eq!(painted_text("<p>x</p>", css), Some((255, 255, 255)));
    }

    /// Und wenn das Attribut DA ist, gewinnt der Dunkelblock — sonst waere
    /// die Regel darueber bloss ein „nie".
    #[test]
    fn the_same_theme_block_wins_when_it_does_match() {
        let css = ":root,[data-t=light]{--bg:#fff} [data-t=dark]{--bg:#000} p{color:var(--bg)}";
        assert_eq!(painted_text("<html data-t='dark'><body><p>x</p></body></html>", css),
                   Some((0, 0, 0)));
    }

    /// MediaWiki liefert eine Definition je Benutzereinstellung und die Seite
    /// traegt genau eine davon. Die andere darf nicht gewinnen — frueher
    /// brauchte es dafuer eine Heuristik, heute reicht das Treffen.
    #[test]
    fn only_the_class_the_root_carries_counts() {
        let css = "html.pref-1{--s:#f00} html.pref-2{--s:#0f0} p{color:var(--s)}";
        assert_eq!(painted_text("<html class='pref-1'><body><p>x</p></body></html>", css),
                   Some((255, 0, 0)));
    }

    /// Spezifitaet schlaegt Reihenfolge, wie bei jeder anderen Eigenschaft.
    #[test]
    fn specificity_decides_before_order() {
        let css = "#id{--c:#f00} .cls{--c:#0f0} p{color:var(--c)}";
        assert_eq!(painted_text("<p id='id' class='cls'>x</p>", css), Some((255, 0, 0)));
    }

    /// Ein `@media`, das nicht gilt, liefert auch keine Variablen.
    #[test]
    fn a_media_block_that_does_not_apply_contributes_nothing() {
        let css = ":root{--c:#f00} @media (max-width:480px){:root{--c:#0f0}} p{color:var(--c)}";
        assert_eq!(painted_text("<p>x</p>", css), Some((255, 0, 0)));
    }

    // ── Die Einsetzung: WAS aus einem Wert wird ─────────────────────────────

    #[test]
    fn simple_substitution() {
        assert_eq!(expand("var(--c)", &map(&[("--c", "#f00")]), None), "#f00");
    }

    #[test]
    fn fallback_used_when_undefined() {
        assert_eq!(expand("var(--missing, blue)", &map(&[]), None), "blue");
    }

    #[test]
    fn fallback_ignored_when_defined() {
        assert_eq!(expand("var(--c, blue)", &map(&[("--c", "green")]), None), "green");
    }

    /// Kein Wert und kein Rueckfall: das `var()` bleibt stehen, der
    /// Wertparser scheitert daran, und die Deklaration faellt weg. Genau das
    /// verlangt CSS Variables 1 §3 — ein leerer Wert waere etwas anderes.
    #[test]
    fn undefined_without_fallback_stays_and_invalidates() {
        assert_eq!(expand("var(--nope)", &map(&[]), None), "var(--nope)");
    }

    #[test]
    fn nested_var_in_value() {
        assert_eq!(expand("var(--a)", &map(&[("--a", "var(--b)"), ("--b", "#0f0")]), None), "#0f0");
    }

    #[test]
    fn nested_var_in_fallback() {
        assert_eq!(expand("var(--x, calc(1px + var(--y, 2px)))", &map(&[]), None),
                   "calc(1px + 2px)");
    }

    #[test]
    fn fallback_with_commas_and_parens() {
        assert_eq!(expand("0 0 0 var(--x, rgba(0,0,0,.1))", &map(&[]), None),
                   "0 0 0 rgba(0,0,0,.1)");
    }

    #[test]
    fn whitespace_variations() {
        assert_eq!(expand("var( --c )", &map(&[("--c", "#abc")]), None), "#abc");
        assert_eq!(expand("var(  --missing ,  blue  )", &map(&[]), None), "blue");
    }

    #[test]
    fn uppercase_var_function() {
        assert_eq!(expand("VAR(--c)", &map(&[("--c", "#0f0")]), None), "#0f0");
    }

    #[test]
    fn var_inside_string_is_not_expanded() {
        assert_eq!(expand(r#""var(--c)""#, &map(&[("--c", "red")]), None), r#""var(--c)""#);
    }

    #[test]
    fn multiple_uses_in_one_value() {
        assert_eq!(expand("var(--c) var(--c)", &map(&[("--c", "1px")]), None), "1px 1px");
    }

    /// **Wikipedias Schreibweise.** `--fs: var(--fs, 1rem)` heisst „nimm den
    /// geerbten Wert, sonst 1rem". Duerfte sie sich selbst finden, bliebe ein
    /// `var()` stehen und die Deklaration waere ungueltig — die Suchleiste
    /// verlor daran einmal ihre Lupe.
    #[test]
    fn a_self_referential_declaration_takes_the_fallback() {
        assert_eq!(expand("var(--fs,1rem)", &map(&[]), Some("--fs")), "1rem");
    }

    /// Ein Ring aus zwei Namen dreht sich nicht ewig.
    #[test]
    fn a_cycle_terminates() {
        let m = map(&[("--a", "var(--b)"), ("--b", "var(--a)")]);
        let out = expand("var(--a)", &m, None);
        assert!(out.contains("var("), "der Ring haette einen Wert liefern muessen? {out}");
    }

    #[test]
    fn a_deep_chain_is_not_a_cycle() {
        let m = map(&[("--a", "var(--b)"), ("--b", "var(--c)"), ("--c", "#0f0")]);
        assert_eq!(expand("var(--a)", &m, None), "#0f0");
    }

    /// Die Form, in der Bootstrap seine Palette weiterreicht.
    #[test]
    fn bootstrap_like_chain() {
        let m = map(&[("--bs-blue", "#0d6efd"), ("--bs-primary", "var(--bs-blue)")]);
        assert_eq!(expand("var(--bs-primary)", &m, None), "#0d6efd");
        assert_eq!(expand("var(--bs-primary, #ccc)", &m, None), "#0d6efd");
    }

    #[test]
    fn a_value_without_var_is_returned_unchanged() {
        assert_eq!(expand("1px solid red", &map(&[("--c", "x")]), None), "1px solid red");
    }
}
