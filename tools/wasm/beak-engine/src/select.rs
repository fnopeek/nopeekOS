//! Text auf der Seite markieren — und damit kopieren und finden.
//!
//! **Warum das fehlte und warum es zaehlt.** Nach „Links anklicken" ist
//! Markieren und Kopieren die meistbenutzte Handlung in einem Browser
//! ueberhaupt, und beak konnte sie nicht: `grep` ueber die Shell zaehlte
//! null Treffer (`docs/plan/BROWSER_TABS.md` §B4). Die Adresszeile kann es
//! seit 0.150.0 — die Seite ist ein Bild auf einer Leinwand, und ein Bild
//! markiert man nicht.
//!
//! **Die Vorarbeit war schon da.** Die Anzeigeliste ist keine Pixelwand: sie
//! ist eine Folge von `DrawOp::Text`, jeder mit Ort, Groesse, Schriftwahl und
//! seinem Text. Damit ist „welcher Buchstabe liegt unter diesem Punkt" eine
//! Rechnung und keine Suche — und dieselbe Rechnung rueckwaerts gibt die
//! Rechtecke zum Hervorheben.
//!
//! Die Masse gehoeren dem MOTOR, nicht dem Layout: eine Textbreite haengt am
//! Gesicht, und die Gesichter haelt `Engine`. Deshalb steht die
//! Schnittstelle dort (`Engine::text_pos_at` und Nachbarn) und hier nur die
//! Rechnung.
//!
//! **Reihenfolge = Anzeigeliste.** Ein Bereich ist ein Paar aus Orten, und
//! „von … bis" braucht eine Ordnung. Genommen wird die der Anzeigeliste, und
//! die ist fuer Text die Dokumentreihenfolge — nicht, weil das immer stimmt
//! (ein `position:absolute`-Kasten wird spaeter gemalt und steht vielleicht
//! weiter oben), sondern weil es die einzige Ordnung ist, die es umsonst
//! gibt. Was dabei herauskommt, ist die Reihenfolge, in der die Seite
//! GEZEICHNET wird; fuer gewoehnlichen Fliesstext ist das die gelesene.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fonts::Fonts;
use crate::layout::{DrawOp, Layout};

/// Ein Ort im Text der Seite: der wievielte Zeichenbefehl, und das wievielte
/// Byte darin.
///
/// `Ord` vergleicht erst den Befehl, dann das Byte — genau die Ordnung, die
/// „von hier bis dort" braucht. Ein Bereich wird deshalb nie verkehrt herum
/// gehalten, sondern beim Gebrauch sortiert: wer nach links zieht, meint
/// dasselbe wie wer nach rechts zieht.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct TextPos {
    pub op: u32,
    pub off: u32,
}

/// Ein Zeichenbefehl, aufbereitet: sein Kasten und was zum Messen noetig ist.
struct Run<'a> {
    idx: u32,
    x: i32,
    y: i32,
    text: &'a str,
    size: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    family: u32,
    sp: (f32, f32),
}

fn runs(lay: &Layout) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    for (i, op) in lay.ops.iter().enumerate() {
        if let DrawOp::Text { x, y, size, bold, italic, mono, family, sp, text, .. } = op {
            if text.is_empty() { continue }
            out.push(Run {
                idx: i as u32, x: *x, y: *y, text, size: *size,
                bold: *bold, italic: *italic, mono: *mono, family: *family, sp: *sp,
            });
        }
    }
    out
}

/// Breite eines Stuecks in diesem Lauf.
fn width(fonts: &Fonts, r: &Run, s: &str) -> f32 {
    let face = fonts.pick(r.bold, r.italic, r.mono, r.family);
    crate::layout::measure_sp_pub(face, s, r.size, r.sp)
}

fn height(fonts: &Fonts, r: &Run) -> i32 {
    let face = fonts.pick(r.bold, r.italic, r.mono, r.family);
    libm::ceilf(crate::layout::line_gap_pub(face, r.size)) as i32
}

/// Der Ort im Text unter `(x, y)` in Dokumentkoordinaten.
///
/// Trifft der Punkt keinen Lauf, wird der NAECHSTE genommen — erst senkrecht
/// (welche Zeile), dann waagerecht (welches Ende). Das ist kein
/// Entgegenkommen, sondern die Bedingung dafuer, dass Ziehen funktioniert:
/// wer ueber den rechten Rand hinauszieht, meint „bis zum Zeilenende", und
/// wer in den Rand zwischen zwei Absaetzen faehrt, meint einen von beiden.
pub fn text_pos_at(fonts: &Fonts, lay: &Layout, x: i32, y: i32) -> Option<TextPos> {
    let rs = runs(lay);
    if rs.is_empty() { return None }

    // Abstand einer Zeile zum Zeiger, senkrecht. 0 heisst: der Punkt liegt
    // darin.
    let vdist = |r: &Run| -> i32 {
        let h = height(fonts, r).max(1);
        if y < r.y { r.y - y } else if y >= r.y + h { y - (r.y + h) + 1 } else { 0 }
    };
    let best = rs.iter().map(vdist).min()?;
    // Alle Laeufe DIESER Zeile, in Anzeigereihenfolge.
    let line: Vec<&Run> = rs.iter().filter(|r| vdist(r) == best).collect();

    // Waagerecht: der Lauf, der den Punkt enthaelt; sonst der letzte, der
    // davor beginnt; sonst der erste.
    let mut chosen = line[0];
    for r in &line {
        let w = libm::ceilf(width(fonts, r, r.text)) as i32;
        if x >= r.x && x < r.x + w { chosen = r; break }
        if r.x <= x { chosen = r }
    }
    Some(TextPos { op: chosen.idx, off: byte_at(fonts, chosen, x) })
}

/// Das Byte, an dem der Punkt in DIESEM Lauf liegt.
///
/// Gerundet auf die naechste Zeichengrenze: wer auf die linke Haelfte eines
/// Buchstabens zeigt, meint davor. So macht es jeder Editor, und ohne das
/// laesst sich das erste Zeichen einer Zeile nicht mitnehmen.
fn byte_at(fonts: &Fonts, r: &Run, x: i32) -> u32 {
    let rel = (x - r.x) as f32;
    if rel <= 0.0 { return 0 }
    let mut prev_w = 0.0f32;
    for (b, c) in r.text.char_indices() {
        let upto = b + c.len_utf8();
        let w = width(fonts, r, &r.text[..upto]);
        if rel < (prev_w + w) / 2.0 { return b as u32 }
        if rel < w { return upto as u32 }
        prev_w = w;
    }
    r.text.len() as u32
}

/// Die Rechtecke, die den Bereich hervorheben — Dokumentkoordinaten.
pub fn selection_rects(fonts: &Fonts, lay: &Layout, a: TextPos, b: TextPos)
    -> Vec<(i32, i32, i32, i32)>
{
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    for r in runs(lay) {
        if r.idx < a.op || r.idx > b.op { continue }
        let lo = if r.idx == a.op { a.off as usize } else { 0 };
        let hi = if r.idx == b.op { b.off as usize } else { r.text.len() };
        let (lo, hi) = (lo.min(r.text.len()), hi.min(r.text.len()));
        if hi <= lo { continue }
        if !r.text.is_char_boundary(lo) || !r.text.is_char_boundary(hi) { continue }
        let x0 = r.x + libm::roundf(width(fonts, &r, &r.text[..lo])) as i32;
        let w = (libm::roundf(width(fonts, &r, &r.text[..hi])) as i32
                 - libm::roundf(width(fonts, &r, &r.text[..lo])) as i32).max(1);
        out.push((x0, r.y, w, height(fonts, &r).max(1)));
    }
    out
}

/// Der ausgewaehlte Text.
///
/// Zwischen zwei Laeufen steht ein Zeilenumbruch, wenn sie auf
/// verschiedenen Zeilen liegen, sonst nichts — ein Absatz kommt aus mehreren
/// Laeufen (fett, kursiv, ein Link mittendrin), und dazwischen gehoert kein
/// Trenner. Ein Leerzeichen zu setzen waere falsch: die Laeufe tragen ihre
/// Leerzeichen selbst, und `wortA` + `wortB` sind im Original vielleicht
/// `wortAwortB`.
pub fn selected_text(lay: &Layout, a: TextPos, b: TextPos) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut out = String::new();
    let mut last_y: Option<i32> = None;
    for r in runs(lay) {
        if r.idx < a.op || r.idx > b.op { continue }
        let lo = if r.idx == a.op { a.off as usize } else { 0 };
        let hi = if r.idx == b.op { b.off as usize } else { r.text.len() };
        let (lo, hi) = (lo.min(r.text.len()), hi.min(r.text.len()));
        if hi <= lo { continue }
        if !r.text.is_char_boundary(lo) || !r.text.is_char_boundary(hi) { continue }
        if last_y.is_some_and(|ly| ly != r.y) { out.push('\n'); }
        out.push_str(&r.text[lo..hi]);
        last_y = Some(r.y);
    }
    out
}

/// Alle Vorkommen von `needle` im Text der Seite, ohne Ruecksicht auf Gross-
/// und Kleinschreibung — als Bereiche, die `selection_rects` hervorheben kann.
///
/// **Je Lauf, nicht ueber die ganze Seite.** Ein Wort, das eine Zeile
/// ueberschreitet oder mitten im Wort fett wird, faellt damit durch. Das ist
/// die ehrliche Grenze dieser Fassung, und sie ist benannt statt versteckt:
/// laufuebergreifend zu suchen hiesse, den Text zusammenzusetzen und die
/// Rueckabbildung auf Laeufe zu fuehren — machbar, aber eine andere Groesse.
pub fn find_all(lay: &Layout, needle: &str) -> Vec<(TextPos, TextPos)> {
    let mut out = Vec::new();
    if needle.is_empty() { return out }
    let low_needle = needle.to_lowercase();
    for r in runs(lay) {
        // Kleinschreiben kann die BYTELAENGE aendern (ẞ → ß ist gleich lang,
        // aber İ → i̇ nicht), und dann zeigen die Fundstellen ins Leere.
        // Deshalb nur dann ueber die kleingeschriebene Fassung suchen, wenn
        // sie dieselbe Laenge hat — sonst zeichengenau vergleichen.
        let low = r.text.to_lowercase();
        if low.len() == r.text.len() {
            let mut from = 0usize;
            while let Some(i) = low[from..].find(&low_needle) {
                let s = from + i;
                let e = s + low_needle.len();
                if r.text.is_char_boundary(s) && r.text.is_char_boundary(e) {
                    out.push((TextPos { op: r.idx, off: s as u32 },
                              TextPos { op: r.idx, off: e as u32 }));
                }
                from = s + 1;
                if from >= low.len() { break }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    fn lay(html: &str) -> (Engine, Layout) {
        let eng = Engine::new();
        let l = eng.layout(html, 800);
        (eng, l)
    }

    /// Der Punkt trifft das Zeichen, auf das er zeigt — und die Rueckreise
    /// gibt denselben Text her.
    #[test]
    fn a_point_hits_the_character_under_it() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        // Ganz links im Absatz: vor dem H.
        let runs = super::runs(&l);
        let r = &runs[0];
        assert_eq!(r.text, "Hallo Welt");
        let a = eng.text_pos_at(&l, r.x, r.y + 4).expect("Anfang");
        assert_eq!(a.off, 0, "links vom ersten Buchstaben ist Offset 0");
        // Weit rechts: hinter dem letzten.
        let b = eng.text_pos_at(&l, r.x + 10_000, r.y + 4).expect("Ende");
        assert_eq!(b.off as usize, r.text.len(), "rechts vom letzten ist das Ende");
        assert_eq!(eng.selected_text(&l, a, b), "Hallo Welt");
    }

    /// Rueckwaerts gezogen ist derselbe Bereich. Ohne das gibt jede Auswahl
    /// von rechts nach links nichts her.
    #[test]
    fn dragging_backwards_selects_the_same_thing() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        let r = &super::runs(&l)[0];
        let a = eng.text_pos_at(&l, r.x, r.y + 4).unwrap();
        let b = eng.text_pos_at(&l, r.x + 10_000, r.y + 4).unwrap();
        assert_eq!(eng.selected_text(&l, a, b), eng.selected_text(&l, b, a));
        assert_eq!(eng.selection_rects(&l, a, b).len(), eng.selection_rects(&l, b, a).len());
    }

    /// Ueber zwei Absaetze hinweg kommt ein Zeilenumbruch dazwischen — aber
    /// NICHT zwischen zwei Laeufen derselben Zeile.
    #[test]
    fn a_line_break_only_between_lines() {
        let (eng, l) = lay("<body><p>eins</p><p>zwei</p></body>");
        let rs = super::runs(&l);
        assert!(rs.len() >= 2, "zwei Absaetze, zwei Laeufe");
        let a = TextPos { op: rs[0].idx, off: 0 };
        let b = TextPos { op: rs[1].idx, off: rs[1].text.len() as u32 };
        assert_eq!(eng.selected_text(&l, a, b), "eins\nzwei");

        // Fett mitten im Satz: ein Absatz, mehrere Laeufe, KEIN Umbruch.
        let (eng2, l2) = lay("<body><p>a<b>b</b>c</p></body>");
        let rs2 = super::runs(&l2);
        let a2 = TextPos { op: rs2[0].idx, off: 0 };
        let last = rs2.last().unwrap();
        let b2 = TextPos { op: last.idx, off: last.text.len() as u32 };
        let got = eng2.selected_text(&l2, a2, b2);
        assert!(!got.contains('\n'), "eine Zeile, kein Umbruch: {got:?}");
    }

    /// Die Hervorhebung liegt AUF dem Text, nicht daneben.
    #[test]
    fn the_highlight_covers_the_text_it_names() {
        let (eng, l) = lay("<body><p>Hallo Welt</p></body>");
        let r = &super::runs(&l)[0];
        let a = TextPos { op: r.idx, off: 0 };
        let b = TextPos { op: r.idx, off: 5 };   // "Hallo"
        let rects = eng.selection_rects(&l, a, b);
        assert_eq!(rects.len(), 1);
        let (x, y, w, h) = rects[0];
        assert_eq!(x, r.x, "faengt am Lauf an");
        assert_eq!(y, r.y);
        assert!(h > 0 && w > 0, "{w}x{h}");
        // "Hallo" ist kuerzer als "Hallo Welt".
        let all = eng.selection_rects(&l, a, TextPos { op: r.idx, off: r.text.len() as u32 });
        assert!(w < all[0].2, "der Teil ist schmaler als das Ganze: {w} vs {}", all[0].2);
    }

    /// Suchen ist gross-/kleinschreibungsblind und findet jedes Vorkommen.
    #[test]
    fn find_is_case_blind_and_finds_every_hit() {
        let (eng, l) = lay("<body><p>Welt welt WELT</p></body>");
        let hits = eng.find_all(&l, "welt");
        assert_eq!(hits.len(), 3, "drei Schreibweisen, drei Treffer");
        for (a, b) in &hits {
            assert_eq!(eng.selected_text(&l, *a, *b).to_lowercase(), "welt");
        }
        assert!(eng.find_all(&l, "").is_empty(), "nichts zu suchen, nichts zu finden");
        assert!(eng.find_all(&l, "gibtesnicht").is_empty());
    }

    /// Ein Umlaut ist zwei Bytes — eine Auswahl darf nie mitten hinein.
    #[test]
    fn a_selection_never_splits_a_character() {
        let (eng, l) = lay("<body><p>Grüezi wohl</p></body>");
        let r = &super::runs(&l)[0];
        // Jeden Pixel des Laufs abtasten: jeder Offset muss eine
        // Zeichengrenze sein, sonst schneidet `selected_text` in ein Zeichen.
        let w = super::width(&eng_fonts(&eng), r, r.text) as i32;
        for x in r.x..=(r.x + w + 4) {
            let p = eng.text_pos_at(&l, x, r.y + 4).unwrap();
            assert!(r.text.is_char_boundary(p.off as usize),
                    "Offset {} bei x={} ist keine Zeichengrenze in {:?}", p.off, x, r.text);
        }
    }

    fn eng_fonts(e: &Engine) -> core::cell::Ref<'_, crate::fonts::Fonts> { e.fonts_ref() }
}
